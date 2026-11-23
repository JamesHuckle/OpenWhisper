package com.openwhisper.android.transcription

import com.openwhisper.android.audio.WavEncoder
import java.io.ByteArrayOutputStream
import java.io.EOFException
import java.io.File
import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class OpenAiRequestFactoryTest {
    @Test
    fun theDefaultModelIsTheSharedDefault() {
        val protocolDir = generateSequence(File(System.getProperty("user.dir")!!).absoluteFile) {
            it.parentFile
        }.map { File(it, "packages/protocol") }
            .first { File(it, "models.json").isFile }
        val shared = JSONObject(File(protocolDir, "models.json").readText())

        assertEquals(shared.getString("default_transcription_model"), OpenAiRequestFactory.MODEL)
    }

    @Test
    fun buildsAuthenticatedMultipartRequestWithoutMutatingAudio() {
        val audio = byteArrayOf(0, 1, 2, 3, 13, 10, -1)
        val request = factory().create(
            apiKey = "sk-test-secret",
            audio = UploadAudio.bytes("dictation.flac", OpenAiRequestFactory.FLAC_MIME_TYPE, audio),
            model = "gpt-transcribe",
        )

        assertEquals("https://api.openai.com/v1/audio/transcriptions", request.url)
        assertEquals("Bearer sk-test-secret", request.headers["Authorization"])
        assertEquals(
            "multipart/form-data; boundary=test-boundary",
            request.headers["Content-Type"],
        )

        val body = request.body()
        assertEquals(body.size.toLong(), request.contentLength)
        val modelMarker = "name=\"model\"\r\n\r\ngpt-transcribe".encodeToByteArray()
        val fileHeader = (
            "name=\"file\"; filename=\"dictation.flac\"\r\n" +
                "Content-Type: audio/flac\r\n\r\n"
            ).encodeToByteArray()
        assertTrue(body.containsBytes(modelMarker))
        val fileStart = body.indexOfBytes(fileHeader) + fileHeader.size
        assertTrue(fileStart >= fileHeader.size)
        assertArrayEquals(audio, body.copyOfRange(fileStart, fileStart + audio.size))
        assertTrue(body.decodeToString().endsWith("\r\n--test-boundary--\r\n"))
    }

    @Test
    fun aWavPartStreamedFromItsFileMatchesTheWholeWavRequest() {
        val pcm = ByteArray(3 * UploadAudio.COPY_BUFFER_BYTES + 6) { (it % 97).toByte() }
        val offset = 44L + 10
        val file = tempFile(ByteArray(offset.toInt()) { 7 } + pcm + ByteArray(20) { 9 })

        val request = factory().create(
            apiKey = "sk-test",
            audio = UploadAudio.wav(file, offset, pcm.size.toLong(), 16_000),
            prompt = "Earlier text.",
        )

        // The body the request factory built in memory before it streamed.
        val expected = (
            "--test-boundary\r\n" +
                "Content-Disposition: form-data; name=\"model\"\r\n\r\n" +
                "${OpenAiRequestFactory.MODEL}\r\n" +
                "--test-boundary\r\n" +
                "Content-Disposition: form-data; name=\"prompt\"\r\n\r\n" +
                "Earlier text.\r\n" +
                "--test-boundary\r\n" +
                "Content-Disposition: form-data; name=\"file\"; filename=\"dictation.wav\"\r\n" +
                "Content-Type: audio/wav\r\n\r\n"
            ).encodeToByteArray() +
            WavEncoder.encodePcm16Mono(pcm, 16_000) +
            "\r\n--test-boundary--\r\n".encodeToByteArray()
        val body = request.body()
        assertArrayEquals(expected, body)
        assertEquals(expected.size.toLong(), request.contentLength)
    }

    @Test
    fun aWholeFileIsSentUnderItsOwnNameAndType() {
        val m4a = ByteArray(1_000) { (it % 13).toByte() }
        val file = tempFile(m4a, ".m4a")

        val body = factory().create(apiKey = "sk-test", audio = UploadAudio.file(file, "audio/x-m4a")).body()

        val fileHeader = (
            "name=\"file\"; filename=\"${file.name}\"\r\n" +
                "Content-Type: audio/x-m4a\r\n\r\n"
            ).encodeToByteArray()
        val fileStart = body.indexOfBytes(fileHeader) + fileHeader.size
        assertTrue(fileStart >= fileHeader.size)
        assertArrayEquals(m4a, body.copyOfRange(fileStart, fileStart + m4a.size))
    }

    @Test
    fun aCancelWhileCopyingStopsTheBody() {
        val file = tempFile(ByteArray(10 * UploadAudio.COPY_BUFFER_BYTES))
        val request = factory().create(apiKey = "sk-test", audio = UploadAudio.file(file, "audio/mpeg"))
        val output = ByteArrayOutputStream()
        var checks = 0

        assertThrows(TranscriptionException::class.java) {
            request.writeBody(output) {
                checks += 1
                if (checks == 3) throw TranscriptionException("Transcription cancelled")
            }
        }

        assertTrue(output.size() < 3 * UploadAudio.COPY_BUFFER_BYTES)
    }

    @Test
    fun aFileThatShrankFailsInsteadOfSendingTooFewBytes() {
        val file = tempFile(ByteArray(1_000))
        val request = factory().create(apiKey = "sk-test", audio = UploadAudio.wav(file, 44, 2_000, 16_000))

        assertThrows(EOFException::class.java) { request.body() }
    }

    @Test
    fun parsesSuccessfulTranscriptionResponse() {
        assertEquals(
            "Hello from Android.",
            OpenAiResponseParser.parse(200, "{\"text\":\"Hello from Android.\"}"),
        )
    }

    @Test(expected = TranscriptionException::class)
    fun surfacesApiErrorMessageWithoutLeakingKey() {
        OpenAiResponseParser.parse(
            401,
            "{\"error\":{\"message\":\"Incorrect API key\"}}",
        )
    }

    private fun factory() = OpenAiRequestFactory(boundaryFactory = { "test-boundary" })

    private fun TranscriptionHttpRequest.body(): ByteArray =
        ByteArrayOutputStream().also { writeBody(it) }.toByteArray()

    private fun tempFile(bytes: ByteArray, suffix: String = ".wav"): File =
        File.createTempFile("openwhisper-request-", suffix).apply {
            deleteOnExit()
            writeBytes(bytes)
        }

    private fun ByteArray.containsBytes(needle: ByteArray): Boolean = indexOfBytes(needle) >= 0

    private fun ByteArray.indexOfBytes(needle: ByteArray): Int {
        if (needle.isEmpty()) return 0
        for (index in 0..size - needle.size) {
            if (needle.indices.all { this[index + it] == needle[it] }) return index
        }
        return -1
    }
}
