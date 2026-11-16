package com.openwhisper.android.dictation

import com.openwhisper.android.audio.PcmRecorder
import com.openwhisper.android.audio.WavEncoder
import com.openwhisper.android.recordings.RecordingStore
import com.openwhisper.android.transcription.NoSpeechException
import com.openwhisper.android.transcription.TranscriptionAudio
import com.openwhisper.android.transcription.TranscriptionClient
import java.util.concurrent.atomic.AtomicLong

fun interface BackgroundRunner {
    fun run(task: () -> Unit)
}

fun interface CallbackDispatcher {
    fun dispatch(task: () -> Unit)
}

class BufferedDictationBackend(
    private val recorder: PcmRecorder,
    private val client: TranscriptionClient,
    private val background: BackgroundRunner,
    private val callbacks: CallbackDispatcher,
    private val recordingStore: RecordingStore? = null,
) : DictationBackend {
    private val generation = AtomicLong(0)
    @Volatile
    private var listener: DictationBackend.Listener? = null

    override fun start(listener: DictationBackend.Listener) {
        generation.incrementAndGet()
        this.listener = listener
        try {
            recorder.start()
        } catch (error: Exception) {
            this.listener = null
            listener.onError(error.safeMessage("Unable to start microphone"))
        }
    }

    override fun finish() {
        val expectedGeneration = generation.get()
        val expectedListener = listener ?: return
        background.run {
            var recordingId: String? = null
            try {
                val recording = recorder.stop()
                recordingId = recording.id
                if (recording.byteCount == 0L) throw NoSpeechException("no_bytes")
                val transcript = client.transcribe(
                    TranscriptionAudio(
                        file = recording.file,
                        mimeType = "audio/wav",
                        pcmSampleRate = recording.sampleRate,
                        pcmDataOffset = WavEncoder.HEADER_BYTES.toLong(),
                        pcmDataLength = recording.byteCount,
                        partsFile = recordingStore?.partsFile(recording.id),
                    ),
                )
                recordingStore?.finish(recording.id, "complete", transcript = transcript)
                callbacks.dispatch {
                    if (generation.get() == expectedGeneration && listener === expectedListener) {
                        expectedListener.onFinal(transcript)
                    }
                }
            } catch (error: Exception) {
                val cancelled = generation.get() != expectedGeneration
                recordingId?.let { id ->
                    if (cancelled) {
                        recordingStore?.finish(id, "cancelled")
                    } else {
                        recordingStore?.finish(
                            id,
                            "failed",
                            error = error.safeMessage("Dictation failed"),
                        )
                    }
                }
                callbacks.dispatch {
                    if (generation.get() == expectedGeneration && listener === expectedListener) {
                        val message = error.safeMessage("Dictation failed")
                        expectedListener.onError(
                            // "No audio found" is all there is to say about a tap.
                            if (error is NoSpeechException) message
                            else if (message.contains("raw audio", ignoreCase = true)) message
                            else "$message. Raw audio is available in OpenWhisper.",
                        )
                    }
                }
            }
        }
    }

    override fun cancel() {
        generation.incrementAndGet()
        listener = null
        recorder.cancel()
        // Stop the upload too, so the single background thread is free for
        // the next dictation.
        client.cancel()
    }

    private fun Throwable.safeMessage(fallback: String): String =
        message?.takeIf { it.isNotBlank() } ?: fallback
}
