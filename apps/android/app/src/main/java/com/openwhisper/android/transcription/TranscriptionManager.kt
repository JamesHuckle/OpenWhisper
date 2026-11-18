package com.openwhisper.android.transcription

import com.openwhisper.android.audio.WavEncoder
import com.openwhisper.android.dictation.BackgroundRunner
import com.openwhisper.android.dictation.CallbackDispatcher
import com.openwhisper.android.recordings.RecordingStore
import com.openwhisper.android.recordings.StoredRecording
import java.util.concurrent.CopyOnWriteArraySet

/**
 * Every transcription upload in the app, one job per recording. The app
 * screen and the dictation overlay both send their recordings here, and the
 * application object holds the one manager, so a job outlives the screen that
 * started it: when Android recreates the screen (rotation, theme change, low
 * memory) the upload carries on and the new screen hears its result.
 *
 * Submitting a recording that already has a job returns that job, so the same
 * recording is never uploaded (and paid for) twice at once. Each job has its
 * own client, so cancelling one job never stops another. The manager writes
 * each recording's status: "transcribing", then "complete", "failed" or
 * "cancelled". Each job reserves its recording in the store from the moment
 * it starts until its result is saved, so newer imports and captures never
 * delete the audio or saved parts of a recording still being transcribed.
 *
 * Each job has a [TranscriptionOrigin]. Dictations run on [dictationRunner]
 * and imports and retries on [bulkRunner], so a short keyboard dictation never
 * waits behind a long import. A job is still found by its recording alone: an
 * app-screen request for a dictation that is running joins that job.
 *
 * Nothing resumes by itself. A job lost with the app's process leaves its
 * finished parts saved, and [recoverInterrupted] marks it failed, so the user
 * taps Transcribe to send only the parts that did not finish.
 */
class TranscriptionManager(
    private val store: RecordingStore,
    /** A client for one job; it stops before any upload once the argument returns true. */
    private val newClient: (isCancelled: () -> Boolean) -> TranscriptionClient,
    /** Runs dictation jobs, apart from imports and retries. */
    private val dictationRunner: BackgroundRunner,
    /** Runs imports and retries. */
    private val bulkRunner: BackgroundRunner,
    private val callbacks: CallbackDispatcher,
) {
    interface Listener {
        /** A job started; its recording is now "transcribing". */
        fun onStarted(job: TranscriptionJob) = Unit

        /** A job ended, with its transcript or why it failed or was cancelled. */
        fun onFinished(job: TranscriptionJob, result: Result<String>)
    }

    private val lock = Any()
    private val jobs = mutableMapOf<String, TranscriptionJob>()
    private val listeners = CopyOnWriteArraySet<Listener>()

    fun addListener(listener: Listener) {
        listeners += listener
    }

    fun removeListener(listener: Listener) {
        listeners -= listener
    }

    fun isRunning(recordingId: String): Boolean = synchronized(lock) { recordingId in jobs }

    /**
     * Transcribes a saved recording, or returns its job if one is running.
     * [showResultInApp] marks the job's result for the app screen to show.
     */
    fun submit(
        recording: StoredRecording,
        origin: TranscriptionOrigin,
        showResultInApp: Boolean = false,
    ): TranscriptionJob = submit(recording.id, origin, showResultInApp) { audioFor(recording) }

    /**
     * Transcribes the recording [recordingId], or returns its job if one is
     * running. [audio] is read on the job's own thread; its finished parts are
     * saved beside the recording. [origin] picks the job's thread; a running
     * job keeps its own.
     */
    fun submit(
        recordingId: String,
        origin: TranscriptionOrigin,
        showResultInApp: Boolean = false,
        audio: () -> TranscriptionAudio,
    ): TranscriptionJob {
        val job = synchronized(lock) {
            jobs[recordingId]?.let { running ->
                if (showResultInApp) running.showResultInApp = true
                return running
            }
            TranscriptionJob(recordingId, origin, audio, newClient, callbacks).also { job ->
                job.showResultInApp = showResultInApp
                jobs[recordingId] = job
                // Reserved before the status change, whose cleanup could
                // otherwise delete an old recording just as its retry starts.
                store.reserve(recordingId)
                try {
                    store.finish(recordingId, "transcribing")
                } catch (error: Exception) {
                    // Never run, so never left registered or reserved.
                    jobs.remove(recordingId, job)
                    store.release(recordingId)
                    throw error
                }
            }
        }
        callbacks.dispatch { listeners.forEach { it.onStarted(job) } }
        val runner = when (origin) {
            TranscriptionOrigin.DICTATION -> dictationRunner
            TranscriptionOrigin.BULK -> bulkRunner
        }
        runner.run { run(job) }
        return job
    }

    /**
     * Marks recordings left "transcribing" by an earlier process as failed,
     * keeping their finished parts, so the user can tap to finish them. A job
     * of this process is never touched.
     */
    fun recoverInterrupted() {
        store.list().filter { it.status == "transcribing" }.forEach { recording ->
            synchronized(lock) {
                // Read again: a job of this process may have finished, and
                // saved its transcript, since the list was read.
                if (recording.id !in jobs && store.get(recording.id)?.status == "transcribing") {
                    store.finish(recording.id, "failed", error = INTERRUPTED_MESSAGE)
                }
            }
        }
    }

    private fun run(job: TranscriptionJob) {
        val result = try {
            transcribeAndSave(job)
        } finally {
            // Past the newest recordings, it goes at the store's next cleanup.
            store.release(job.recordingId)
        }
        callbacks.dispatch {
            job.finish(result)
            listeners.forEach { it.onFinished(job, result) }
        }
    }

    /** Transcribes [job]'s recording, saves its status and ends the job. */
    private fun transcribeAndSave(job: TranscriptionJob): Result<String> {
        val transcribed = if (job.isCancelled) {
            Result.failure(cancelled())
        } else {
            runCatching {
                // Finished parts are saved beside the recording, so a retry
                // sends only the parts that did not finish.
                val audio = job.audio().copy(partsFile = store.partsFile(job.recordingId))
                // An empty microphone recording is never sent.
                if (audio.pcmSampleRate != null && audio.pcmDataLength == 0L) {
                    throw NoSpeechException("no_bytes")
                }
                job.client.transcribe(audio)
            }
        }
        // A transcript that came back is kept even when the job was cancelled
        // meanwhile: it has been paid for. Any failure after a cancel is the
        // cancel.
        val result = if (transcribed.isFailure && job.isCancelled) {
            Result.failure(cancelled())
        } else {
            transcribed
        }
        synchronized(lock) {
            // A status that cannot be saved (storage full, say) still ends
            // the job and delivers its result: a transcript is paid for.
            runCatching {
                result.fold(
                    onSuccess = { transcript ->
                        store.finish(job.recordingId, "complete", transcript = transcript)
                    },
                    onFailure = { error ->
                        if (job.isCancelled) {
                            store.finish(job.recordingId, "cancelled")
                        } else {
                            store.finish(
                                job.recordingId,
                                "failed",
                                error = error.message?.takeIf(String::isNotBlank) ?: FAILED_MESSAGE,
                            )
                        }
                    },
                )
            }
            jobs.remove(job.recordingId, job)
        }
        return result
    }

    private fun audioFor(recording: StoredRecording): TranscriptionAudio {
        val file = store.file(recording)
        val wav = if (
            recording.mimeType.contains("wav") ||
            recording.fileName.endsWith(".wav", true)
        ) {
            WavEncoder.inspectPcm16Mono(file)
        } else {
            null
        }
        return TranscriptionAudio(
            file = file,
            mimeType = recording.mimeType,
            pcmSampleRate = wav?.sampleRate,
            pcmDataOffset = wav?.dataOffset ?: 0,
            pcmDataLength = wav?.dataLength ?: file.length(),
        )
    }

    private fun cancelled() = TranscriptionException(OpenAiHttpTranscriptionClient.CANCELLED_MESSAGE)

    companion object {
        const val FAILED_MESSAGE = "Transcription failed"
        const val INTERRUPTED_MESSAGE =
            "Transcription stopped when OpenWhisper closed. Tap Transcribe to finish it; " +
                "parts that finished are not sent again."
    }
}

/** Where a transcription came from, which picks the thread it runs on. */
enum class TranscriptionOrigin {
    /** A recording from the dictation overlay. */
    DICTATION,

    /** An imported file, or a recording the user taps Transcribe on again. */
    BULK,
}

/** One recording's transcription, run by [TranscriptionManager]. */
class TranscriptionJob internal constructor(
    val recordingId: String,
    val origin: TranscriptionOrigin,
    internal val audio: () -> TranscriptionAudio,
    newClient: (isCancelled: () -> Boolean) -> TranscriptionClient,
    private val callbacks: CallbackDispatcher,
) {
    @Volatile
    var isCancelled = false
        private set

    /** Whether the app screen shows this job's result when it ends. */
    @Volatile
    var showResultInApp = false
        internal set

    internal val client: TranscriptionClient = newClient { isCancelled }

    private var result: Result<String>? = null
    private val finishCallbacks = mutableListOf<(Result<String>) -> Unit>()

    /** Stops this job's upload; other jobs carry on. */
    fun cancel() {
        isCancelled = true
        client.cancel()
    }

    /**
     * Calls [callback] on the callback thread when the job ends, or soon if
     * it has ended already. Any thread may call this.
     */
    fun onFinished(callback: (Result<String>) -> Unit) {
        val ended = synchronized(this) {
            result ?: return@synchronized null.also { finishCallbacks += callback }
        }
        if (ended != null) callbacks.dispatch { callback(ended) }
    }

    /** Called on the callback thread. */
    internal fun finish(ended: Result<String>) {
        val waiting = synchronized(this) {
            result = ended
            finishCallbacks.toList().also { finishCallbacks.clear() }
        }
        waiting.forEach { it(ended) }
    }
}
