package com.openwhisper.android.settings

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

class SecureApiKeyStore(context: Context) {
    private val preferences = context.applicationContext.getSharedPreferences(PREFERENCES, 0)

    fun save(apiKey: String) {
        val normalized = apiKey.trim()
        if (normalized.isEmpty()) {
            clear()
            return
        }
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, getOrCreateKey())
        val encrypted = cipher.doFinal(normalized.encodeToByteArray())
        val encoded = listOf(
            VERSION,
            Base64.encodeToString(cipher.iv, Base64.NO_WRAP),
            Base64.encodeToString(encrypted, Base64.NO_WRAP),
        ).joinToString(":")
        preferences.edit().putString(API_KEY, encoded).apply()
    }

    fun load(): String? {
        val encoded = preferences.getString(API_KEY, null) ?: return null
        return runCatching {
            val parts = encoded.split(":", limit = 3)
            require(parts.size == 3 && parts[0] == VERSION)
            val cipher = Cipher.getInstance(TRANSFORMATION)
            val iv = Base64.decode(parts[1], Base64.NO_WRAP)
            cipher.init(Cipher.DECRYPT_MODE, getOrCreateKey(), GCMParameterSpec(128, iv))
            cipher.doFinal(Base64.decode(parts[2], Base64.NO_WRAP)).decodeToString()
        }.getOrElse {
            clear()
            null
        }
    }

    fun clear() {
        preferences.edit().remove(API_KEY).apply()
    }

    private fun getOrCreateKey(): SecretKey {
        val keyStore = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        (keyStore.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }

    private companion object {
        const val PREFERENCES = "openwhisper_secrets"
        const val API_KEY = "api_key_encrypted"
        const val VERSION = "v1"
        const val KEYSTORE = "AndroidKeyStore"
        const val KEY_ALIAS = "openwhisper_api_key_v1"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
    }
}
