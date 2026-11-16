package com.openwhisper.android.transcription

import androidx.core.util.AtomicFile
import java.io.File
import java.io.IOException
import java.security.MessageDigest
import org.json.JSONArray
import org.json.JSONObject

/** One finished part of a long recording: its bytes, prompt and text. */
internal data class SavedPart(
    val start: Long,
    val end: Long,
    val prompt: String,
    /** May be empty, for a silent part. */
    val text: String,
)

/**
 * The finished parts of a long transcription, saved beside its recording.
 *
 * Each part's text is saved with [AtomicFile] as soon as it arrives, so a
 * retry after a failure uploads (and pays for) only the parts that did not
 * finish. The file is tagged with a key made from the audio's size and
 * modified time, the model, the prompt and [CHUNKING_VERSION]: when any of
 * them changes, the saved parts are ignored and the transcription starts
 * fresh. The recording store deletes the file when the transcription
 * completes or the recording is pruned. Without a file nothing is saved.
 */
internal class SavedParts(private val file: File?, private val key: String) {
    private val parts: MutableList<SavedPart> = file?.let(::load).orEmpty().toMutableList()

    /** The saved part [index], when it covers exactly these bytes. */
    fun get(index: Int, start: Long, end: Long): SavedPart? =
        parts.getOrNull(index)?.takeIf { it.start == start && it.end == end }

    /**
     * Keeps a finished part and saves every part so far. A part that cannot
     * be saved is only lost for a retry, which then sends it again.
     */
    fun add(index: Int, part: SavedPart) {
        while (parts.size > index) parts.removeAt(parts.lastIndex)
        parts += part
        val target = file ?: return
        val json = JSONObject()
            .put("key", key)
            .put(
                "parts",
                JSONArray(
                    parts.map {
                        JSONObject()
                            .put("start", it.start)
                            .put("end", it.end)
                            .put("prompt", it.prompt)
                            .put("text", it.text)
                    },
                ),
            )
        val atomic = AtomicFile(target)
        val output = try {
            atomic.startWrite()
        } catch (_: IOException) {
            return
        }
        try {
            output.write(json.toString().encodeToByteArray())
            atomic.finishWrite(output)
        } catch (_: IOException) {
            atomic.failWrite(output)
        }
    }

    private fun load(file: File): List<SavedPart> = try {
        val json = JSONObject(AtomicFile(file).readFully().decodeToString())
        if (json.getString("key") != key) {
            emptyList()
        } else {
            val saved = json.getJSONArray("parts")
            List(saved.length()) { index ->
                val part = saved.getJSONObject(index)
                SavedPart(
                    start = part.getLong("start"),
                    end = part.getLong("end"),
                    prompt = part.getString("prompt"),
                    text = part.getString("text"),
                )
            }
        }
    } catch (_: Exception) {
        // Missing or damaged: start fresh.
        emptyList()
    }

    companion object {
        /**
         * Raise this when the way a recording is split into parts, or the
         * prompt each part is sent with, changes: older saved parts then no
         * longer match.
         */
        const val CHUNKING_VERSION = 1

        fun key(audio: File, model: String, prompt: String): String {
            val material = JSONArray()
                .put(CHUNKING_VERSION)
                .put(audio.length())
                .put(audio.lastModified())
                .put(model)
                .put(prompt)
                .toString()
            return MessageDigest.getInstance("SHA-256")
                .digest(material.encodeToByteArray())
                .joinToString("") { "%02x".format(it) }
        }
    }
}
