package com.openwhisper.android.accessibility

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.drawable.Drawable
import android.os.SystemClock
import android.view.View
import com.openwhisper.android.overlay.OverlayKeyGeometry
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sin

/**
 * The draggable overlay control. Renders three ways:
 *
 *  - [Mode.ICON] — a static glyph (microphone / error), same as the previous button.
 *  - [Mode.LISTENING] — a live equalizer whose bars react to the microphone level
 *    fed through [onAmplitude], so the user can see we are listening.
 *  - [Mode.PROCESSING] — an indeterminate loading wave that travels across the bars
 *    while the transcript is being prepared.
 *
 * Everything is drawn inside the same small key footprint; nothing overflows it.
 */
class DictationControlView(context: Context) : View(context) {

    enum class Mode { ICON, LISTENING, PROCESSING }

    private val density = resources.displayMetrics.density
    private val barPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.FILL }
    private val barRect = RectF()

    private val bars = FloatArray(BAR_COUNT) { REST }
    private val targets = FloatArray(BAR_COUNT) { REST }

    private var mode = Mode.ICON
    private var icon: Drawable? = null
    private var contentColor = 0
    private var amplitude = 0f
    private var phase = 0f
    private var animating = false

    private val frame = object : Runnable {
        override fun run() {
            if (!animating) return
            step()
            invalidate()
            postOnAnimation(this)
        }
    }

    /** Show a static glyph (idle microphone or error). */
    fun showIcon(drawable: Drawable?, color: Int) {
        stopAnimation()
        mode = Mode.ICON
        icon = drawable
        contentColor = color
        invalidate()
    }

    /** Show the live, mic-reactive equalizer. */
    fun showListening(color: Int) {
        contentColor = color
        if (mode != Mode.LISTENING) {
            mode = Mode.LISTENING
            resetBars()
        }
        startAnimation()
    }

    /** Show the indeterminate processing wave. */
    fun showProcessing(color: Int) {
        contentColor = color
        if (mode != Mode.PROCESSING) {
            mode = Mode.PROCESSING
            resetBars()
        }
        startAnimation()
    }

    /** Feed a normalized microphone level (0..1); called from the audio pipeline. */
    fun onAmplitude(level: Float) {
        amplitude = level.coerceIn(0f, 1f)
    }

    private fun resetBars() {
        for (i in bars.indices) {
            bars[i] = REST
            targets[i] = REST
        }
        amplitude = 0f
        phase = 0f
    }

    private fun startAnimation() {
        if (animating) return
        animating = true
        removeCallbacks(frame)
        postOnAnimation(frame)
    }

    private fun stopAnimation() {
        animating = false
        removeCallbacks(frame)
    }

    override fun onDetachedFromWindow() {
        super.onDetachedFromWindow()
        stopAnimation()
    }

    private fun step() {
        when (mode) {
            Mode.LISTENING -> {
                // A single amplitude value drives every bar; a slow per-bar wobble
                // spreads it out so it reads like a real multi-band equalizer.
                phase += LISTEN_SPEED
                val energy = min(1f, amplitude * GAIN)
                for (i in bars.indices) {
                    val wobble = 0.5f + 0.5f * sin(phase + i * 1.9f)
                    targets[i] = (REST + energy * (0.4f + 0.6f * wobble)).coerceIn(REST, 1f)
                }
            }
            Mode.PROCESSING -> {
                // A smooth wave travels across the bars, independent of the mic.
                phase += LOADING_SPEED
                for (i in bars.indices) {
                    val wave = 0.5f + 0.5f * sin(phase - i * 0.95f)
                    targets[i] = 0.22f + 0.68f * wave
                }
            }
            Mode.ICON -> return
        }
        for (i in bars.indices) {
            val target = targets[i]
            // Fast attack, gentler release keeps it lively without jitter.
            val ease = if (target > bars[i]) ATTACK else RELEASE
            bars[i] += (target - bars[i]) * ease
        }
    }

    override fun onDraw(canvas: Canvas) {
        when (mode) {
            Mode.ICON -> drawIcon(canvas)
            Mode.LISTENING, Mode.PROCESSING -> drawBars(canvas)
        }
    }

    private fun drawIcon(canvas: Canvas) {
        val drawable = icon ?: return
        val size = dp(OverlayKeyGeometry.ICON_SIZE_DP)
        val left = (width - size) / 2
        val top = (height - size) / 2
        drawable.setBounds(left, top, left + size, top + size)
        drawable.setTint(contentColor)
        drawable.draw(canvas)
    }

    private fun drawBars(canvas: Canvas) {
        barPaint.color = contentColor
        val barWidth = dp(BAR_WIDTH_DP).toFloat()
        val gap = dp(BAR_GAP_DP).toFloat()
        val totalWidth = BAR_COUNT * barWidth + (BAR_COUNT - 1) * gap
        val startX = (width - totalWidth) / 2f
        val centerY = height / 2f
        val maxHeight = height * MAX_BAR_FRACTION
        val radius = barWidth / 2f
        for (i in 0 until BAR_COUNT) {
            val barHeight = max(barWidth, bars[i] * maxHeight)
            val left = startX + i * (barWidth + gap)
            val top = centerY - barHeight / 2f
            barRect.set(left, top, left + barWidth, top + barHeight)
            canvas.drawRoundRect(barRect, radius, radius, barPaint)
        }
    }

    private fun dp(value: Int): Int = (value * density).toInt()

    private companion object {
        const val BAR_COUNT = 4
        const val BAR_WIDTH_DP = 3
        const val BAR_GAP_DP = 3
        const val MAX_BAR_FRACTION = 0.52f
        const val REST = 0.12f
        const val GAIN = 6f
        const val ATTACK = 0.5f
        const val RELEASE = 0.16f
        const val LISTEN_SPEED = 0.28f
        const val LOADING_SPEED = 0.22f
    }
}
