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

    private class FakeService(vararg responses: String) {
        private val texts = ArrayDeque(responses.toList())
        val uploads = mutableListOf<ByteArray>()
        val client = OpenAiHttpTranscriptionClient(
            apiKey = { "sk-test" },
            openConnection = { url -> FakeConnection(URL(url), texts.removeFirst(), uploads) },
        )
    }

    private class FakeConnection(
        url: URL,
        private val text: String,
        private val uploads: MutableList<ByteArray>,
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
        override fun getResponseCode() = 200
        override fun getInputStream(): InputStream =
            "{\"text\":\"$text\"}".byteInputStream()
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
        )
    }
}
