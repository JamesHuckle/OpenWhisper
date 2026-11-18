package com.openwhisper.android.transcription

import com.openwhisper.android.BuildConfig
import com.openwhisper.android.audio.WavEncoder
import java.io.File
import java.io.RandomAccessFile
import kotlin.math.abs

/**
 * The dictation rules the desktop app also follows. The values come from
 * packages/protocol/audio-rules.json when the app is built, and the example
 * recordings in packages/protocol/audio-examples test both apps.
 */
object AudioRules {
    /** A recording shorter than this is an accidental tap and is not sent. */
    const val MIN_SPEECH_SECONDS: Double = BuildConfig.MIN_SPEECH_SECONDS

    /** 16-bit audio that never reaches this level is a muted microphone. */
    const val SILENT_PCM_PEAK: Int = BuildConfig.SILENT_PCM_PEAK

    /** A transcript that repeats at least this share of its prompt is no speech. */
    const val PROMPT_ECHO_MIN_SHARE: Double = BuildConfig.PROMPT_ECHO_MIN_SHARE

    /** The largest file sent in one transcription request. */
    const val MAX_UPLOAD_BYTES: Long = BuildConfig.MAX_UPLOAD_BYTES

    /** The most PCM sent in one request: the upload limit less the WAV header, in whole samples. */
    val MAX_PCM_BYTES_PER_UPLOAD: Int =
        (MAX_UPLOAD_BYTES - WavEncoder.HEADER_BYTES).let { it - it % 2 }.toInt()

    const val NO_AUDIO_MESSAGE: String = BuildConfig.NO_AUDIO_MESSAGE

    private const val SCAN_BYTES = 1024 * 1024

    /** How much of the text before it a long recording's next part is prompted with. */
    private const val PREVIOUS_PART_CHARS = 1_200
    // Letters and digits of any script, as the desktop app reads them.
    private val WORD = Regex("[\\p{IsAlphabetic}\\p{N}]+")

    /**
     * Why 16-bit mono PCM holds no speech ("too_short" or "silent"), or null
     * when it is worth sending. [dataOffset] and [dataLength] place the
     * samples in [file].
     */
    fun noSpeechReason(file: File, sampleRate: Int, dataOffset: Long, dataLength: Long): String? {
        require(sampleRate > 0) { "sampleRate must be positive" }
        val seconds = (dataLength / 2).toDouble() / sampleRate
        if (seconds < MIN_SPEECH_SECONDS) return "too_short"
        return if (isSilent(file, dataOffset, dataLength)) "silent" else null
    }

    /**
     * Transcription models answer silent or near-silent audio by repeating the
     * prompt they were sent. Such a transcript is no speech, not dictation.
     */
    fun echoesPrompt(text: String, prompt: String): Boolean {
        val spoken = normalizeWords(text)
        val hints = normalizeWords(prompt)
        if (spoken.isEmpty() || hints.isEmpty()) return false
        return hints.contains(spoken) && spoken.length >= hints.length * PROMPT_ECHO_MIN_SHARE
    }

    /**
     * The text a long recording's next part is prompted with: the end of the
     * last part that had text.
     */
    fun previousPartText(transcripts: List<String>): String =
        transcripts.lastOrNull(String::isNotBlank)?.takeLast(PREVIOUS_PART_CHARS).orEmpty()

    /**
     * A later part's text, or nothing when it only repeats [previous], the
     * earlier text its prompt quoted: a part that is mostly silence makes the
     * model say its prompt back, which would paste a sentence twice. The
     * desktop app keeps the same rule; both apps test it on
     * packages/protocol/audio-examples/cases.json (long_recording_parts).
     */
    fun keptPartText(text: String, previous: String): String =
        if (echoesPrompt(text, previous)) "" else text

    private fun normalizeWords(text: String): String =
        WORD.findAll(text.lowercase()).joinToString(" ") { it.value }

    /**
     * True when the 16-bit PCM at [dataOffset] and [dataLength] in [file]
     * never reaches [SILENT_PCM_PEAK]. The same check decides whether a whole
     * recording is a muted microphone and whether one part of a long
     * recording is worth uploading.
     */
    fun isSilent(file: File, dataOffset: Long, dataLength: Long): Boolean {
        RandomAccessFile(file, "r").use { source ->
            source.seek(dataOffset)
            val block = ByteArray(SCAN_BYTES)
            var remaining = dataLength - dataLength % 2
            while (remaining > 0) {
                val count = source.read(block, 0, minOf(remaining, SCAN_BYTES.toLong()).toInt())
                if (count <= 0) return true
                val usable = count - count % 2
                var index = 0
                while (index < usable) {
                    val sample = ((block[index].toInt() and 0xff) or
                        (block[index + 1].toInt() shl 8)).toShort().toInt()
                    if (abs(sample) >= SILENT_PCM_PEAK) return false
                    index += 2
                }
                remaining -= count
            }
            return true
        }
    }
}

/** The recording holds no speech; [reason] says why, for tests and logs. */
class NoSpeechException(val reason: String) : TranscriptionException(AudioRules.NO_AUDIO_MESSAGE)
