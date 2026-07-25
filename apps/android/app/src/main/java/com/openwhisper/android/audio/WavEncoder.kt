package com.openwhisper.android.audio

import java.io.File
import java.io.RandomAccessFile

data class PcmWavInfo(
    val sampleRate: Int,
    val dataOffset: Long,
    val dataLength: Long,
)

object WavEncoder {
    fun encodePcm16Mono(pcm: ByteArray, sampleRate: Int): ByteArray {
        require(sampleRate > 0) { "sampleRate must be positive" }
        val result = ByteArray(44 + pcm.size)
        writeAscii(result, 0, "RIFF")
        writeInt(result, 4, 36 + pcm.size)
        writeAscii(result, 8, "WAVE")
        writeAscii(result, 12, "fmt ")
        writeInt(result, 16, 16)
        writeShort(result, 20, 1)
        writeShort(result, 22, 1)
        writeInt(result, 24, sampleRate)
        writeInt(result, 28, sampleRate * 2)
        writeShort(result, 32, 2)
        writeShort(result, 34, 16)
        writeAscii(result, 36, "data")
        writeInt(result, 40, pcm.size)
        pcm.copyInto(result, destinationOffset = 44)
        return result
    }

    fun inspectPcm16Mono(file: File): PcmWavInfo? = runCatching {
        RandomAccessFile(file, "r").use { wav ->
            if (wav.length() < 44 || wav.readAscii(4) != "RIFF") return null
            wav.skipBytes(4)
            if (wav.readAscii(4) != "WAVE") return null
            var sampleRate: Int? = null
            var dataOffset: Long? = null
            var dataLength: Long? = null
            while (wav.filePointer + 8 <= wav.length()) {
                val chunk = wav.readAscii(4)
                val size = wav.readUInt32Le()
                val contentStart = wav.filePointer
                if (chunk == "fmt " && size >= 16) {
                    val format = wav.readUInt16Le()
                    val channels = wav.readUInt16Le()
                    val rate = wav.readUInt32Le().toInt()
                    wav.skipBytes(6)
                    val bits = wav.readUInt16Le()
                    if (format != 1 || channels != 1 || bits != 16) return null
                    sampleRate = rate
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

    private fun writeAscii(target: ByteArray, offset: Int, value: String) {
        value.encodeToByteArray().copyInto(target, destinationOffset = offset)
    }

    private fun writeInt(target: ByteArray, offset: Int, value: Int) {
        target[offset] = value.toByte()
        target[offset + 1] = (value ushr 8).toByte()
        target[offset + 2] = (value ushr 16).toByte()
        target[offset + 3] = (value ushr 24).toByte()
    }

    private fun writeShort(target: ByteArray, offset: Int, value: Int) {
        target[offset] = value.toByte()
        target[offset + 1] = (value ushr 8).toByte()
    }

    private fun RandomAccessFile.readAscii(length: Int): String =
        ByteArray(length).also(::readFully).decodeToString()

    private fun RandomAccessFile.readUInt16Le(): Int {
        val low = read()
        val high = read()
        check(low >= 0 && high >= 0) { "Unexpected end of WAV" }
        return low or (high shl 8)
    }

    private fun RandomAccessFile.readUInt32Le(): Long {
        var value = 0L
        repeat(4) { shift ->
            val byte = read()
            check(byte >= 0) { "Unexpected end of WAV" }
            value = value or (byte.toLong() shl (shift * 8))
        }
        return value
    }
}
