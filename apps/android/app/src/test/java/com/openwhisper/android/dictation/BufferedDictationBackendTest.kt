package com.openwhisper.android.dictation

import com.openwhisper.android.audio.PcmRecording
import com.openwhisper.android.audio.PcmRecorder
import com.openwhisper.android.transcription.TranscriptionClient
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class BufferedDictationBackendTest {
    @Test
    fun finalizesPcmAsWavAndReturnsTranscript() {
        val recorder = FakePcmRecorder(PcmRecording(byteArrayOf(1, 2, 3, 4), 16_000))
        val client = FakeTranscriptionClient("Hello")
        val runner = QueuedRunner()
        val listener = CapturingListener()
        val backend = BufferedDictationBackend(recorder, client, runner, DirectDispatcher)

        backend.start(listener)
        backend.finish()
        assertNull(listener.final)

        runner.runNext()

        assertEquals(1, recorder.startCount)
        assertEquals(1, recorder.stopCount)
        assertEquals("RIFF", client.lastWav?.copyOfRange(0, 4)?.decodeToString())
        assertEquals("Hello", listener.final)
    }

    @Test
    fun cancelledGenerationCannotDeliverLateTranscript() {
        val recorder = FakePcmRecorder(PcmRecording(byteArrayOf(1, 2), 16_000))
        val client = FakeTranscriptionClient("late")
        val runner = QueuedRunner()
        val listener = CapturingListener()
        val backend = BufferedDictationBackend(recorder, client, runner, DirectDispatcher)

        backend.start(listener)
        backend.finish()
        backend.cancel()
        runner.runNext()

        assertEquals(1, recorder.cancelCount)
        assertNull(listener.final)
        assertNull(listener.error)
    }

    @Test
    fun recorderAndNetworkErrorsAreRecoverableMessages() {
        val recorder = FakePcmRecorder(PcmRecording(byteArrayOf(), 16_000), stopError = "No audio")
        val runner = QueuedRunner()
        val listener = CapturingListener()
        val backend = BufferedDictationBackend(
            recorder,
            FakeTranscriptionClient("unused"),
            runner,
            DirectDispatcher,
        )

        backend.start(listener)
        backend.finish()
        runner.runNext()

        assertEquals("No audio", listener.error)
    }
}

private class FakePcmRecorder(
    private val recording: PcmRecording,
    private val stopError: String? = null,
) : PcmRecorder {
    var startCount = 0
    var stopCount = 0
    var cancelCount = 0

    override fun start() {
        startCount += 1
    }

    override fun stop(): PcmRecording {
        stopCount += 1
        stopError?.let { throw IllegalStateException(it) }
        return recording
    }

    override fun cancel() {
        cancelCount += 1
    }
}

private class FakeTranscriptionClient(private val response: String) : TranscriptionClient {
    var lastWav: ByteArray? = null

    override fun transcribe(wav: ByteArray): String {
        lastWav = wav
        return response
    }
}

private class QueuedRunner : BackgroundRunner {
    private val tasks = ArrayDeque<() -> Unit>()
    override fun run(task: () -> Unit) {
        tasks.add(task)
    }

    fun runNext() = tasks.removeFirst().invoke()
}

private data object DirectDispatcher : CallbackDispatcher {
    override fun dispatch(task: () -> Unit) = task()
}

private class CapturingListener : DictationBackend.Listener {
    var final: String? = null
    var error: String? = null

    override fun onPartial(text: String) = Unit
    override fun onFinal(text: String) {
        final = text
    }

    override fun onError(message: String) {
        error = message
    }
}
