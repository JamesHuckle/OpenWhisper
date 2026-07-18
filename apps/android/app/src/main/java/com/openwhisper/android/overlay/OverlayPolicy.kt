package com.openwhisper.android.overlay

data class ScreenPoint(val x: Int, val y: Int)

data class ScreenRect(
    val left: Int,
    val top: Int,
    val right: Int,
    val bottom: Int,
) {
    init {
        require(right >= left) { "right must not be left of left" }
        require(bottom >= top) { "bottom must not be above top" }
    }
}

data class OverlayContext(
    val keyboardBounds: ScreenRect?,
    val hasEditableFocus: Boolean,
    val isPassword: Boolean = false,
    val isSensitive: Boolean = false,
    val displayBounds: ScreenRect = ScreenRect(0, 0, 1080, 2400),
    val controlSizePx: Int = 128,
    val marginPx: Int = 24,
)

sealed interface OverlayDecision {
    data object Hidden : OverlayDecision
    data class Show(val topLeft: ScreenPoint) : OverlayDecision
}

class OverlayPolicy {
    fun decide(context: OverlayContext): OverlayDecision {
        val keyboard = context.keyboardBounds ?: return OverlayDecision.Hidden
        if (!context.hasEditableFocus || context.isPassword || context.isSensitive) {
            return OverlayDecision.Hidden
        }

        val minX = context.displayBounds.left + context.marginPx
        val maxX = (context.displayBounds.right - context.marginPx - context.controlSizePx)
            .coerceAtLeast(minX)
        val minY = context.displayBounds.top + context.marginPx
        val maxY = (context.displayBounds.bottom - context.marginPx - context.controlSizePx)
            .coerceAtLeast(minY)
        val desiredX = keyboard.right - context.marginPx - context.controlSizePx
        val desiredY = keyboard.top - context.marginPx - context.controlSizePx

        return OverlayDecision.Show(
            ScreenPoint(
                x = desiredX.coerceIn(minX, maxX),
                y = desiredY.coerceIn(minY, maxY),
            ),
        )
    }
}
