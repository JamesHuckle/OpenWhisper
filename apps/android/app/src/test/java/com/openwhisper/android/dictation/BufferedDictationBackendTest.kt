package com.openwhisper.android.dictation

import com.openwhisper.android.audio.ActiveRecording
import com.openwhisper.android.audio.PcmRecorder
import com.openwhisper.android.audio.WavEncoder
import com.openwhisper.android.recordings.RecordingStore
import com.openwhisper.android.recordings.StoredRecording
import com.openwhisper.android.transcription.AudioRules
import com.openwhisper.android.transcription.NoSpeechException
import com.openwhisper.android.transcription.TranscriptionAudio
import com.openwhisper.android.transcription.TranscriptionClient
import com.openwhisper.android.transcription.TranscriptionManager
import java.io.File
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class BufferedDictationBackendTest {
    private val root = File(
        System.getProperty("java.io.tmpdir"),
        "openwhisper-dictation-${System.nanoTime()}",
    )

    /** Holds the recordings, as the app's one store does for the recorder and the manager. */
    private val store = RecordingStore.forDirectory(root)

    @After
    fun cleanUp() {
        root.deleteRecursively()
    }

    /** A microphone recording of [bytes], saved in [store] as the real recorder saves it. */
    private fun pcmRecording(bytes: ByteArray): StoredRecording =
        store.beginPcm("Dictation", 16_000).apply { write(bytes, bytes.size) }.finish("saved")

    private fun manager(client: TranscriptionClient, runner: BackgroundRunner) = TranscriptionManager(
        store = store,
        newClient = { client },
        dictationRunner = runner,
        // A dictation never waits behind imports and retries.
        bulkRunner = BackgroundRunner { error("A dictation ran on the bulk thread") },
        callbacks = DirectDispatcher,
    )

    @Test
    fun finalizesPcmAsWavAndReturnsTranscript() {
        val recorder = FakePcmRecorder(pcmRecording(byteArrayOf(1, 2, 3, 4)))
        val client = FakeTranscriptionClient("Hello")
        val runner = QueuedRunner()
        val listener = CapturingListener()
        val backend = BufferedDictationBackend(recorder, manager(client, runner), runner, DirectDispatcher)

        backend.start(listener)
        backend.finish()
        assertNull(listener.final)

        runner.runAll()

        assertEquals(1, recorder.startCount)
        assertEquals(1, recorder.stopCount)
        assertEquals("RIFF", client.lastWav?.copyOfRange(0, 4)?.decodeToString())
        assertEquals("Hello", listener.final)
    }

    @Test
    fun aDictationIsSentAsItsSavedPcmWav() {
        val pcm = byteArrayOf(1, 2, 3, 4)
        val recorder = FakePcmRecorder(pcmRecording(pcm))
        val client = FakeTranscriptionClient("Hello")
        val runner = QueuedRunner()
        val backend = BufferedDictationBackend(recorder, manager(client, runner), runner, DirectDispatcher)

        backend.start(CapturingListener())
        backend.finish()
        runner.runAll()

        val audio = requireNotNull(client.lastAudio)
        assertEquals("audio/wav", audio.mimeType)
        assertEquals(16_000, audio.pcmSampleRate)
        assertEquals(WavEncoder.HEADER_BYTES.toLong(), audio.pcmDataOffset)
        assertEquals(pcm.size.toLong(), audio.pcmDataLength)
        assertArrayEquals(WavEncoder.encodePcm16Mono(pcm, 16_000), client.lastWav)
    }

    @Test
    fun cancelledGenerationCannotDeliverLateTranscript() {
        val recorder = FakePcmRecorder(pcmRecording(byteArrayOf(1, 2)))
        val client = FakeTranscriptionClient("late")
        val runner = QueuedRunner()
        val listener = CapturingListener()
        val backend = BufferedDictationBackend(recorder, manager(client, runner), runner, DirectDispatcher)

        backend.start(listener)
        backend.finish()
        backend.cancel()
        runner.runAll()

        assertEquals(1, recorder.cancelCount)
        assertNull(listener.final)
        assertNull(listener.error)
    }

    @Test
    fun aStopQueuedBeforeACancelDoesNotEndTheNextRecording() {
        val recorder = FakePcmRecorder(pcmRecording(byteArrayOf(1, 2)))
        val client = FakeTranscriptionClient("second")
        val runner = QueuedRunner()
        val first = CapturingListener()
        val second = CapturingListener()
        val backend = BufferedDictationBackend(recorder, manager(client, runner), runner, DirectDispatcher)

        // Recording A's stop is queued, then the user cancels and starts B
        // before the background thread runs it.
        backend.start(first)
        backend.finish()
        backend.cancel()
        backend.start(second)
        runner.runAll()

        val (recordingA, recordingB) = recorder.started
        assertEquals("cancelled", recordingA.ended)
        assertNull(recordingB.ended)
        assertNull(first.error)

        backend.finish()
        runner.runAll()

        assertEquals("stopped", recordingB.ended)
        assertEquals("second", second.final)
        assertNull(second.error)
        assertNull(first.final)
    }

    @Test
    fun cancelStopsTheUploadInProgress() {
        val recorder = FakePcmRecorder(pcmRecording(byteArrayOf(1, 2)))
        val client = FakeTranscriptionClient("late")
        val dictation = QueuedRunner()
        val uploads = QueuedRunner()
        val listener = CapturingListener()
        val backend =
            BufferedDictationBackend(recorder, manager(client, uploads), dictation, DirectDispatcher)

        backend.start(listener)
        backend.finish()
        // The recording has been handed to the transcription manager.
        dictation.runAll()
        backend.cancel()
        uploads.runAll()

        assertEquals(1, client.cancelCount)
        assertNull(client.lastWav)
        assertNull(listener.final)
        assertNull(listener.error)
    }

    @Test
    fun aCancelWhileTheMicrophoneStopsSendsNoAudio() {
        val recorder = FakePcmRecorder(pcmRecording(byteArrayOf(1, 2)))
        val client = FakeTranscriptionClient("late")
        val dictation = QueuedRunner()
        val listener = CapturingListener()
        lateinit var backend: BufferedDictationBackend
        // The microphone stops cleanly, then the user cancels before the
        // recording is handed over. Uploads start the moment they are queued.
        recorder.onStop = { backend.cancel() }
        val uploads = BackgroundRunner { it() }
        backend = BufferedDictationBackend(recorder, manager(client, uploads), dictation, DirectDispatcher)

        backend.start(listener)
        backend.finish()
        dictation.runAll()

        assertNull(client.lastWav)
        assertNull(listener.final)
        assertNull(listener.error)
    }

    @Test
    fun recorderAndNetworkErrorsAreRecoverableMessages() {
        val recorder = FakePcmRecorder(pcmRecording(byteArrayOf()), stopError = "No audio")
        val runner = QueuedRunner()
        val listener = CapturingListener()
        val backend = BufferedDictationBackend(
            recorder,
            manager(FakeTranscriptionClient("unused"), runner),
            runner,
            DirectDispatcher,
        )

        backend.start(listener)
        backend.finish()
        runner.runAll()

        assertEquals("No audio. Raw audio is available in OpenWhisper.", listener.error)
    }

    @Test
    fun aRecordingWithoutSpeechShowsNoAudioFoundLikeTheDesktop() {
        val recorder = FakePcmRecorder(pcmRecording(byteArrayOf(1, 2)))
        val client = FakeTranscriptionClient("unused", error = NoSpeechException("too_short"))
        val runner = QueuedRunner()
        val listener = CapturingListener()
        val backend = BufferedDictationBackend(recorder, manager(client, runner), runner, DirectDispatcher)

        backend.start(listener)
        backend.finish()
        runner.runAll()

        assertEquals("No audio found", listener.error)
        assertEquals(AudioRules.NO_AUDIO_MESSAGE, listener.error)
        assertNull(listener.final)
    }

    @Test
    fun aRecordingWithNoBytesIsNotSentForTranscription() {
        val recorder = FakePcmRecorder(pcmRecording(byteArrayOf()))
        val client = FakeTranscriptionClient("unused")
        val runner = QueuedRunner()
        val listener = CapturingListener()
        val backend = BufferedDictationBackend(recorder, manager(client, runner), runner, DirectDispatcher)

        backend.start(listener)
        backend.finish()
        runner.runAll()

        assertNull(client.lastWav)
        assertEquals(AudioRules.NO_AUDIO_MESSAGE, listener.error)
    }
}

private class FakePcmRecorder(
    private val recording: StoredRecording,
    private val stopError: String? = null,
) : PcmRecorder {
    val started = mutableListOf<FakeRecording>()
    val startCount get() = started.size
    var stopCount = 0
    var cancelCount = 0
    var onStop: () -> Unit = {}

    override fun start() = FakeRecording().also(started::add)

    /** Ends once, like the real recorder: a later stop throws, a later cancel does nothing. */
    inner class FakeRecording : ActiveRecording {
        var ended: String? = null

        override fun stop(): StoredRecording {
            stopCount += 1
            check(ended == null) { "Microphone was not recording" }
            ended = "stopped"
            stopError?.let { throw IllegalStateException(it) }
            onStop()
            return recording
        }

        override fun cancel() {
            if (ended != null) return
            ended = "cancelled"
            cancelCount += 1
        }
    }
}

private class FakeTranscriptionClient(
    private val response: String,
    private val error: Exception? = null,
) : TranscriptionClient {
    var lastWav: ByteArray? = null
    var lastAudio: TranscriptionAudio? = null
    var cancelCount = 0

    override fun cancel() {
        cancelCount += 1
    }

    override fun transcribe(audio: TranscriptionAudio): String {
        lastAudio = audio
        lastWav = audio.file.readBytes()
        error?.let { throw it }
        return response
    }
}

private class QueuedRunner : BackgroundRunner {
    private val tasks = ArrayDeque<() -> Unit>()
    override fun run(task: () -> Unit) {
        tasks.add(task)
    }

    /** Runs every task, including tasks the running ones queue. */
    fun runAll() {
        while (tasks.isNotEmpty()) tasks.removeFirst().invoke()
    }
}

private data object DirectDispatcher : CallbackDispatcher {
    override fun dispatch(task: () -> Unit) = task()
}

private class CapturingListener : DictationBackend.Listener {
    var final: String? = null
    var error: String? = null

    override fun onFinal(text: String) {
        final = text
    }

    override fun onError(message: String) {
        error = message
    }
}
