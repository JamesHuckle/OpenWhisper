package com.openwhisper.android.transcription

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LongAudioChunkingTest {
    @Test
    fun sixtyMinutesOfPcmIsSplitBelowUploadLimitWithoutLosingBytes() {
        assertChunkPlan(hours = 1)
    }

    @Test
    fun fourHoursOfPcmIsSplitBelowUploadLimitWithoutLosingBytes() {
        assertChunkPlan(hours = 4)
    }

    private fun assertChunkPlan(hours: Int) {
        val pcmBytes = hours * 60L * 60L * 16_000L * 2L
        val maxChunkBytes = 20 * 1024 * 1024
        val chunks = pcmChunkSizes(pcmBytes, maxChunkBytes)

        assertTrue(chunks.size > 1)
        assertTrue(chunks.all { it in 1..maxChunkBytes })
        assertEquals(pcmBytes, chunks.sumOf(Int::toLong))
    }
}
