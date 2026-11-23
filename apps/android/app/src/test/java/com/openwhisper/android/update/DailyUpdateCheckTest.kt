package com.openwhisper.android.update

import com.openwhisper.android.update.DailyUpdateCheck.Companion.CHECK_INTERVAL_MS
import java.io.IOException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class DailyUpdateCheckTest {
    private var now = CHECK_INTERVAL_MS
    private var lastAttempt = 0L
    private val fetches = mutableListOf<() -> UpdateManifest?>()
    private var fetchCount = 0
    private val shown = mutableListOf<UpdateManifest>()
    private val waiting = mutableListOf<Runnable>()
    private val logged = mutableListOf<String>()

    private val check = DailyUpdateCheck(
        currentTimeMillis = { now },
        lastAttemptMillis = { lastAttempt },
        recordAttemptMillis = { lastAttempt = it },
        findUpdate = { fetches[fetchCount++]() },
        showUpdate = { shown += it },
        runInBackground = { waiting += it },
        logFailure = { logged += it },
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

    @Test
    fun aFailedFetchAndABadManifestDoNotEscapeAndALaterCheckStillShowsTheUpdate() {
        fetches += { throw IOException("Unable to resolve host") }
        fetches += { UpdateManifestCodec.parse("<html>private response body</html>") }
        fetches += { throw IllegalArgumentException("Update server returned HTTP 503") }
        fetches += { update(31) }

        repeat(4) { attempt ->
            if (attempt > 0) now += CHECK_INTERVAL_MS
            check.runIfDue()
            runWaiting()
        }

        assertEquals(4, fetchCount)
        assertEquals(listOf(update(31)), shown)
        assertEquals(
            listOf(
                "Automatic update check failed: IOException: Unable to resolve host",
                "Automatic update check failed: JSONException",
                "Automatic update check failed: IllegalArgumentException: Update server returned HTTP 503",
            ),
            logged,
        )
        assertFalse(logged.any { "private response body" in it })
    }

    @Test
    fun aFailedFetchStillWaitsADayBeforeTheNextAttempt() {
        fetches += { throw IOException("timeout") }
        fetches += { update(31) }

        check.runIfDue()
        runWaiting()
        now += CHECK_INTERVAL_MS - 1
        check.runIfDue()
        runWaiting()

        assertEquals(1, fetchCount)
        assertEquals(emptyList<UpdateManifest>(), shown)
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
