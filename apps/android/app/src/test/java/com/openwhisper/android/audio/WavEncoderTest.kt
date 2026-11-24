package com.openwhisper.android.audio

import java.io.File
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class WavEncoderTest {
    @get:Rule
    val temporaryFolder = TemporaryFolder()

    @Test
    fun encodesMono16BitPcmWithValidLittleEndianHeader() {
        val pcm = byteArrayOf(0x01, 0x02, 0x03, 0x04)
        val wav = pcm16MonoWav(pcm, sampleRate = 16_000)

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

    @Test
    fun headerMatchesEveryExpectedByte() {
        val expected = ascii("RIFF") + bytes(0x28, 0x5f, 0xd0, 0xb2) + ascii("WAVE") +
            ascii("fmt ") + bytes(0x10, 0, 0, 0) + bytes(1, 0) + bytes(1, 0) +
            bytes(0x44, 0xac, 0, 0) + bytes(0x88, 0x58, 0x01, 0) + bytes(2, 0) + bytes(0x10, 0) +
            ascii("data") + bytes(0x04, 0x5f, 0xd0, 0xb2)

        assertArrayEquals(expected, WavEncoder.header(3_000_000_260L, sampleRate = 44_100))
        assertArrayEquals(
            ascii("RIFF") + bytes(0xff, 0xff, 0xff, 0xff) + ascii("WAVE") +
                ascii("fmt ") + bytes(0x10, 0, 0, 0) + bytes(1, 0) + bytes(1, 0) +
                bytes(0x80, 0x3e, 0, 0) + bytes(0, 0x7d, 0, 0) + bytes(2, 0) + bytes(0x10, 0) +
                ascii("data") + bytes(0xdb, 0xff, 0xff, 0xff),
            WavEncoder.header(0xffffffffL - 36, sampleRate = 16_000),
        )
    }

    @Test
    fun headerOfARecordingOverTwoGigabytesKeepsItsUnsignedSizes() {
        // A long recording in progress rewrites this header as it grows.
        val pcmBytes = 3_000_000_000L
        val header = WavEncoder.header(pcmBytes, sampleRate = 16_000)

        assertEquals(WavEncoder.HEADER_BYTES, header.size)
        assertEquals(36 + pcmBytes, littleEndianInt(header, 4).toLong() and 0xffffffffL)
        assertEquals(pcmBytes, littleEndianInt(header, 40).toLong() and 0xffffffffL)
        assertThrows(IllegalArgumentException::class.java) {
            WavEncoder.header(0xffffffffL, sampleRate = 16_000)
        }
        assertThrows(IllegalArgumentException::class.java) {
            WavEncoder.header(-1, sampleRate = 16_000)
        }
        assertThrows(IllegalArgumentException::class.java) {
            WavEncoder.header(0, sampleRate = 0)
        }
    }

    @Test
    fun inspectsTheWavTheEncoderWrites() {
        val pcm = ByteArray(10) { it.toByte() }
        val info = inspect(pcm16MonoWav(pcm, sampleRate = 48_000))

        assertEquals(PcmWavInfo(48_000, 44, 10), info)
    }

    @Test
    fun inspectSkipsOtherChunksAndTheirOddPadding() {
        val wav = ascii("RIFF") + uint32(0) + ascii("WAVE") +
            ascii("LIST") + uint32(3) + bytes(9, 9, 9, 0) +
            fmtChunk(sampleRate = 22_050, format = 1, channels = 1, bits = 16, size = 18) +
            ascii("junk") + uint32(5) + bytes(1, 2, 3, 4, 5, 0) +
            ascii("data") + uint32(6) + ByteArray(6) +
            ByteArray(6)

        val dataOffset = 12L + 12 + 26 + 14 + 8
        assertEquals(PcmWavInfo(22_050, dataOffset, 6), inspect(wav))
    }

    @Test
    fun inspectReadsSizesAboveTheSignedIntRange() {
        val wav = ascii("RIFF") + uint32(0) + ascii("WAVE") +
            fmtChunk(sampleRate = 16_000, format = 1, channels = 1, bits = 16) +
            ascii("data") + uint32(0xfffffff0L) + ByteArray(4)

        assertEquals(PcmWavInfo(16_000, 44, 4), inspect(wav))
    }

    @Test
    fun inspectCapsATruncatedDataChunkAtTheBytesPresent() {
        val wav = WavEncoder.header(1_000, sampleRate = 16_000) + ByteArray(8)

        assertEquals(PcmWavInfo(16_000, 44, 8), inspect(wav))
    }

    @Test
    fun inspectReturnsNullForUnsupportedOrUnreadableWavs() {
        val pcm = ByteArray(8)
        fun withFmt(format: Int = 1, channels: Int = 1, bits: Int = 16) =
            ascii("RIFF") + uint32(0) + ascii("WAVE") +
                fmtChunk(16_000, format, channels, bits) + ascii("data") + uint32(8) + pcm

        assertNull(inspect(withFmt(format = 3)))
        assertNull(inspect(withFmt(channels = 2)))
        assertNull(inspect(withFmt(bits = 8)))
        assertNull(inspect(ByteArray(43)))
        assertNull(inspect(ascii("RIFX") + pcm16MonoWav(pcm, 16_000).copyOfRange(4, 52)))
        assertNull(inspect(pcm16MonoWav(pcm, 16_000).also { it[8] = 'X'.code.toByte() }))
        // No fmt chunk before the data chunk.
        assertNull(
            inspect(
                ascii("RIFF") + uint32(0) + ascii("WAVE") +
                    ascii("data") + uint32(24) + ByteArray(24),
            ),
        )
        // No data chunk.
        assertNull(
            inspect(
                ascii("RIFF") + uint32(0) + ascii("WAVE") +
                    fmtChunk(16_000, 1, 1, 16) + ascii("junk") + uint32(4) + ByteArray(4),
            ),
        )
        // A fmt chunk cut off before its bits-per-sample field.
        assertNull(
            inspect(
                ascii("RIFF") + uint32(0) + ascii("WAVE") +
                    ascii("junk") + uint32(10) + ByteArray(10) +
                    ascii("fmt ") + uint32(16) + bytes(1, 0, 1, 0) + uint32(16_000) + bytes(0, 0),
            ),
        )
        assertNull(WavEncoder.inspectPcm16Mono(File(temporaryFolder.root, "missing.wav")))
    }

    private fun inspect(wav: ByteArray): PcmWavInfo? =
        WavEncoder.inspectPcm16Mono(temporaryFolder.newFile().apply { writeBytes(wav) })

    private fun fmtChunk(
        sampleRate: Int,
        format: Int,
        channels: Int,
        bits: Int,
        size: Int = 16,
    ): ByteArray =
        ascii("fmt ") + uint32(size.toLong()) + uint16(format) + uint16(channels) +
            uint32(sampleRate.toLong()) + uint32(sampleRate * 2L) + uint16(2) + uint16(bits) +
            ByteArray(size - 16)

    private fun ascii(value: String): ByteArray = value.encodeToByteArray()

    private fun bytes(vararg values: Int): ByteArray = ByteArray(values.size) { values[it].toByte() }

    private fun uint16(value: Int): ByteArray = bytes(value and 0xff, value shr 8 and 0xff)

    private fun uint32(value: Long): ByteArray =
        ByteArray(4) { (value shr (it * 8) and 0xff).toByte() }

    private fun littleEndianInt(value: ByteArray, offset: Int): Int =
        (value[offset].toInt() and 0xff) or
            ((value[offset + 1].toInt() and 0xff) shl 8) or
            ((value[offset + 2].toInt() and 0xff) shl 16) or
            ((value[offset + 3].toInt() and 0xff) shl 24)

    private fun littleEndianShort(value: ByteArray, offset: Int): Int =
        (value[offset].toInt() and 0xff) or ((value[offset + 1].toInt() and 0xff) shl 8)
}
