package com.openwhisper.android.settings

import android.content.Context
import com.openwhisper.android.BuildConfig

class SettingsRepository(context: Context) {
    private val preferences = context.applicationContext.getSharedPreferences("openwhisper_settings", 0)

    var demoMode: Boolean
        get() = preferences.getBoolean("demo_mode", BuildConfig.DEMO_TRANSCRIBER)
        set(value) = preferences.edit().putBoolean("demo_mode", value).apply()
}
