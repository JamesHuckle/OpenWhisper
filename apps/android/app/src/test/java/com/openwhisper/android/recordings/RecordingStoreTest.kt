package com.openwhisper.android.recordings

import java.io.ByteArrayInputStream
import java.io.File
import java.io.InputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
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
    fun aReservedRecordingOutlivesTheLimitUntilReleasedThenGoesAtTheNextCleanup() {
        withRoot { root ->
            // A retry holds the oldest recording while the app screen, with
            // its own store of the same folder, imports more files.
            val manager = RecordingStore.forDirectory(root)
            val screen = RecordingStore.forDirectory(root)
            val oldest = importInto(screen, "oldest.wav")
            manager.reserve(oldest.id)
            manager.reserve(oldest.id)
            val parts = manager.partsFile(oldest.id).apply { writeText("{\"key\":\"k\",\"parts\":[]}") }
            repeat(RecordingStore.RETAIN_COUNT + 2) { index -> importInto(screen, "clip-$index.wav") }

            assertEquals(RecordingStore.RETAIN_COUNT + 1, screen.list().size)
            assertTrue(screen.file(oldest).isFile)
            assertTrue(parts.exists())

            // Held twice, so one release still keeps it.
            manager.release(oldest.id)
            importInto(screen, "one-more.wav")
            assertNotNull(screen.get(oldest.id))

            // Released: kept until the next cleanup, which deletes it.
            manager.finish(oldest.id, "complete", transcript = "Saved after the retry")
            manager.release(oldest.id)
            assertEquals("Saved after the retry", screen.get(oldest.id)?.transcript)
            importInto(screen, "last.wav")
            assertNull(screen.get(oldest.id))
            assertFalse(File(root, oldest.id).exists())
            assertEquals(RecordingStore.RETAIN_COUNT, screen.list().size)
        }
    }

    @Test
    fun aLongCaptureIsNeverDeletedByNewerImports() {
        withRoot { root ->
            val service = RecordingStore.forDirectory(root)
            val screen = RecordingStore.forDirectory(root)
            val writer = service.beginPcm("Long dictation", 16_000)
            val oneSecond = ByteArray(32_000) { (it % 97).toByte() }
            writer.write(oneSecond, oneSecond.size)
            repeat(RecordingStore.RETAIN_COUNT + 1) { index -> importInto(screen, "clip-$index.wav") }
            writer.write(oneSecond, oneSecond.size)

            val recording = writer.finish("saved")
            assertTrue(recording.file.isFile)
            assertEquals(44L + 2 * oneSecond.size, service.get(recording.id)?.bytes)

            // Finished, it is no longer held: the next cleanup deletes it.
            importInto(screen, "last.wav")
            assertNull(service.get(recording.id))
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
    fun savedPartsAreKeptForARetryAndRemovedOnceComplete() {
        withStore { store ->
            val stored = store.import(
                name = "long.wav",
                mimeType = "audio/wav",
                extension = "wav",
                input = ByteArrayInputStream(byteArrayOf(1, 2, 3, 4)),
            )
            val parts = store.partsFile(stored.id)
            assertEquals(File(store.file(stored).parentFile, "parts.json"), parts)
            parts.writeText("{\"key\":\"k\",\"parts\":[]}")

            listOf("failed", "cancelled", "transcribing").forEach { status ->
                store.finish(stored.id, status, error = "Network")
                assertTrue("$status keeps the finished parts for a retry", parts.exists())
            }
            store.finish(stored.id, "complete", transcript = "Hello.")
            assertFalse(parts.exists())
            assertEquals("Hello.", store.get(stored.id)?.transcript)
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
    fun recordingCheckpointsNeverReadOrRewriteAnyDetails() {
        withRoot { root ->
            val reads = mutableListOf<String>()
            val store = RecordingStore.forDirectory(root) { id -> synchronized(reads) { reads += id } }
            val longText = "words ".repeat(20_000)
            val saved = List(5) { index ->
                val stored = store.import(
                    name = "clip-$index.wav",
                    mimeType = "audio/wav",
                    extension = "wav",
                    input = ByteArrayInputStream(byteArrayOf(1, 2, 3, 4)),
                )
                store.finish(stored.id, "complete", transcript = "$index $longText")
                stored.id
            }
            val writer = store.beginPcm("Live recording", 16_000)
            val active = store.list().first { it.status == "recording" }
            val details = File(root, "${active.id}/metadata.json")
            val savedDetails = details.readText()
            assertTrue(details.setLastModified(1_000_000))

            reads.clear()
            val oneSecond = ByteArray(32_000)
            repeat(5) { writer.write(oneSecond, oneSecond.size) }

            assertEquals(emptyList<String>(), reads)
            assertEquals(1_000_000L, details.lastModified())
            assertEquals(savedDetails, details.readText())
            assertEquals(active.copy(bytes = 44L + 5 * 32_000), store.get(active.id))
            saved.forEachIndexed { index, id ->
                assertEquals("$index $longText", store.get(id)?.transcript)
            }
            writer.finish("saved")
            assertEquals(44L + 5 * 32_000, store.get(active.id)?.bytes)
        }
    }

    @Test
    fun theShownSizeOfALiveRecordingGrowsAndOneLeftOpenStaysReadable() {
        withStore { store ->
            val writer = store.beginPcm("Left open", 16_000)
            val id = store.list().single().id
            assertEquals(44L, store.get(id)?.bytes)

            val oneSecond = ByteArray(32_000) { (it % 101).toByte() }
            writer.write(oneSecond, oneSecond.size)
            assertEquals(44L + 32_000, store.list().single().bytes)
            writer.write(oneSecond, oneSecond.size)
            assertEquals(44L + 64_000, store.list().single().bytes)

            // Half a second more, then the app dies before the next checkpoint
            // or finish: the size shown is what reached the file, and the
            // header still describes the last checkpoint, so it plays.
            writer.write(oneSecond, 16_000)
            val left = store.get(id)!!
            assertEquals("recording", left.status)
            assertEquals(44L + 80_000, left.bytes)
            val wav = store.file(left).readBytes()
            assertEquals("RIFF", wav.copyOfRange(0, 4).decodeToString())
            assertEquals(64_000, littleEndianInt(wav, 40))
            assertArrayEquals(oneSecond, wav.copyOfRange(44, 44 + 32_000))
            writer.finish("cancelled")
        }
    }

    @Test
    fun anIdThatIsNotOneRecordingFolderFindsNothing() {
        withRoot { root ->
            val reads = mutableListOf<String>()
            val store = RecordingStore.forDirectory(File(root, "recordings")) { id -> reads += id }
            val stored = store.import(
                name = "memo.wav",
                mimeType = "audio/wav",
                extension = "wav",
                input = ByteArrayInputStream(byteArrayOf(1, 2, 3, 4)),
            )
            File(root, "metadata.json").writeText(File(store.file(stored).parentFile, "metadata.json").readText())
            reads.clear()

            listOf("", ".", "..", "../recordings/${stored.id}", "${stored.id}/..", "a\\b").forEach { id ->
                assertNull("\"$id\" is not a recording", store.get(id))
            }
            assertEquals(emptyList<String>(), reads)
            assertEquals(stored, store.get(stored.id))
        }
    }

    @Test
    fun aSlowImportDoesNotHoldUpTheOtherStores() {
        withRoot { root ->
            val screen = RecordingStore.forDirectory(root)
            val service = RecordingStore.forDirectory(root)
            val reading = CountDownLatch(1)
            val release = CountDownLatch(1)
            val slowInput = object : InputStream() {
                private var sent = false

                override fun read(): Int {
                    reading.countDown()
                    release.await()
                    return if (sent) -1 else 7.also { sent = true }
                }
            }
            var imported: StoredRecording? = null
            val importing = Thread {
                imported = screen.import("slow.wav", "audio/wav", "wav", slowInput)
            }.apply { start() }

            val listed = Executors.newSingleThreadExecutor()
            // Inside the try, so a failed wait still releases the importer.
            try {
                assertTrue(reading.await(5, TimeUnit.SECONDS))
                val recording = listed.submit<PcmRecording> {
                    service.beginPcm("Dictation", 16_000).finish("saved")
                }
                assertNotNull(recording.get(5, TimeUnit.SECONDS))
                assertEquals(1, listed.submit<Int> { service.list().size }.get(5, TimeUnit.SECONDS))
            } finally {
                release.countDown()
                importing.join()
                listed.shutdown()
            }
            assertEquals(2, service.list().size)
            assertNotNull(service.get(imported!!.id))
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

    /** Imports a small file, a little later than the last, so the newest is clear. */
    private fun importInto(store: RecordingStore, name: String): StoredRecording {
        Thread.sleep(2)
        return store.import(
            name = name,
            mimeType = "audio/wav",
            extension = "wav",
            input = ByteArrayInputStream(byteArrayOf(1, 2, 3, 4)),
        )
    }

    private fun littleEndianInt(bytes: ByteArray, offset: Int): Int =
        (0 until 4).sumOf { (bytes[offset + it].toInt() and 0xFF) shl (8 * it) }

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
