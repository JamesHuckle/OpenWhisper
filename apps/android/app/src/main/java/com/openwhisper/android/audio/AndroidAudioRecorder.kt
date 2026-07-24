package com.openwhisper.android.audio

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import java.io.ByteArrayOutputStream
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sqrt

/**
 * @param onAmplitude optional sink invoked from the audio thread with a normalized
 *   RMS level (0..1) roughly every [AMPLITUDE_WINDOW_BYTES] of captured audio.
 *   Used to drive the live volume meter in the overlay.
 */
class AndroidAudioRecorder(
    private val context: Context,
    private val onAmplitude: ((Float) -> Unit)? = null,
) : PcmRecorder {
    private val recording = AtomicBoolean(false)
    private var audioRecord: AudioRecord? = null
    private var readThread: Thread? = null
    private var output: ByteArrayOutputStream? = null
    @Volatile
    private var readFailure: String? = null

    @Synchronized
    override fun start() {
        check(!recording.get()) { "Microphone is already recording" }
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

        val sink = ByteArrayOutputStream()
        audioRecord = recorder
        output = sink
        readFailure = null
        recording.set(true)
        try {
            recorder.startRecording()
        } catch (error: Exception) {
            recording.set(false)
            audioRecord = null
            output = null
            recorder.release()
            throw error
        }

        readThread = Thread({ readLoop(recorder, sink, bufferSize) }, "openwhisper-audio").apply {
            priority = Thread.MAX_PRIORITY
            start()
        }
    }

    override fun stop(): PcmRecording {
        val recorder: AudioRecord
        val thread: Thread?
        val sink: ByteArrayOutputStream
        synchronized(this) {
            check(recording.getAndSet(false)) { "Microphone was not recording" }
            recorder = requireNotNull(audioRecord)
            thread = readThread
            sink = requireNotNull(output)
        }
        runCatching { recorder.stop() }
        thread?.join(2_000)
        recorder.release()
        synchronized(this) {
            audioRecord = null
            readThread = null
            output = null
        }
        readFailure?.let { throw IllegalStateException(it) }
        return PcmRecording(synchronized(sink) { sink.toByteArray() }, SAMPLE_RATE)
    }

    override fun cancel() {
        val recorder: AudioRecord?
        val thread: Thread?
        synchronized(this) {
            recording.set(false)
            recorder = audioRecord
            thread = readThread
            audioRecord = null
            readThread = null
            output = null
        }
        runCatching { recorder?.stop() }
        thread?.join(1_000)
        recorder?.release()
    }

    private fun readLoop(recorder: AudioRecord, sink: ByteArrayOutputStream, bufferSize: Int) {
        val buffer = ByteArray(bufferSize)
        // Read in short windows so the live meter updates ~16×/second.
        val readLength = min(buffer.size, AMPLITUDE_WINDOW_BYTES)
        while (recording.get()) {
            val count = recorder.read(buffer, 0, readLength, AudioRecord.READ_BLOCKING)
            if (count > 0) {
                synchronized(sink) { sink.write(buffer, 0, count) }
                emitAmplitude(buffer, count)
            } else {
                audioReadFailure(count, recording.get())?.let { failure ->
                    readFailure = failure
                    recording.set(false)
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
        const val SAMPLE_RATE = 16_000
        // 1024 samples (16-bit mono) ≈ 64 ms at 16 kHz.
        const val AMPLITUDE_WINDOW_BYTES = 2_048
    }
}

internal fun audioReadFailure(count: Int, isRecording: Boolean): String? = when {
    !isRecording || count >= 0 -> null
    count == AudioRecord.ERROR_DEAD_OBJECT -> "Microphone disconnected while recording"
    else -> "Microphone read failed ($count)"
}
