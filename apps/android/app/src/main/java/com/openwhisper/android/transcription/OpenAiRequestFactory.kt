package com.openwhisper.android.transcription

import com.openwhisper.android.audio.WavEncoder
import java.io.EOFException
import java.io.File
import java.io.OutputStream
import java.io.RandomAccessFile
import java.util.UUID
import org.json.JSONObject

/**
 * A request whose body is written straight into the connection, so a long
 * recording is never held in memory: [contentLength] bytes, written by
 * [writeBody].
 */
class TranscriptionHttpRequest(
    val url: String,
    val headers: Map<String, String>,
    val contentLength: Long,
    private val body: (OutputStream, () -> Unit) -> Unit,
) {
    /** Writes the body, calling [checkCancelled] between blocks of audio. */
    fun writeBody(output: OutputStream, checkCancelled: () -> Unit = {}) = body(output, checkCancelled)
}

/**
 * The audio file one request uploads: [head] first, then [length] bytes of
 * [file] from [offset], copied through a small buffer rather than read whole.
 */
class UploadAudio private constructor(
    val fileName: String,
    val mimeType: String,
    private val head: ByteArray,
    private val file: File?,
    private val offset: Long,
    private val length: Long,
) {
    val size: Long = head.size + length

    fun writeTo(output: OutputStream, checkCancelled: () -> Unit) {
        output.write(head)
        if (file == null || length == 0L) return
        RandomAccessFile(file, "r").use { source ->
            source.seek(offset)
            val buffer = ByteArray(COPY_BUFFER_BYTES)
            var remaining = length
            while (remaining > 0) {
                checkCancelled()
                val count = source.read(buffer, 0, minOf(remaining, buffer.size.toLong()).toInt())
                if (count < 0) throw EOFException("The audio file ended early")
                output.write(buffer, 0, count)
                remaining -= count
            }
        }
    }

    companion object {
        /** The size of each block copied from the file into the request. */
        const val COPY_BUFFER_BYTES = 64 * 1024

        /** Audio already in memory, such as an encoded FLAC part. */
        fun bytes(fileName: String, mimeType: String, audio: ByteArray) =
            UploadAudio(fileName, mimeType, audio, null, 0, 0)

        /** A whole file, sent with its own name and type. */
        fun file(file: File, mimeType: String) =
            UploadAudio(file.name, mimeType, ByteArray(0), file, 0, file.length())

        /**
         * [length] bytes of 16-bit mono PCM from [offset] in [file], sent as a
         * WAV file behind a generated header.
         */
        fun wav(file: File, offset: Long, length: Long, sampleRate: Int) = UploadAudio(
            "dictation.wav",
            OpenAiRequestFactory.WAV_MIME_TYPE,
            WavEncoder.header(length, sampleRate),
            file,
            offset,
            length,
        )
    }
}

open class TranscriptionException(message: String, cause: Throwable? = null) :
    Exception(message, cause)

/**
 * The service answered with an error status, such as 400 for audio it cannot
 * read, with the API's error code and, for a rate limit, how many seconds it
 * asked us to wait.
 */
class TranscriptionHttpException(
    val statusCode: Int,
    message: String,
    val code: String? = null,
    val retryAfterSeconds: Long? = null,
) : TranscriptionException(message)

data class TranscriptionAudio(
    val file: File,
    val mimeType: String,
    val pcmSampleRate: Int? = null,
    val pcmDataOffset: Long = 0,
    val pcmDataLength: Long = file.length(),
    /**
     * Where the finished parts of a long recording are saved, so a retry
     * sends only the parts that did not finish. None saves nothing.
     */
    val partsFile: File? = null,
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
        audio: UploadAudio,
        model: String = MODEL,
        prompt: String = "",
    ): TranscriptionHttpRequest {
        require(apiKey.isNotBlank()) { "API key is required" }
        require(audio.size > 0) { "Audio is required" }
        val boundary = boundaryFactory()
        val head = buildString {
            append("--$boundary\r\n")
            append("Content-Disposition: form-data; name=\"model\"\r\n\r\n")
            append(model)
            append("\r\n")
            if (prompt.isNotBlank()) {
                append("--$boundary\r\n")
                append("Content-Disposition: form-data; name=\"prompt\"\r\n\r\n")
                append(prompt)
                append("\r\n")
            }
            append("--$boundary\r\n")
            append(
                "Content-Disposition: form-data; name=\"file\"; " +
                    "filename=\"${audio.fileName}\"\r\n",
            )
            append("Content-Type: ${audio.mimeType}\r\n\r\n")
        }.encodeToByteArray()
        val tail = "\r\n--$boundary--\r\n".encodeToByteArray()

        return TranscriptionHttpRequest(
            url = "https://api.openai.com/v1/audio/transcriptions",
            headers = mapOf(
                "Authorization" to "Bearer $apiKey",
                "Content-Type" to "multipart/form-data; boundary=$boundary",
                "Accept" to "application/json",
            ),
            contentLength = head.size + audio.size + tail.size,
        ) { output, checkCancelled ->
            output.write(head)
            audio.writeTo(output, checkCancelled)
            output.write(tail)
        }
    }

    companion object {
        const val MODEL = "gpt-transcribe"
        const val WAV_MIME_TYPE = "audio/wav"
        const val FLAC_MIME_TYPE = "audio/flac"
    }
}

object OpenAiResponseParser {
    fun parse(statusCode: Int, body: String, retryAfterSeconds: Long? = null): String {
        if (statusCode !in 200..299) {
            val error = runCatching { JSONObject(body).getJSONObject("error") }.getOrNull()
            val message = error?.optString("message")?.takeIf(String::isNotEmpty)
                ?: "Transcription request failed with HTTP $statusCode"
            val code = error?.takeUnless { it.isNull("code") }?.optString("code")
                ?.takeIf(String::isNotEmpty)
            throw TranscriptionHttpException(statusCode, message, code, retryAfterSeconds)
        }

        // An empty text is a valid answer: a silent part of a long recording.
        return runCatching { JSONObject(body).getString("text") }
            .getOrElse { throw TranscriptionException("Transcription response did not contain text", it) }
    }
}
