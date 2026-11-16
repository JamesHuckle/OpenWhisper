package com.openwhisper.android.transcription

import com.openwhisper.android.audio.WavEncoder
import java.io.File
import java.io.RandomAccessFile
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference

class OpenAiHttpTranscriptionClient(
    private val apiKey: () -> String?,
    private val requestFactory: OpenAiRequestFactory = OpenAiRequestFactory(),
    private val openConnection: (String) -> HttpURLConnection = { url ->
        URL(url).openConnection() as HttpURLConnection
    },
) : TranscriptionClient {
    private val generation = AtomicLong(0)
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
        return transcribeRequest(key, audio.file.readBytes(), "", started).trim()
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
        val transcripts = mutableListOf<String>()
        RandomAccessFile(audio.file, "r").use { source ->
            source.seek(audio.pcmDataOffset)
            for (count in pcmChunkSizes(audio.pcmDataLength, AudioRules.MAX_PCM_BYTES_PER_UPLOAD)) {
                val pcm = ByteArray(count)
                source.readFully(pcm)
                val wav = WavEncoder.encodePcm16Mono(pcm, sampleRate)
                val prompt = transcripts.lastOrNull(String::isNotBlank)?.takeLast(1_200).orEmpty()
                val text = transcribeRequest(key, wav, prompt, started).trim()
                // Each later part is sent with the previous part's text as its
                // prompt; a silent part makes the model repeat it.
                transcripts += if (AudioRules.echoesPrompt(text, prompt)) "" else text
            }
        }
        return transcripts.filter(String::isNotBlank).joinToString(" ").trim()
            .ifBlank { throw NoSpeechException("empty_transcript") }
    }

    private fun transcribeRequest(
        key: String,
        wav: ByteArray,
        prompt: String,
        started: Long,
    ): String {
        throwIfCancelled(started)
        val request = requestFactory.create(key, wav, prompt = prompt)
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
