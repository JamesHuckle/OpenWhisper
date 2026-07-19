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
    val controlWidthPx: Int = 64,
    val controlHeightPx: Int = 128,
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
        val maxX = (context.displayBounds.right - context.marginPx - context.controlWidthPx)
            .coerceAtLeast(minX)
        val minY = context.displayBounds.top + context.marginPx
        val maxY = (context.displayBounds.bottom - context.marginPx - context.controlHeightPx)
            .coerceAtLeast(minY)
        // Android IMEs do not expose individual key bounds. On conventional phone
        // layouts the A/home row is centered about 40% down the IME window, and the
        // staggered row leaves a small gutter immediately to its left.
        val desiredX = keyboard.left + context.marginPx
        val homeRowCenterY = keyboard.top + (keyboard.bottom - keyboard.top) * HOME_ROW_FRACTION
        val desiredY = (homeRowCenterY - context.controlHeightPx / 2f).toInt()

        return OverlayDecision.Show(
            ScreenPoint(
                x = desiredX.coerceIn(minX, maxX),
                y = desiredY.coerceIn(minY, maxY),
            ),
        )
    }

    private companion object {
        const val HOME_ROW_FRACTION = 0.4f
    }
}
