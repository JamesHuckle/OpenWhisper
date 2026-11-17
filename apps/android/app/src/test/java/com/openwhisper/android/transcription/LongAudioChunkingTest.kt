package com.openwhisper.android.transcription

import java.io.File
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LongAudioChunkingTest {
    private val cases = generateSequence(File(System.getProperty("user.dir")).absoluteFile) {
        it.parentFile
    }.map { File(it, "packages/protocol/audio-examples/cases.json") }
        .first(File::isFile)
        .let { JSONObject(it.readText()).getJSONArray("long_recording_parts") }

    /**
     * Android joins a long recording's parts like the desktop app: the
     * shared examples give each part's reply (null for a silent part, which
     * is never sent), the earlier text each prompt quotes, the parts dropped
     * as echoes and the transcript pasted.
     */
    @Test
    fun longRecordingsFollowThePartRulesSharedWithTheDesktop() {
        assertTrue(cases.length() > 0)
        for (index in 0 until cases.length()) {
            val case = cases.getJSONObject(index)
            val about = case.getString("about")
            val replies = case.getJSONArray("parts")
            val quoted = case.getJSONArray("quoted")
            val dropped = case.getJSONArray("dropped").ints()
            val transcripts = mutableListOf<String>()
            for (part in 0 until replies.length()) {
                if (replies.isNull(part)) {
                    assertTrue(about, quoted.isNull(part))
                    transcripts += ""
                    continue
                }
                val previous = AudioRules.previousPartText(transcripts)
                assertEquals(about, quoted.getString(part), previous)
                val reply = replies.getString(part).trim()
                val kept = AudioRules.keptPartText(reply, previous)
                assertEquals(about, if (part in dropped) "" else reply, kept)
                transcripts += kept
            }
            assertEquals(
                about,
                case.getString("transcript"),
                transcripts.filter(String::isNotBlank).joinToString(" "),
            )
        }
    }

    private fun JSONArray.ints(): List<Int> = (0 until length()).map(::getInt)

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
        val maxChunkBytes = AudioRules.MAX_PCM_BYTES_PER_UPLOAD
        val chunks = pcmChunkSizes(pcmBytes, maxChunkBytes)

        assertTrue(chunks.size > 1)
        assertTrue(chunks.all { it in 1..maxChunkBytes })
        assertEquals(pcmBytes, chunks.sumOf(Int::toLong))
    }
}
