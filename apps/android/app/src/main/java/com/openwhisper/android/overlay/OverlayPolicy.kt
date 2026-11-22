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
    val displayBounds: ScreenRect = ScreenRect(0, 0, 1080, 2400),
    val controlWidthPx: Int = 64,
    val controlHeightPx: Int = 128,
    val marginPx: Int = 24,
    /** Dictation is recording or transcribing, so the control must stay reachable. */
    val dictationActive: Boolean = false,
) {
    /** Where the control may sit; the overlay window drags and restores it within these too. */
    val movementBounds =
        OverlayMovementBounds.within(displayBounds, controlWidthPx, controlHeightPx, marginPx)
}

/** What the overlay needs to know about the field that has input focus. */
data class FocusedField(
    val isPassword: Boolean = false,
    val isSensitive: Boolean = false,
)

sealed interface OverlayDecision {
    data object Hidden : OverlayDecision
    data class Show(val topLeft: ScreenPoint) : OverlayDecision

    /** No safe keyboard to dock beside, but dictation is running: keep floating where it is. */
    data object Float : OverlayDecision
}

class OverlayPolicy {
    /**
     * Decides from [context] and the field that has focus, looking for the field only
     * while a keyboard is open. Without a keyboard the control is hidden, or floats
     * during dictation, whatever has focus; finding the field can walk every window's
     * accessibility tree, so [focusedField] is not called then. A null field means no
     * editable field has focus.
     */
    fun decide(context: OverlayContext, focusedField: () -> FocusedField?): OverlayDecision {
        val keyboard = context.keyboardBounds
        val field = keyboard?.let { focusedField() }
        if (keyboard == null || field == null || field.isPassword || field.isSensitive) {
            return if (context.dictationActive) OverlayDecision.Float else OverlayDecision.Hidden
        }

        // Android IMEs do not expose individual key bounds. On conventional phone
        // layouts the A/home row is centered about 40% down the IME window, and the
        // staggered row leaves a small gutter immediately to its left.
        val desiredX = keyboard.left + context.marginPx
        val homeRowCenterY = keyboard.top + (keyboard.bottom - keyboard.top) * HOME_ROW_FRACTION
        val desiredY = (homeRowCenterY - context.controlHeightPx / 2f).toInt()

        return OverlayDecision.Show(context.movementBounds.clamp(ScreenPoint(desiredX, desiredY)))
    }

    private companion object {
        const val HOME_ROW_FRACTION = 0.4f
    }
}
