package dev.figura.munda

import android.content.Context
import android.net.Uri
import android.os.BatteryManager
import android.provider.DocumentsContract
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters
import java.io.File

/**
 * One sweep, driven from the host side.
 *
 * NOT COMPILED OR RUN. See mobile/android/README.md.
 *
 * Two things here are deliberate rather than incidental:
 *
 * The journal is persisted after every file, because the system will kill this
 * worker partway and a journal that only existed at the end would mean every
 * sweep starting over.
 *
 * A file is read only when [Native.plan] asks for it. Deciding from the size
 * and modification time that nothing has changed is the difference between a
 * sweep the user never notices and one that reads a thousand photographs to
 * learn nothing.
 */
class SweepWorker(
    context: Context,
    params: WorkerParameters,
) : CoroutineWorker(context, params) {

    override suspend fun doWork(): Result {
        val profile = Profiles.read(applicationContext) ?: return Result.success()
        val roots = Grants.folders(applicationContext)
        if (roots.isEmpty()) return Result.success()

        val journalFile = File(applicationContext.filesDir, "journal.json")
        val handle = Native.open(profile, journalFile.takeIf { it.exists() }?.readText() ?: "{}")
        try {
            val now = System.currentTimeMillis() / 1000
            val declined = Native.due(handle, charging(), battery(), now)
            if (declined.isNotEmpty()) {
                Notices.show(applicationContext, declined)
                return Result.success()
            }
            for (root in roots) {
                walk(handle, root, now, journalFile)
            }
            Native.finish(handle, now)
            journalFile.writeText(Native.journal(handle))
            Notices.show(applicationContext, Native.report(handle))
            return Result.success()
        } finally {
            Native.close(handle)
        }
    }

    private fun walk(handle: Long, tree: Uri, now: Long, journalFile: File) {
        val resolver = applicationContext.contentResolver
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(
            tree,
            DocumentsContract.getTreeDocumentId(tree),
        )
        val columns = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_SIZE,
            DocumentsContract.Document.COLUMN_LAST_MODIFIED,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
        )
        resolver.query(children, columns, null, null, null)?.use { cursor ->
            while (cursor.moveToNext() && !isStopped) {
                val documentId = cursor.getString(0)
                val name = cursor.getString(1)
                val size = cursor.getLong(2)
                // The platform reports milliseconds; the runner works in seconds.
                val mtime = cursor.getLong(3) / 1000
                val mime = cursor.getString(4)

                val uri = DocumentsContract.buildDocumentUriUsingTree(tree, documentId)
                if (mime == DocumentsContract.Document.MIME_TYPE_DIR) {
                    walk(handle, uri, now, journalFile)
                    continue
                }
                val ext = name.substringAfterLast('.', "").lowercase()
                val id = uri.toString()

                if (Native.plan(handle, id, name, size, mtime, ext, now) != Native.PLAN_EXAMINE) {
                    continue
                }
                val data = resolver.openInputStream(uri)?.use { it.readBytes() } ?: continue
                val cleaned = Native.resolve(handle, id, name, size, mtime, ext, data)

                var wroteBytes = -1L
                var wroteMtime = 0L
                if (cleaned.isNotEmpty()) {
                    // "wt" truncates, so the document keeps its identity and its
                    // place in the gallery rather than becoming a new file.
                    resolver.openOutputStream(uri, "wt")?.use { it.write(cleaned) }
                    resolver.query(uri, columns, null, null, null)?.use { after ->
                        if (after.moveToFirst()) {
                            wroteBytes = after.getLong(2)
                            wroteMtime = after.getLong(3) / 1000
                        }
                    }
                }
                Native.record(
                    handle, id, name, size, mtime, ext, wroteBytes, wroteMtime, now,
                )
                // After every file: this worker will be killed partway.
                journalFile.writeText(Native.journal(handle))
            }
        }
    }

    private fun charging(): Boolean {
        val manager = applicationContext.getSystemService(BatteryManager::class.java)
        return manager?.isCharging ?: false
    }

    private fun battery(): Int {
        val manager = applicationContext.getSystemService(BatteryManager::class.java)
        return manager?.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY) ?: 100
    }
}
