package com.openwhisper.android.update

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
}
