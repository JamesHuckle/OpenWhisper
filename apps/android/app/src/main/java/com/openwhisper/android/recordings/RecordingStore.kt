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
 */
class RecordingStore private constructor(private val root: File) {
    constructor(context: Context) : this(File(context.filesDir, "recordings"))

    internal val lock: Any = lockFor(root)

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
                status = "recording",
                bytes = WavEncoder.HEADER_BYTES.toLong(),
                transcript = "",
                error = "",
            ),
        )
        prune()
        PcmRecordingWriter(this, id, file, sampleRate)
    }

    fun import(
        name: String,
        mimeType: String,
        extension: String,
        input: InputStream,
    ): StoredRecording = synchronized(lock) {
        root.mkdirs()
        val id = UUID.randomUUID().toString()
        val directory = File(root, id).apply { mkdirs() }
        val safeExtension = extension.lowercase().takeIf { it in SUPPORTED_EXTENSIONS } ?: "audio"
        val file = File(directory, "audio.$safeExtension")
        input.use { source -> file.outputStream().buffered().use(source::copyTo) }
        check(file.length() > 0) { "The selected audio file is empty" }
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
        prune()
    }

    fun list(): List<StoredRecording> = synchronized(lock) {
        if (!root.exists()) return emptyList()
        root.listFiles()
            .orEmpty()
            .asSequence()
            .filter(File::isDirectory)
            .mapNotNull { directory ->
                runCatching {
                    parseMetadata(metadataFile(directory).readFully().decodeToString())
                }.getOrNull()
            }
            .sortedByDescending(StoredRecording::createdAtMs)
            .toList()
    }

    fun get(id: String): StoredRecording? = list().firstOrNull { it.id == id }

    fun file(recording: StoredRecording): File = File(File(root, recording.id), recording.fileName)

    internal fun updateCapturedBytes(id: String, bytes: Long) {
        synchronized(lock) {
            val current = get(id) ?: return
            writeMetadata(current.copy(bytes = bytes + WavEncoder.HEADER_BYTES))
        }
    }

    private fun metadataFile(directory: File) = AtomicFile(File(directory, METADATA_FILE))

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

    private fun prune() {
        list().drop(RETAIN_COUNT).forEach { recording ->
            File(root, recording.id).takeIf { it.parentFile == root }?.deleteRecursively()
        }
    }

    companion object {
        const val RETAIN_COUNT = 10
        private const val METADATA_FILE = "metadata.json"
        private val SUPPORTED_EXTENSIONS =
            setOf("wav", "mp3", "m4a", "mp4", "mpeg", "mpga", "webm", "ogg")

        private val folderLocks = ConcurrentHashMap<String, Any>()

        private fun lockFor(root: File): Any =
            folderLocks.computeIfAbsent(root.absoluteFile.normalize().path) { Any() }

        internal fun forDirectory(root: File): RecordingStore = RecordingStore(root)
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
            checkpoint()
            wav.close()
            closed = true
            store.finish(id, status)
        }
        return PcmRecording(id, file, sampleRate, pcmBytes)
    }

    private fun checkpoint() {
        val end = WavEncoder.HEADER_BYTES + pcmBytes
        wav.seek(0)
        wav.write(WavEncoder.header(pcmBytes, sampleRate))
        wav.seek(end)
        wav.fd.sync()
        store.updateCapturedBytes(id, pcmBytes)
        bytesSinceCheckpoint = 0
    }
}
