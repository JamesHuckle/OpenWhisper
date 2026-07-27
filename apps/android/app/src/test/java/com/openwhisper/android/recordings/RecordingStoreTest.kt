package com.openwhisper.android.recordings

import java.io.ByteArrayInputStream
import java.io.File
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class RecordingStoreTest {
    @Test
    fun keepsLatestTenImportsAndEveryRawFileIsReadable() {
        withStore { store ->
            repeat(12) { index ->
                store.import(
                    name = "clip-$index.mp3",
                    mimeType = "audio/mpeg",
                    extension = "mp3",
                    input = ByteArrayInputStream(byteArrayOf(index.toByte(), 42)),
                )
                Thread.sleep(2)
            }

            val retained = store.list()
            assertEquals(RecordingStore.RETAIN_COUNT, retained.size)
            retained.forEach { recording ->
                assertTrue(store.file(recording).isFile)
                assertTrue(store.file(recording).readBytes().isNotEmpty())
            }
        }
    }

    @Test
    fun activePcmRecordingIsCheckpointedAsPlayableWav() {
        withStore { store ->
            val writer = store.beginPcm("Long recording", 16_000)
            val pcm = ByteArray(32_000) { (it % 127).toByte() }
            writer.write(pcm, pcm.size)
            val recording = writer.finish("saved")

            val wav = recording.file.readBytes()
            assertEquals("RIFF", wav.copyOfRange(0, 4).decodeToString())
            assertEquals("WAVE", wav.copyOfRange(8, 12).decodeToString())
            assertArrayEquals(pcm, wav.copyOfRange(44, wav.size))
            assertEquals(44L + pcm.size, store.get(recording.id)?.bytes)
        }
    }

    @Test
    fun retranscriptionStatusUpdatesKeepTheOriginalRawFile() {
        withStore { store ->
            val stored = store.import(
                name = "retry.wav",
                mimeType = "audio/wav",
                extension = "wav",
                input = ByteArrayInputStream(byteArrayOf(1, 2, 3, 4)),
            )
            val original = store.file(stored).readBytes()

            store.finish(stored.id, "transcribing")
            store.finish(stored.id, "complete", transcript = "Retried transcript")

            assertArrayEquals(original, store.file(stored).readBytes())
            assertEquals("complete", store.get(stored.id)?.status)
            assertEquals("Retried transcript", store.get(stored.id)?.transcript)
        }
    }

    private fun withStore(block: (RecordingStore) -> Unit) {
        val root = File(
            System.getProperty("java.io.tmpdir"),
            "openwhisper-recording-store-${System.nanoTime()}",
        )
        try {
            block(RecordingStore.forDirectory(root))
        } finally {
            root.deleteRecursively()
        }
    }
}
