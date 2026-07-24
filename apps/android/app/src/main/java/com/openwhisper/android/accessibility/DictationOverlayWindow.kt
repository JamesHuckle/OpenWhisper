package com.openwhisper.android.accessibility

import android.content.Context
import android.content.res.ColorStateList
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.PixelFormat
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.RippleDrawable
import android.util.Log
import android.view.Gravity
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import android.view.WindowManager
import androidx.core.content.ContextCompat
import com.openwhisper.android.R
import com.openwhisper.android.dictation.DictationState
import com.openwhisper.android.overlay.NormalizedOverlayPosition
import com.openwhisper.android.overlay.OverlayKeyGeometry
import com.openwhisper.android.overlay.OverlayMovementBounds
import com.openwhisper.android.overlay.ScreenPoint
import com.openwhisper.android.overlay.ScreenRect
import kotlin.math.hypot

class DictationOverlayWindow(
    private val context: Context,
    private val windowManager: WindowManager,
    onClick: () -> Unit,
) {
    private val density = context.resources.displayMetrics.density
    private val preferences = context.applicationContext.getSharedPreferences(PREFERENCES_NAME, 0)
    private val touchSlop = ViewConfiguration.get(context).scaledTouchSlop.toFloat()
    val controlWidthPx = dp(OverlayKeyGeometry.WIDTH_DP)
    val controlHeightPx = dp(OverlayKeyGeometry.HEIGHT_DP)
    private var attached = false
    private var movementBounds: OverlayMovementBounds? = null
    private var customPosition = loadCustomPosition()
    private var touchDown = false
    private var dragActive = false
    private var gestureCancelled = false
    private var downRawX = 0f
    private var downRawY = 0f
    private var dragStartX = 0
    private var dragStartY = 0
    private val activateDrag = Runnable {
        if (touchDown && !gestureCancelled && attached) {
            dragActive = true
            control.performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
        }
    }
    private val control = DictationControlView(context).apply {
        id = R.id.overlay_microphone
        elevation = dp(2).toFloat()
        contentDescription = context.getString(R.string.start_dictation)
        tooltipText = context.getString(R.string.move_microphone_hint)
        setOnClickListener { onClick() }
        setOnTouchListener { _, event -> handleTouch(event) }
    }
    private val layout = WindowManager.LayoutParams(
        controlWidthPx,
        controlHeightPx,
        WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
        WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
            WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL or
            WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN or
            WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
        PixelFormat.TRANSLUCENT,
    ).apply {
        gravity = Gravity.TOP or Gravity.START
    }

    init {
        render(DictationState.Idle)
    }

    fun show(position: ScreenPoint, displayBounds: ScreenRect): Boolean {
        val bounds = OverlayMovementBounds.within(
            displayBounds = displayBounds,
            controlWidthPx = controlWidthPx,
            controlHeightPx = controlHeightPx,
            marginPx = dp(OverlayKeyGeometry.GUTTER_MARGIN_DP).coerceAtLeast(1),
        )
        movementBounds = bounds
        val resolvedPosition = customPosition?.resolve(bounds) ?: bounds.clamp(position)
        layout.x = resolvedPosition.x
        layout.y = resolvedPosition.y
        return runCatching {
            if (attached) {
                windowManager.updateViewLayout(control, layout)
            } else {
                windowManager.addView(control, layout)
                attached = true
            }
            true
        }.getOrElse { error ->
            Log.e("OpenWhisperOverlay", "Unable to show accessibility overlay", error)
            attached = false
            false
        }
    }

    fun hide() {
        if (!attached) return
        cancelTouch()
        runCatching { windowManager.removeView(control) }
        attached = false
    }

    fun render(state: DictationState) {
        when (state) {
            DictationState.Idle -> styleIcon(
                R.drawable.ic_microphone,
                context.getString(R.string.start_dictation),
                palette(
                    lightBackground = Color.rgb(247, 247, 251),
                    lightIcon = Color.rgb(78, 79, 207),
                    darkBackground = Color.rgb(58, 58, 66),
                    darkIcon = Color.rgb(195, 196, 255),
                ),
            )
            is DictationState.Listening -> styleMeter(
                DictationControlView.Mode.LISTENING,
                context.getString(R.string.stop_dictation),
                palette(
                    lightBackground = Color.rgb(251, 234, 236),
                    lightIcon = Color.rgb(199, 53, 77),
                    darkBackground = Color.rgb(91, 48, 56),
                    darkIcon = Color.rgb(255, 179, 191),
                ),
            )
            is DictationState.Finalizing,
            is DictationState.ReadyToInsert,
            -> styleMeter(
                DictationControlView.Mode.PROCESSING,
                context.getString(R.string.transcribing),
                palette(
                    lightBackground = Color.rgb(252, 240, 225),
                    lightIcon = Color.rgb(183, 109, 23),
                    darkBackground = Color.rgb(90, 65, 44),
                    darkIcon = Color.rgb(255, 208, 152),
                ),
            )
            is DictationState.Failed -> styleIcon(
                R.drawable.ic_error,
                context.getString(R.string.dictation_error, state.message),
                palette(
                    lightBackground = Color.rgb(238, 240, 244),
                    lightIcon = Color.rgb(91, 92, 110),
                    darkBackground = Color.rgb(58, 58, 66),
                    darkIcon = Color.rgb(209, 209, 218),
                ),
            )
        }
    }

    /** Push a live microphone level (0..1) into the equalizer while listening. */
    fun onAmplitude(level: Float) {
        control.onAmplitude(level)
    }

    fun destroy() = hide()

    private fun styleIcon(icon: Int, description: String, colors: ControlColors) {
        prepareControl(description, colors)
        control.showIcon(ContextCompat.getDrawable(context, icon)?.mutate(), colors.icon)
    }

    private fun styleMeter(mode: DictationControlView.Mode, description: String, colors: ControlColors) {
        prepareControl(description, colors)
        when (mode) {
            DictationControlView.Mode.LISTENING -> control.showListening(colors.icon)
            DictationControlView.Mode.PROCESSING -> control.showProcessing(colors.icon)
            DictationControlView.Mode.ICON -> Unit
        }
    }

    private fun prepareControl(description: String, colors: ControlColors) {
        control.contentDescription = context.getString(R.string.movable_microphone_description, description)
        control.background = roundedKey(colors)
        control.visibility = View.VISIBLE
    }

    private fun roundedKey(colors: ControlColors): RippleDrawable {
        val shape = GradientDrawable().apply {
            shape = GradientDrawable.RECTANGLE
            cornerRadius = dp(OverlayKeyGeometry.CORNER_RADIUS_DP).toFloat()
            setColor(colors.background)
            setStroke(dp(1).coerceAtLeast(1), colors.border)
        }
        return RippleDrawable(
            ColorStateList.valueOf(withAlpha(colors.icon, 0x2E)),
            shape,
            null,
        )
    }

    private fun palette(
        lightBackground: Int,
        lightIcon: Int,
        darkBackground: Int,
        darkIcon: Int,
    ): ControlColors {
        val darkMode = context.resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK ==
            Configuration.UI_MODE_NIGHT_YES
        return if (darkMode) {
            ControlColors(darkBackground, darkIcon, Color.rgb(94, 94, 102))
        } else {
            ControlColors(lightBackground, lightIcon, Color.rgb(216, 217, 226))
        }
    }

    private fun withAlpha(color: Int, alpha: Int): Int =
        Color.argb(alpha, Color.red(color), Color.green(color), Color.blue(color))

    private fun handleTouch(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                touchDown = true
                dragActive = false
                gestureCancelled = false
                downRawX = event.rawX
                downRawY = event.rawY
                dragStartX = layout.x
                dragStartY = layout.y
                control.isPressed = true
                control.postDelayed(activateDrag, ViewConfiguration.getLongPressTimeout().toLong())
            }
            MotionEvent.ACTION_MOVE -> {
                val distance = hypot(event.rawX - downRawX, event.rawY - downRawY)
                if (!dragActive && distance > touchSlop) {
                    gestureCancelled = true
                    control.removeCallbacks(activateDrag)
                    control.isPressed = false
                }
                if (dragActive) {
                    moveTo(
                        ScreenPoint(
                            x = dragStartX + (event.rawX - downRawX).toInt(),
                            y = dragStartY + (event.rawY - downRawY).toInt(),
                        ),
                    )
                }
            }
            MotionEvent.ACTION_UP -> {
                control.removeCallbacks(activateDrag)
                if (dragActive) {
                    persistCustomPosition()
                } else if (!gestureCancelled) {
                    control.performClick()
                }
                resetTouchState()
            }
            MotionEvent.ACTION_CANCEL -> {
                control.removeCallbacks(activateDrag)
                if (dragActive) persistCustomPosition()
                resetTouchState()
            }
        }
        return true
    }

    private fun moveTo(position: ScreenPoint) {
        val bounds = movementBounds ?: return
        val clamped = bounds.clamp(position)
        layout.x = clamped.x
        layout.y = clamped.y
        customPosition = NormalizedOverlayPosition.from(clamped, bounds)
        if (attached) {
            runCatching { windowManager.updateViewLayout(control, layout) }
                .onFailure { Log.e("OpenWhisperOverlay", "Unable to move accessibility overlay", it) }
        }
    }

    private fun persistCustomPosition() {
        val position = customPosition ?: return
        preferences.edit()
            .putFloat(PREFERENCE_X, position.xFraction)
            .putFloat(PREFERENCE_Y, position.yFraction)
            .apply()
    }

    private fun loadCustomPosition(): NormalizedOverlayPosition? {
        if (!preferences.contains(PREFERENCE_X) || !preferences.contains(PREFERENCE_Y)) return null
        return NormalizedOverlayPosition(
            xFraction = preferences.getFloat(PREFERENCE_X, 0f),
            yFraction = preferences.getFloat(PREFERENCE_Y, 0f),
        )
    }

    private fun cancelTouch() {
        control.removeCallbacks(activateDrag)
        resetTouchState()
    }

    private fun resetTouchState() {
        touchDown = false
        dragActive = false
        gestureCancelled = false
        control.isPressed = false
    }

    private fun dp(value: Int): Int = (value * density).toInt()

    private data class ControlColors(val background: Int, val icon: Int, val border: Int)

    private companion object {
        const val PREFERENCES_NAME = "openwhisper_overlay_position"
        const val PREFERENCE_X = "x_fraction"
        const val PREFERENCE_Y = "y_fraction"
    }
}
