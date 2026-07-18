package com.openwhisper.android.accessibility

import android.content.Context
import android.content.res.ColorStateList
import android.graphics.Color
import android.graphics.PixelFormat
import android.graphics.drawable.GradientDrawable
import android.util.Log
import android.view.Gravity
import android.view.View
import android.view.WindowManager
import android.widget.ImageButton
import com.openwhisper.android.R
import com.openwhisper.android.dictation.DictationState
import com.openwhisper.android.overlay.ScreenPoint

class DictationOverlayWindow(
    private val context: Context,
    private val windowManager: WindowManager,
    onClick: () -> Unit,
) {
    private val density = context.resources.displayMetrics.density
    val controlSizePx = (64 * density).toInt()
    private var attached = false
    private val control = ImageButton(context).apply {
        id = R.id.overlay_microphone
        setImageResource(R.drawable.ic_microphone)
        imageTintList = ColorStateList.valueOf(Color.WHITE)
        scaleType = android.widget.ImageView.ScaleType.CENTER_INSIDE
        setPadding(dp(17), dp(17), dp(17), dp(17))
        elevation = (10 * resources.displayMetrics.density)
        contentDescription = context.getString(R.string.start_dictation)
        background = circle(Color.rgb(91, 92, 226))
        setOnClickListener { onClick() }
    }
    private val layout = WindowManager.LayoutParams(
        controlSizePx,
        controlSizePx,
        WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
        WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
            WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL or
            WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
        PixelFormat.TRANSLUCENT,
    ).apply {
        gravity = Gravity.TOP or Gravity.START
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
                Color.rgb(91, 92, 226),
            )
            is DictationState.Listening -> style(
                R.drawable.ic_stop,
                context.getString(R.string.stop_dictation),
                Color.rgb(218, 62, 82),
            )
            is DictationState.Finalizing,
            is DictationState.ReadyToInsert,
            -> style(
                R.drawable.ic_transcribing,
                context.getString(R.string.transcribing),
                Color.rgb(240, 152, 51),
            )
            is DictationState.Failed -> style(
                R.drawable.ic_error,
                context.getString(R.string.dictation_error, state.message),
                Color.rgb(91, 92, 110),
            )
        }
    }

    fun destroy() = hide()

    private fun style(icon: Int, description: String, color: Int) {
        control.setImageResource(icon)
        control.contentDescription = description
        control.background = circle(color)
        control.visibility = View.VISIBLE
    }

    private fun circle(color: Int) = GradientDrawable().apply {
        shape = GradientDrawable.OVAL
        setColor(color)
        setStroke((2 * density).toInt(), Color.WHITE)
    }

    private fun dp(value: Int): Int = (value * density).toInt()
}
