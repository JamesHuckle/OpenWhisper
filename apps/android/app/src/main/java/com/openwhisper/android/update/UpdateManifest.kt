package com.openwhisper.android.update

import org.json.JSONObject

data class UpdateManifest(
    val versionCode: Int,
    val versionName: String,
    val apkUrl: String,
    val sha256: String,
    val releaseNotes: String,
) {
    fun isNewerThan(installedVersionCode: Int): Boolean = versionCode > installedVersionCode
}

object UpdateManifestCodec {
    fun parse(json: String): UpdateManifest {
        val value = JSONObject(json)
        val manifest = UpdateManifest(
            versionCode = value.getInt("versionCode"),
            versionName = value.getString("versionName").trim(),
            apkUrl = value.getString("apkUrl").trim(),
            sha256 = value.getString("sha256").trim().lowercase(),
            releaseNotes = value.optString("releaseNotes", "").trim(),
        )
        require(manifest.versionCode > 0) { "versionCode must be positive" }
        require(manifest.versionName.isNotEmpty()) { "versionName must not be empty" }
        require(manifest.apkUrl.startsWith("https://")) { "apkUrl must use HTTPS" }
        require(manifest.sha256.matches(Regex("[0-9a-f]{64}"))) { "sha256 is invalid" }
        return manifest
    }
}
