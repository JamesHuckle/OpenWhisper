package com.openwhisper.android.transcription

import com.openwhisper.android.audio.WavEncoder
import java.io.File
import java.io.RandomAccessFile
import java.net.HttpURLConnection
import java.net.URL

class OpenAiHttpTranscriptionClient(
    private val apiKey: () -> String?,
    private val requestFactory: OpenAiRequestFactory = OpenAiRequestFactory(),
) : TranscriptionClient {
    override fun transcribe(audio: TranscriptionAudio): String {
        val key = apiKey()?.takeIf { it.isNotBlank() }
            ?: throw TranscriptionException("Save an OpenAI API key or enable demo mode")
        if (!audio.file.exists() || audio.file.length() == 0L) {
            throw TranscriptionException("The saved audio file is empty")
        }
        if (audio.pcmSampleRate != null) return transcribePcm(key, audio)
        if (audio.file.length() > MAX_UPLOAD_BYTES) {
            throw TranscriptionException(
                "This audio file is larger than 24 MB. Use a smaller MP3/M4A, " +
                    "split the file, or import a PCM WAV file.",
            )
        }
        return transcribeRequest(key, audio.file.readBytes(), "")
    }

    private fun transcribePcm(key: String, audio: TranscriptionAudio): String {
        val sampleRate = requireNotNull(audio.pcmSampleRate)
        val transcripts = mutableListOf<String>()
        val bytesPerChunk = MAX_PCM_CHUNK_BYTES - (MAX_PCM_CHUNK_BYTES % 2)
        RandomAccessFile(audio.file, "r").use { source ->
            source.seek(audio.pcmDataOffset)
            for (count in pcmChunkSizes(audio.pcmDataLength, bytesPerChunk)) {
                val pcm = ByteArray(count)
                source.readFully(pcm)
                val wav = WavEncoder.encodePcm16Mono(pcm, sampleRate)
                val prompt = transcripts.lastOrNull()?.takeLast(1_200).orEmpty()
                transcripts += transcribeRequest(key, wav, prompt)
            }
        }
        return transcripts.filter(String::isNotBlank).joinToString(" ").trim()
            .ifBlank { throw TranscriptionException("Transcription was empty") }
    }

    private fun transcribeRequest(key: String, wav: ByteArray, prompt: String): String {
        val request = requestFactory.create(key, wav, prompt = prompt)
        val connection = URL(request.url).openConnection() as HttpURLConnection
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
            val detail = error.message?.takeIf(String::isNotBlank)
            throw TranscriptionException(
                "Could not reach the transcription service" +
                    if (detail == null) "" else ": $detail",
                error,
            )
        } finally {
            connection.disconnect()
        }
    }

    private companion object {
        const val MAX_UPLOAD_BYTES = 24L * 1024L * 1024L
        const val MAX_PCM_CHUNK_BYTES = 20 * 1024 * 1024
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
