package com.openwhisper.android.dictation

import com.openwhisper.android.audio.ActiveRecording
import com.openwhisper.android.audio.PcmRecorder
import com.openwhisper.android.transcription.NoSpeechException
import com.openwhisper.android.transcription.TranscriptionJob
import com.openwhisper.android.transcription.TranscriptionManager
import com.openwhisper.android.transcription.TranscriptionOrigin
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference

fun interface BackgroundRunner {
    fun run(task: () -> Unit)
}

fun interface CallbackDispatcher {
    fun dispatch(task: () -> Unit)
}

/**
 * Records on the microphone, then hands the recording to the app's
 * [TranscriptionManager], which uploads it and saves its status.
 */
class BufferedDictationBackend(
    private val recorder: PcmRecorder,
    private val transcriptions: TranscriptionManager,
    private val background: BackgroundRunner,
    private val callbacks: CallbackDispatcher,
) : DictationBackend {
    private val generation = AtomicLong(0)
    private val activeRecording = AtomicReference<ActiveRecording?>(null)
    private val activeJob = AtomicReference<TranscriptionJob?>(null)
    /** Held while a stopped recording is handed to [transcriptions], and by [cancel]. */
    private val handover = Any()
    @Volatile
    private var listener: DictationBackend.Listener? = null

    override fun start(listener: DictationBackend.Listener) {
        generation.incrementAndGet()
        this.listener = listener
        try {
            activeRecording.set(recorder.start())
        } catch (error: Exception) {
            this.listener = null
            listener.onError(error.safeMessage("Unable to start microphone"))
        }
    }

    override fun finish() {
        val expectedGeneration = generation.get()
        val expectedListener = listener ?: return
        // Stops this recording only, even if a cancel and a new recording
        // come before the background thread gets to it.
        val recording = activeRecording.get() ?: return
        background.run {
            try {
                val saved = recording.stop()
                val job = synchronized(handover) {
                    // A cancel that came while the microphone was stopping:
                    // the saved audio is never sent.
                    if (generation.get() != expectedGeneration) return@run
                    transcriptions.submit(saved, TranscriptionOrigin.DICTATION).also(activeJob::set)
                }
                // A new dictation that started while the job was being handed over.
                if (generation.get() != expectedGeneration) job.cancel()
                job.onFinished { result ->
                    activeJob.compareAndSet(job, null)
                    result.fold(
                        onSuccess = { transcript ->
                            deliver(expectedGeneration, expectedListener) { it.onFinal(transcript) }
                        },
                        onFailure = { error -> fail(expectedGeneration, expectedListener, error) },
                    )
                }
            } catch (error: Exception) {
                callbacks.dispatch { fail(expectedGeneration, expectedListener, error) }
            }
        }
    }

    override fun cancel() {
        // A job handed over before this sees the cancel below; one after it
        // is never submitted.
        synchronized(handover) {
            generation.incrementAndGet()
            listener = null
        }
        activeRecording.getAndSet(null)?.cancel()
        // Stop this dictation's upload too; other transcriptions carry on.
        activeJob.getAndSet(null)?.cancel()
    }

    private fun deliver(
        expectedGeneration: Long,
        expectedListener: DictationBackend.Listener,
        event: (DictationBackend.Listener) -> Unit,
    ) {
        if (generation.get() == expectedGeneration && listener === expectedListener) {
            event(expectedListener)
        }
    }

    private fun fail(
        expectedGeneration: Long,
        expectedListener: DictationBackend.Listener,
        error: Throwable,
    ) = deliver(expectedGeneration, expectedListener) {
        val message = error.safeMessage("Dictation failed")
        it.onError(
            // "No audio found" is all there is to say about a tap.
            if (error is NoSpeechException) message
            else if (message.contains("raw audio", ignoreCase = true)) message
            else "$message. Raw audio is available in OpenWhisper.",
        )
    }

    private fun Throwable.safeMessage(fallback: String): String =
        message?.takeIf { it.isNotBlank() } ?: fallback
}
