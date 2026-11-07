package dev.figura.munda

/**
 * The Rust runner. See mobile/runner/src/android.rs.
 *
 * NOT COMPILED OR RUN. This is the declared interface; nothing here has been
 * built against a real NDK.
 *
 * A session holds the profile and the journal across calls so that a sweep over
 * a thousand photographs does not re-parse its own bookkeeping a thousand
 * times. [close] must be called exactly once, which [Sweep] does in a finally.
 */
internal object Native {
    init {
        System.loadLibrary("fm_runner")
    }

    /** Codes from [plan]. Mirrored in android.rs. */
    const val PLAN_EXAMINE = 0
    const val PLAN_SETTLING = 1
    const val PLAN_DONE = 2
    const val PLAN_GIVEN_UP = 3

    external fun open(profileToml: String, journalJson: String): Long

    /** Empty means a sweep is owed; anything else is the sentence to show. */
    external fun due(handle: Long, charging: Int, battery: Int, now: Long): String

    /** Whether this file needs reading at all, from its metadata alone. */
    external fun plan(
        handle: Long,
        id: String,
        name: String,
        bytes: Long,
        mtime: Long,
        ext: String,
        now: Long,
    ): Int

    /** The cleaned bytes, or empty when there is nothing to write. */
    external fun resolve(
        handle: Long,
        id: String,
        name: String,
        bytes: Long,
        mtime: Long,
        ext: String,
        data: ByteArray,
    ): ByteArray

    /** Why [resolve] produced nothing. Empty when it produced bytes. */
    external fun outcome(handle: Long): String

    /** What the host actually wrote. Pass -1 for wroteBytes if it wrote nothing. */
    external fun record(
        handle: Long,
        id: String,
        name: String,
        bytes: Long,
        mtime: Long,
        ext: String,
        wroteBytes: Long,
        wroteMtime: Long,
        now: Long,
    )

    /** The journal as JSON, to persist after every file. */
    external fun journal(handle: Long): String

    /** Stamp the sweep and forget files no longer listed. */
    external fun finish(handle: Long, now: Long)

    /** The line for the notification. */
    external fun report(handle: Long): String

    external fun close(handle: Long)
}
