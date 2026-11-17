package com.openwhisper.android.transcription

import com.openwhisper.android.audio.PcmRecorder
import com.openwhisper.android.dictation.BackgroundRunner
import com.openwhisper.android.dictation.BufferedDictationBackend
import com.openwhisper.android.dictation.CallbackDispatcher
import com.openwhisper.android.dictation.DictationBackend
import com.openwhisper.android.recordings.PcmRecording
import com.openwhisper.android.recordings.RecordingStore
import com.openwhisper.android.recordings.StoredRecording
import java.io.ByteArrayInputStream
import java.io.File
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class TranscriptionManagerTest {
    private val root = File(
        System.getProperty("java.io.tmpdir"),
        "openwhisper-transcriptions-${System.nanoTime()}",
    )
    private val store = RecordingStore.forDirectory(root)
    private val executor = Executors.newFixedThreadPool(2)

    @After
    fun cleanUp() {
        executor.shutdownNow()
        root.deleteRecursively()
    }

    @Test
    fun recreatingTheScreenDuringAnUploadSendsNoSecondRequest() {
        val client = BlockingClient("Hello from the first screen")
        val manager = manager(newClient = { client }, background = threads())
        val recording = imported("memo.mp3")

        // The first screen starts the upload, then Android recreates it.
        val firstScreen = CapturingListener()
        manager.addListener(firstScreen)
        val job = manager.submit(recording, showResultInApp = true)
        assertTrue(client.entered.await(5, TimeUnit.SECONDS))
        manager.removeListener(firstScreen)

        // The new screen attaches and, say, the user taps Transcribe again.
        val secondScreen = CapturingListener()
        manager.addListener(secondScreen)
        assertSame(job, manager.submit(recording, showResultInApp = true))
        client.release.countDown()

        assertTrue(secondScreen.finished.await(5, TimeUnit.SECONDS))
        assertEquals(1, client.requests.get())
        assertEquals(listOf("Hello from the first screen"), secondScreen.transcripts)
        assertTrue(secondScreen.shownInApp)
        assertTrue(firstScreen.transcripts.isEmpty())
        assertEquals("complete", store.get(recording.id)?.status)
        assertEquals("Hello from the first screen", store.get(recording.id)?.transcript)
        assertFalse(manager.isRunning(recording.id))
    }

    @Test
    fun theSameRecordingFromTheOverlayAndTheAppIsSentOnce() {
        val client = BlockingClient("One upload")
        client.release.countDown()
        val uploads = QueuedRunner()
        val manager = manager(newClient = { client }, background = uploads)
        val writer = store.beginPcm("Dictation", 16_000)
        val pcm = ByteArray(32_000) { (it % 50).toByte() }
        writer.write(pcm, pcm.size)
        val recording = writer.finish("saved")

        // The overlay finishes a dictation and hands the recording over.
        val dictation = QueuedRunner()
        val overlay = BufferedDictationBackend(
            OneRecording(recording),
            manager,
            dictation,
            Direct,
        )
        val overlayListener = OverlayListener()
        overlay.start(overlayListener)
        overlay.finish()
        dictation.runAll()
        assertTrue(manager.isRunning(recording.id))
        assertEquals("transcribing", store.get(recording.id)?.status)

        // The app screen asks for the same recording before it is sent.
        val app = CapturingListener()
        manager.addListener(app)
        manager.submit(requireNotNull(store.get(recording.id)), showResultInApp = true)
        uploads.runAll()

        assertEquals(1, client.requests.get())
        assertEquals("One upload", overlayListener.final)
        assertEquals(listOf("One upload"), app.transcripts)
        assertTrue(app.shownInApp)
        assertEquals("complete", store.get(recording.id)?.status)
    }

    @Test
    fun cancellingOneJobLeavesAnotherRunning() {
        val clients = CopyOnWriteArrayList<BlockingClient>()
        val manager = manager(
            newClient = { BlockingClient("text ${clients.size + 1}").also(clients::add) },
            background = threads(),
        )
        val listener = CapturingListener(expected = 2)
        manager.addListener(listener)
        val first = manager.submit(imported("first.mp3"))
        val second = manager.submit(imported("second.mp3"))
        val (firstClient, secondClient) = clients
        assertTrue(firstClient.entered.await(5, TimeUnit.SECONDS))
        assertTrue(secondClient.entered.await(5, TimeUnit.SECONDS))

        first.cancel()
        assertTrue(manager.isRunning(second.recordingId))
        secondClient.release.countDown()

        assertTrue(listener.finished.await(5, TimeUnit.SECONDS))
        assertEquals(1, firstClient.cancels.get())
        assertEquals(0, secondClient.cancels.get())
        assertEquals("cancelled", store.get(first.recordingId)?.status)
        assertEquals("complete", store.get(second.recordingId)?.status)
        assertEquals("text 2", store.get(second.recordingId)?.transcript)
        assertEquals(listOf("text 2"), listener.transcripts)
    }

    @Test
    fun aJobLostWithTheProcessWaitsForTheUserToTapTranscribe() {
        val client = BlockingClient("unused")
        val uploads = QueuedRunner()
        // The earlier process was transcribing this recording when it died.
        val interrupted = imported("long.mp3")
        store.finish(interrupted.id, "transcribing")
        store.partsFile(interrupted.id).writeText("saved parts")
        val manager = manager(newClient = { client }, background = uploads)
        // A job of this process is never marked interrupted.
        val running = manager.submit(imported("new.mp3"))

        manager.recoverInterrupted()

        val recovered = requireNotNull(store.get(interrupted.id))
        assertEquals("failed", recovered.status)
        assertEquals(TranscriptionManager.INTERRUPTED_MESSAGE, recovered.error)
        assertTrue(store.partsFile(interrupted.id).isFile)
        assertFalse(manager.isRunning(interrupted.id))
        assertEquals("transcribing", store.get(running.recordingId)?.status)
        assertEquals(0, client.requests.get())
    }

    @Test
    fun aJobCancelledBeforeItRunsIsNeverSent() {
        val client = BlockingClient("unused")
        val uploads = QueuedRunner()
        val manager = manager(newClient = { client }, background = uploads)
        val listener = CapturingListener()
        manager.addListener(listener)

        val job = manager.submit(imported("queued.mp3"))
        job.cancel()
        uploads.runAll()

        assertEquals(0, client.requests.get())
        assertEquals("cancelled", store.get(job.recordingId)?.status)
        assertEquals(OpenAiHttpTranscriptionClient.CANCELLED_MESSAGE, listener.errors.single())
    }

    private fun manager(
        newClient: (() -> Boolean) -> TranscriptionClient,
        background: BackgroundRunner,
    ) = TranscriptionManager(store, newClient, background, Direct)

    private fun threads() = BackgroundRunner { task -> executor.execute(task) }

    private fun imported(name: String): StoredRecording = store.import(
        name = name,
        mimeType = "audio/mpeg",
        extension = "mp3",
        input = ByteArrayInputStream(byteArrayOf(1, 2, 3, 4)),
    )
}

/** Holds each upload until [release], and stops at once when cancelled. */
private class BlockingClient(private val text: String) : TranscriptionClient {
    val requests = AtomicInteger()
    val cancels = AtomicInteger()
    val entered = CountDownLatch(1)
    val release = CountDownLatch(1)
    private val cancelled = CountDownLatch(1)

    override fun transcribe(audio: TranscriptionAudio): String {
        requests.incrementAndGet()
        entered.countDown()
        while (release.count > 0) {
            if (cancelled.await(10, TimeUnit.MILLISECONDS)) {
                throw TranscriptionException(OpenAiHttpTranscriptionClient.CANCELLED_MESSAGE)
            }
        }
        return text
    }

    override fun cancel() {
        cancels.incrementAndGet()
        cancelled.countDown()
    }
}

private class CapturingListener(expected: Int = 1) : TranscriptionManager.Listener {
    val finished = CountDownLatch(expected)
    val transcripts = CopyOnWriteArrayList<String>()
    val errors = CopyOnWriteArrayList<String>()

    @Volatile
    var shownInApp = false

    override fun onFinished(job: TranscriptionJob, result: Result<String>) {
        result.onSuccess { transcripts += it }.onFailure { errors += it.message.orEmpty() }
        shownInApp = job.showResultInApp
        finished.countDown()
    }
}

private class OverlayListener : DictationBackend.Listener {
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

private class OneRecording(private val recording: PcmRecording) : PcmRecorder {
    override fun start() = Unit
    override fun stop(): PcmRecording = recording
    override fun cancel() = Unit
}

private class QueuedRunner : BackgroundRunner {
    private val tasks = ArrayDeque<() -> Unit>()

    override fun run(task: () -> Unit) {
        tasks.add(task)
    }

    fun runAll() {
        while (tasks.isNotEmpty()) tasks.removeFirst().invoke()
    }
}

private data object Direct : CallbackDispatcher {
    override fun dispatch(task: () -> Unit) = task()
}
