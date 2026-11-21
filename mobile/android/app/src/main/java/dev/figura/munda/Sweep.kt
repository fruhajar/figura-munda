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
 *
 * And scoped storage has no atomic replace, so the original is written aside
 * before the document is truncated. See [rescue].
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
            // A sweep killed mid-write left a truncated document behind, so
            // anything written aside last time goes back before this one
            // decides anything.
            restorePending()
            // The boundary is integers, so the flag crosses as 0 or 1.
            val declined = Native.due(handle, if (charging()) 1 else 0, battery(), now)
            if (declined.isNotEmpty()) {
                Notices.show(applicationContext, declined)
                return Result.success()
            }
            for (root in roots) {
                walk(handle, root, DocumentsContract.getTreeDocumentId(root), now, journalFile)
            }
            Native.finish(handle, now)
            journalFile.writeText(Native.journal(handle))
            Notices.show(applicationContext, Native.report(handle))
            return Result.success()
        } finally {
            Native.close(handle)
        }
    }

    /**
     * Every document under one granted tree.
     *
     * The tree and the document to list are separate arguments on purpose.
     * `getTreeDocumentId` answers with the tree's own root whatever document it
     * is handed, so recursing with a subfolder's URI listed the root again and
     * the sweep walked the same folder until the system killed it, cleaning the
     * same files over and over on the way. The depth cap is for a provider that
     * reports a cycle.
     */
    private fun walk(
        handle: Long,
        tree: Uri,
        documentId: String,
        now: Long,
        journalFile: File,
        depth: Int = 0,
    ) {
        if (depth > MAX_DEPTH) return
        val resolver = applicationContext.contentResolver
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, documentId)
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
                    walk(handle, tree, documentId, now, journalFile, depth + 1)
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
                // No copy of the original put aside, no write.
                val saved = cleaned.isNotEmpty() && rescue(id, data)
                if (saved) {
                    // "wt" truncates, so the document keeps its identity and its
                    // place in the gallery rather than becoming a new file. It
                    // also means the original is gone the moment this starts,
                    // which is what the rescue copy above is for.
                    try {
                        resolver.openOutputStream(uri, "wt")?.use { it.write(cleaned) }
                        resolver.query(uri, columns, null, null, null)?.use { after ->
                            if (after.moveToFirst()) {
                                wroteBytes = after.getLong(2)
                                wroteMtime = after.getLong(3) / 1000
                            }
                        }
                        forgetRescue()
                    } catch (e: Exception) {
                        // Put the original back before anything else happens,
                        // and leave the rescue copy if even that fails so the
                        // next sweep can try again.
                        try {
                            resolver.openOutputStream(uri, "wt")?.use { it.write(data) }
                            forgetRescue()
                        } catch (restoreFailed: Exception) {
                            Notices.show(
                                applicationContext,
                                "$name could not be written and could not be put back. " +
                                    "The original is saved inside the app until the next sweep.",
                            )
                        }
                        wroteBytes = -1L
                        wroteMtime = 0L
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

    /**
     * The original, written aside before the document is truncated.
     *
     * Scoped storage offers no atomic replace: the bytes are gone from the
     * moment `"wt"` opens the stream, and this worker is killed partway as a
     * matter of course. One copy is enough because a sweep writes one file at a
     * time, and the marker says which document it belongs to.
     *
     * It is a copy of an uncleaned file inside app-private storage, which is
     * exactly the thing this tool exists to reduce, so it is deleted as soon as
     * the write is known to have finished. `res/xml/data_extraction_rules.xml`
     * excludes the whole file domain from cloud backup and device transfer, so
     * it does not leave the device while it is there.
     *
     * Returns whether the copy was made: no copy, no write.
     */
    private fun rescue(id: String, original: ByteArray): Boolean {
        return try {
            rescueBytes().writeBytes(original)
            rescueMarker().writeText(id)
            true
        } catch (e: Exception) {
            Notices.show(
                applicationContext,
                "There is no room to save a copy before cleaning, so nothing was changed.",
            )
            false
        }
    }

    private fun rescueDir(): File =
        File(applicationContext.filesDir, "rescue").apply { mkdirs() }

    private fun rescueMarker(): File = File(rescueDir(), "pending.uri")

    private fun rescueBytes(): File = File(rescueDir(), "pending.bin")

    private fun forgetRescue() {
        rescueMarker().delete()
        rescueBytes().delete()
    }

    /**
     * Put back whatever the last sweep was in the middle of writing.
     *
     * Restoring is the safe answer even when the write had in fact finished:
     * the user keeps their file, and the next sweep cleans it again. Losing it
     * is not recoverable, and a sweep that runs twice is.
     */
    private fun restorePending() {
        val marker = rescueMarker()
        val bytes = rescueBytes()
        if (!marker.exists() || !bytes.exists()) return
        val uri = Uri.parse(marker.readText())
        try {
            applicationContext.contentResolver
                .openOutputStream(uri, "wt")
                ?.use { it.write(bytes.readBytes()) }
        } catch (e: Exception) {
            // Keep the copy rather than delete the only remaining original.
            Notices.show(
                applicationContext,
                "A file from the last sweep could not be put back yet. It is still saved " +
                    "inside the app.",
            )
            return
        }
        forgetRescue()
    }

    private fun charging(): Boolean {
        val manager = applicationContext.getSystemService(BatteryManager::class.java)
        return manager?.isCharging ?: false
    }

    private fun battery(): Int {
        val manager = applicationContext.getSystemService(BatteryManager::class.java)
        return manager?.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY) ?: 100
    }

    private companion object {
        /** Deep enough for any real gallery, shallow enough to stop a cycle. */
        const val MAX_DEPTH = 32
    }
}
