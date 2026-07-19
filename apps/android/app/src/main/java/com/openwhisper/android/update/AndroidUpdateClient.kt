package com.openwhisper.android.update

import java.io.File
import java.net.HttpURLConnection
import java.net.URL

class AndroidUpdateClient {
    fun findUpdate(installedVersionCode: Int): UpdateManifest? {
        val connection = open(UPDATE_MANIFEST_URL)
        return try {
            requireSuccess(connection)
            val json = connection.inputStream.bufferedReader().use { reader ->
                val value = reader.readText()
                require(value.length <= MAX_MANIFEST_LENGTH) { "Update manifest is too large" }
                value
            }
            UpdateManifestCodec.parse(json).takeIf { it.isNewerThan(installedVersionCode) }
        } finally {
            connection.disconnect()
        }
    }

    fun download(update: UpdateManifest, destination: File): File {
        destination.parentFile?.mkdirs()
        val temporary = File(destination.parentFile, "${destination.name}.download")
        temporary.delete()
        val connection = open(update.apkUrl)
        try {
            requireSuccess(connection)
            val expectedLength = connection.contentLengthLong
            require(expectedLength <= 0 || expectedLength <= MAX_APK_BYTES) { "Update is too large" }
            connection.inputStream.buffered().use { input ->
                temporary.outputStream().buffered().use { output ->
                    val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
                    var total = 0L
                    while (true) {
                        val read = input.read(buffer)
                        if (read < 0) break
                        total += read
                        require(total <= MAX_APK_BYTES) { "Update is too large" }
                        output.write(buffer, 0, read)
                    }
                }
            }
            require(UpdateIntegrity.sha256(temporary) == update.sha256) {
                "Downloaded update failed its integrity check"
            }
            if (destination.exists()) destination.delete()
            require(temporary.renameTo(destination)) { "Unable to prepare downloaded update" }
            return destination
        } catch (error: Throwable) {
            temporary.delete()
            throw error
        } finally {
            connection.disconnect()
        }
    }

    private fun open(url: String): HttpURLConnection =
        (URL(url).openConnection() as HttpURLConnection).apply {
            connectTimeout = CONNECT_TIMEOUT_MS
            readTimeout = READ_TIMEOUT_MS
            instanceFollowRedirects = true
            setRequestProperty("Accept", "application/json, application/octet-stream")
            setRequestProperty("User-Agent", "OpenWhisper-Android-Updater")
        }

    private fun requireSuccess(connection: HttpURLConnection) {
        val status = connection.responseCode
        require(status in 200..299) { "Update server returned HTTP $status" }
    }

    private companion object {
        const val UPDATE_MANIFEST_URL =
            "https://github.com/JamesHuckle/OpenWhisper/releases/latest/download/OpenWhisper-Android-update.json"
        const val CONNECT_TIMEOUT_MS = 15_000
        const val READ_TIMEOUT_MS = 60_000
        const val MAX_MANIFEST_LENGTH = 64 * 1024
        const val MAX_APK_BYTES = 200L * 1024L * 1024L
    }
}
