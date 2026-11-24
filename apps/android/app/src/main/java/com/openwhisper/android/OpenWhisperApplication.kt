package com.openwhisper.android

import android.app.Application
import android.content.Context
import android.os.Handler
import android.os.Looper
import com.openwhisper.android.recordings.RecordingStore
import com.openwhisper.android.settings.SecureApiKeyStore
import com.openwhisper.android.transcription.OpenAiHttpTranscriptionClient
import com.openwhisper.android.transcription.TranscriptionManager
import java.util.concurrent.Executor
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Holds the one [RecordingStore] and the one [TranscriptionManager] the app
 * screen and the dictation overlay share, so every recording is guarded by one
 * lock and an upload outlives the screen that started it.
 */
class OpenWhisperApplication : Application() {
    lateinit var recordings: RecordingStore
        private set

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
        val mainThread = Handler(Looper.getMainLooper())
        recordings = RecordingStore(this)
        transcriptions = TranscriptionManager(
            store = recordings,
            newClient = { cancelled ->
                OpenAiHttpTranscriptionClient(
                    secrets::load,
                    flacRefused = flacRefused,
                    cancelled = cancelled,
                )
            },
            dictationRunner = dictationExecutor,
            bulkRunner = bulkExecutor,
            callbacks = Executor { mainThread.post(it) },
        )
        // Recordings an earlier process was transcribing wait for the user to
        // tap Transcribe; their finished parts are kept.
        bulkExecutor.execute(transcriptions::recoverInterrupted)
    }

    companion object {
        fun recordings(context: Context): RecordingStore =
            (context.applicationContext as OpenWhisperApplication).recordings

        fun transcriptions(context: Context): TranscriptionManager =
            (context.applicationContext as OpenWhisperApplication).transcriptions
    }
}
