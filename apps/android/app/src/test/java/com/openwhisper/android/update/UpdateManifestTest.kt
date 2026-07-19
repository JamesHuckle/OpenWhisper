package com.openwhisper.android.update

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class UpdateManifestTest {
    @Test
    fun parsesUpdateFeedAndUsesMonotonicVersionCodes() {
        val manifest = UpdateManifestCodec.parse(
            """
            {
              "versionCode": 12,
              "versionName": "0.4.0",
              "apkUrl": "https://github.com/example/update.apk",
              "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
              "releaseNotes": "Faster transcription"
            }
            """.trimIndent(),
        )

        assertEquals("0.4.0", manifest.versionName)
        assertTrue(manifest.isNewerThan(11))
        assertFalse(manifest.isNewerThan(12))
    }

    @Test(expected = IllegalArgumentException::class)
    fun rejectsAnInsecureDownloadUrl() {
        UpdateManifestCodec.parse(
            """
            {
              "versionCode": 2,
              "versionName": "0.2.0",
              "apkUrl": "http://example.com/update.apk",
              "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }
            """.trimIndent(),
        )
    }

    @Test
    fun hashesDownloadedArtifacts() {
        val file = File.createTempFile("openwhisper-update", ".apk")
        try {
            file.writeText("openwhisper")
            assertEquals(
                "59631e7364ddb94218e4f35acaf73f9cd5bd0a67223c4c7ac1dfcd2d46c5afe2",
                UpdateIntegrity.sha256(file),
            )
        } finally {
            file.delete()
        }
    }
}
