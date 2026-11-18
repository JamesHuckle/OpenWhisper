package com.openwhisper.android.recordings

import android.content.Context
import androidx.core.util.AtomicFile
import com.openwhisper.android.audio.WavEncoder
import java.io.File
import java.io.InputStream
import java.io.RandomAccessFile
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import org.json.JSONObject

data class StoredRecording(
    val id: String,
    val createdAtMs: Long,
    val name: String,
    val source: String,
    val mimeType: String,
    val fileName: String,
    val status: String,
    val bytes: Long,
    val transcript: String,
    val error: String,
)

data class PcmRecording(
    val id: String,
    val file: File,
    val sampleRate: Int,
    val byteCount: Long,
)

/**
 * The saved recordings in one folder. The app screen and the accessibility
 * service each make their own store, so every store of the same folder shares
 * one lock, and each recording's details are saved with [AtomicFile]: a crash
 * while saving leaves the previous details, never a half-written file.
 *
 * While a long recording is transcribed in parts, each finished part is saved
 * in its [partsFile], so a retry sends only the parts that did not finish. The
 * file goes when the transcription completes or the recording is pruned.
 *
 * While a recording is still being captured its details are written only when
 * it begins and when it finishes, never in between: the size shown for it is
 * read from its audio file instead, so the microphone never waits on this
 * store's lock.
 *
 * Only the newest [RETAIN_COUNT] recordings are kept, except that a recording
 * still in use is never deleted: a capture holds its recording from
 * [beginPcm] until [PcmRecordingWriter.finish], and a transcription holds its
 * recording with [reserve] until [release]. A released recording past the
 * limit goes at the next cleanup. Every store of the same folder shares these
 * reservations, so an import on the app screen never deletes a recording the
 * dictation overlay or a retry is still using.
 */
class RecordingStore private constructor(
    private val root: File,
    private val onMetadataRead: (String) -> Unit = {},
) {
    constructor(context: Context) : this(File(context.filesDir, "recordings"))

    private val folder: Folder = folderFor(root)

    internal val lock: Any = folder

    /**
     * Keeps [id] from being deleted until a matching [release], however many
     * newer recordings arrive. Reservations are counted, so a recording held
     * twice (a new job started just as the last one ends) stays held until
     * both release it.
     */
    fun reserve(id: String) {
        synchronized(lock) {
            folder.reservations[id] = (folder.reservations[id] ?: 0) + 1
        }
    }

    /** Ends one [reserve]; past the limit, the recording goes at the next cleanup. */
    fun release(id: String) {
        synchronized(lock) {
            val count = folder.reservations[id] ?: return
            if (count > 1) folder.reservations[id] = count - 1 else folder.reservations.remove(id)
        }
    }

    fun beginPcm(name: String, sampleRate: Int): PcmRecordingWriter = synchronized(lock) {
        root.mkdirs()
        val id = UUID.randomUUID().toString()
        val directory = File(root, id).apply { mkdirs() }
        val file = File(directory, "audio.wav")
        RandomAccessFile(file, "rw").use { wav ->
            wav.write(WavEncoder.header(0, sampleRate))
        }
        writeMetadata(
            StoredRecording(
                id = id,
                createdAtMs = System.currentTimeMillis(),
                name = name,
                source = "microphone",
                mimeType = "audio/wav",
                fileName = file.name,
                status = RECORDING,
                bytes = WavEncoder.HEADER_BYTES.toLong(),
                transcript = "",
                error = "",
            ),
        )
        // Held until the writer finishes, so a long capture survives any
        // number of newer imports.
        reserve(id)
        prune()
        try {
            PcmRecordingWriter(this, id, file, sampleRate)
        } catch (failure: Throwable) {
            release(id)
            throw failure
        }
    }

    fun import(
        name: String,
        mimeType: String,
        extension: String,
        input: InputStream,
    ): StoredRecording {
        // Copied outside the shared lock, so a slow document provider never
        // holds up a recording or the screen. Until its details are saved the
        // new folder is not listed, so nothing reads or prunes it meanwhile.
        val id = UUID.randomUUID().toString()
        val directory = File(root, id).apply { mkdirs() }
        val safeExtension = extension.lowercase().takeIf { it in SUPPORTED_EXTENSIONS } ?: "audio"
        val file = File(directory, "audio.$safeExtension")
        input.use { source -> file.outputStream().buffered().use(source::copyTo) }
        check(file.length() > 0) { "The selected audio file is empty" }
        return synchronized(lock) {
            val recording = StoredRecording(
                id = id,
                createdAtMs = System.currentTimeMillis(),
                name = name,
                source = "import",
                mimeType = mimeType,
                fileName = file.name,
                status = "saved",
                bytes = file.length(),
                transcript = "",
                error = "",
            )
            writeMetadata(recording)
            prune()
            recording
        }
    }

    fun finish(id: String, status: String, transcript: String = "", error: String = "") = synchronized(lock) {
        val current = get(id) ?: return
        writeMetadata(
            current.copy(
                status = status,
                bytes = file(current).length(),
                transcript = transcript,
                error = error,
            ),
        )
        // A completed transcription no longer needs its saved parts, so
        // transcribing it again starts fresh.
        if (status == "complete") AtomicFile(partsFile(id)).delete()
        prune()
    }

    fun list(): List<StoredRecording> = synchronized(lock) {
        if (!root.exists()) return emptyList()
        root.listFiles()
            .orEmpty()
            .asSequence()
            .filter(File::isDirectory)
            .mapNotNull(::readMetadata)
            .sortedByDescending(StoredRecording::createdAtMs)
            .toList()
    }

    /** One recording's details, read from its own folder only. */
    fun get(id: String): StoredRecording? = synchronized(lock) {
        if (!isSafeId(id)) return null
        readMetadata(File(root, id))?.takeIf { it.id == id }
    }

    fun file(recording: StoredRecording): File = File(File(root, recording.id), recording.fileName)

    /** Where the finished parts of this recording's transcription are saved. */
    fun partsFile(id: String): File = File(File(root, id), PARTS_FILE)

    private fun metadataFile(directory: File) = AtomicFile(File(directory, METADATA_FILE))

    private fun readMetadata(directory: File): StoredRecording? {
        onMetadataRead(directory.name)
        val recording = runCatching {
            parseMetadata(metadataFile(directory).readFully().decodeToString())
        }.getOrNull() ?: return null
        // A recording still being captured (or left so by a crash) has the
        // size of its audio file so far; its details are not kept up to date.
        return if (recording.status == RECORDING) {
            recording.copy(bytes = File(directory, recording.fileName).length())
        } else {
            recording
        }
    }

    /** A recording id names one folder directly inside [root], nothing else. */
    private fun isSafeId(id: String): Boolean =
        id.isNotEmpty() && id != "." && ".." !in id && '/' !in id && '\\' !in id

    private fun writeMetadata(recording: StoredRecording) {
        val directory = File(root, recording.id).apply { mkdirs() }
        val json =
            JSONObject()
                .put("id", recording.id)
                .put("createdAtMs", recording.createdAtMs)
                .put("name", recording.name)
                .put("source", recording.source)
                .put("mimeType", recording.mimeType)
                .put("fileName", recording.fileName)
                .put("status", recording.status)
                .put("bytes", recording.bytes)
                .put("transcript", recording.transcript)
                .put("error", recording.error)
                .toString(2)
        val file = metadataFile(directory)
        val output = file.startWrite()
        try {
            output.write(json.encodeToByteArray())
            file.finishWrite(output)
        } catch (failure: Throwable) {
            file.failWrite(output)
            throw failure
        }
    }

    private fun parseMetadata(raw: String): StoredRecording {
        val json = JSONObject(raw)
        return StoredRecording(
            id = json.getString("id"),
            createdAtMs = json.getLong("createdAtMs"),
            name = json.getString("name"),
            source = json.getString("source"),
            mimeType = json.getString("mimeType"),
            fileName = json.getString("fileName"),
            status = json.getString("status"),
            bytes = json.optLong("bytes"),
            transcript = json.optString("transcript"),
            error = json.optString("error"),
        )
    }

    /** Deletes every recording past the newest [RETAIN_COUNT] that is not reserved. */
    private fun prune() {
        list().drop(RETAIN_COUNT).filter { it.id !in folder.reservations }.forEach { recording ->
            File(root, recording.id).takeIf { it.parentFile == root }?.deleteRecursively()
        }
    }

    companion object {
        const val RETAIN_COUNT = 10
        private const val RECORDING = "recording"
        private const val METADATA_FILE = "metadata.json"
        private const val PARTS_FILE = "parts.json"
        private val SUPPORTED_EXTENSIONS =
            setOf("wav", "mp3", "m4a", "mp4", "mpeg", "mpga", "webm", "ogg")

        /** One folder's shared lock, and the recordings reserved in it (guarded by that lock). */
        private class Folder {
            val reservations = mutableMapOf<String, Int>()
        }

        private val folders = ConcurrentHashMap<String, Folder>()

        private fun folderFor(root: File): Folder =
            folders.computeIfAbsent(root.absoluteFile.normalize().path) { Folder() }

        /** A store of [root]; [onMetadataRead] hears the id of every recording whose details are read. */
        internal fun forDirectory(
            root: File,
            onMetadataRead: (String) -> Unit = {},
        ): RecordingStore = RecordingStore(root, onMetadataRead)
    }
}

class PcmRecordingWriter internal constructor(
    private val store: RecordingStore,
    private val id: String,
    private val file: File,
    private val sampleRate: Int,
) {
    private val wav = RandomAccessFile(file, "rw").apply {
        seek(WavEncoder.HEADER_BYTES.toLong())
    }
    private var pcmBytes = 0L
    private var bytesSinceCheckpoint = 0L
    private var closed = false

    /** Whether this capture still holds its recording; see [RecordingStore.reserve]. */
    private var reserved = true

    @Synchronized
    fun write(bytes: ByteArray, count: Int) {
        check(!closed) { "Recording is already closed" }
        wav.write(bytes, 0, count)
        pcmBytes += count
        bytesSinceCheckpoint += count
        if (bytesSinceCheckpoint >= sampleRate * 2L) checkpoint()
    }

    @Synchronized
    fun finish(status: String): PcmRecording {
        if (!closed) {
            try {
                checkpoint()
                wav.close()
                closed = true
                store.finish(id, status)
            } finally {
                // Released once, even when finishing fails, so a broken
                // capture is never kept past the limit for ever.
                if (reserved) {
                    reserved = false
                    store.release(id)
                }
            }
        }
        return PcmRecording(id, file, sampleRate, pcmBytes)
    }

    /**
     * Makes the file playable up to here and forces it to disk. It touches
     * only the audio file, never the store, so it takes no shared lock.
     */
    private fun checkpoint() {
        val end = WavEncoder.HEADER_BYTES + pcmBytes
        wav.seek(0)
        wav.write(WavEncoder.header(pcmBytes, sampleRate))
        wav.seek(end)
        wav.fd.sync()
        bytesSinceCheckpoint = 0
    }
}
