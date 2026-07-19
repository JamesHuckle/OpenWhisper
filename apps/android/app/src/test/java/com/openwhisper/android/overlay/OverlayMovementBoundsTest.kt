package com.openwhisper.android.overlay

import org.junit.Assert.assertEquals
import org.junit.Test

class OverlayMovementBoundsTest {
    private val bounds = OverlayMovementBounds.within(
        displayBounds = ScreenRect(0, 0, 1080, 2400),
        controlWidthPx = 64,
        controlHeightPx = 128,
        marginPx = 24,
    )

    @Test
    fun clampsControlInsideTheVisibleDisplay() {
        assertEquals(ScreenPoint(24, 24), bounds.clamp(ScreenPoint(-100, -100)))
        assertEquals(ScreenPoint(992, 2248), bounds.clamp(ScreenPoint(2_000, 3_000)))
    }

    @Test
    fun normalizedPositionSurvivesDisplaySizeChanges() {
        val position = NormalizedOverlayPosition.from(ScreenPoint(508, 1136), bounds)
        val landscapeBounds = OverlayMovementBounds.within(
            displayBounds = ScreenRect(0, 0, 2400, 1080),
            controlWidthPx = 64,
            controlHeightPx = 128,
            marginPx = 24,
        )

        assertEquals(ScreenPoint(1168, 476), position.resolve(landscapeBounds))
    }

    @Test
    fun handlesDisplayThatIsSmallerThanTheControlAndMargins() {
        val tinyBounds = OverlayMovementBounds.within(
            displayBounds = ScreenRect(100, 200, 120, 220),
            controlWidthPx = 64,
            controlHeightPx = 128,
            marginPx = 24,
        )

        assertEquals(ScreenPoint(124, 224), tinyBounds.clamp(ScreenPoint(1_000, 1_000)))
    }
}
