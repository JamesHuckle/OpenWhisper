package com.openwhisper.android.transcription

import com.openwhisper.android.audio.FlacFileEncoder
import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.SocketTimeoutException
import java.net.URL
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

class OpenAiHttpTranscriptionClient(
    private val apiKey: () -> String?,
    private val requestFactory: OpenAiRequestFactory = OpenAiRequestFactory(),
    private val openConnection: (String) -> HttpURLConnection = { url ->
        URL(url).openConnection() as HttpURLConnection
    },
    /**
     * Encodes the 16-bit mono PCM at an offset and length in a file, at a
     * sample rate, to FLAC, or throws. It stops, throwing, once the last
     * argument returns true (the transcription was cancelled).
     */
    private val encodeFlac: (File, Long, Int, Int, () -> Boolean) -> ByteArray =
        FlacFileEncoder::encodePcm16Mono,
    /**
     * Set when the service refused this phone's FLAC but took the same audio
     * as WAV, so later parts and dictations skip the refused upload. The
     * transcription manager gives every job's client the same one.
     */
    private val flacRefused: AtomicBoolean = AtomicBoolean(false),
    /**
     * Counted down once this client's job, or [cancel], cancels it, even
     * before [transcribe] starts: then nothing is encoded or uploaded, and a
     * wait before a retry ends at once. Each job builds its own client, so
     * once counted down, this client never sends again.
     */
    private val cancelled: CountDownLatch = CountDownLatch(1),
    /**
     * The waits before the second and third tries of a request that failed
     * for a passing reason (see [isTemporary]). Tests use shorter ones.
     */
    private val retryDelaysMillis: List<Long> = RETRY_DELAYS_MILLIS,
) : TranscriptionClient {
    private val activeConnection = AtomicReference<HttpURLConnection?>(null)

    // Makes cancelling and registering a connection one step each, so a
    // cancel that lands while a connection opens still closes it.
    private val connectionLock = Any()

    /**
     * Closes the upload in progress, so the background thread is free at once
     * instead of waiting for a result nobody will see.
     */
    override fun cancel() {
        synchronized(connectionLock) {
            cancelled.countDown()
            activeConnection.getAndSet(null)
        }?.disconnect()
    }

    override fun transcribe(audio: TranscriptionAudio): String {
        val key = apiKey()?.takeIf { it.isNotBlank() }
            ?: throw TranscriptionException("Save an OpenAI API key or enable demo mode")
        if (!audio.file.exists() || audio.file.length() == 0L) {
            throw TranscriptionException("The saved audio file is empty")
        }
        if (audio.pcmSampleRate != null) return transcribePcm(key, audio)
        if (audio.file.length() > AudioRules.MAX_UPLOAD_BYTES) {
            throw TranscriptionException(
                "This audio file is larger than ${AudioRules.MAX_UPLOAD_BYTES / (1024 * 1024)} MB. " +
                    "Use a smaller MP3/M4A, split the file, or import a PCM WAV file.",
            )
        }
        // A compressed import goes as it is, under its own name and type.
        val upload = UploadAudio.file(audio.file, audio.mimeType)
        return transcribeRequest(key, upload, "").trim()
            .ifBlank { throw NoSpeechException("empty_transcript") }
    }

    private fun transcribePcm(key: String, audio: TranscriptionAudio): String {
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
        var start = audio.pcmDataOffset
        val counts = pcmChunkSizes(audio.pcmDataLength, AudioRules.MAX_PCM_BYTES_PER_UPLOAD)
        for ((index, count) in counts.withIndex()) {
            val end = start + count
            // A part finished on an earlier try is reused: its text builds
            // the next part's prompt as before.
            transcripts += saved.get(index, start, end)?.text ?: run {
                val text = if (AudioRules.isSilent(audio.file, start, count.toLong())) {
                    // A silent part is never uploaded: it costs a request and
                    // the model answers it by repeating its prompt. It is
                    // saved as finished and empty, so a retry skips it too.
                    ""
                } else {
                    // Each later part is sent with the previous part's text as
                    // its prompt; a near-silent part can still make the model
                    // repeat it.
                    val prompt = AudioRules.previousPartText(transcripts)
                    AudioRules.keptPartText(
                        transcribePart(key, audio.file, start, count, sampleRate, prompt).trim(),
                        prompt,
                    )
                }
                text.also { saved.add(index, SavedPart(start, end, it)) }
            }
            start = end
        }
        return transcripts.filter(String::isNotBlank).joinToString(" ").trim()
            .ifBlank { throw NoSpeechException("empty_transcript") }
    }

    /**
     * Sends one part as lossless FLAC, about half the bytes of WAV for
     * speech. It goes as WAV when the phone cannot encode FLAC, and once more
     * as WAV when the service refuses the FLAC. The part is read from [file]
     * in small blocks, never whole.
     */
    private fun transcribePart(
        key: String,
        file: File,
        offset: Long,
        length: Int,
        sampleRate: Int,
        prompt: String,
    ): String {
        val flac = if (flacRefused.get()) {
            null
        } else {
            try {
                encodeFlac(file, offset, length, sampleRate) { isCancelled() }
                    .takeIf(ByteArray::isNotEmpty)
            } catch (_: Exception) {
                // A cancel is reported as one, never answered with the WAV.
                throwIfCancelled()
                null
            }
        }
        if (flac != null) {
            try {
                val upload = UploadAudio.bytes("dictation.flac", FLAC, flac)
                return transcribeRequest(key, upload, prompt)
            } catch (error: TranscriptionHttpException) {
                if (error.statusCode != 400) throw error
            }
        }
        val wav = UploadAudio.wav(file, offset, length.toLong(), sampleRate)
        return transcribeRequest(key, wav, prompt)
            .also { if (flac != null) flacRefused.set(true) }
    }

    /**
     * Sends one upload, trying it again after 1 s and then 3 s when the
     * connection drops or the service answers 429, 500, 502, 503 or 504. A
     * rate limit's Retry-After, up to 10 s, sets the wait instead. A cancel
     * ends the wait at once.
     */
    private fun transcribeRequest(
        key: String,
        audio: UploadAudio,
        prompt: String,
    ): String {
        var tries = 0
        while (true) {
            try {
                return sendOnce(key, audio, prompt)
            } catch (error: TranscriptionException) {
                val delay = retryDelaysMillis.getOrNull(tries)
                    ?.takeIf { isTemporary(error) && !isCancelled() }
                    ?: throw error
                tries += 1
                val asked = (error as? TranscriptionHttpException)?.retryAfterSeconds
                waitUnlessCancelled(
                    asked?.let { minOf(it * 1_000, MAX_RETRY_AFTER_MILLIS) } ?: delay,
                )
            }
        }
    }

    private fun sendOnce(
        key: String,
        audio: UploadAudio,
        prompt: String,
    ): String {
        throwIfCancelled()
        val request = requestFactory.create(key, audio, prompt = prompt)
        val connection = openConnection(request.url)
        val registered = synchronized(connectionLock) {
            // A cancel that came just before the connection was registered.
            (!isCancelled()).also { if (it) activeConnection.set(connection) }
        }
        if (!registered) {
            connection.disconnect()
            throw TranscriptionException(CANCELLED_MESSAGE)
        }
        return try {
            connection.requestMethod = "POST"
            connection.connectTimeout = 30_000
            connection.readTimeout = readTimeoutMillis(request.contentLength)
            connection.doOutput = true
            request.headers.forEach(connection::setRequestProperty)
            connection.setFixedLengthStreamingMode(request.contentLength)
            connection.outputStream.use { output ->
                request.writeBody(output) { throwIfCancelled() }
            }

            val status = connection.responseCode
            val stream = if (status in 200..299) connection.inputStream else connection.errorStream
            val response = stream?.bufferedReader(Charsets.UTF_8)?.use { it.readText() }.orEmpty()
            // Only whole seconds: an HTTP date is left to the usual wait.
            val retryAfter = connection.getHeaderField("Retry-After")?.trim()?.toLongOrNull()
                ?.takeIf { status == 429 && it >= 0 }
            OpenAiResponseParser.parse(status, response, retryAfter)
        } catch (error: TranscriptionException) {
            throw error
        } catch (error: Exception) {
            throwIfCancelled()
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

    /** Waits for [millis]; a cancel ends the wait at once. */
    private fun waitUnlessCancelled(millis: Long) {
        if (cancelled.await(millis, TimeUnit.MILLISECONDS)) {
            throw TranscriptionException(CANCELLED_MESSAGE)
        }
    }

    private fun isCancelled(): Boolean = cancelled.count == 0L

    private fun throwIfCancelled() {
        if (isCancelled()) throw TranscriptionException(CANCELLED_MESSAGE)
    }

    companion object {
        const val CANCELLED_MESSAGE = "Transcription cancelled"
        private const val FLAC = OpenAiRequestFactory.FLAC_MIME_TYPE
        val RETRY_DELAYS_MILLIS = listOf(1_000L, 3_000L)
        private const val MAX_RETRY_AFTER_MILLIS = 10_000L

        /**
         * The wait for the service's answer once an upload is sent: two
         * minutes, plus a second for every 50 kB uploaded, as the service
         * hears the whole part before it answers. Never more than 30 minutes.
         */
        internal fun readTimeoutMillis(uploadBytes: Long): Int =
            minOf(120_000L + uploadBytes / 50, 30 * 60 * 1_000L).toInt()

        /**
         * A failure that may pass within seconds: the connection dropped, a
         * rate limit or a server error. A stalled answer (a timeout), a spent
         * quota and every other error do not pass.
         */
        internal fun isTemporary(error: TranscriptionException): Boolean = when (error) {
            is TranscriptionHttpException ->
                error.statusCode in setOf(500, 502, 503, 504) ||
                    (error.statusCode == 429 && error.code != "insufficient_quota")
            else -> error.cause is IOException && error.cause !is SocketTimeoutException
        }
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
