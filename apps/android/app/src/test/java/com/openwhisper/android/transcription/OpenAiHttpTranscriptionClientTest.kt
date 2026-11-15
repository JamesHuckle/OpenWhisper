package com.openwhisper.android.transcription

import com.openwhisper.android.audio.WavEncoder
import java.io.File
import java.net.HttpURLConnection
import java.net.InetAddress
import java.net.ServerSocket
import java.net.URL
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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

    private fun pcmAudio(): TranscriptionAudio {
        val pcm = ByteArray(16_000 * 2) { (it % 50).toByte() }
        val file = File.createTempFile("openwhisper-upload-", ".wav").apply {
            deleteOnExit()
            writeBytes(WavEncoder.encodePcm16Mono(pcm, 16_000))
        }
        return TranscriptionAudio(
            file = file,
            mimeType = "audio/wav",
            pcmSampleRate = 16_000,
            pcmDataOffset = 44,
            pcmDataLength = pcm.size.toLong(),
        )
    }
}
