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

class AndroidAudioRecorder(private val context: Context) : PcmRecorder {
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
        while (recording.get()) {
            val count = recorder.read(buffer, 0, buffer.size, AudioRecord.READ_BLOCKING)
            if (count > 0) {
                synchronized(sink) { sink.write(buffer, 0, count) }
            } else {
                audioReadFailure(count, recording.get())?.let { failure ->
                    readFailure = failure
                    recording.set(false)
                }
            }
        }
    }

    private companion object {
        const val SAMPLE_RATE = 16_000
    }
}

internal fun audioReadFailure(count: Int, isRecording: Boolean): String? = when {
    !isRecording || count >= 0 -> null
    count == AudioRecord.ERROR_DEAD_OBJECT -> "Microphone disconnected while recording"
    else -> "Microphone read failed ($count)"
}
