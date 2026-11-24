package com.openwhisper.android

import android.Manifest
import android.accessibilityservice.AccessibilityServiceInfo
import android.app.Activity
import android.app.AlertDialog
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.provider.OpenableColumns
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
import com.openwhisper.android.recordings.RecordingStore
import com.openwhisper.android.recordings.StoredRecording
import com.openwhisper.android.settings.SecureApiKeyStore
import com.openwhisper.android.settings.SettingsRepository
import com.openwhisper.android.transcription.TranscriptionJob
import com.openwhisper.android.transcription.TranscriptionManager
import com.openwhisper.android.transcription.TranscriptionOrigin
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
    private lateinit var recordingStore: RecordingStore
    private lateinit var transcriptions: TranscriptionManager
    private lateinit var recordingsContainer: LinearLayout
    private lateinit var importStatus: TextView
    private val updateClient = AndroidUpdateClient()
    private val updateExecutor = Executors.newSingleThreadExecutor { task ->
        Thread(task, "openwhisper-updater")
    }
    private val audioExecutor = Executors.newSingleThreadExecutor { task ->
        Thread(task, "openwhisper-audio-import")
    }
    private var automaticUpdateCheckCompleted = false
    private var updateCheckInFlight = false
    private var promptedVersionCode: Int? = null
    private var pendingUpdateFile: File? = null
    private var waitingForInstallPermission = false
    // Only the copy of an imported file; its upload is the transcription
    // manager's, so it carries on when Android recreates this screen.
    private var importInFlight = false
    private val transcriptionListener = object : TranscriptionManager.Listener {
        override fun onStarted(job: TranscriptionJob) = refreshRecordings()

        override fun onFinished(job: TranscriptionJob, result: Result<String>) {
            refreshRecordings()
            if (!job.showResultInApp || isFinishing || isDestroyed) return
            val recording = recordingStore.get(job.recordingId)
            result.fold(
                onSuccess = { transcript -> showImportedTranscript(transcript, recording) },
                onFailure = { error -> showAudioFailure(error, recording) },
            )
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        secretStore = SecureApiKeyStore(this)
        settingsRepository = SettingsRepository(this)
        recordingStore = OpenWhisperApplication.recordings(this)
        transcriptions = OpenWhisperApplication.transcriptions(this)
        setContentView(buildContent())
        transcriptions.addListener(transcriptionListener)
        showDictationRecoveryFrom(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        if (UpdateNotifier.isUpdateIntent(intent)) checkForUpdates(showUpToDate = false, showFailure = true)
        showDictationRecoveryFrom(intent)
    }

    override fun onResume() {
        super.onResume()
        refreshStatus()
        refreshRecordings()
        if (waitingForInstallPermission && canInstallPackages()) {
            waitingForInstallPermission = false
            pendingUpdateFile?.takeIf(File::exists)?.let(::launchPackageInstaller)
            return
        }
        if (!automaticUpdateCheckCompleted) {
            // Opened from an update notification: a failed refresh is shown.
            checkForUpdates(showUpToDate = false, showFailure = UpdateNotifier.isUpdateIntent(intent))
        }
    }

    override fun onDestroy() {
        transcriptions.removeListener(transcriptionListener)
        updateExecutor.shutdownNow()
        audioExecutor.shutdownNow()
        super.onDestroy()
    }

    private fun buildContent(): View {
        val content = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(24), dp(28), dp(24), dp(32))
            setBackgroundColor(Color.rgb(247, 247, 250))
        }

        content.addView(label(getString(R.string.app_name), TITLE_TEXT, sizeSp = 30f))
        content.addView(label(getString(R.string.main_tagline), sizeSp = 16f, paddingTopDp = 8, paddingBottomDp = 24))

        accessibilityStatus = statusRow(getString(R.string.accessibility_service_label))
        content.addView(accessibilityStatus)
        content.addView(button(getString(R.string.enable_accessibility)) {
            startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS))
        })

        microphoneStatus = statusRow(getString(R.string.microphone_permission_label), paddingTopDp = 22)
        content.addView(microphoneStatus)
        content.addView(button(getString(R.string.grant_microphone)) { requestAudioPermissions() })

        keyStatus = statusRow(getString(R.string.api_key_label), paddingTopDp = 22)
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
            addView(button(getString(R.string.clear)) {
                secretStore.clear()
                apiKey.text.clear()
                refreshStatus()
            })
            addView(button(getString(R.string.save_securely)) {
                secretStore.save(apiKey.text.toString())
                apiKey.setText(secretStore.load().orEmpty())
                apiKey.setSelection(apiKey.text.length)
                refreshStatus()
            })
        })

        content.addView(Switch(this).apply {
            text = getString(R.string.demo_mode)
            textSize = 16f
            isChecked = settingsRepository.demoMode
            setPadding(0, dp(22), 0, dp(10))
            setOnCheckedChangeListener { _, checked -> settingsRepository.demoMode = checked }
        })
        content.addView(label(getString(R.string.demo_mode_explanation)))

        content.addView(button(getString(R.string.open_demo)) {
            startActivity(Intent(this, DemoTargetActivity::class.java))
        }.apply { setPadding(0, dp(20), 0, 0) })

        content.addView(
            label(getString(R.string.audio_and_recovery), HEADING_TEXT, 19f, paddingTopDp = 26, paddingBottomDp = 8),
        )
        content.addView(button(getString(R.string.transcribe_audio_file)) { chooseAudioFile() })
        // Its text is set by importAndTranscribe and refreshRecordings.
        importStatus = label("", paddingTopDp = 7, paddingBottomDp = 7)
        content.addView(importStatus)
        recordingsContainer = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
        }
        content.addView(recordingsContainer)

        content.addView(button(getString(R.string.check_for_updates)) { checkForUpdates(showUpToDate = true) })

        return ScrollView(this).apply { addView(content) }
    }

    @Deprecated("Legacy Activity result API is sufficient for the platform-only UI.")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode == REQUEST_IMPORT_AUDIO && resultCode == RESULT_OK) {
            data?.data?.let(::importAndTranscribe)
        }
    }

    private fun chooseAudioFile() {
        startActivityForResult(
            Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = "audio/*"
            },
            REQUEST_IMPORT_AUDIO,
        )
    }

    private fun importAndTranscribe(uri: Uri) {
        if (importInFlight) return
        importInFlight = true
        importStatus.text = getString(R.string.saving_audio)
        audioExecutor.execute {
            val imported = runCatching {
                val name = displayName(uri)
                val mimeType = contentResolver.getType(uri) ?: mimeTypeForName(name)
                val extension = name.substringAfterLast('.', "audio")
                val input = contentResolver.openInputStream(uri)
                    ?: error("Android could not open the selected audio file")
                recordingStore.import(name, mimeType, extension, input)
            }
            // Handed over even if this screen has gone meanwhile: the screen
            // Android recreates shows the result.
            imported.onSuccess { transcriptions.submit(it, TranscriptionOrigin.BULK, showResultInApp = true) }
            runOnUiThread {
                if (isFinishing || isDestroyed) return@runOnUiThread
                importInFlight = false
                refreshRecordings()
                imported.onFailure { error -> showAudioFailure(error, null) }
            }
        }
    }

    private fun refreshRecordings() {
        if (!::recordingsContainer.isInitialized) return
        recordingsContainer.removeAllViews()
        val recordings = recordingStore.list()
        importStatus.text = when {
            importInFlight -> getString(R.string.saving_audio)
            else -> recordings
                .firstOrNull { transcriptions.isRunning(it.id) }
                ?.let { getString(R.string.transcribing_recording, it.name) }
                .orEmpty()
        }
        if (recordings.isEmpty()) {
            recordingsContainer.addView(
                label(getString(R.string.no_saved_recordings), paddingTopDp = 5, paddingBottomDp = 5),
            )
            return
        }
        recordings.take(10).forEach { recording ->
            val openButton = button(
                getString(
                    R.string.saved_recording_format,
                    recording.name,
                    recording.status,
                    formatBytes(recording.bytes),
                ),
            ) { openRawRecording(recording) }.apply {
                gravity = Gravity.START or Gravity.CENTER_VERTICAL
                contentDescription = getString(R.string.open_raw_recording, recording.name)
            }
            val transcribeButton = button(getString(R.string.quick_transcribe)) {
                retranscribeRecording(recording)
            }.apply {
                contentDescription = getString(R.string.retranscribe_recording, recording.name)
                isEnabled = !transcriptions.isRunning(recording.id)
            }
            recordingsContainer.addView(LinearLayout(this).apply {
                orientation = LinearLayout.HORIZONTAL
                gravity = Gravity.CENTER_VERTICAL
                addView(
                    openButton,
                    LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f),
                )
                addView(transcribeButton)
            })
        }
    }

    private fun retranscribeRecording(recording: StoredRecording) {
        // A recording already being transcribed (by the overlay, say) is not
        // sent again; its result shows here when it ends.
        transcriptions.submit(recording, TranscriptionOrigin.BULK, showResultInApp = true)
    }

    private fun showImportedTranscript(transcript: String, recording: StoredRecording?) {
        AlertDialog.Builder(this)
            .setTitle(R.string.transcription_complete)
            .setMessage(transcript)
            .setPositiveButton(R.string.copy_transcript) { _, _ ->
                getSystemService(ClipboardManager::class.java)
                    .setPrimaryClip(ClipData.newPlainText(getString(R.string.clipboard_transcript_label), transcript))
            }
            .setNeutralButton(R.string.view_raw_audio) { _, _ ->
                recording?.let(::openRawRecording)
            }
            .setNegativeButton(android.R.string.cancel, null)
            .show()
    }

    private fun showAudioFailure(error: Throwable, recording: StoredRecording?) {
        val detail = error.message?.takeIf { it.isNotBlank() } ?: "Transcription failed"
        AlertDialog.Builder(this)
            .setTitle(R.string.transcription_failed)
            .setMessage(
                if (recording == null) detail
                else getString(R.string.transcription_failed_saved, detail),
            )
            .setPositiveButton(android.R.string.ok, null)
            .setNeutralButton(R.string.view_raw_audio) { _, _ ->
                recording?.let(::openRawRecording)
            }
            .show()
    }

    private fun openRawRecording(recording: StoredRecording) {
        val uri = FileProvider.getUriForFile(
            this,
            "$packageName.updates",
            recordingStore.file(recording),
        )
        val intent = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, recording.mimeType)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        runCatching { startActivity(intent) }.onFailure {
            startActivity(
                Intent.createChooser(
                    Intent(Intent.ACTION_SEND).apply {
                        type = recording.mimeType
                        putExtra(Intent.EXTRA_STREAM, uri)
                        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    },
                    getString(R.string.view_raw_audio),
                ),
            )
        }
    }

    private fun displayName(uri: Uri): String {
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
            if (it.moveToFirst()) return it.getString(0)
        }
        return uri.lastPathSegment ?: "Imported audio"
    }

    private fun mimeTypeForName(name: String): String = when {
        name.endsWith(".wav", true) -> "audio/wav"
        name.endsWith(".mp3", true) -> "audio/mpeg"
        name.endsWith(".m4a", true) -> "audio/x-m4a"
        name.endsWith(".mp4", true) -> "audio/mp4"
        name.endsWith(".ogg", true) -> "audio/ogg"
        else -> "audio/webm"
    }

    private fun formatBytes(bytes: Long): String =
        if (bytes < 1024 * 1024) "${(bytes / 1024).coerceAtLeast(1)} KB"
        else String.format("%.1f MB", bytes / (1024.0 * 1024.0))

    private fun statusRow(text: String, paddingTopDp: Int = 0) =
        label(text, HEADING_TEXT, 17f, paddingTopDp, paddingBottomDp = 8)

    /** A text line in [color]; with no [sizeSp] it keeps the platform's default size. */
    private fun label(
        text: CharSequence,
        color: Int = SECONDARY_TEXT,
        sizeSp: Float? = null,
        paddingTopDp: Int = 0,
        paddingBottomDp: Int = 0,
    ) = TextView(this).apply {
        this.text = text
        sizeSp?.let { textSize = it }
        setTextColor(color)
        setPadding(0, dp(paddingTopDp), 0, dp(paddingBottomDp))
    }

    /**
     * A button showing [text] as written: the theme's default would capitalise
     * it. Set here, not in the theme, so dialog buttons keep their capitals.
     */
    private fun button(text: CharSequence, onClick: () -> Unit) = Button(this).apply {
        this.text = text
        isAllCaps = false
        setOnClickListener { onClick() }
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

    private fun checkForUpdates(showUpToDate: Boolean, showFailure: Boolean = showUpToDate) {
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
                        if (showFailure) showMessage(
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

    private fun canInstallPackages(): Boolean = packageManager.canRequestPackageInstalls()

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

    private fun showDictationRecoveryFrom(source: Intent) {
        val error = source.getStringExtra(EXTRA_DICTATION_ERROR) ?: return
        source.removeExtra(EXTRA_DICTATION_ERROR)
        window.decorView.post {
            if (isFinishing || isDestroyed) return@post
            AlertDialog.Builder(this)
                .setTitle(R.string.dictation_recovery_title)
                .setMessage(getString(R.string.dictation_recovery_message, error))
                .setPositiveButton(android.R.string.ok, null)
                .show()
        }
    }

    companion object {
        const val EXTRA_DICTATION_ERROR = "com.openwhisper.android.extra.DICTATION_ERROR"
        private val TITLE_TEXT = Color.rgb(24, 24, 32)
        private val HEADING_TEXT = Color.rgb(35, 35, 45)
        private val SECONDARY_TEXT = Color.rgb(82, 82, 96)
        private const val REQUEST_AUDIO = 100
        const val REQUEST_IMPORT_AUDIO = 101
    }
}
