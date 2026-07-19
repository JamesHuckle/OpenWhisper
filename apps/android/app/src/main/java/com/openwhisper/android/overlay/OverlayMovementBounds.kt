package com.openwhisper.android.overlay

import kotlin.math.roundToInt

data class OverlayMovementBounds(
    val minX: Int,
    val maxX: Int,
    val minY: Int,
    val maxY: Int,
) {
    init {
        require(maxX >= minX) { "maxX must not be left of minX" }
        require(maxY >= minY) { "maxY must not be above minY" }
    }

    fun clamp(position: ScreenPoint) = ScreenPoint(
        x = position.x.coerceIn(minX, maxX),
        y = position.y.coerceIn(minY, maxY),
    )

    companion object {
        fun within(
            displayBounds: ScreenRect,
            controlWidthPx: Int,
            controlHeightPx: Int,
            marginPx: Int,
        ): OverlayMovementBounds {
            val minX = displayBounds.left + marginPx
            val minY = displayBounds.top + marginPx
            return OverlayMovementBounds(
                minX = minX,
                maxX = (displayBounds.right - marginPx - controlWidthPx).coerceAtLeast(minX),
                minY = minY,
                maxY = (displayBounds.bottom - marginPx - controlHeightPx).coerceAtLeast(minY),
            )
        }
    }
}

data class NormalizedOverlayPosition(
    val xFraction: Float,
    val yFraction: Float,
) {
    fun resolve(bounds: OverlayMovementBounds) = ScreenPoint(
        x = resolveAxis(xFraction, bounds.minX, bounds.maxX),
        y = resolveAxis(yFraction, bounds.minY, bounds.maxY),
    )

    companion object {
        fun from(position: ScreenPoint, bounds: OverlayMovementBounds): NormalizedOverlayPosition {
            val clamped = bounds.clamp(position)
            return NormalizedOverlayPosition(
                xFraction = normalizeAxis(clamped.x, bounds.minX, bounds.maxX),
                yFraction = normalizeAxis(clamped.y, bounds.minY, bounds.maxY),
            )
        }

        private fun normalizeAxis(value: Int, minimum: Int, maximum: Int): Float =
            if (maximum == minimum) 0f else (value - minimum).toFloat() / (maximum - minimum)

        private fun resolveAxis(fraction: Float, minimum: Int, maximum: Int): Int =
            (minimum + fraction.coerceIn(0f, 1f) * (maximum - minimum)).roundToInt()
    }
}
