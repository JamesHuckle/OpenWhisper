package com.openwhisper.android.accessibility

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.InputMethod
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.Rect
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.DisplayMetrics
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
import androidx.annotation.RequiresApi
import com.openwhisper.android.MainActivity
import com.openwhisper.android.audio.AndroidAudioRecorder
import com.openwhisper.android.dictation.BufferedDictationBackend
import com.openwhisper.android.dictation.DeterministicDemoBackend
import com.openwhisper.android.dictation.DictationCoordinator
import com.openwhisper.android.dictation.DictationState
import com.openwhisper.android.dictation.ExecutorBackgroundRunner
import com.openwhisper.android.dictation.MainThreadCallbackDispatcher
import com.openwhisper.android.dictation.SelectingDictationBackend
import com.openwhisper.android.editor.EditorSessionController
import com.openwhisper.android.editor.InputConnectionEditorSessionController
import com.openwhisper.android.editor.RoutingEditorSessionPort
import com.openwhisper.android.overlay.OverlayContext
import com.openwhisper.android.overlay.OverlayDecision
import com.openwhisper.android.overlay.OverlayKeyGeometry
import com.openwhisper.android.overlay.OverlayPolicy
import com.openwhisper.android.overlay.ScreenRect
import com.openwhisper.android.settings.SecureApiKeyStore
import com.openwhisper.android.settings.SettingsRepository
import com.openwhisper.android.transcription.OpenAiHttpTranscriptionClient
import com.openwhisper.android.update.UpdateNotifier
import java.util.concurrent.Executors

class OpenWhisperAccessibilityService : AccessibilityService() {
    private val handler = Handler(Looper.getMainLooper())
    private val backgroundExecutor = Executors.newSingleThreadExecutor { task ->
        Thread(task, "openwhisper-transcription")
    }
    private lateinit var settings: SettingsRepository
    private lateinit var editorProvider: AccessibilityEditorProvider
    private var accessibilityInputMethod: OpenWhisperInputMethod? = null
    private var inputConnectionEditor: InputConnectionEditorSessionController? = null
    private lateinit var coordinator: DictationCoordinator
    private lateinit var overlay: DictationOverlayWindow
    private var refreshQueued = false

    override fun onServiceConnected() {
        super.onServiceConnected()
        settings = SettingsRepository(this)
        editorProvider = AccessibilityEditorProvider(
            focusedNode = {
                findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
                    ?: rootInActiveWindow?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
            },
            windowRoots = {
                windows
                    .asSequence()
                    .filter { it.type != AccessibilityWindowInfo.TYPE_INPUT_METHOD }
                    .mapNotNull(AccessibilityWindowInfo::getRoot)
                    .toList()
            },
        )
        val accessibilityEditor = EditorSessionController(editorProvider)
        val editorController = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            InputConnectionEditorSessionController(
                provider = {
                    if (editorProvider.focusedEditor() == null) {
                        accessibilityInputMethod?.currentEditor()
                    } else {
                        null
                    }
                },
            ).also { inputConnectionEditor = it }
                .let { RoutingEditorSessionPort(it, accessibilityEditor) }
        } else {
            accessibilityEditor
        }
        val secrets = SecureApiKeyStore(this)
        val liveBackend = BufferedDictationBackend(
            recorder = AndroidAudioRecorder(this) { level ->
                // Deliver live mic levels to the overlay's equalizer on the UI thread.
                handler.post { if (::overlay.isInitialized) overlay.onAmplitude(level) }
            },
            client = OpenAiHttpTranscriptionClient(secrets::load),
            background = ExecutorBackgroundRunner(backgroundExecutor),
            callbacks = MainThreadCallbackDispatcher(handler),
        )
        val demoBackend = DeterministicDemoBackend(handler)
        val backend = SelectingDictationBackend {
            if (settings.demoMode) demoBackend else liveBackend
        }
        overlay = DictationOverlayWindow(
            context = this,
            windowManager = getSystemService(WindowManager::class.java),
        ) {
            coordinator.onMicPressed()
        }
        coordinator = DictationCoordinator(backend, editorController, ::render)
        createNotificationChannel()
        UpdateNotifier.checkIfDue(this)
        handler.postDelayed(updateCheck, UPDATE_CHECK_INTERVAL_MS)
        scheduleRefresh(100)
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        scheduleRefresh(80)
    }

    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    override fun onCreateInputMethod(): InputMethod =
        OpenWhisperInputMethod(this) { scheduleRefresh(0) }
            .also { accessibilityInputMethod = it }

    override fun onInterrupt() {
        if (::coordinator.isInitialized) coordinator.cancel()
        if (::overlay.isInitialized) overlay.hide()
    }

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        if (::coordinator.isInitialized) coordinator.cancel()
        if (::overlay.isInitialized) overlay.destroy()
        backgroundExecutor.shutdownNow()
        stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }

    private fun scheduleRefresh(delayMs: Long) {
        if (refreshQueued || !::overlay.isInitialized) return
        refreshQueued = true
        handler.postDelayed({
            refreshQueued = false
            refreshOverlay()
        }, delayMs)
    }

    private fun refreshOverlay() {
        val editor = editorProvider.focusedEditor()
        val snapshot = editor?.snapshot()
        val inputAvailability = if (editor == null) {
            inputConnectionEditor?.currentAvailability()
        } else {
            null
        }
        val keyboard = windows
            .firstOrNull { it.type == AccessibilityWindowInfo.TYPE_INPUT_METHOD }
            ?.let { window -> Rect().also(window::getBoundsInScreen) }
            ?.takeIf { it.width() > 0 && it.height() > 0 }
        val display = displayBounds()
        val density = resources.displayMetrics.density
        val decision = OverlayPolicy().decide(
            OverlayContext(
                keyboardBounds = keyboard?.toScreenRect(),
                hasEditableFocus = editor != null || inputAvailability != null,
                isPassword = snapshot?.isPassword == true ||
                    inputAvailability?.isPassword == true,
                isSensitive = snapshot?.isSensitive == true ||
                    inputAvailability?.isSensitive == true,
                displayBounds = display.toScreenRect(),
                controlWidthPx = overlay.controlWidthPx,
                controlHeightPx = overlay.controlHeightPx,
                marginPx = (OverlayKeyGeometry.GUTTER_MARGIN_DP * density).toInt()
                    .coerceAtLeast(1),
            ),
        )
        when (decision) {
            OverlayDecision.Hidden -> {
                if (coordinator.currentState !is DictationState.Idle) coordinator.cancel()
                overlay.hide()
            }
            is OverlayDecision.Show -> {
                if (!overlay.show(decision.topLeft, display.toScreenRect())) scheduleRefresh(250)
            }
        }
    }

    private fun render(state: DictationState) {
        overlay.render(state)
        if (!settings.demoMode && state is DictationState.Listening) {
            startMicrophoneForeground()
        } else if (state is DictationState.Idle || state is DictationState.Failed) {
            stopForeground(STOP_FOREGROUND_REMOVE)
        }
        scheduleRefresh(0)
    }

    private fun startMicrophoneForeground() {
        val launch = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val notification = android.app.Notification.Builder(this, NOTIFICATION_CHANNEL)
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setContentTitle(getString(com.openwhisper.android.R.string.listening_notification_title))
            .setContentText(getString(com.openwhisper.android.R.string.listening_notification_text))
            .setContentIntent(launch)
            .setOngoing(true)
            .build()
        if (Build.VERSION.SDK_INT >= 30) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun createNotificationChannel() {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(
                NOTIFICATION_CHANNEL,
                getString(com.openwhisper.android.R.string.notification_channel_name),
                NotificationManager.IMPORTANCE_LOW,
            ),
        )
    }

    private fun Rect.toScreenRect() = ScreenRect(left, top, right, bottom)

    private val updateCheck = object : Runnable {
        override fun run() {
            UpdateNotifier.checkIfDue(this@OpenWhisperAccessibilityService)
            handler.postDelayed(this, UPDATE_CHECK_INTERVAL_MS)
        }
    }

    @Suppress("DEPRECATION")
    private fun displayBounds(): Rect {
        if (Build.VERSION.SDK_INT >= 30) {
            return getSystemService(WindowManager::class.java).maximumWindowMetrics.bounds
        }
        val metrics = DisplayMetrics()
        getSystemService(WindowManager::class.java).defaultDisplay.getRealMetrics(metrics)
        return Rect(0, 0, metrics.widthPixels, metrics.heightPixels)
    }

    private companion object {
        const val NOTIFICATION_CHANNEL = "openwhisper_dictation"
        const val NOTIFICATION_ID = 7001
        const val UPDATE_CHECK_INTERVAL_MS = 24L * 60L * 60L * 1000L
    }
}
