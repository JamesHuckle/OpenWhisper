package com.openwhisper.android

import android.Manifest
import android.accessibilityservice.AccessibilityServiceInfo
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.text.InputType
import android.view.Gravity
import android.view.View
import android.view.accessibility.AccessibilityManager
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.Switch
import android.widget.TextView
import com.openwhisper.android.accessibility.OpenWhisperAccessibilityService
import com.openwhisper.android.demo.DemoTargetActivity
import com.openwhisper.android.settings.SecureApiKeyStore
import com.openwhisper.android.settings.SettingsRepository

class MainActivity : Activity() {
    private lateinit var accessibilityStatus: TextView
    private lateinit var microphoneStatus: TextView
    private lateinit var keyStatus: TextView
    private lateinit var secretStore: SecureApiKeyStore
    private lateinit var settingsRepository: SettingsRepository

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        secretStore = SecureApiKeyStore(this)
        settingsRepository = SettingsRepository(this)
        setContentView(buildContent())
    }

    override fun onResume() {
        super.onResume()
        refreshStatus()
    }

    private fun buildContent(): View {
        val content = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(24), dp(28), dp(24), dp(32))
            setBackgroundColor(Color.rgb(247, 247, 250))
        }

        content.addView(TextView(this).apply {
            text = getString(R.string.app_name)
            textSize = 30f
            setTextColor(Color.rgb(24, 24, 32))
        })
        content.addView(TextView(this).apply {
            text = getString(R.string.main_tagline)
            textSize = 16f
            setTextColor(Color.rgb(82, 82, 96))
            setPadding(0, dp(8), 0, dp(24))
        })

        accessibilityStatus = statusRow(getString(R.string.accessibility_service_label))
        content.addView(accessibilityStatus)
        content.addView(Button(this).apply {
            text = getString(R.string.enable_accessibility)
            isAllCaps = false
            setOnClickListener {
                startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS))
            }
        })

        microphoneStatus = statusRow(getString(R.string.microphone_permission_label))
        microphoneStatus.setPadding(0, dp(22), 0, dp(8))
        content.addView(microphoneStatus)
        content.addView(Button(this).apply {
            text = getString(R.string.grant_microphone)
            isAllCaps = false
            setOnClickListener { requestAudioPermissions() }
        })

        keyStatus = statusRow(getString(R.string.api_key_label))
        keyStatus.setPadding(0, dp(22), 0, dp(8))
        content.addView(keyStatus)
        val apiKey = EditText(this).apply {
            hint = getString(R.string.api_key_hint)
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            importantForAutofill = View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS
            contentDescription = getString(R.string.api_key_label)
        }
        content.addView(apiKey)
        content.addView(LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.END
            addView(Button(context).apply {
                text = getString(R.string.clear)
                isAllCaps = false
                setOnClickListener {
                    secretStore.clear()
                    apiKey.text.clear()
                    refreshStatus()
                }
            })
            addView(Button(context).apply {
                text = getString(R.string.save_securely)
                isAllCaps = false
                setOnClickListener {
                    secretStore.save(apiKey.text.toString())
                    apiKey.text.clear()
                    refreshStatus()
                }
            })
        })

        content.addView(Switch(this).apply {
            text = getString(R.string.demo_mode)
            textSize = 16f
            isChecked = settingsRepository.demoMode
            setPadding(0, dp(22), 0, dp(10))
            setOnCheckedChangeListener { _, checked -> settingsRepository.demoMode = checked }
        })
        content.addView(TextView(this).apply {
            text = getString(R.string.demo_mode_explanation)
            setTextColor(Color.rgb(82, 82, 96))
        })

        content.addView(Button(this).apply {
            text = getString(R.string.open_demo)
            isAllCaps = false
            setPadding(0, dp(20), 0, 0)
            setOnClickListener { startActivity(Intent(this@MainActivity, DemoTargetActivity::class.java)) }
        })

        return ScrollView(this).apply { addView(content) }
    }

    private fun statusRow(label: String) = TextView(this).apply {
        text = label
        textSize = 17f
        setTextColor(Color.rgb(35, 35, 45))
        setPadding(0, 0, 0, dp(8))
    }

    private fun requestAudioPermissions() {
        val permissions = mutableListOf(Manifest.permission.RECORD_AUDIO)
        if (Build.VERSION.SDK_INT >= 33) permissions += Manifest.permission.POST_NOTIFICATIONS
        requestPermissions(permissions.toTypedArray(), REQUEST_AUDIO)
    }

    private fun refreshStatus() {
        accessibilityStatus.text = getString(
            R.string.status_format,
            getString(R.string.accessibility_service_label),
            getString(if (isAccessibilityServiceEnabled()) R.string.enabled else R.string.not_enabled),
        )
        microphoneStatus.text = getString(
            R.string.status_format,
            getString(R.string.microphone_permission_label),
            getString(
                if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
                    R.string.granted
                } else {
                    R.string.not_granted
                },
            ),
        )
        keyStatus.text = getString(
            R.string.status_format,
            getString(R.string.api_key_label),
            getString(if (secretStore.load() == null) R.string.not_saved else R.string.saved_securely),
        )
    }

    private fun isAccessibilityServiceEnabled(): Boolean {
        val manager = getSystemService(AccessibilityManager::class.java)
        return manager
            .getEnabledAccessibilityServiceList(AccessibilityServiceInfo.FEEDBACK_ALL_MASK)
            .any { info ->
                info.resolveInfo.serviceInfo.packageName == packageName &&
                    info.resolveInfo.serviceInfo.name == OpenWhisperAccessibilityService::class.java.name
            }
    }

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    private companion object {
        const val REQUEST_AUDIO = 100
    }
}
