package com.openwhisper.android.transcription

import java.io.ByteArrayOutputStream
import java.util.UUID
import org.json.JSONObject

data class TranscriptionHttpRequest(
    val url: String,
    val headers: Map<String, String>,
    val body: ByteArray,
)

class TranscriptionException(message: String, cause: Throwable? = null) :
    Exception(message, cause)

fun interface TranscriptionClient {
    fun transcribe(wav: ByteArray): String
}

class OpenAiRequestFactory(
    private val boundaryFactory: () -> String = { "OpenWhisper-${UUID.randomUUID()}" },
) {
    fun create(
        apiKey: String,
        wav: ByteArray,
        model: String = "gpt-4o-mini-transcribe",
    ): TranscriptionHttpRequest {
        require(apiKey.isNotBlank()) { "API key is required" }
        require(wav.isNotEmpty()) { "WAV audio is required" }
        val boundary = boundaryFactory()
        val output = ByteArrayOutputStream()

        output.writeUtf8("--$boundary\r\n")
        output.writeUtf8("Content-Disposition: form-data; name=\"model\"\r\n\r\n")
        output.writeUtf8(model)
        output.writeUtf8("\r\n")
        output.writeUtf8("--$boundary\r\n")
        output.writeUtf8(
            "Content-Disposition: form-data; name=\"file\"; " +
                "filename=\"dictation.wav\"\r\n",
        )
        output.writeUtf8("Content-Type: audio/wav\r\n\r\n")
        output.write(wav)
        output.writeUtf8("\r\n--$boundary--\r\n")

        return TranscriptionHttpRequest(
            url = "https://api.openai.com/v1/audio/transcriptions",
            headers = mapOf(
                "Authorization" to "Bearer $apiKey",
                "Content-Type" to "multipart/form-data; boundary=$boundary",
                "Accept" to "application/json",
            ),
            body = output.toByteArray(),
        )
    }

    private fun ByteArrayOutputStream.writeUtf8(value: String) {
        write(value.encodeToByteArray())
    }
}

object OpenAiResponseParser {
    fun parse(statusCode: Int, body: String): String {
        if (statusCode !in 200..299) {
            val message = runCatching {
                JSONObject(body).getJSONObject("error").getString("message")
            }.getOrDefault("Transcription request failed with HTTP $statusCode")
            throw TranscriptionException(message)
        }

        val text = runCatching { JSONObject(body).getString("text") }
            .getOrElse { throw TranscriptionException("Transcription response did not contain text", it) }
        if (text.isBlank()) throw TranscriptionException("Transcription was empty")
        return text
    }
}
