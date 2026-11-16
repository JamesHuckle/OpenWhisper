package com.openwhisper.android.transcription

import java.io.ByteArrayOutputStream
import java.io.File
import java.util.UUID
import org.json.JSONObject

data class TranscriptionHttpRequest(
    val url: String,
    val headers: Map<String, String>,
    val body: ByteArray,
)

open class TranscriptionException(message: String, cause: Throwable? = null) :
    Exception(message, cause)

/** The service answered with an error status, such as 400 for audio it cannot read. */
class TranscriptionHttpException(val statusCode: Int, message: String) :
    TranscriptionException(message)

data class TranscriptionAudio(
    val file: File,
    val mimeType: String,
    val pcmSampleRate: Int? = null,
    val pcmDataOffset: Long = 0,
    val pcmDataLength: Long = file.length(),
)

fun interface TranscriptionClient {
    fun transcribe(audio: TranscriptionAudio): String

    /** Stops a transcription that is running on another thread, if there is one. */
    fun cancel() = Unit
}

class OpenAiRequestFactory(
    private val boundaryFactory: () -> String = { "OpenWhisper-${UUID.randomUUID()}" },
) {
    fun create(
        apiKey: String,
        audio: ByteArray,
        model: String = "gpt-transcribe",
        prompt: String = "",
        mimeType: String = WAV_MIME_TYPE,
    ): TranscriptionHttpRequest {
        require(apiKey.isNotBlank()) { "API key is required" }
        require(audio.isNotEmpty()) { "Audio is required" }
        val extension = if (mimeType == FLAC_MIME_TYPE) "flac" else "wav"
        val boundary = boundaryFactory()
        val output = ByteArrayOutputStream()

        output.writeUtf8("--$boundary\r\n")
        output.writeUtf8("Content-Disposition: form-data; name=\"model\"\r\n\r\n")
        output.writeUtf8(model)
        output.writeUtf8("\r\n")
        if (prompt.isNotBlank()) {
            output.writeUtf8("--$boundary\r\n")
            output.writeUtf8("Content-Disposition: form-data; name=\"prompt\"\r\n\r\n")
            output.writeUtf8(prompt)
            output.writeUtf8("\r\n")
        }
        output.writeUtf8("--$boundary\r\n")
        output.writeUtf8(
            "Content-Disposition: form-data; name=\"file\"; " +
                "filename=\"dictation.$extension\"\r\n",
        )
        output.writeUtf8("Content-Type: $mimeType\r\n\r\n")
        output.write(audio)
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

    companion object {
        const val WAV_MIME_TYPE = "audio/wav"
        const val FLAC_MIME_TYPE = "audio/flac"
    }
}

object OpenAiResponseParser {
    fun parse(statusCode: Int, body: String): String {
        if (statusCode !in 200..299) {
            val message = runCatching {
                JSONObject(body).getJSONObject("error").getString("message")
            }.getOrDefault("Transcription request failed with HTTP $statusCode")
            throw TranscriptionHttpException(statusCode, message)
        }

        // An empty text is a valid answer: a silent part of a long recording.
        return runCatching { JSONObject(body).getString("text") }
            .getOrElse { throw TranscriptionException("Transcription response did not contain text", it) }
    }
}
