package dev.figura.munda

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle

/**
 * The share sheet: cleaning a file on its way out.
 *
 * NOT COMPILED OR RUN. See README.md in this folder.
 *
 * This is the other half of the product, and on iOS it is the whole of it. It is
 * also the half with a user watching, which changes what it may do: the runner
 * cleans silently because nobody is there, and this must show what it found
 * before it writes anything, for the same reason the desktop window has two
 * phases.
 *
 * The one thing it must not do is clean the user's original. A share is a copy
 * going somewhere; the file in the gallery is not what is being sent. So the
 * cleaned bytes go to a cache file and that is what gets forwarded.
 */
class ShareActivity : Activity() {
    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        val incoming: List<Uri> = when (intent?.action) {
            Intent.ACTION_SEND -> listOfNotNull(intent.getParcelableExtra(Intent.EXTRA_STREAM))
            Intent.ACTION_SEND_MULTIPLE ->
                intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM) ?: emptyList()
            else -> emptyList()
        }
        if (incoming.isEmpty()) {
            finish()
            return
        }
        // The report goes here, then a button, then clean(incoming) below.
        // Inspection first and writing second is not a style choice: it is the
        // only reason a user has to believe any of this worked.
    }

    /**
     * Clean each shared file into the cache and forward the copies.
     *
     * Originals are never touched here. The runner is what cleans files in
     * place, on purpose, with the user having asked for exactly that.
     */
    private fun clean(incoming: List<Uri>): List<Uri> {
        val profile = Profiles.read(applicationContext) ?: return incoming
        val handle = Native.open(profile, "{}")
        val out = mutableListOf<Uri>()
        try {
            val now = System.currentTimeMillis() / 1000
            for (uri in incoming) {
                val name = displayName(uri) ?: continue
                val ext = name.substringAfterLast('.', "").lowercase()
                val data = contentResolver.openInputStream(uri)?.use { it.readBytes() } ?: continue
                val cleaned = Native.resolve(
                    handle, uri.toString(), name, data.size.toLong(), now, ext, data,
                )
                if (cleaned.isEmpty()) {
                    // Already clean, refused, or failed. Forward the original
                    // rather than silently dropping the user's file; the report
                    // says which it was.
                    out.add(uri)
                    continue
                }
                val copy = java.io.File(cacheDir, name)
                copy.writeBytes(cleaned)
                out.add(
                    androidx.core.content.FileProvider.getUriForFile(
                        this, "$packageName.shared", copy,
                    )
                )
            }
        } finally {
            Native.close(handle)
        }
        return out
    }

    private fun displayName(uri: Uri): String? =
        contentResolver.query(uri, null, null, null, null)?.use { cursor ->
            val column = cursor.getColumnIndex(android.provider.OpenableColumns.DISPLAY_NAME)
            if (column >= 0 && cursor.moveToFirst()) cursor.getString(column) else null
        }
}
