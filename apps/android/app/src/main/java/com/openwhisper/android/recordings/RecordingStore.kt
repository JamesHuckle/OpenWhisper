package com.openwhisper.android.recordings

import android.content.Context
import java.io.File
import java.io.InputStream
import java.io.RandomAccessFile
import java.util.UUID
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

class RecordingStore private constructor(private val root: File) {
    constructor(context: Context) : this(File(context.filesDir, "recordings"))

    @Synchronized
    fun beginPcm(name: String, sampleRate: Int): PcmRecordingWriter {
        root.mkdirs()
        val id = UUID.randomUUID().toString()
        val directory = File(root, id).apply { mkdirs() }
        val file = File(directory, "audio.wav")
        RandomAccessFile(file, "rw").use { wav ->
            wav.write(wavHeader(0, sampleRate))
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
                bytes = WAV_HEADER_BYTES.toLong(),
                transcript = "",
                error = "",
            ),
        )
        prune()
        return PcmRecordingWriter(this, id, file, sampleRate)
    }

    @Synchronized
    fun import(
        name: String,
        mimeType: String,
        extension: String,
        input: InputStream,
    ): StoredRecording {
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
        return recording
    }

    @Synchronized
    fun finish(id: String, status: String, transcript: String = "", error: String = "") {
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

    @Synchronized
    fun list(): List<StoredRecording> {
        if (!root.exists()) return emptyList()
        return root.listFiles()
            .orEmpty()
            .asSequence()
            .filter(File::isDirectory)
            .mapNotNull { directory ->
                runCatching { parseMetadata(File(directory, METADATA_FILE).readText()) }.getOrNull()
            }
            .sortedByDescending(StoredRecording::createdAtMs)
            .toList()
    }

    @Synchronized
    fun get(id: String): StoredRecording? = list().firstOrNull { it.id == id }

    fun file(recording: StoredRecording): File = File(File(root, recording.id), recording.fileName)

    internal fun updateCapturedBytes(id: String, bytes: Long) {
        synchronized(this) {
            val current = get(id) ?: return
            writeMetadata(current.copy(bytes = bytes + WAV_HEADER_BYTES))
        }
    }

    private fun writeMetadata(recording: StoredRecording) {
        val directory = File(root, recording.id).apply { mkdirs() }
        File(directory, METADATA_FILE).writeText(
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
                .toString(2),
        )
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
        const val WAV_HEADER_BYTES = 44
        private const val METADATA_FILE = "metadata.json"
        private val SUPPORTED_EXTENSIONS =
            setOf("wav", "mp3", "m4a", "mp4", "mpeg", "mpga", "webm", "ogg")

        internal fun forDirectory(root: File): RecordingStore = RecordingStore(root)

        internal fun wavHeader(pcmBytes: Long, sampleRate: Int): ByteArray {
            require(sampleRate > 0) { "sampleRate must be positive" }
            require(pcmBytes in 0..UInt.MAX_VALUE.toLong()) { "Recording is too large for WAV" }
            val result = ByteArray(WAV_HEADER_BYTES)
            writeAscii(result, 0, "RIFF")
            writeUInt(result, 4, 36 + pcmBytes)
            writeAscii(result, 8, "WAVE")
            writeAscii(result, 12, "fmt ")
            writeUInt(result, 16, 16)
            writeShort(result, 20, 1)
            writeShort(result, 22, 1)
            writeUInt(result, 24, sampleRate.toLong())
            writeUInt(result, 28, sampleRate.toLong() * 2)
            writeShort(result, 32, 2)
            writeShort(result, 34, 16)
            writeAscii(result, 36, "data")
            writeUInt(result, 40, pcmBytes)
            return result
        }

        private fun writeAscii(target: ByteArray, offset: Int, value: String) {
            value.encodeToByteArray().copyInto(target, destinationOffset = offset)
        }

        private fun writeUInt(target: ByteArray, offset: Int, value: Long) {
            target[offset] = value.toByte()
            target[offset + 1] = (value ushr 8).toByte()
            target[offset + 2] = (value ushr 16).toByte()
            target[offset + 3] = (value ushr 24).toByte()
        }

        private fun writeShort(target: ByteArray, offset: Int, value: Int) {
            target[offset] = value.toByte()
            target[offset + 1] = (value ushr 8).toByte()
        }
    }
}

class PcmRecordingWriter internal constructor(
    private val store: RecordingStore,
    private val id: String,
    private val file: File,
    private val sampleRate: Int,
) {
    private val wav = RandomAccessFile(file, "rw").apply {
        seek(RecordingStore.WAV_HEADER_BYTES.toLong())
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
        val end = RecordingStore.WAV_HEADER_BYTES + pcmBytes
        wav.seek(0)
        wav.write(RecordingStore.wavHeader(pcmBytes, sampleRate))
        wav.seek(end)
        wav.fd.sync()
        store.updateCapturedBytes(id, pcmBytes)
        bytesSinceCheckpoint = 0
    }
}
