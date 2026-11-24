package com.openwhisper.android.update

import java.util.concurrent.atomic.AtomicBoolean
import org.json.JSONException

/**
 * The automatic update check: at most one attempt a day and one at a time, run
 * in the background. The attempt is recorded before fetching, so a failed
 * attempt also waits a day. A failure (no network, an HTTP error, a bad
 * manifest) is logged and dropped: it runs in the dictation service's process,
 * and an exception escaping the background thread would kill that process.
 */
internal class DailyUpdateCheck(
    private val currentTimeMillis: () -> Long,
    private val lastAttemptMillis: () -> Long,
    private val recordAttemptMillis: (Long) -> Unit,
    private val findUpdate: () -> UpdateManifest?,
    private val showUpdate: (UpdateManifest) -> Unit,
    private val runInBackground: (Runnable) -> Unit,
    private val logFailure: (String) -> Unit,
) {
    private val checking = AtomicBoolean(false)

    fun runIfDue() {
        val now = currentTimeMillis()
        if (!checking.compareAndSet(false, true)) return
        // Read while holding the flag, so a check that just finished is seen.
        if (now - lastAttemptMillis() < CHECK_INTERVAL_MS) {
            checking.set(false)
            return
        }
        recordAttemptMillis(now)
        runInBackground {
            try {
                findUpdate()?.let(showUpdate)
            } catch (error: Exception) {
                logFailure(describeFailure(error))
            } finally {
                checking.set(false)
            }
        }
    }

    companion object {
        const val CHECK_INTERVAL_MS = 24L * 60L * 60L * 1000L

        /** One short line: the error class and message, never the response body. */
        fun describeFailure(error: Exception): String {
            val name = error.javaClass.simpleName
            // A JSON parse error quotes the text it could not read, which is the response.
            if (error is JSONException) return "Automatic update check failed: $name"
            return "Automatic update check failed: $name: ${error.message.orEmpty()}"
                .lineSequence().first().take(MAX_LOG_LENGTH)
        }

        private const val MAX_LOG_LENGTH = 200
    }
}
