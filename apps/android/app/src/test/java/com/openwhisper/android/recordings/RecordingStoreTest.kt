package com.openwhisper.android.recordings

import java.io.ByteArrayInputStream
import java.io.File
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertSame
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

    @Test
    fun aHalfWrittenSaveLeftByACrashIsIgnored() {
        withStore { store ->
            val stored = store.import(
                name = "memo.wav",
                mimeType = "audio/wav",
                extension = "wav",
                input = ByteArrayInputStream(byteArrayOf(1, 2, 3, 4)),
            )
            val directory = store.file(stored).parentFile!!
            File(directory, "metadata.json.new").writeText("{\"id\": \"half wri")

            assertEquals("saved", store.get(stored.id)?.status)
            store.finish(stored.id, "complete", transcript = "Hello")
            assertEquals("Hello", store.get(stored.id)?.transcript)
            assertTrue(File(directory, "metadata.json").readText().contains("\"transcript\": \"Hello\""))
        }
    }

    @Test
    fun storesOfTheSameFolderShareOneLock() {
        withRoot { root ->
            val screen = RecordingStore.forDirectory(root)
            val service = RecordingStore.forDirectory(File(root, "../${root.name}"))
            assertSame(screen.lock, service.lock)
        }
    }

    @Test
    fun twoStoresUpdatingTheSameRecordingNeverDamageIt() {
        withRoot { root ->
            val screen = RecordingStore.forDirectory(root)
            val service = RecordingStore.forDirectory(root)
            val stored = screen.import(
                name = "shared.wav",
                mimeType = "audio/wav",
                extension = "wav",
                input = ByteArrayInputStream(byteArrayOf(1, 2, 3, 4)),
            )
            val longText = "words ".repeat(2_000)
            val failures = mutableListOf<Throwable>()
            val threads = listOf(screen, service).mapIndexed { index, store ->
                Thread {
                    try {
                        repeat(200) { round ->
                            store.finish(stored.id, "complete", transcript = "$index-$round $longText")
                            assertNotNull("the recording vanished", store.get(stored.id))
                        }
                    } catch (failure: Throwable) {
                        synchronized(failures) { failures += failure }
                    }
                }.apply { start() }
            }
            threads.forEach(Thread::join)

            assertEquals(emptyList<Throwable>(), failures)
            val last = screen.get(stored.id)
            assertNotNull(last)
            assertTrue(last!!.transcript.endsWith(longText))
            assertTrue(last.transcript.startsWith("0-199 ") || last.transcript.startsWith("1-199 "))
        }
    }

    private fun withStore(block: (RecordingStore) -> Unit) {
        withRoot { root -> block(RecordingStore.forDirectory(root)) }
    }

    private fun withRoot(block: (File) -> Unit) {
        val root = File(
            System.getProperty("java.io.tmpdir"),
            "openwhisper-recording-store-${System.nanoTime()}",
        )
        try {
            block(root)
        } finally {
            root.deleteRecursively()
        }
    }
}
