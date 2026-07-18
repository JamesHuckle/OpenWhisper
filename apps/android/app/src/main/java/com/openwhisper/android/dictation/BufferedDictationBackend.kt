package com.openwhisper.android.dictation

import com.openwhisper.android.audio.PcmRecorder
import com.openwhisper.android.audio.WavEncoder
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
            try {
                val recording = recorder.stop()
                if (recording.bytes.isEmpty()) throw IllegalStateException("No audio was captured")
                val wav = WavEncoder.encodePcm16Mono(recording.bytes, recording.sampleRate)
                val transcript = client.transcribe(wav)
                callbacks.dispatch {
                    if (generation.get() == expectedGeneration && listener === expectedListener) {
                        expectedListener.onFinal(transcript)
                    }
                }
            } catch (error: Exception) {
                callbacks.dispatch {
                    if (generation.get() == expectedGeneration && listener === expectedListener) {
                        expectedListener.onError(error.safeMessage("Dictation failed"))
                    }
                }
            }
        }
    }

    override fun cancel() {
        generation.incrementAndGet()
        listener = null
        recorder.cancel()
    }

    private fun Throwable.safeMessage(fallback: String): String =
        message?.takeIf { it.isNotBlank() } ?: fallback
}
