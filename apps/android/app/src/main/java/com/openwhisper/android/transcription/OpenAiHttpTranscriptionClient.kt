package com.openwhisper.android.transcription

import com.openwhisper.android.audio.FlacFileEncoder
import com.openwhisper.android.audio.WavEncoder
import java.io.File
import java.io.RandomAccessFile
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference

class OpenAiHttpTranscriptionClient(
    private val apiKey: () -> String?,
    private val requestFactory: OpenAiRequestFactory = OpenAiRequestFactory(),
    private val openConnection: (String) -> HttpURLConnection = { url ->
        URL(url).openConnection() as HttpURLConnection
    },
    /** Encodes 16-bit mono PCM at a sample rate to FLAC, or throws. */
    private val encodeFlac: (ByteArray, Int) -> ByteArray = FlacFileEncoder::encodePcm16Mono,
) : TranscriptionClient {
    private val generation = AtomicLong(0)

    // Set when the service refused this phone's FLAC but took the same audio
    // as WAV, so later parts and dictations skip the refused upload.
    private val flacRefused = AtomicBoolean(false)
    private val activeConnection = AtomicReference<HttpURLConnection?>(null)

    // Makes cancelling and registering a connection one step each, so a
    // cancelled call can never replace a newer call's connection.
    private val connectionLock = Any()

    /**
     * Closes the upload in progress, so the background thread is free at once
     * instead of waiting for a result nobody will see.
     */
    override fun cancel() {
        synchronized(connectionLock) {
            generation.incrementAndGet()
            activeConnection.getAndSet(null)
        }?.disconnect()
    }

    override fun transcribe(audio: TranscriptionAudio): String {
        val started = generation.get()
        val key = apiKey()?.takeIf { it.isNotBlank() }
            ?: throw TranscriptionException("Save an OpenAI API key or enable demo mode")
        if (!audio.file.exists() || audio.file.length() == 0L) {
            throw TranscriptionException("The saved audio file is empty")
        }
        if (audio.pcmSampleRate != null) return transcribePcm(key, audio, started)
        if (audio.file.length() > AudioRules.MAX_UPLOAD_BYTES) {
            throw TranscriptionException(
                "This audio file is larger than ${AudioRules.MAX_UPLOAD_BYTES / (1024 * 1024)} MB. " +
                    "Use a smaller MP3/M4A, split the file, or import a PCM WAV file.",
            )
        }
        return transcribeRequest(key, audio.file.readBytes(), WAV, "", started).trim()
            .ifBlank { throw NoSpeechException("empty_transcript") }
    }

    private fun transcribePcm(key: String, audio: TranscriptionAudio, started: Long): String {
        val sampleRate = requireNotNull(audio.pcmSampleRate)
        // A tap or a muted microphone is never uploaded: it costs money and
        // the service rejects it or answers with the prompt.
        AudioRules.noSpeechReason(
            audio.file,
            sampleRate,
            audio.pcmDataOffset,
            audio.pcmDataLength,
        )?.let { throw NoSpeechException(it) }
        // Each finished part is saved, so a retry after a failure sends only
        // the parts that did not finish.
        val saved = SavedParts(
            audio.partsFile,
            SavedParts.key(audio.file, OpenAiRequestFactory.MODEL, prompt = ""),
        )
        val transcripts = mutableListOf<String>()
        RandomAccessFile(audio.file, "r").use { source ->
            var start = audio.pcmDataOffset
            val counts = pcmChunkSizes(audio.pcmDataLength, AudioRules.MAX_PCM_BYTES_PER_UPLOAD)
            for ((index, count) in counts.withIndex()) {
                val end = start + count
                val done = saved.get(index, start, end)
                if (done != null) {
                    // Finished on an earlier try: its text is reused, and
                    // builds the next part's prompt as before.
                    transcripts += done.text
                    start = end
                    continue
                }
                val pcm = ByteArray(count)
                source.seek(start)
                source.readFully(pcm)
                val prompt = transcripts.lastOrNull(String::isNotBlank)?.takeLast(1_200).orEmpty()
                val text = transcribePart(key, pcm, sampleRate, prompt, started).trim()
                // Each later part is sent with the previous part's text as its
                // prompt; a silent part makes the model repeat it.
                val kept = if (AudioRules.echoesPrompt(text, prompt)) "" else text
                saved.add(index, SavedPart(start, end, prompt, kept))
                transcripts += kept
                start = end
            }
        }
        return transcripts.filter(String::isNotBlank).joinToString(" ").trim()
            .ifBlank { throw NoSpeechException("empty_transcript") }
    }

    /**
     * Sends one part as lossless FLAC, about half the bytes of WAV for
     * speech. It goes as WAV when the phone cannot encode FLAC, and once more
     * as WAV when the service refuses the FLAC.
     */
    private fun transcribePart(
        key: String,
        pcm: ByteArray,
        sampleRate: Int,
        prompt: String,
        started: Long,
    ): String {
        val flac = if (flacRefused.get()) {
            null
        } else {
            try {
                encodeFlac(pcm, sampleRate).takeIf(ByteArray::isNotEmpty)
            } catch (_: Exception) {
                null
            }
        }
        if (flac != null) {
            try {
                return transcribeRequest(key, flac, FLAC, prompt, started)
            } catch (error: TranscriptionHttpException) {
                if (error.statusCode != 400) throw error
            }
        }
        val wav = WavEncoder.encodePcm16Mono(pcm, sampleRate)
        return transcribeRequest(key, wav, WAV, prompt, started)
            .also { if (flac != null) flacRefused.set(true) }
    }

    private fun transcribeRequest(
        key: String,
        audio: ByteArray,
        mimeType: String,
        prompt: String,
        started: Long,
    ): String {
        throwIfCancelled(started)
        val request = requestFactory.create(key, audio, prompt = prompt, mimeType = mimeType)
        val connection = openConnection(request.url)
        val registered = synchronized(connectionLock) {
            // A cancel that came just before the connection was registered.
            (generation.get() == started).also { if (it) activeConnection.set(connection) }
        }
        if (!registered) {
            connection.disconnect()
            throw TranscriptionException(CANCELLED_MESSAGE)
        }
        return try {
            connection.requestMethod = "POST"
            connection.connectTimeout = 30_000
            connection.readTimeout = 30 * 60 * 1_000
            connection.doOutput = true
            request.headers.forEach(connection::setRequestProperty)
            connection.setFixedLengthStreamingMode(request.body.size)
            connection.outputStream.use { it.write(request.body) }

            val status = connection.responseCode
            val stream = if (status in 200..299) connection.inputStream else connection.errorStream
            val response = stream?.bufferedReader(Charsets.UTF_8)?.use { it.readText() }.orEmpty()
            OpenAiResponseParser.parse(status, response)
        } catch (error: TranscriptionException) {
            throw error
        } catch (error: Exception) {
            throwIfCancelled(started)
            val detail = error.message?.takeIf(String::isNotBlank)
            throw TranscriptionException(
                "Could not reach the transcription service" +
                    if (detail == null) "" else ": $detail",
                error,
            )
        } finally {
            activeConnection.compareAndSet(connection, null)
            connection.disconnect()
        }
    }

    private fun throwIfCancelled(started: Long) {
        if (generation.get() != started) throw TranscriptionException(CANCELLED_MESSAGE)
    }

    companion object {
        const val CANCELLED_MESSAGE = "Transcription cancelled"
        private const val WAV = OpenAiRequestFactory.WAV_MIME_TYPE
        private const val FLAC = OpenAiRequestFactory.FLAC_MIME_TYPE
    }
}

internal fun pcmChunkSizes(totalBytes: Long, maxChunkBytes: Int): List<Int> {
    require(totalBytes >= 0) { "totalBytes must not be negative" }
    require(maxChunkBytes > 0) { "maxChunkBytes must be positive" }
    val chunks = mutableListOf<Int>()
    var remaining = totalBytes
    while (remaining > 0) {
        val size = minOf(remaining, maxChunkBytes.toLong()).toInt()
        chunks += size
        remaining -= size
    }
    return chunks
}
