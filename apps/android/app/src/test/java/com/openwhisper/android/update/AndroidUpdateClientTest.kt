package com.openwhisper.android.update

import java.io.File
import java.net.InetAddress
import java.net.ServerSocket
import java.nio.file.Files
import kotlin.concurrent.thread
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

class AndroidUpdateClientTest {
    private val apk = "openwhisper".toByteArray()
    private val apkSha256 = "59631e7364ddb94218e4f35acaf73f9cd5bd0a67223c4c7ac1dfcd2d46c5afe2"
    private lateinit var server: ServerSocket
    private lateinit var folder: File

    @Before
    fun startServer() {
        folder = Files.createTempDirectory("openwhisper-update").toFile()
        server = ServerSocket(0, 1, InetAddress.getLoopbackAddress())
        thread(isDaemon = true) {
            while (!server.isClosed) {
                val socket = try { server.accept() } catch (_: Exception) { break }
                socket.use {
                    val reader = it.getInputStream().bufferedReader()
                    while (reader.readLine().orEmpty().isNotEmpty()) Unit
                    val output = it.getOutputStream()
                    output.write(
                        "HTTP/1.1 200 OK\r\nContent-Length: ${apk.size}\r\nConnection: close\r\n\r\n".toByteArray(),
                    )
                    output.write(apk)
                    output.flush()
                }
            }
        }
    }

    @After
    fun stopServer() {
        server.close()
        folder.deleteRecursively()
    }

    @Test
    fun downloadsAnApkWhoseHashMatches() {
        val destination = File(folder, "update.apk")

        val result = AndroidUpdateClient().download(manifest(apkSha256), destination)

        assertEquals(destination, result)
        assertArrayEquals(apk, destination.readBytes())
        assertFalse(File(folder, "update.apk.download").exists())
    }

    @Test
    fun deletesTheDownloadWhenItsHashDoesNotMatch() {
        val destination = File(folder, "update.apk")

        try {
            AndroidUpdateClient().download(manifest("a".repeat(64)), destination)
            fail("A mismatched hash must be rejected")
        } catch (error: IllegalArgumentException) {
            assertEquals("Downloaded update failed its integrity check", error.message)
        }

        assertFalse(destination.exists())
        assertFalse(File(folder, "update.apk.download").exists())
    }

    private fun manifest(sha256: String) = UpdateManifest(
        versionCode = 2,
        versionName = "0.2.0",
        apkUrl = "http://127.0.0.1:${server.localPort}/update.apk",
        sha256 = sha256,
        releaseNotes = "",
    )
}
