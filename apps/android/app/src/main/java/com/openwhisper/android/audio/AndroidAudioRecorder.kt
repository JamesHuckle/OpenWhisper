package com.openwhisper.android.audio

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import com.openwhisper.android.recordings.PcmRecording
import com.openwhisper.android.recordings.PcmRecordingWriter
import com.openwhisper.android.recordings.RecordingStore
import java.text.DateFormat
import java.util.Date
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sqrt

/**
 * @param onAmplitude optional sink invoked from the audio thread with a normalized
 *   RMS level (0..1) roughly every 64 ms of captured audio.
 *   Used to drive the live volume meter in the overlay.
 */
class AndroidAudioRecorder(
    private val context: Context,
    private val recordingStore: RecordingStore,
    private val onAmplitude: ((Float) -> Unit)? = null,
) : PcmRecorder {
    /** The recording that holds the microphone, until its clean-up finishes. */
    private var active: MicrophoneRecording? = null

    @Synchronized
    override fun start(): ActiveRecording {
        check(active == null) { "Microphone is already recording" }
        if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            throw SecurityException("Microphone permission is required")
        }

        val minimum = AudioRecord.getMinBufferSize(
            SAMPLE_RATE,
            AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_16BIT,
        )
        check(minimum > 0) { "This device does not support 16 kHz microphone capture" }
        val bufferSize = max(minimum * 2, 8_192)
        val recorder = AudioRecord.Builder()
            .setAudioSource(MediaRecorder.AudioSource.MIC)
            .setAudioFormat(
                AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(SAMPLE_RATE)
                    .setChannelMask(AudioFormat.CHANNEL_IN_MONO)
                    .build(),
            )
            .setBufferSizeInBytes(bufferSize)
            .build()
        if (recorder.state != AudioRecord.STATE_INITIALIZED) {
            recorder.release()
            throw IllegalStateException("Microphone could not be initialized")
        }

        val sink = recordingStore.beginPcm(
            "Recording ${DateFormat.getDateTimeInstance().format(Date())}",
            SAMPLE_RATE,
        )
        try {
            recorder.startRecording()
        } catch (error: Exception) {
            recorder.release()
            runCatching { sink.finish("capture_failed") }
            throw error
        }

        val recording = MicrophoneRecording(recorder, sink, bufferSize, onAmplitude) {
            synchronized(this@AndroidAudioRecorder) { active = null }
        }
        active = recording
        recording.startReading()
        return recording
    }

    private companion object {
        const val SAMPLE_RATE = 16_000
    }
}

/**
 * One recording on the microphone, with its own recorder, file and read
 * thread. Stop or cancel cleans it up once, and [onReleased] runs when the
 * microphone is free again.
 */
internal class MicrophoneRecording(
    private val recorder: AudioRecord,
    private val sink: PcmRecordingWriter,
    bufferSize: Int,
    private val onAmplitude: ((Float) -> Unit)?,
    private val onReleased: () -> Unit,
) : ActiveRecording {
    private val capturing = AtomicBoolean(true)
    private val ended = AtomicBoolean(false)
    private val released = CountDownLatch(1)
    @Volatile
    private var readFailure: String? = null
    private val readThread = Thread({ readLoop(bufferSize) }, "openwhisper-audio").apply {
        priority = Thread.MAX_PRIORITY
    }

    fun startReading() = readThread.start()

    override fun stop(): PcmRecording {
        check(ended.compareAndSet(false, true)) { "Microphone was not recording" }
        release(joinMillis = 2_000)
        val recording = sink.finish(if (readFailure == null) "saved" else "capture_failed")
        readFailure?.let { throw IllegalStateException("$it. Raw audio was saved in OpenWhisper.") }
        return recording
    }

    override fun cancel() {
        if (ended.compareAndSet(false, true)) {
            release(joinMillis = 1_000)
            runCatching { sink.finish("cancelled") }
        } else {
            // A stop is cleaning up; wait for it so the next recording can start.
            released.await(STOP_CLEANUP_MILLIS, TimeUnit.MILLISECONDS)
        }
    }

    private fun release(joinMillis: Long) {
        try {
            capturing.set(false)
            runCatching { recorder.stop() }
            readThread.join(joinMillis)
            recorder.release()
        } finally {
            onReleased()
            released.countDown()
        }
    }

    private fun readLoop(bufferSize: Int) {
        val buffer = ByteArray(bufferSize)
        // Read in short windows so the live meter updates ~16×/second.
        val readLength = min(buffer.size, AMPLITUDE_WINDOW_BYTES)
        while (capturing.get()) {
            val count = recorder.read(buffer, 0, readLength, AudioRecord.READ_BLOCKING)
            if (count > 0) {
                sink.write(buffer, count)
                emitAmplitude(buffer, count)
            } else {
                audioReadFailure(count, capturing.get())?.let { failure ->
                    readFailure = failure
                    capturing.set(false)
                }
            }
        }
    }

    private fun emitAmplitude(buffer: ByteArray, count: Int) {
        val sink = onAmplitude ?: return
        val samples = count / 2
        if (samples <= 0) return
        var sumSquares = 0.0
        var i = 0
        while (i + 1 < count) {
            // Signed 16-bit little-endian sample.
            val sample = ((buffer[i + 1].toInt() shl 8) or (buffer[i].toInt() and 0xFF)).toShort()
            val normalized = sample / 32_768.0
            sumSquares += normalized * normalized
            i += 2
        }
        val rms = sqrt(sumSquares / samples)
        sink(rms.toFloat())
    }

    private companion object {
        // 1024 samples (16-bit mono) ≈ 64 ms at 16 kHz.
        const val AMPLITUDE_WINDOW_BYTES = 2_048
        // A stop's longest clean-up: its 2 s wait for the read thread, and some.
        const val STOP_CLEANUP_MILLIS = 2_500L
    }
}

internal fun audioReadFailure(count: Int, isRecording: Boolean): String? = when {
    !isRecording || count >= 0 -> null
    count == AudioRecord.ERROR_DEAD_OBJECT -> "Microphone disconnected while recording"
    else -> "Microphone read failed ($count)"
}
