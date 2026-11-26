package com.openwhisper.android.audio

import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import com.openwhisper.android.recordings.PcmRecordingWriter
import com.openwhisper.android.recordings.RecordingStore
import com.openwhisper.android.recordings.StoredRecording
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import kotlin.concurrent.thread
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Assume.assumeTrue
import org.junit.Test

class AndroidAudioRecorderTest {
    @Test
    fun aCancelDuringAStopCleansUpOnceAndWaitsForTheMicrophone() {
        val store = store()
        val microphone = FakeAudioRecord(stopBlocks = true)
        val freed = AtomicInteger()
        val recording = MicrophoneRecording(
            microphone,
            store.beginPcm("A", 16_000),
            bufferSize = 8_192,
            onAmplitude = null,
        ) { freed.incrementAndGet() }
        recording.startReading()

        var saved: StoredRecording? = null
        val stopping = thread { saved = recording.stop() }
        assertTrue(microphone.stopEntered.await(5, TimeUnit.SECONDS))
        val cancelling = thread { recording.cancel() }

        // The cancel waits while the stop still holds the microphone.
        awaitState(cancelling, Thread.State.TIMED_WAITING)
        assertEquals(0, freed.get())

        microphone.unblockStop.countDown()
        stopping.join(5_000)
        cancelling.join(5_000)

        assertFalse(cancelling.isAlive)
        assertEquals(1, microphone.releaseCount.get())
        assertEquals(1, freed.get())
        assertEquals("saved", store.get(saved!!.id)?.status)
    }

    @Test
    fun anInterruptedStopStillFreesTheMicrophoneAndSavesTheFile() {
        val store = store()
        // The stuck read returns samples only after the stop has closed the file.
        val microphone = FakeAudioRecord(stopBlocks = false, readBlocks = true, bytesRead = 2_048)
        val freed = AtomicInteger()
        val recording = MicrophoneRecording(microphone, store.beginPcm("A", 16_000), 8_192, null) {
            freed.incrementAndGet()
        }
        recording.startReading()
        assertTrue(microphone.readEntered.await(5, TimeUnit.SECONDS))
        val reader = microphone.reader!!
        val readerFailure = AtomicReference<Throwable?>(null)
        reader.setUncaughtExceptionHandler { _, error -> readerFailure.set(error) }

        var saved: StoredRecording? = null
        var interruptKept = false
        val stopping = thread {
            saved = recording.stop()
            interruptKept = Thread.currentThread().isInterrupted
        }
        // The stop is waiting for the read thread, which is stuck in a read.
        awaitState(stopping, Thread.State.TIMED_WAITING)
        stopping.interrupt()
        stopping.join(5_000)
        microphone.unblockRead.countDown()
        reader.join(5_000)

        assertFalse(stopping.isAlive)
        assertTrue(interruptKept)
        assertFalse(reader.isAlive)
        assertNull(readerFailure.get())
        assertEquals(1, microphone.releaseCount.get())
        assertEquals(1, freed.get())
        assertEquals("saved", store.get(saved!!.id)?.status)
    }

    @Test
    fun aFailedWriteEndsTheRecordingWithoutCrashingTheApp() {
        // Every write to /dev/full fails as a full phone's storage does.
        val fullStorage = File("/dev/full")
        assumeTrue(fullStorage.canWrite())
        // The read is held until the handler is in place, so the failing write cannot run first.
        val microphone = FakeAudioRecord(stopBlocks = false, readBlocks = true, bytesRead = 2_048)
        val freed = AtomicInteger()
        val sink = PcmRecordingWriter(store(), "full", fullStorage, 16_000)
        val recording = MicrophoneRecording(microphone, sink, 8_192, null) { freed.incrementAndGet() }
        recording.startReading()
        assertTrue(microphone.readEntered.await(5, TimeUnit.SECONDS))
        val reader = microphone.reader!!
        val readerFailure = AtomicReference<Throwable?>(null)
        reader.setUncaughtExceptionHandler { _, error -> readerFailure.set(error) }
        microphone.unblockRead.countDown()
        reader.join(5_000)

        assertFalse(reader.isAlive)
        assertNull(readerFailure.get())
        try {
            recording.stop()
            fail("A recording that could not be written was stopped as saved")
        } catch (error: IllegalStateException) {
            // Saving the file fails too, so it must not claim the audio was saved.
            assertEquals("Could not save the rest of the recording", error.message)
        }
        assertEquals(1, microphone.releaseCount.get())
        assertEquals(1, freed.get())
    }

    @Test
    fun aWriteThatFailsDuringAStopIsReportedNotSaved() {
        val fullStorage = File("/dev/full")
        assumeTrue(fullStorage.canWrite())
        // The read returns only once the stop has ended capturing, so its write is the last.
        val microphone = FakeAudioRecord(stopBlocks = false, readBlocks = true, bytesRead = 2_048)
        val sink = PcmRecordingWriter(store(), "full", fullStorage, 16_000)
        val recording = MicrophoneRecording(microphone, sink, 8_192, null) {}
        recording.startReading()
        assertTrue(microphone.readEntered.await(5, TimeUnit.SECONDS))

        val stopFailure = AtomicReference<Throwable?>(null)
        val stopping = thread {
            try {
                recording.stop()
            } catch (error: Throwable) {
                stopFailure.set(error)
            }
        }
        // The stop is waiting for the read thread, which is stuck in a read.
        awaitState(stopping, Thread.State.TIMED_WAITING)
        microphone.unblockRead.countDown()
        stopping.join(5_000)

        assertFalse(stopping.isAlive)
        val failure = stopFailure.get()
        assertTrue("The failed last write was hidden: $failure", failure is IllegalStateException)
        assertEquals("Could not save the rest of the recording", failure!!.message)
    }

    @Test
    fun aCancelledRecordingCannotBeStoppedLater() {
        val store = store()
        val microphone = FakeAudioRecord(stopBlocks = false)
        val freed = AtomicInteger()
        val sink = store.beginPcm("A", 16_000)
        val recording = MicrophoneRecording(microphone, sink, 8_192, null) { freed.incrementAndGet() }
        recording.startReading()

        recording.cancel()
        try {
            recording.stop()
            fail("A cancelled recording was stopped")
        } catch (error: IllegalStateException) {
            assertEquals("Microphone was not recording", error.message)
        }
        recording.cancel()

        assertEquals(1, microphone.releaseCount.get())
        assertEquals(1, freed.get())
        assertEquals("cancelled", store.list().single().status)
    }
}

/** The microphone, with reads that return [bytesRead] and a stop that can be held. */
private class FakeAudioRecord(
    private val stopBlocks: Boolean,
    private val readBlocks: Boolean = false,
    private val bytesRead: Int = 0,
) : AudioRecord(
    MediaRecorder.AudioSource.MIC,
    16_000,
    AudioFormat.CHANNEL_IN_MONO,
    AudioFormat.ENCODING_PCM_16BIT,
    8_192,
) {
    val stopEntered = CountDownLatch(1)
    val unblockStop = CountDownLatch(1)
    val readEntered = CountDownLatch(1)
    val unblockRead = CountDownLatch(1)
    val releaseCount = AtomicInteger()
    @Volatile
    var reader: Thread? = null

    override fun read(audioData: ByteArray, offsetInBytes: Int, sizeInBytes: Int, readMode: Int): Int {
        reader = Thread.currentThread()
        readEntered.countDown()
        if (readBlocks) unblockRead.await(5, TimeUnit.SECONDS)
        Thread.sleep(5)
        return bytesRead
    }

    override fun stop() {
        stopEntered.countDown()
        if (stopBlocks) unblockStop.await(5, TimeUnit.SECONDS)
    }

    override fun release() {
        releaseCount.incrementAndGet()
    }
}

private fun store() = RecordingStore.forDirectory(
    File.createTempFile("openwhisper-recorder-", "").apply {
        delete()
        mkdirs()
        deleteOnExit()
    },
)

/** Waits up to 5 s for [thread] to reach [state]. */
private fun awaitState(thread: Thread, state: Thread.State) {
    val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5)
    while (thread.state != state && thread.isAlive && System.nanoTime() < deadline) Thread.sleep(5)
    assertEquals(state, thread.state)
}
