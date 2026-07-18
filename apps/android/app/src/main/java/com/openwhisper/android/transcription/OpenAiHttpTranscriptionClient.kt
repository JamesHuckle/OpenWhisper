package com.openwhisper.android.transcription

import java.net.HttpURLConnection
import java.net.URL

class OpenAiHttpTranscriptionClient(
    private val apiKey: () -> String?,
    private val requestFactory: OpenAiRequestFactory = OpenAiRequestFactory(),
) : TranscriptionClient {
    override fun transcribe(wav: ByteArray): String {
        val key = apiKey()?.takeIf { it.isNotBlank() }
            ?: throw TranscriptionException("Save an OpenAI API key or enable demo mode")
        val request = requestFactory.create(key, wav)
        val connection = URL(request.url).openConnection() as HttpURLConnection
        return try {
            connection.requestMethod = "POST"
            connection.connectTimeout = 30_000
            connection.readTimeout = 90_000
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
            throw TranscriptionException("Could not reach the transcription service", error)
        } finally {
            connection.disconnect()
        }
    }
}
