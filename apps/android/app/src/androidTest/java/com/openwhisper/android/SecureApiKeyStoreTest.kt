package com.openwhisper.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.openwhisper.android.settings.SecureApiKeyStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SecureApiKeyStoreTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private lateinit var store: SecureApiKeyStore

    @Before
    fun resetStore() {
        store = SecureApiKeyStore(context)
        store.clear()
    }

    @Test
    fun roundTripsWithoutPersistingPlaintext() {
        val secret = "sk-instrumentation-secret"
        store.save(secret)

        assertEquals(secret, store.load())
        val rawPreferences = context
            .getSharedPreferences("openwhisper_secrets", 0)
            .all
            .values
            .joinToString()
        assertFalse(rawPreferences.contains(secret))
    }

    @Test
    fun clearRemovesStoredKey() {
        store.save("sk-temporary")
        store.clear()
        assertEquals(null, store.load())
    }
}
