package com.openwhisper.android.update

import java.util.concurrent.atomic.AtomicBoolean

/**
 * The automatic update check: at most one attempt a day and one at a time, run
 * in the background. The attempt is recorded before fetching, so a failed
 * attempt also waits a day.
 */
internal class DailyUpdateCheck(
    private val currentTimeMillis: () -> Long,
    private val lastAttemptMillis: () -> Long,
    private val recordAttemptMillis: (Long) -> Unit,
    private val findUpdate: () -> UpdateManifest?,
    private val showUpdate: (UpdateManifest) -> Unit,
    private val runInBackground: (Runnable) -> Unit,
) {
    private val checking = AtomicBoolean(false)

    fun runIfDue() {
        val now = currentTimeMillis()
        if (now - lastAttemptMillis() < CHECK_INTERVAL_MS) return
        if (!checking.compareAndSet(false, true)) return
        recordAttemptMillis(now)
        runInBackground {
            try {
                findUpdate()?.let(showUpdate)
            } finally {
                checking.set(false)
            }
        }
    }

    companion object {
        const val CHECK_INTERVAL_MS = 24L * 60L * 60L * 1000L
    }
}
