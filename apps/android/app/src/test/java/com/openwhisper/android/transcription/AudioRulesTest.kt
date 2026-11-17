package com.openwhisper.android.transcription

import com.openwhisper.android.audio.WavEncoder
import java.io.File
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Android decides like the desktop app on the shared example audio. */
class AudioRulesTest {
    private val protocolDir: File = generateSequence(File(System.getProperty("user.dir")).absoluteFile) {
        it.parentFile
    }.map { File(it, "packages/protocol") }
        .first { File(it, "audio-rules.json").isFile }
    private val examplesDir = File(protocolDir, "audio-examples")
    private val cases = JSONObject(File(examplesDir, "cases.json").readText())

    @Test
    fun theBuildUsesExactlyTheSharedRules() {
        val shared = JSONObject(File(protocolDir, "audio-rules.json").readText())
        assertEquals(shared.getDouble("min_speech_seconds"), AudioRules.MIN_SPEECH_SECONDS, 0.0)
        assertEquals(shared.getInt("silent_pcm_peak"), AudioRules.SILENT_PCM_PEAK)
        assertEquals(
            shared.getDouble("prompt_echo_min_share"),
            AudioRules.PROMPT_ECHO_MIN_SHARE,
            0.0,
        )
        assertEquals(shared.getLong("max_upload_bytes"), AudioRules.MAX_UPLOAD_BYTES)
        assertEquals(shared.getString("no_audio_message"), AudioRules.NO_AUDIO_MESSAGE)
    }

    @Test
    fun everyExampleRecordingGetsItsExpectedAnswer() {
        val audio = cases.getJSONArray("audio")
        assertTrue(audio.length() > 0)
        for (index in 0 until audio.length()) {
            val case = audio.getJSONObject(index)
            val file = File(examplesDir, case.getString("file"))
            val wav = WavEncoder.inspectPcm16Mono(file)
            assertNotNull(file.name, wav)
            val reason = AudioRules.noSpeechReason(
                file,
                wav!!.sampleRate,
                wav.dataOffset,
                wav.dataLength,
            )
            assertEquals(file.name, case.getString("expected"), reason ?: "speech")
        }
    }

    @Test
    fun everyExampleTranscriptGetsItsExpectedAnswer() {
        val echoes = cases.getJSONArray("prompt_echo")
        assertTrue(echoes.length() > 0)
        for (index in 0 until echoes.length()) {
            val case = echoes.getJSONObject(index)
            assertEquals(
                case.getString("about"),
                case.getBoolean("expected"),
                AudioRules.echoesPrompt(case.getString("text"), case.getString("prompt")),
            )
        }
    }

    @Test
    fun eachUploadIsAWavNoLargerThanTheSharedLimit() {
        val pcm = AudioRules.MAX_PCM_BYTES_PER_UPLOAD.toLong()
        assertEquals(0L, pcm % 2)
        assertTrue(pcm + WavEncoder.HEADER_BYTES <= AudioRules.MAX_UPLOAD_BYTES)
        assertTrue(pcm + WavEncoder.HEADER_BYTES + 2 > AudioRules.MAX_UPLOAD_BYTES)
    }
}
