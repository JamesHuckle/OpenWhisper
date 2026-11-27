package com.openwhisper.android.dictation

import com.openwhisper.android.audio.ActiveRecording
import com.openwhisper.android.audio.PcmRecorder
import com.openwhisper.android.transcription.NoSpeechException
import com.openwhisper.android.transcription.TranscriptionJob
import com.openwhisper.android.transcription.TranscriptionManager
import com.openwhisper.android.transcription.TranscriptionOrigin
import java.util.concurrent.Executor

/**
 * Records on the microphone, then hands the recording to the app's
 * [TranscriptionManager], which uploads it and saves its status.
 */
class BufferedDictationBackend(
    private val recorder: PcmRecorder,
    private val transcriptions: TranscriptionManager,
    private val background: Executor,
    private val callbacks: Executor,
) : DictationBackend {
    /** One microphone dictation: its recording, who hears its result, and its upload. */
    private class Capture(
        val recording: ActiveRecording,
        val listener: DictationBackend.Listener,
    ) {
        @Volatile
        var job: TranscriptionJob? = null
    }

    /** The dictation whose result is still wanted; a cancel or a new start replaces it. */
    @Volatile
    private var current: Capture? = null
    /** Held while a stopped recording is handed to [transcriptions], and by [cancel]. */
    private val handover = Any()

    override fun start(listener: DictationBackend.Listener) {
        val recording = try {
            recorder.start()
        } catch (error: Exception) {
            current = null
            listener.onError(error.safeMessage("Unable to start microphone"))
            return
        }
        current = Capture(recording, listener)
    }

    override fun finish() {
        // Stops this recording only, even if a cancel and a new recording
        // come before the background thread gets to it.
        val capture = current ?: return
        background.execute {
            try {
                val saved = capture.recording.stop()
                val job = synchronized(handover) {
                    // A cancel that came while the microphone was stopping:
                    // the saved audio is never sent.
                    if (current !== capture) return@execute
                    transcriptions.submit(saved, TranscriptionOrigin.DICTATION).also { capture.job = it }
                }
                // A new dictation that started while the job was being handed over.
                if (current !== capture) job.cancel()
                job.onFinished { result ->
                    capture.job = null
                    result.fold(
                        onSuccess = { transcript -> deliver(capture) { it.onFinal(transcript) } },
                        onFailure = { error -> fail(capture, error) },
                    )
                }
            } catch (error: Exception) {
                callbacks.execute { fail(capture, error) }
            }
        }
    }

    override fun cancel() {
        // A job handed over before this is cancelled below; one after it is
        // never submitted.
        val capture = synchronized(handover) { current.also { current = null } } ?: return
        capture.recording.cancel()
        // Stop this dictation's upload too; other transcriptions carry on.
        capture.job?.cancel()
    }

    private fun deliver(capture: Capture, event: (DictationBackend.Listener) -> Unit) {
        if (current === capture) event(capture.listener)
    }

    private fun fail(capture: Capture, error: Throwable) = deliver(capture) {
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
