package com.openwhisper.android.transcription

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class OpenAiRequestFactoryTest {
    @Test
    fun buildsAuthenticatedMultipartWavRequestWithoutMutatingAudio() {
        val wav = byteArrayOf(0, 1, 2, 3, 13, 10, -1)
        val request = OpenAiRequestFactory(boundaryFactory = { "test-boundary" }).create(
            apiKey = "sk-test-secret",
            wav = wav,
            model = "gpt-4o-mini-transcribe",
        )

        assertEquals("https://api.openai.com/v1/audio/transcriptions", request.url)
        assertEquals("Bearer sk-test-secret", request.headers["Authorization"])
        assertEquals(
            "multipart/form-data; boundary=test-boundary",
            request.headers["Content-Type"],
        )

        val body = request.body
        val modelMarker = "name=\"model\"\r\n\r\ngpt-4o-mini-transcribe".encodeToByteArray()
        val fileHeader = (
            "name=\"file\"; filename=\"dictation.wav\"\r\n" +
                "Content-Type: audio/wav\r\n\r\n"
            ).encodeToByteArray()
        assertTrue(body.containsBytes(modelMarker))
        val fileStart = body.indexOfBytes(fileHeader) + fileHeader.size
        assertTrue(fileStart >= fileHeader.size)
        assertArrayEquals(wav, body.copyOfRange(fileStart, fileStart + wav.size))
        assertTrue(body.decodeToString().contains("--test-boundary--"))
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

    private fun ByteArray.containsBytes(needle: ByteArray): Boolean = indexOfBytes(needle) >= 0

    private fun ByteArray.indexOfBytes(needle: ByteArray): Int {
        if (needle.isEmpty()) return 0
        for (index in 0..size - needle.size) {
            if (needle.indices.all { this[index + it] == needle[it] }) return index
        }
        return -1
    }
}
