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
import android.view.View
import android.view.WindowManager
import android.widget.ImageButton
import com.openwhisper.android.R
import com.openwhisper.android.dictation.DictationState
import com.openwhisper.android.overlay.OverlayKeyGeometry
import com.openwhisper.android.overlay.ScreenPoint

class DictationOverlayWindow(
    private val context: Context,
    private val windowManager: WindowManager,
    onClick: () -> Unit,
) {
    private val density = context.resources.displayMetrics.density
    val controlWidthPx = dp(OverlayKeyGeometry.WIDTH_DP)
    val controlHeightPx = dp(OverlayKeyGeometry.HEIGHT_DP)
    private var attached = false
    private val control = ImageButton(context).apply {
        id = R.id.overlay_microphone
        setImageResource(R.drawable.ic_microphone)
        scaleType = android.widget.ImageView.ScaleType.CENTER_INSIDE
        val horizontalPadding = (controlWidthPx - dp(OverlayKeyGeometry.ICON_SIZE_DP)) / 2
        val verticalPadding = (controlHeightPx - dp(OverlayKeyGeometry.ICON_SIZE_DP)) / 2
        setPadding(horizontalPadding, verticalPadding, horizontalPadding, verticalPadding)
        elevation = dp(2).toFloat()
        contentDescription = context.getString(R.string.start_dictation)
        setOnClickListener { onClick() }
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

    fun show(position: ScreenPoint): Boolean {
        layout.x = position.x
        layout.y = position.y
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
        runCatching { windowManager.removeView(control) }
        attached = false
    }

    fun render(state: DictationState) {
        when (state) {
            DictationState.Idle -> style(
                R.drawable.ic_microphone,
                context.getString(R.string.start_dictation),
                palette(
                    lightBackground = Color.rgb(247, 247, 251),
                    lightIcon = Color.rgb(78, 79, 207),
                    darkBackground = Color.rgb(58, 58, 66),
                    darkIcon = Color.rgb(195, 196, 255),
                ),
            )
            is DictationState.Listening -> style(
                R.drawable.ic_stop,
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
            -> style(
                R.drawable.ic_transcribing,
                context.getString(R.string.transcribing),
                palette(
                    lightBackground = Color.rgb(252, 240, 225),
                    lightIcon = Color.rgb(183, 109, 23),
                    darkBackground = Color.rgb(90, 65, 44),
                    darkIcon = Color.rgb(255, 208, 152),
                ),
            )
            is DictationState.Failed -> style(
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

    fun destroy() = hide()

    private fun style(icon: Int, description: String, colors: ControlColors) {
        control.setImageResource(icon)
        control.imageTintList = ColorStateList.valueOf(colors.icon)
        control.contentDescription = description
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

    private fun dp(value: Int): Int = (value * density).toInt()

    private data class ControlColors(val background: Int, val icon: Int, val border: Int)
}
