package com.openwhisper.android.audio

import java.io.File
import java.io.RandomAccessFile
import java.nio.ByteBuffer
import java.nio.ByteOrder

data class PcmWavInfo(
    val sampleRate: Int,
    val dataOffset: Long,
    val dataLength: Long,
)

object WavEncoder {
    /** The size of the header [header] writes, before the PCM samples. */
    const val HEADER_BYTES = 44

    fun encodePcm16Mono(pcm: ByteArray, sampleRate: Int): ByteArray =
        header(pcm.size.toLong(), sampleRate) + pcm

    /**
     * The header of a 16-bit mono PCM WAV file holding [pcmBytes] of samples.
     * A recording in progress writes it again as it grows.
     */
    fun header(pcmBytes: Long, sampleRate: Int): ByteArray {
        require(sampleRate > 0) { "sampleRate must be positive" }
        require(pcmBytes in 0..UInt.MAX_VALUE.toLong() - 36) { "Recording is too large for WAV" }
        // putInt keeps the low 32 bits, so sizes up to UInt.MAX_VALUE are written unsigned.
        return ByteBuffer.allocate(HEADER_BYTES).order(ByteOrder.LITTLE_ENDIAN)
            .put("RIFF".encodeToByteArray())
            .putInt((36 + pcmBytes).toInt())
            .put("WAVE".encodeToByteArray())
            .put("fmt ".encodeToByteArray())
            .putInt(16)
            .putShort(1)
            .putShort(1)
            .putInt(sampleRate)
            .putInt(sampleRate * 2)
            .putShort(2)
            .putShort(16)
            .put("data".encodeToByteArray())
            .putInt(pcmBytes.toInt())
            .array()
    }

    fun inspectPcm16Mono(file: File): PcmWavInfo? = runCatching {
        RandomAccessFile(file, "r").use { wav ->
            if (wav.length() < HEADER_BYTES) return null
            val riff = wav.readLittleEndian(12)
            if (riff.ascii(0) != "RIFF" || riff.ascii(8) != "WAVE") return null
            var sampleRate: Int? = null
            var dataOffset: Long? = null
            var dataLength: Long? = null
            while (wav.filePointer + 8 <= wav.length()) {
                val chunkHeader = wav.readLittleEndian(8)
                val chunk = chunkHeader.ascii(0)
                val size = chunkHeader.getInt(4).toUInt().toLong()
                val contentStart = wav.filePointer
                if (chunk == "fmt " && size >= 16) {
                    val fmt = wav.readLittleEndian(16)
                    val format = fmt.getShort(0).toInt()
                    val channels = fmt.getShort(2).toInt()
                    val bits = fmt.getShort(14).toInt()
                    if (format != 1 || channels != 1 || bits != 16) return null
                    sampleRate = fmt.getInt(4)
                } else if (chunk == "data") {
                    dataOffset = contentStart
                    dataLength = size.coerceAtMost(wav.length() - contentStart)
                    break
                }
                wav.seek(contentStart + size + (size % 2))
            }
            PcmWavInfo(
                sampleRate = sampleRate ?: return null,
                dataOffset = dataOffset ?: return null,
                dataLength = dataLength ?: return null,
            )
        }
    }.getOrNull()

    /** Reads [length] bytes, throwing EOFException if the file ends first. */
    private fun RandomAccessFile.readLittleEndian(length: Int): ByteBuffer =
        ByteBuffer.wrap(ByteArray(length).also(::readFully)).order(ByteOrder.LITTLE_ENDIAN)

    private fun ByteBuffer.ascii(offset: Int): String = array().decodeToString(offset, offset + 4)
}
