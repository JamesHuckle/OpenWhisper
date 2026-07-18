package com.openwhisper.android.audio

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test

class WavEncoderTest {
    @Test
    fun encodesMono16BitPcmWithValidLittleEndianHeader() {
        val pcm = byteArrayOf(0x01, 0x02, 0x03, 0x04)
        val wav = WavEncoder.encodePcm16Mono(pcm, sampleRate = 16_000)

        assertEquals("RIFF", wav.copyOfRange(0, 4).decodeToString())
        assertEquals(40, littleEndianInt(wav, 4))
        assertEquals("WAVE", wav.copyOfRange(8, 12).decodeToString())
        assertEquals(1, littleEndianShort(wav, 22))
        assertEquals(16_000, littleEndianInt(wav, 24))
        assertEquals(32_000, littleEndianInt(wav, 28))
        assertEquals(16, littleEndianShort(wav, 34))
        assertEquals(4, littleEndianInt(wav, 40))
        assertArrayEquals(pcm, wav.copyOfRange(44, 48))
    }

    private fun littleEndianInt(value: ByteArray, offset: Int): Int =
        (value[offset].toInt() and 0xff) or
            ((value[offset + 1].toInt() and 0xff) shl 8) or
            ((value[offset + 2].toInt() and 0xff) shl 16) or
            ((value[offset + 3].toInt() and 0xff) shl 24)

    private fun littleEndianShort(value: ByteArray, offset: Int): Int =
        (value[offset].toInt() and 0xff) or ((value[offset + 1].toInt() and 0xff) shl 8)
}
