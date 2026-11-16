package com.openwhisper.android.audio

import android.media.AudioFormat
import android.media.MediaCodec
import android.media.MediaFormat
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.RandomAccessFile
import java.nio.ByteBuffer

/**
 * Encodes recorded PCM to FLAC with the phone's own encoder before upload.
 * FLAC is lossless, so the service hears exactly what was recorded, in
 * roughly half the bytes of a WAV file for speech.
 *
 * Any failure throws; the caller then uploads the same audio as WAV.
 */
object FlacFileEncoder {
    private const val TIMEOUT_US = 10_000L
    private const val INPUT_BYTES = 64 * 1024
    private const val DEADLINE_NANOS = 60_000_000_000L
    private val FLAC_MARKER = "fLaC".encodeToByteArray()

    /**
     * Encodes [length] bytes of 16-bit mono PCM from [offset] in [file]. The
     * samples are read in blocks as the encoder takes them, never whole.
     */
    fun encodePcm16Mono(file: File, offset: Long, length: Int, sampleRate: Int): ByteArray {
        require(length > 0) { "No audio to encode" }
        val format: MediaFormat =
            MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_FLAC, sampleRate, 1)
        format.setInteger(MediaFormat.KEY_PCM_ENCODING, AudioFormat.ENCODING_PCM_16BIT)
        format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, INPUT_BYTES)
        format.setInteger(MediaFormat.KEY_FLAC_COMPRESSION_LEVEL, 5)
        val codec: MediaCodec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_FLAC)
        try {
            codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            codec.start()
            return RandomAccessFile(file, "r").use { source ->
                source.seek(offset)
                withStreamHeader(drain(codec, source, length, sampleRate))
            }
        } finally {
            runCatching { codec.stop() }
            codec.release()
        }
    }

    private class Encoded(val bytes: ByteArray, val formatHeader: ByteArray?)

    private fun drain(
        codec: MediaCodec,
        source: RandomAccessFile,
        length: Int,
        sampleRate: Int,
    ): Encoded {
        val output = ByteArrayOutputStream(length / 2)
        val block = ByteArray(INPUT_BYTES)
        val info = MediaCodec.BufferInfo()
        val deadline = System.nanoTime() + DEADLINE_NANOS
        var formatHeader: ByteArray? = null
        var offset = 0
        var inputDone = false
        while (true) {
            check(System.nanoTime() < deadline) { "FLAC encoder did not finish" }
            if (!inputDone) {
                val index = codec.dequeueInputBuffer(TIMEOUT_US)
                if (index >= 0) {
                    val buffer = checkNotNull(codec.getInputBuffer(index))
                    buffer.clear()
                    // Whole 16-bit samples only.
                    val count = minOf(buffer.remaining(), block.size, length - offset) and 1.inv()
                    check(count > 0 || offset == length) { "FLAC encoder input is too small" }
                    source.readFully(block, 0, count)
                    buffer.put(block, 0, count)
                    val presentationUs = offset / 2 * 1_000_000L / sampleRate
                    offset += count
                    inputDone = offset >= length - 1
                    codec.queueInputBuffer(
                        index,
                        0,
                        count,
                        presentationUs,
                        if (inputDone) MediaCodec.BUFFER_FLAG_END_OF_STREAM else 0,
                    )
                }
            }
            val index = codec.dequeueOutputBuffer(info, TIMEOUT_US)
            if (index == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                formatHeader = codec.outputFormat.getByteBuffer("csd-0")?.toByteArray()
            } else if (index >= 0) {
                val buffer = checkNotNull(codec.getOutputBuffer(index))
                buffer.position(info.offset)
                buffer.limit(info.offset + info.size)
                output.write(buffer.toByteArray())
                codec.releaseOutputBuffer(index, false)
                if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) break
            }
        }
        return Encoded(output.toByteArray(), formatHeader)
    }

    /**
     * The encoder hands out the stream header ("fLaC" and its metadata) as
     * its first buffer. Should a phone give it only in the output format,
     * it is put in front here; without it the result is not a FLAC file.
     */
    private fun withStreamHeader(encoded: Encoded): ByteArray {
        val bytes = encoded.bytes
        if (bytes.startsWith(FLAC_MARKER)) return bytes
        val header = encoded.formatHeader
        check(header != null && header.startsWith(FLAC_MARKER)) { "FLAC encoder gave no stream header" }
        return header + bytes
    }

    private fun ByteArray.startsWith(prefix: ByteArray): Boolean =
        size > prefix.size && prefix.indices.all { this[it] == prefix[it] }

    private fun ByteBuffer.toByteArray(): ByteArray = ByteArray(remaining()).also(::get)
}
