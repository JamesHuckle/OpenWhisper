package com.openwhisper.android.audio

import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import com.openwhisper.android.recordings.RecordingStore
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlin.concurrent.thread
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
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

        var saved: PcmRecording? = null
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
        val microphone = FakeAudioRecord(stopBlocks = false, readBlocks = true)
        val freed = AtomicInteger()
        val recording = MicrophoneRecording(microphone, store.beginPcm("A", 16_000), 8_192, null) {
            freed.incrementAndGet()
        }
        recording.startReading()
        assertTrue(microphone.readEntered.await(5, TimeUnit.SECONDS))

        var saved: PcmRecording? = null
        val stopping = thread { saved = recording.stop() }
        // The stop is waiting for the read thread, which is stuck in a read.
        awaitState(stopping, Thread.State.TIMED_WAITING)
        stopping.interrupt()
        stopping.join(5_000)
        microphone.unblockRead.countDown()

        assertFalse(stopping.isAlive)
        assertEquals(1, microphone.releaseCount.get())
        assertEquals(1, freed.get())
        assertEquals("saved", store.get(saved!!.id)?.status)
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

/** The microphone, with reads that return nothing and a stop that can be held. */
private class FakeAudioRecord(
    private val stopBlocks: Boolean,
    private val readBlocks: Boolean = false,
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

    override fun read(audioData: ByteArray, offsetInBytes: Int, sizeInBytes: Int, readMode: Int): Int {
        readEntered.countDown()
        if (readBlocks) unblockRead.await(5, TimeUnit.SECONDS)
        Thread.sleep(5)
        return 0
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
