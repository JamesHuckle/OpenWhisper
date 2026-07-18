package com.openwhisper.android.audio

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
}
