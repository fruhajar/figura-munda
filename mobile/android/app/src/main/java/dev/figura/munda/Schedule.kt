package dev.figura.munda

import android.content.Context
import android.net.Uri
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import java.io.File
import java.util.concurrent.TimeUnit

/**
 * Scheduling, the folder grants and the profile file.
 *
 * NOT COMPILED OR RUN. See mobile/android/README.md.
 */
object Schedule {
    private const val NAME = "figura-munda-sweep"

    /**
     * Ask for a sweep every [hours].
     *
     * The period is a request, not a promise: under Doze the system runs this
     * when it suits, which is why the runner checks for itself whether a sweep
     * is owed instead of assuming it was woken on time. The battery constraint
     * here is belt to the runner's braces, because `WorkManager` and the runner
     * disagree about what "low" means and the stricter answer should win.
     */
    fun every(context: Context, hours: Long) {
        val constraints = Constraints.Builder()
            .setRequiresBatteryNotLow(true)
            .build()
        val request = PeriodicWorkRequestBuilder<SweepWorker>(hours, TimeUnit.HOURS)
            .setConstraints(constraints)
            .build()
        WorkManager.getInstance(context).enqueueUniquePeriodicWork(
            NAME,
            ExistingPeriodicWorkPolicy.UPDATE,
            request,
        )
    }

    fun stop(context: Context) {
        WorkManager.getInstance(context).cancelUniqueWork(NAME)
    }
}

/**
 * The folders the user has granted, which outlive a reboot only because the
 * grant is taken persistably at the moment it is given.
 */
object Grants {
    fun remember(context: Context, tree: Uri) {
        context.contentResolver.takePersistableUriPermission(
            tree,
            android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION or
                android.content.Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
        )
    }

    fun folders(context: Context): List<Uri> =
        context.contentResolver.persistedUriPermissions
            .filter { it.isReadPermission && it.isWritePermission }
            .map { it.uri }

    fun forget(context: Context, tree: Uri) {
        context.contentResolver.releasePersistableUriPermission(
            tree,
            android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION or
                android.content.Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
        )
    }
}

/**
 * The profile file, which is exactly the file the desktop writes. One profile
 * can be carried between a desktop and a phone unchanged.
 */
object Profiles {
    fun file(context: Context) = File(context.filesDir, "profile.toml")

    fun read(context: Context): String? = file(context).takeIf { it.exists() }?.readText()

    fun write(context: Context, toml: String) = file(context).writeText(toml)
}
