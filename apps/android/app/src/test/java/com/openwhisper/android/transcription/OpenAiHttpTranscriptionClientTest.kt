package com.openwhisper.android.transcription

import com.openwhisper.android.audio.WavEncoder
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
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class OpenAiHttpTranscriptionClientTest {
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
    fun aCancelledCallNeverReplacesTheNextCallsConnection() {
        val newerUploading = CountDownLatch(1)
        val newerClosed = CountDownLatch(1)
        lateinit var client: OpenAiHttpTranscriptionClient
        var calls = 0
        var newer: Thread? = null
        client = OpenAiHttpTranscriptionClient(
            apiKey = { "sk-test" },
            openConnection = { url ->
                calls += 1
                if (calls == 1) {
                    // The first call is cancelled and a newer call starts
                    // uploading before the first registers its connection.
                    client.cancel()
                    newer = thread(isDaemon = true) {
                        runCatching { client.transcribe(pcmAudio()) }
                    }
                    assertTrue(newerUploading.await(5, TimeUnit.SECONDS))
                    FakeConnection(URL(url), "stale", mutableListOf())
                } else {
                    BlockingConnection(URL(url), newerUploading, newerClosed)
                }
            },
        )

        val error = assertThrows(TranscriptionException::class.java) {
            client.transcribe(pcmAudio())
        }
        assertEquals(OpenAiHttpTranscriptionClient.CANCELLED_MESSAGE, error.message)

        // Cancelling now still reaches the newer call's upload.
        client.cancel()
        assertTrue(newerClosed.await(5, TimeUnit.SECONDS))
        newer?.join(5_000)
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
        val failing = FakeService("Hello from the first part.", "Server error", statuses = listOf(200, 500))
        assertThrows(TranscriptionHttpException::class.java) { failing.client.transcribe(audio) }
        assertEquals(2, failing.uploads.size)

        val retry = FakeService("And the second.")
        assertEquals("Hello from the first part. And the second.", retry.client.transcribe(audio))

        assertEquals(1, retry.uploads.size)
        // Its prompt is rebuilt from the saved text of the part before it.
        assertEquals("Hello from the first part.", promptOf(retry.uploads.single()))
    }

    @Test
    fun anEmptyPartThatFinishedIsNotSentAgain() {
        val audio = twoPartAudio()
        val failing = FakeService("", "Server error", statuses = listOf(200, 500))
        assertThrows(TranscriptionHttpException::class.java) { failing.client.transcribe(audio) }

        val retry = FakeService("The second part.")
        assertEquals("The second part.", retry.client.transcribe(audio))

        assertEquals(1, retry.uploads.size)
        assertEquals(null, promptOf(retry.uploads.single()))
    }

    @Test
    fun aChangedRecordingStartsItsPartsAfresh() {
        val audio = twoPartAudio()
        val failing = FakeService("One.", "Server error", statuses = listOf(200, 500))
        assertThrows(TranscriptionHttpException::class.java) { failing.client.transcribe(audio) }
        assertTrue(audio.file.setLastModified(audio.file.lastModified() + 10_000))

        val retry = FakeService("Uno.", "Dos.")
        assertEquals("Uno. Dos.", retry.client.transcribe(audio))

        assertEquals(2, retry.uploads.size)
    }

    @Test
    fun partsAreUploadedAsLosslessFlacWhenThePhoneCanEncodeIt() {
        val encoded = mutableListOf<Int>()
        val service = FakeService("Hello.", encodeFlac = { pcm, rate ->
            encoded += rate
            "fLaC".encodeToByteArray() + pcm.copyOf(pcm.size / 2)
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
            encodeFlac = { _, _ -> encoded += 1; "fLaC1234".encodeToByteArray() },
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
            encodeFlac = { _, _ -> "fLaC1234".encodeToByteArray() },
        )

        val error = assertThrows(TranscriptionHttpException::class.java) {
            service.client.transcribe(pcmAudio())
        }

        assertEquals(401, error.statusCode)
        assertEquals("Incorrect API key", error.message)
        assertEquals(1, service.uploads.size)
    }

    private class FakeService(
        vararg responses: String,
        statuses: List<Int> = emptyList(),
        encodeFlac: (ByteArray, Int) -> ByteArray = { _, _ -> error("No FLAC encoder in unit tests") },
    ) {
        private val texts = ArrayDeque(responses.toList())
        private val statuses = ArrayDeque(statuses)
        val uploads = mutableListOf<ByteArray>()
        val client = OpenAiHttpTranscriptionClient(
            apiKey = { "sk-test" },
            openConnection = { url ->
                FakeConnection(URL(url), texts.removeFirst(), uploads, this.statuses.removeFirstOrNull() ?: 200)
            },
            encodeFlac = encodeFlac,
        )
    }

    private class FakeConnection(
        url: URL,
        private val text: String,
        private val uploads: MutableList<ByteArray>,
        private val status: Int = 200,
    ) : HttpURLConnection(url) {
        private val body = object : ByteArrayOutputStream() {
            override fun close() {
                uploads += toByteArray()
            }
        }

        override fun connect() = Unit
        override fun disconnect() = Unit
        override fun usingProxy() = false
        override fun getOutputStream(): OutputStream = body
        override fun getResponseCode() = status
        override fun getInputStream(): InputStream =
            "{\"text\":\"$text\"}".byteInputStream()
        override fun getErrorStream(): InputStream =
            "{\"error\":{\"message\":\"$text\"}}".byteInputStream()
    }

    private class BlockingConnection(
        url: URL,
        private val uploading: CountDownLatch,
        private val closed: CountDownLatch,
    ) : HttpURLConnection(url) {
        override fun connect() = Unit
        override fun disconnect() = closed.countDown()
        override fun usingProxy() = false
        override fun getOutputStream(): OutputStream = object : OutputStream() {
            override fun write(b: Int) = Unit
            override fun write(b: ByteArray, off: Int, len: Int) {
                uploading.countDown()
                closed.await(5, TimeUnit.SECONDS)
                throw java.io.IOException("closed")
            }
        }
    }

    private fun pcmAudio(
        pcm: ByteArray = ByteArray(16_000 * 2) { (it % 50).toByte() },
        saveParts: Boolean = false,
    ): TranscriptionAudio {
        val file = File.createTempFile("openwhisper-upload-", ".wav").apply {
            deleteOnExit()
            writeBytes(WavEncoder.encodePcm16Mono(pcm, 16_000))
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

    /** Just over one upload of PCM, so the recording is sent in two parts. */
    private fun twoPartAudio(): TranscriptionAudio = pcmAudio(
        ByteArray(AudioRules.MAX_PCM_BYTES_PER_UPLOAD + 16_000 * 2) { (it % 50).toByte() },
        saveParts = true,
    )

    private fun promptOf(upload: ByteArray): String? =
        Regex("name=\"prompt\"\r\n\r\n([^\r]*)\r\n")
            .find(upload.copyOf(2_048).decodeToString())
            ?.groupValues
            ?.get(1)
}
