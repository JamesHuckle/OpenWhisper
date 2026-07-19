package com.openwhisper.android

import android.Manifest
import android.accessibilityservice.AccessibilityServiceInfo
import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.net.Uri
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
import androidx.core.content.FileProvider
import com.openwhisper.android.accessibility.OpenWhisperAccessibilityService
import com.openwhisper.android.demo.DemoTargetActivity
import com.openwhisper.android.settings.SecureApiKeyStore
import com.openwhisper.android.settings.SettingsRepository
import com.openwhisper.android.update.AndroidUpdateClient
import com.openwhisper.android.update.UpdateManifest
import com.openwhisper.android.update.UpdateNotifier
import java.io.File
import java.util.concurrent.Executors

class MainActivity : Activity() {
    private lateinit var accessibilityStatus: TextView
    private lateinit var microphoneStatus: TextView
    private lateinit var keyStatus: TextView
    private lateinit var secretStore: SecureApiKeyStore
    private lateinit var settingsRepository: SettingsRepository
    private val updateClient = AndroidUpdateClient()
    private val updateExecutor = Executors.newSingleThreadExecutor { task ->
        Thread(task, "openwhisper-updater")
    }
    private var automaticUpdateCheckCompleted = false
    private var updateCheckInFlight = false
    private var promptedVersionCode: Int? = null
    private var pendingUpdateFile: File? = null
    private var waitingForInstallPermission = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        secretStore = SecureApiKeyStore(this)
        settingsRepository = SettingsRepository(this)
        setContentView(buildContent())
        UpdateNotifier.fromIntent(intent)?.let { update ->
            window.decorView.post { showUpdatePrompt(update) }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        UpdateNotifier.fromIntent(intent)?.let(::showUpdatePrompt)
    }

    override fun onResume() {
        super.onResume()
        refreshStatus()
        if (waitingForInstallPermission && canInstallPackages()) {
            waitingForInstallPermission = false
            pendingUpdateFile?.takeIf(File::exists)?.let(::launchPackageInstaller)
            return
        }
        if (!automaticUpdateCheckCompleted) checkForUpdates(showUpToDate = false)
    }

    override fun onDestroy() {
        updateExecutor.shutdownNow()
        super.onDestroy()
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
            setText(secretStore.load().orEmpty())
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
                    apiKey.setText(secretStore.load().orEmpty())
                    apiKey.setSelection(apiKey.text.length)
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

        content.addView(Button(this).apply {
            text = getString(R.string.check_for_updates)
            isAllCaps = false
            setOnClickListener { checkForUpdates(showUpToDate = true) }
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

    private fun checkForUpdates(showUpToDate: Boolean) {
        if (updateCheckInFlight) return
        updateCheckInFlight = true
        updateExecutor.execute {
            val result = runCatching { updateClient.findUpdate(BuildConfig.VERSION_CODE) }
            runOnUiThread {
                updateCheckInFlight = false
                automaticUpdateCheckCompleted = true
                if (isFinishing || isDestroyed) return@runOnUiThread
                result.fold(
                    onSuccess = { update ->
                        if (update != null) showUpdatePrompt(update)
                        else if (showUpToDate) showMessage(
                            R.string.up_to_date_title,
                            R.string.up_to_date_message,
                        )
                    },
                    onFailure = {
                        if (showUpToDate) showMessage(
                            R.string.update_failed_title,
                            R.string.update_failed_message,
                        )
                    },
                )
            }
        }
    }

    private fun showUpdatePrompt(update: UpdateManifest) {
        if (promptedVersionCode == update.versionCode) return
        promptedVersionCode = update.versionCode
        val message = buildString {
            append(getString(R.string.update_available_message, update.versionName))
            if (update.releaseNotes.isNotBlank()) {
                append("\n\n")
                append(update.releaseNotes)
            }
        }
        AlertDialog.Builder(this)
            .setTitle(R.string.update_available_title)
            .setMessage(message)
            .setPositiveButton(R.string.update_now) { _, _ -> downloadUpdate(update) }
            .setNegativeButton(R.string.later, null)
            .show()
    }

    private fun downloadUpdate(update: UpdateManifest) {
        val progress = AlertDialog.Builder(this)
            .setTitle(R.string.update_available_title)
            .setMessage(R.string.downloading_update)
            .setCancelable(false)
            .show()
        updateExecutor.execute {
            val destination = File(File(cacheDir, "updates"), "OpenWhisper-Android.apk")
            val result = runCatching { updateClient.download(update, destination) }
            runOnUiThread {
                progress.dismiss()
                if (isFinishing || isDestroyed) return@runOnUiThread
                result.fold(
                    onSuccess = { file ->
                        pendingUpdateFile = file
                        requestInstallOrLaunch(file)
                    },
                    onFailure = {
                        showMessage(R.string.update_failed_title, R.string.update_failed_message)
                    },
                )
            }
        }
    }

    private fun requestInstallOrLaunch(file: File) {
        if (canInstallPackages()) {
            launchPackageInstaller(file)
            return
        }
        AlertDialog.Builder(this)
            .setTitle(R.string.update_ready_title)
            .setMessage(R.string.allow_updates_message)
            .setPositiveButton(R.string.open_settings) { _, _ ->
                waitingForInstallPermission = true
                startActivity(
                    Intent(
                        Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                        Uri.parse("package:$packageName"),
                    ),
                )
            }
            .setNegativeButton(R.string.later, null)
            .show()
    }

    private fun canInstallPackages(): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.O || packageManager.canRequestPackageInstalls()

    private fun launchPackageInstaller(file: File) {
        val uri = FileProvider.getUriForFile(this, "$packageName.updates", file)
        startActivity(
            Intent(Intent.ACTION_VIEW).apply {
                setDataAndType(uri, "application/vnd.android.package-archive")
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            },
        )
    }

    private fun showMessage(title: Int, message: Int) {
        AlertDialog.Builder(this)
            .setTitle(title)
            .setMessage(message)
            .setPositiveButton(android.R.string.ok, null)
            .show()
    }

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    private companion object {
        const val REQUEST_AUDIO = 100
    }
}
