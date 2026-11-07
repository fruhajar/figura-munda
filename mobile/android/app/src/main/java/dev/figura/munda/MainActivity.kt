package dev.figura.munda

import android.app.Activity
import android.content.Intent
import android.os.Bundle

/**
 * Settings: which folders, which profile, how often.
 *
 * NOT COMPILED OR RUN, and deliberately a skeleton. The layout is a product
 * question that should be answered on a device rather than guessed at here; what
 * this file pins down is the flow, because the flow has consequences that the
 * layout does not.
 *
 * Two of them:
 *
 * The folder grant has to be taken persistably at the moment it is given, or it
 * does not survive a reboot and the runner silently stops working. That is
 * [Grants.remember], called from the picker result and nowhere else.
 *
 * Nothing is scheduled until a folder exists. A periodic worker that wakes every
 * six hours to discover it has no folders is a battery cost with no benefit.
 */
class MainActivity : Activity() {
    private val pickFolder = 1

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        // The settings UI goes here. Until it does, the pieces it drives are
        // reachable and tested from the Rust side.
    }

    /** Ask for a folder. The result is granted for good, or not at all. */
    fun chooseFolder() {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).apply {
            addFlags(
                Intent.FLAG_GRANT_READ_URI_PERMISSION or
                    Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
                    Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION
            )
        }
        startActivityForResult(intent, pickFolder)
    }

    override fun onActivityResult(request: Int, result: Int, data: Intent?) {
        super.onActivityResult(request, result, data)
        if (request != pickFolder || result != RESULT_OK) return
        val tree = data?.data ?: return
        Grants.remember(this, tree)
        // Only now is there anything for a sweep to do.
        Schedule.every(this, hours = 6)
    }

    /** Stop sweeping and give the folder back. */
    fun forgetEverything() {
        Grants.folders(this).forEach { Grants.forget(this, it) }
        Schedule.stop(this)
    }
}
