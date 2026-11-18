package com.openwhisper.android

import android.app.Application
import android.content.Context
import com.openwhisper.android.dictation.ExecutorBackgroundRunner
import com.openwhisper.android.dictation.MainThreadCallbackDispatcher
import com.openwhisper.android.recordings.RecordingStore
import com.openwhisper.android.settings.SecureApiKeyStore
import com.openwhisper.android.transcription.OpenAiHttpTranscriptionClient
import com.openwhisper.android.transcription.TranscriptionManager
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Holds the one [TranscriptionManager] the app screen and the dictation
 * overlay share, so an upload outlives the screen that started it.
 */
class OpenWhisperApplication : Application() {
    lateinit var transcriptions: TranscriptionManager
        private set

    override fun onCreate() {
        super.onCreate()
        val secrets = SecureApiKeyStore(this)
        // Shared by every job's client, so once the service refuses this
        // phone's FLAC later uploads go straight to WAV.
        val flacRefused = AtomicBoolean(false)
        // Dictations get their own thread, so a keyboard dictation never
        // waits behind an import or retry; those run one at a time.
        val dictationExecutor = Executors.newSingleThreadExecutor { task ->
            Thread(task, "openwhisper-dictation-transcription")
        }
        val bulkExecutor = Executors.newSingleThreadExecutor { task ->
            Thread(task, "openwhisper-bulk-transcription")
        }
        transcriptions = TranscriptionManager(
            store = RecordingStore(this),
            newClient = { isCancelled ->
                OpenAiHttpTranscriptionClient(
                    secrets::load,
                    flacRefused = flacRefused,
                    isCancelled = isCancelled,
                )
            },
            dictationRunner = ExecutorBackgroundRunner(dictationExecutor),
            bulkRunner = ExecutorBackgroundRunner(bulkExecutor),
            callbacks = MainThreadCallbackDispatcher(),
        )
        // Recordings an earlier process was transcribing wait for the user to
        // tap Transcribe; their finished parts are kept.
        bulkExecutor.execute(transcriptions::recoverInterrupted)
    }

    companion object {
        fun transcriptions(context: Context): TranscriptionManager =
            (context.applicationContext as OpenWhisperApplication).transcriptions
    }
}
