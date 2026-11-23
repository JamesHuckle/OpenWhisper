package com.openwhisper.android.update

import com.openwhisper.android.update.DailyUpdateCheck.Companion.CHECK_INTERVAL_MS
import org.junit.Assert.assertEquals
import org.junit.Test

class DailyUpdateCheckTest {
    private var now = CHECK_INTERVAL_MS
    private var lastAttempt = 0L
    private val fetches = mutableListOf<() -> UpdateManifest?>()
    private var fetchCount = 0
    private val shown = mutableListOf<UpdateManifest>()
    private val waiting = mutableListOf<Runnable>()

    private val check = DailyUpdateCheck(
        currentTimeMillis = { now },
        lastAttemptMillis = { lastAttempt },
        recordAttemptMillis = { lastAttempt = it },
        findUpdate = { fetches[fetchCount++]() },
        showUpdate = { shown += it },
        runInBackground = { waiting += it },
    )

    @Test
    fun checksAtMostOncePerDayAndShowsTheUpdate() {
        fetches += { update(30) }
        fetches += { null }

        check.runIfDue()
        runWaiting()
        now += CHECK_INTERVAL_MS - 1
        check.runIfDue()
        runWaiting()

        assertEquals(1, fetchCount)
        assertEquals(listOf(update(30)), shown)

        now += 1
        check.runIfDue()
        runWaiting()

        assertEquals(2, fetchCount)
        assertEquals(CHECK_INTERVAL_MS * 2, lastAttempt)
    }

    @Test
    fun startsNoSecondCheckWhileOneIsRunning() {
        fetches += { null }

        check.runIfDue()
        // Forget the recorded attempt so only the running check can stop the second one.
        lastAttempt = 0
        check.runIfDue()

        assertEquals(1, waiting.size)
    }

    private fun runWaiting() {
        val tasks = waiting.toList()
        waiting.clear()
        tasks.forEach(Runnable::run)
    }

    private fun update(versionCode: Int) = UpdateManifest(
        versionCode = versionCode,
        versionName = "0.$versionCode.0",
        apkUrl = "https://github.com/example/update.apk",
        sha256 = "a".repeat(64),
        releaseNotes = "",
    )
}
