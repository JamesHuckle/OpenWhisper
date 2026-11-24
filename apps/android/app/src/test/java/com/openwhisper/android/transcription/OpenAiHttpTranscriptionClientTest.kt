package com.openwhisper.android.transcription

import com.openwhisper.android.audio.WavEncoder
import com.openwhisper.android.audio.pcm16MonoWav
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.InputStream
import java.io.OutputStream
import java.net.HttpURLConnection
import java.net.InetAddress
import java.net.ServerSocket
import java.net.URL
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class OpenAiHttpTranscriptionClientTest {
    private companion object {
        /** The PCM in one upload: a long recording's part. */
        val PART = AudioRules.MAX_PCM_BYTES_PER_UPLOAD

        /** A part that fails all three tries with a server error. */
        val FAILED_PART = Array(3) { "Server error" }
        val FAILED_STATUSES = List(3) { 500 }

        /** A status that makes the fake connection drop instead of answering. */
        const val DROPPED = -1
    }

    @Test
    fun cancelClosesAnUploadThatIsStillWaitingForTheService() {
        // A local service that takes the upload and never answers.
        ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { service ->
            val received = CountDownLatch(1)
            thread(isDaemon = true) {
                service.accept().use { socket ->
                    val input = socket.getInputStream()
                    val buffer = ByteArray(8 * 1024)
                    received.countDown()
                    runCatching { while (input.read(buffer) >= 0) Unit }
                }
            }
            val client = OpenAiHttpTranscriptionClient(
                apiKey = { "sk-test" },
                openConnection = {
                    URL("http://127.0.0.1:${service.localPort}/v1/audio/transcriptions")
                        .openConnection() as HttpURLConnection
                },
            )
            var error: Throwable? = null
            val upload = thread {
                error = runCatching { client.transcribe(pcmAudio()) }.exceptionOrNull()
            }

            assertTrue(received.await(5, TimeUnit.SECONDS))
            Thread.sleep(200)
            assertTrue(upload.isAlive)
            client.cancel()
            upload.join(5_000)

            assertFalse(upload.isAlive)
            assertTrue(error is TranscriptionException)
            assertEquals(OpenAiHttpTranscriptionClient.CANCELLED_MESSAGE, error?.message)
        }
    }

    @Test
    fun aCancelJustBeforeTheConnectionIsRegisteredSendsNothing() {
        lateinit var client: OpenAiHttpTranscriptionClient
        var opened = 0
        var closed = false
        client = OpenAiHttpTranscriptionClient(
            apiKey = { "sk-test" },
            openConnection = { url ->
                opened += 1
                // The cancel lands after the connection is opened but before
                // the call registers it, so cancel() cannot close it.
                client.cancel()
                object : HttpURLConnection(URL(url)) {
                    override fun connect() = Unit
                    override fun disconnect() {
                        closed = true
                    }
                    override fun usingProxy() = false
                    override fun getOutputStream(): OutputStream =
                        error("Nothing is uploaded after a cancel")
                }
            },
        )

        val error = assertThrows(TranscriptionException::class.java) {
            client.transcribe(pcmAudio())
        }

        assertEquals(OpenAiHttpTranscriptionClient.CANCELLED_MESSAGE, error.message)
        assertEquals(1, opened)
        assertTrue(closed)
    }

    @Test
    fun anAccidentalTapIsNeverUploaded() {
        val service = FakeService("unused")
        val error = assertThrows(NoSpeechException::class.java) {
            service.client.transcribe(pcmAudio(ByteArray(16_000 * 2 / 5) { (it % 50).toByte() }))
        }

        assertEquals("too_short", error.reason)
        assertEquals(AudioRules.NO_AUDIO_MESSAGE, error.message)
        assertEquals(0, service.uploads.size)
    }

    @Test
    fun aMutedMicrophoneIsNeverUploaded() {
        val service = FakeService("unused")
        val error = assertThrows(NoSpeechException::class.java) {
            service.client.transcribe(pcmAudio(ByteArray(16_000 * 2)))
        }

        assertEquals("silent", error.reason)
        assertEquals(0, service.uploads.size)
    }

    @Test
    fun anEmptyTranscriptIsNoAudio() {
        val service = FakeService("")
        val error = assertThrows(NoSpeechException::class.java) {
            service.client.transcribe(pcmAudio())
        }

        assertEquals("empty_transcript", error.reason)
        assertEquals(1, service.uploads.size)
    }

    @Test
    fun aPartThatOnlyRepeatsItsPromptIsDropped() {
        // Just over one upload of PCM, so the recording is sent in two parts.
        val bytes = AudioRules.MAX_PCM_BYTES_PER_UPLOAD + 16_000 * 2
        val service = FakeService("Hello from the first part.", "hello from the first part")

        val transcript = service.client.transcribe(pcmAudio(ByteArray(bytes) { (it % 50).toByte() }))

        assertEquals("Hello from the first part.", transcript)
        assertEquals(2, service.uploads.size)
        assertTrue(service.uploads.all { it.size <= AudioRules.MAX_UPLOAD_BYTES + 1_024 })
    }

    @Test
    fun aRetryAfterAFailedLastPartSendsOnlyThatPart() {
        val audio = twoPartAudio()
        val failing = FakeService(
            "Hello from the first part.", *FAILED_PART,
            statuses = listOf(200) + FAILED_STATUSES,
        )
        assertThrows(TranscriptionHttpException::class.java) { failing.client.transcribe(audio) }
        assertEquals(4, failing.uploads.size)

        val retry = FakeService("And the second.")
        assertEquals("Hello from the first part. And the second.", retry.client.transcribe(audio))

        assertEquals(1, retry.uploads.size)
        // Its prompt is rebuilt from the saved text of the part before it.
        assertEquals("Hello from the first part.", promptOf(retry.uploads.single()))
    }

    @Test
    fun aRetryResumesPartsSavedWithTheirPromptsByAnOlderVersion() {
        val audio = twoPartAudio()
        val failing = FakeService(
            "Hello from the first part.", *FAILED_PART,
            statuses = listOf(200) + FAILED_STATUSES,
        )
        assertThrows(TranscriptionHttpException::class.java) { failing.client.transcribe(audio) }
        // Older versions also saved the prompt each part was sent with.
        val partsFile = requireNotNull(audio.partsFile)
        val saved = JSONObject(partsFile.readText())
        saved.getJSONArray("parts").getJSONObject(0).put("prompt", "")
        partsFile.writeText(saved.toString())

        val retry = FakeService("And the second.")
        assertEquals("Hello from the first part. And the second.", retry.client.transcribe(audio))

        assertEquals(1, retry.uploads.size)
        assertEquals("Hello from the first part.", promptOf(retry.uploads.single()))
    }

    @Test
    fun anEmptyPartThatFinishedIsNotSentAgain() {
        val audio = twoPartAudio()
        val failing = FakeService("", *FAILED_PART, statuses = listOf(200) + FAILED_STATUSES)
        assertThrows(TranscriptionHttpException::class.java) { failing.client.transcribe(audio) }

        val retry = FakeService("The second part.")
        assertEquals("The second part.", retry.client.transcribe(audio))

        assertEquals(1, retry.uploads.size)
        assertEquals(null, promptOf(retry.uploads.single()))
    }

    @Test
    fun silentPartsAfterSpeechAreNeverUploaded() {
        val audio = pcmAudio(speech(PART) + ByteArray(PART) + ByteArray(16_000 * 2), saveParts = true)
        val service = FakeService("Only part.")

        assertEquals("Only part.", service.client.transcribe(audio))
        assertEquals(1, service.uploads.size)

        // Saved as finished, so a retry uploads nothing either.
        val retry = FakeService()
        assertEquals("Only part.", retry.client.transcribe(audio))
        assertEquals(0, retry.uploads.size)
    }

    @Test
    fun aSilentPartBetweenSpeechIsSkippedAndRetriesSkipIt() {
        val audio = pcmAudio(speech(PART) + ByteArray(PART) + speech(16_000 * 2), saveParts = true)
        val failing = FakeService("First part.", *FAILED_PART, statuses = listOf(200) + FAILED_STATUSES)
        assertThrows(TranscriptionHttpException::class.java) { failing.client.transcribe(audio) }
        assertEquals(4, failing.uploads.size)

        val retry = FakeService("Third part.")
        assertEquals("First part. Third part.", retry.client.transcribe(audio))

        // Only the failed part goes again, prompted with the last text.
        assertEquals(1, retry.uploads.size)
        assertEquals("First part.", promptOf(retry.uploads.single()))
    }

    @Test
    fun quietSpeechAndAShortLastPartAreStillUploaded() {
        // The first part only just reaches the silence level, and the last is
        // far shorter than a whole recording must be.
        val quiet = ByteArray(PART).also { setSample(it, 1_000, -AudioRules.SILENT_PCM_PEAK) }
        val audio = pcmAudio(quiet + speech(16_000 * 2 / 10))
        val service = FakeService("Quiet.", "End.")

        assertEquals("Quiet. End.", service.client.transcribe(audio))

        assertEquals(2, service.uploads.size)
        assertEquals("Quiet.", promptOf(service.uploads.last()))
    }

    @Test
    fun aPartJustBelowTheSilenceLevelIsNotUploaded() {
        val muted = ByteArray(PART).also { setSample(it, 1_000, -(AudioRules.SILENT_PCM_PEAK - 1)) }
        val audio = pcmAudio(speech(PART) + muted)
        val service = FakeService("Speech.")

        assertEquals("Speech.", service.client.transcribe(audio))

        assertEquals(1, service.uploads.size)
    }

    @Test
    fun aChangedRecordingStartsItsPartsAfresh() {
        val audio = twoPartAudio()
        val failing = FakeService("One.", *FAILED_PART, statuses = listOf(200) + FAILED_STATUSES)
        assertThrows(TranscriptionHttpException::class.java) { failing.client.transcribe(audio) }
        assertTrue(audio.file.setLastModified(audio.file.lastModified() + 10_000))

        val retry = FakeService("Uno.", "Dos.")
        assertEquals("Uno. Dos.", retry.client.transcribe(audio))

        assertEquals(2, retry.uploads.size)
    }

    @Test
    fun partsAreUploadedAsLosslessFlacWhenThePhoneCanEncodeIt() {
        val encoded = mutableListOf<Int>()
        val service = FakeService("Hello.", encodeFlac = { _, _, length, rate, _ ->
            encoded += rate
            "fLaC".encodeToByteArray() + ByteArray(length / 2)
        })

        assertEquals("Hello.", service.client.transcribe(pcmAudio()))

        assertEquals(listOf(16_000), encoded)
        assertEquals(1, service.uploads.size)
        val upload = service.uploads.single().decodeToString()
        assertTrue(upload.contains("filename=\"dictation.flac\"\r\nContent-Type: audio/flac\r\n\r\nfLaC"))
    }

    @Test
    fun aPhoneThatCannotEncodeFlacUploadsWav() {
        val service = FakeService("Hello.")

        assertEquals("Hello.", service.client.transcribe(pcmAudio()))

        val upload = service.uploads.single().decodeToString()
        assertTrue(upload.contains("filename=\"dictation.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFF"))
    }

    @Test
    fun refusedFlacIsSentOnceMoreAsWavAndSkippedAfterwards() {
        var encoded = 0
        val service = FakeService(
            "Unsupported file", "Hello.", "Again.",
            statuses = listOf(400),
            encodeFlac = { _, _, _, _, _ -> encoded += 1; "fLaC1234".encodeToByteArray() },
        )

        assertEquals("Hello.", service.client.transcribe(pcmAudio()))
        assertEquals("Again.", service.client.transcribe(pcmAudio()))

        val types = service.uploads.map {
            Regex("Content-Type: (audio/[a-z]+)").find(it.decodeToString())?.groupValues?.get(1)
        }
        assertEquals(listOf("audio/flac", "audio/wav", "audio/wav"), types)
        assertEquals(1, encoded)
    }

    @Test
    fun otherServiceErrorsAreReportedWithoutASecondUpload() {
        val service = FakeService(
            "Incorrect API key",
            statuses = listOf(401),
            encodeFlac = { _, _, _, _, _ -> "fLaC1234".encodeToByteArray() },
        )

        val error = assertThrows(TranscriptionHttpException::class.java) {
            service.client.transcribe(pcmAudio())
        }

        assertEquals(401, error.statusCode)
        assertEquals("Incorrect API key", error.message)
        assertEquals(1, service.uploads.size)
    }

    @Test
    fun aServerErrorOrADroppedConnectionIsTriedAgain() {
        for (status in listOf(429, 500, 502, 503, 504, DROPPED)) {
            val service = FakeService("Busy", "Hello.", statuses = listOf(status))

            assertEquals("Hello.", service.client.transcribe(pcmAudio()))
            assertEquals("$status", 2, service.uploads.size)
        }
    }

    @Test
    fun aFailingRequestIsTriedThreeTimesAtMost() {
        val service = FakeService("Busy", "Busy", "Busy", "Unused.", statuses = List(3) { 503 })

        val error = assertThrows(TranscriptionHttpException::class.java) {
            service.client.transcribe(pcmAudio())
        }

        assertEquals(503, error.statusCode)
        assertEquals(3, service.uploads.size)
    }

    @Test
    fun errorsThatWillNotPassAreSentOnce() {
        val service = FakeService("Out of credit", "Unused.", statuses = listOf(429), code = "insufficient_quota")

        val error = assertThrows(TranscriptionHttpException::class.java) {
            service.client.transcribe(pcmAudio())
        }

        assertEquals(429, error.statusCode)
        assertEquals(1, service.uploads.size)
        assertFalse(
            OpenAiHttpTranscriptionClient.isTemporary(
                TranscriptionException("Could not reach", java.net.SocketTimeoutException("Read timed out")),
            ),
        )
    }

    @Test
    fun aRateLimitWaitsAsLongAsItAsks() {
        val service = FakeService("Slow down", "Hello.", statuses = listOf(429), retryAfter = "1")
        val started = System.nanoTime()

        assertEquals("Hello.", service.client.transcribe(pcmAudio()))

        assertEquals(2, service.uploads.size)
        assertTrue(System.nanoTime() - started >= 1_000_000_000L)
    }

    @Test
    fun aCancelDuringTheWaitSendsNothingMore() {
        val service = FakeService("Busy", "Unused.", statuses = listOf(503), retryDelaysMillis = listOf(10_000L))
        var error: Throwable? = null
        val upload = thread {
            error = runCatching { service.client.transcribe(pcmAudio()) }.exceptionOrNull()
        }
        val deadline = System.nanoTime() + 5_000_000_000L
        while (service.uploads.isEmpty() && System.nanoTime() < deadline) Thread.sleep(5)
        Thread.sleep(100)

        service.client.cancel()
        upload.join(2_000)

        assertFalse(upload.isAlive)
        assertEquals(OpenAiHttpTranscriptionClient.CANCELLED_MESSAGE, error?.message)
        assertEquals(1, service.uploads.size)
    }

    @Test
    fun theJobsCancelEndsTheWaitAtOnce() {
        val jobCancelled = CountDownLatch(1)
        val service = FakeService(
            "Busy",
            "Unused.",
            statuses = listOf(503),
            retryDelaysMillis = listOf(10_000L),
            cancelled = jobCancelled,
        )
        var error: Throwable? = null
        val upload = thread {
            error = runCatching { service.client.transcribe(pcmAudio()) }.exceptionOrNull()
        }
        val deadline = System.nanoTime() + 5_000_000_000L
        while (service.uploads.isEmpty() && System.nanoTime() < deadline) Thread.sleep(5)
        Thread.sleep(100)

        // Only the job's signal: the client's own cancel has not run yet.
        val cancelledAt = System.nanoTime()
        jobCancelled.countDown()
        upload.join(2_000)
        val endedAfterMillis = (System.nanoTime() - cancelledAt) / 1_000_000

        assertFalse(upload.isAlive)
        assertEquals(OpenAiHttpTranscriptionClient.CANCELLED_MESSAGE, error?.message)
        assertEquals(1, service.uploads.size)
        assertTrue("The wait ended $endedAfterMillis ms after the cancel", endedAfterMillis < 1_000)
    }

    @Test
    fun theWaitForAnAnswerGrowsWithTheUpload() {
        // A five-second dictation as WAV: about two minutes.
        assertEquals(123_200, OpenAiHttpTranscriptionClient.readTimeoutMillis(160_000))
        // A full-size part: about ten minutes; never more than thirty.
        assertEquals(
            120_000 + AudioRules.MAX_UPLOAD_BYTES.toInt() / 50,
            OpenAiHttpTranscriptionClient.readTimeoutMillis(AudioRules.MAX_UPLOAD_BYTES.toLong()),
        )
        assertEquals(30 * 60 * 1_000, OpenAiHttpTranscriptionClient.readTimeoutMillis(Long.MAX_VALUE / 2))
    }

    @Test
    fun aLongPartIsStreamedFromTheFileInSmallBlocks() {
        // A full upload of PCM: about 24 MB, sent as one WAV part.
        val pcm = ByteArray(AudioRules.MAX_PCM_BYTES_PER_UPLOAD) { (it % 50).toByte() }
        val service = FakeService("Hello.")

        assertEquals("Hello.", service.client.transcribe(pcmAudio(pcm)))

        val connection = service.connections.single()
        val upload = service.uploads.single()
        assertEquals(upload.size.toLong(), connection.declaredLength)
        assertTrue(connection.largestWrite <= UploadAudio.COPY_BUFFER_BYTES)
        val wavStart = upload.indexOf("RIFF".encodeToByteArray())
        assertTrue(
            upload.copyOfRange(wavStart, wavStart + WavEncoder.HEADER_BYTES)
                .contentEquals(WavEncoder.header(pcm.size.toLong(), 16_000)),
        )
        val samples = upload.copyOfRange(
            wavStart + WavEncoder.HEADER_BYTES,
            wavStart + WavEncoder.HEADER_BYTES + pcm.size,
        )
        assertTrue(samples.contentEquals(pcm))
    }

    @Test
    fun aCompressedImportIsSentWithItsOwnNameAndType() {
        val mp3 = ByteArray(200_000) { (it % 251).toByte() }
        val file = File.createTempFile("openwhisper-import-", ".mp3").apply {
            deleteOnExit()
            writeBytes(mp3)
        }
        val service = FakeService("Imported.")

        val transcript = service.client.transcribe(TranscriptionAudio(file = file, mimeType = "audio/mpeg"))

        assertEquals("Imported.", transcript)
        val upload = service.uploads.single()
        val fileHeader = "filename=\"${file.name}\"\r\nContent-Type: audio/mpeg\r\n\r\n"
            .encodeToByteArray()
        val start = upload.indexOf(fileHeader) + fileHeader.size
        assertTrue(start >= fileHeader.size)
        assertTrue(upload.copyOfRange(start, start + mp3.size).contentEquals(mp3))
        assertEquals(upload.size.toLong(), service.connections.single().declaredLength)
        assertTrue(service.connections.single().largestWrite <= UploadAudio.COPY_BUFFER_BYTES)
    }

    @Test
    fun cancellingStopsCopyingTheAudioIntoTheUpload() {
        lateinit var service: FakeService
        service = FakeService("unused", onWrite = { written ->
            if (written > 4 * UploadAudio.COPY_BUFFER_BYTES) service.client.cancel()
        })
        val pcm = ByteArray(AudioRules.MAX_PCM_BYTES_PER_UPLOAD) { (it % 50).toByte() }

        val error = assertThrows(TranscriptionException::class.java) {
            service.client.transcribe(pcmAudio(pcm))
        }

        assertEquals(OpenAiHttpTranscriptionClient.CANCELLED_MESSAGE, error.message)
        assertTrue(service.uploads.single().size < 6 * UploadAudio.COPY_BUFFER_BYTES)
    }

    @Test
    fun cancellingStopsFlacEncodingWithoutAnyUpload() {
        var encoderSawCancel = false
        lateinit var service: FakeService
        service = FakeService("unused", encodeFlac = { _, _, _, _, cancelled ->
            service.client.cancel()
            encoderSawCancel = cancelled()
            error("FLAC encoding cancelled")
        })

        val error = assertThrows(TranscriptionException::class.java) {
            service.client.transcribe(pcmAudio())
        }

        assertEquals(OpenAiHttpTranscriptionClient.CANCELLED_MESSAGE, error.message)
        assertTrue(encoderSawCancel)
        assertTrue(service.uploads.isEmpty())
    }

    @Test
    fun aJobCancelledBeforeItsClientStartsSendsNothing() {
        var encoded = 0
        var opened = 0
        val client = OpenAiHttpTranscriptionClient(
            apiKey = { "sk-test" },
            openConnection = {
                opened += 1
                error("Nothing may be uploaded")
            },
            encodeFlac = { _, _, _, _, cancelled ->
                encoded += 1
                check(!cancelled()) { "FLAC encoding cancelled" }
                ByteArray(8)
            },
            // The job is cancelled, but has not told the client yet.
            cancelled = CountDownLatch(0),
        )

        val error = assertThrows(TranscriptionException::class.java) {
            client.transcribe(pcmAudio())
        }

        assertEquals(OpenAiHttpTranscriptionClient.CANCELLED_MESSAGE, error.message)
        assertEquals(0, opened)
        assertTrue(encoded <= 1)
    }

    private class FakeService(
        vararg responses: String,
        statuses: List<Int> = emptyList(),
        encodeFlac: (File, Long, Int, Int, () -> Boolean) -> ByteArray =
            { _, _, _, _, _ -> error("No FLAC encoder in unit tests") },
        private val onWrite: (Int) -> Unit = {},
        private val code: String? = null,
        private val retryAfter: String? = null,
        retryDelaysMillis: List<Long> = listOf(0L, 0L),
        cancelled: CountDownLatch = CountDownLatch(1),
    ) {
        private val texts = ArrayDeque(responses.toList())
        private val statuses = ArrayDeque(statuses)
        val uploads = mutableListOf<ByteArray>()
        val connections = mutableListOf<FakeConnection>()
        val client = OpenAiHttpTranscriptionClient(
            apiKey = { "sk-test" },
            openConnection = { url ->
                FakeConnection(
                    URL(url),
                    texts.removeFirst(),
                    uploads,
                    this.statuses.removeFirstOrNull() ?: 200,
                    onWrite,
                    code,
                    retryAfter,
                ).also(connections::add)
            },
            encodeFlac = encodeFlac,
            cancelled = cancelled,
            retryDelaysMillis = retryDelaysMillis,
        )
    }

    private class FakeConnection(
        url: URL,
        private val text: String,
        private val uploads: MutableList<ByteArray>,
        private val status: Int = 200,
        private val onWrite: (Int) -> Unit = {},
        private val code: String? = null,
        private val retryAfter: String? = null,
    ) : HttpURLConnection(url) {
        /** The largest single write into the request, to show nothing is sent whole. */
        var largestWrite = 0
        val declaredLength: Long get() = fixedContentLengthLong

        private val body = object : ByteArrayOutputStream() {
            override fun write(b: ByteArray, off: Int, len: Int) {
                largestWrite = maxOf(largestWrite, len)
                super.write(b, off, len)
                onWrite(size())
            }

            override fun close() {
                uploads += toByteArray()
            }
        }

        override fun connect() = Unit
        override fun disconnect() = Unit
        override fun usingProxy() = false
        override fun getOutputStream(): OutputStream = body
        override fun getResponseCode(): Int =
            if (status < 0) throw java.io.IOException("Connection reset") else status
        override fun getHeaderField(name: String): String? =
            retryAfter.takeIf { name.equals("Retry-After", ignoreCase = true) }
        override fun getInputStream(): InputStream =
            "{\"text\":\"$text\"}".byteInputStream()
        override fun getErrorStream(): InputStream =
            "{\"error\":{\"message\":\"$text\",\"code\":${code?.let { "\"$it\"" } ?: "null"}}}"
                .byteInputStream()
    }

    private fun pcmAudio(
        pcm: ByteArray = ByteArray(16_000 * 2) { (it % 50).toByte() },
        saveParts: Boolean = false,
    ): TranscriptionAudio {
        val file = File.createTempFile("openwhisper-upload-", ".wav").apply {
            deleteOnExit()
            writeBytes(pcm16MonoWav(pcm, 16_000))
        }
        return TranscriptionAudio(
            file = file,
            mimeType = "audio/wav",
            pcmSampleRate = 16_000,
            pcmDataOffset = WavEncoder.HEADER_BYTES.toLong(),
            pcmDataLength = pcm.size.toLong(),
            partsFile = if (saveParts) File("${file.path}.parts.json").apply { deleteOnExit() } else null,
        )
    }

    /** Loud 16-bit samples, well above the silence level. */
    private fun speech(bytes: Int) = ByteArray(bytes) { (it % 50).toByte() }

    private fun setSample(pcm: ByteArray, byteIndex: Int, value: Int) {
        pcm[byteIndex] = value.toByte()
        pcm[byteIndex + 1] = (value shr 8).toByte()
    }

    /** Just over one upload of PCM, so the recording is sent in two parts. */
    private fun twoPartAudio(): TranscriptionAudio = pcmAudio(
        ByteArray(AudioRules.MAX_PCM_BYTES_PER_UPLOAD + 16_000 * 2) { (it % 50).toByte() },
        saveParts = true,
    )

    private fun ByteArray.indexOf(needle: ByteArray): Int {
        for (index in 0..size - needle.size) {
            if (needle.indices.all { this[index + it] == needle[it] }) return index
        }
        return -1
    }

    private fun promptOf(upload: ByteArray): String? =
        Regex("name=\"prompt\"\r\n\r\n([^\r]*)\r\n")
            .find(upload.copyOf(2_048).decodeToString())
            ?.groupValues
            ?.get(1)
}
