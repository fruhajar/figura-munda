package dev.figura.munda

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import androidx.core.app.NotificationCompat

/**
 * The one line a sweep says for itself.
 *
 * NOT COMPILED OR RUN. See README.md in this folder.
 *
 * A background tool that never speaks is indistinguishable from a background
 * tool that has died, which is a bad property for something whose whole job is
 * invisible. So a sweep says what it did, including when the answer is that it
 * declined to run and why.
 *
 * The text comes from the Rust side, so the phone and the desktop say the same
 * words about the same work.
 */
object Notices {
    private const val CHANNEL = "sweeps"
    private const val ID = 1

    fun show(context: Context, text: String) {
        if (text.isEmpty()) return
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL,
                context.getString(R.string.channel_sweeps),
                // Low: this is a record, not an interruption.
                NotificationManager.IMPORTANCE_LOW,
            )
        )
        val notice = NotificationCompat.Builder(context, CHANNEL)
            .setContentTitle(context.getString(R.string.app_name))
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().setBigText(text))
            .setSmallIcon(android.R.drawable.ic_menu_view)
            .setOnlyAlertOnce(true)
            .build()
        manager.notify(ID, notice)
    }
}
