package com.openwhisper.android.overlay

import org.junit.Assert.assertEquals
import org.junit.Assert.fail
import org.junit.Test

class OverlayPolicyTest {
    private val policy = OverlayPolicy()

    @Test
    fun hidesWhenKeyboardIsNotVisible() {
        assertEquals(
            OverlayDecision.Hidden,
            policy.decide(OverlayContext(keyboardBounds = null)) { FocusedField() },
        )
    }

    @Test
    fun hidesWithoutAnEditableField() {
        assertEquals(
            OverlayDecision.Hidden,
            policy.decide(OverlayContext(keyboardBounds = ScreenRect(0, 1400, 1080, 2400))) { null },
        )
    }

    @Test
    fun hidesForPasswordsAndSensitiveEditors() {
        val keyboard = OverlayContext(keyboardBounds = ScreenRect(0, 1400, 1080, 2400))
        assertEquals(
            OverlayDecision.Hidden,
            policy.decide(keyboard) { FocusedField(isPassword = true) },
        )
        assertEquals(
            OverlayDecision.Hidden,
            policy.decide(keyboard) { FocusedField(isSensitive = true) },
        )
    }

    @Test
    fun positionsMicInLeftGutterBesideHomeRow() {
        val decision = policy.decide(
            OverlayContext(
                keyboardBounds = ScreenRect(0, 1400, 1080, 2400),
                displayBounds = ScreenRect(0, 0, 1080, 2400),
                controlWidthPx = 64,
                controlHeightPx = 128,
                marginPx = 24,
            ),
        ) { FocusedField() }

        assertEquals(OverlayDecision.Show(ScreenPoint(24, 1736)), decision)
    }

    @Test
    fun clampsMicOnNarrowOrOffsetDisplays() {
        val decision = policy.decide(
            OverlayContext(
                keyboardBounds = ScreenRect(-500, 40, -300, 500),
                displayBounds = ScreenRect(-500, 0, -300, 500),
                controlWidthPx = 64,
                controlHeightPx = 128,
                marginPx = 24,
            ),
        ) { FocusedField() }

        assertEquals(OverlayDecision.Show(ScreenPoint(-476, 160)), decision)
    }

    @Test
    fun pinsMicToTheTopLeftMarginWhenTheDisplayIsSmallerThanTheMic() {
        val decision = policy.decide(
            OverlayContext(
                keyboardBounds = ScreenRect(10, 30, 90, 100),
                displayBounds = ScreenRect(10, 20, 90, 100),
                controlWidthPx = 64,
                controlHeightPx = 128,
                marginPx = 24,
            ),
        ) { FocusedField() }

        assertEquals(OverlayDecision.Show(ScreenPoint(34, 44)), decision)
    }

    @Test
    fun floatsWhileDictatingWithTheKeyboardClosed() {
        assertEquals(
            OverlayDecision.Float,
            policy.decide(
                OverlayContext(keyboardBounds = null, dictationActive = true),
            ) { null },
        )
    }

    @Test
    fun floatsWhileDictatingOverAPasswordKeyboard() {
        assertEquals(
            OverlayDecision.Float,
            policy.decide(
                OverlayContext(
                    keyboardBounds = ScreenRect(0, 1400, 1080, 2400),
                    dictationActive = true,
                ),
            ) { FocusedField(isPassword = true) },
        )
    }

    @Test
    fun docksBesideTheKeyboardWhenItReopensDuringDictation() {
        val decision = policy.decide(
            OverlayContext(
                keyboardBounds = ScreenRect(0, 1400, 1080, 2400),
                controlWidthPx = 64,
                controlHeightPx = 128,
                marginPx = 24,
                dictationActive = true,
            ),
        ) { FocusedField() }

        assertEquals(OverlayDecision.Show(ScreenPoint(24, 1736)), decision)
    }

    @Test
    fun sharesItsMovementBoundsWithTheOverlayWindow() {
        val context = OverlayContext(
            keyboardBounds = null,
            displayBounds = ScreenRect(-500, 0, -300, 500),
            controlWidthPx = 64,
            controlHeightPx = 128,
            marginPx = 24,
        )

        assertEquals(
            OverlayMovementBounds(minX = -476, maxX = -388, minY = 24, maxY = 348),
            context.movementBounds,
        )
    }

    @Test
    fun skipsTheFocusedFieldLookupWhileTheKeyboardIsClosed() {
        val lookup: () -> FocusedField? = { fail("focused field looked up without a keyboard"); null }

        assertEquals(
            OverlayDecision.Hidden,
            policy.decide(OverlayContext(keyboardBounds = null), lookup),
        )
        assertEquals(
            OverlayDecision.Float,
            policy.decide(OverlayContext(keyboardBounds = null, dictationActive = true), lookup),
        )
    }

    @Test
    fun usesTheFocusedFieldOnceWhileTheKeyboardIsOpen() {
        val keyboard = OverlayContext(
            keyboardBounds = ScreenRect(0, 1400, 1080, 2400),
            controlWidthPx = 64,
            controlHeightPx = 128,
            marginPx = 24,
        )
        var lookups = 0

        assertEquals(
            OverlayDecision.Show(ScreenPoint(24, 1736)),
            policy.decide(keyboard) { lookups += 1; FocusedField() },
        )
        assertEquals(1, lookups)
        assertEquals(OverlayDecision.Hidden, policy.decide(keyboard) { null })
        assertEquals(
            OverlayDecision.Hidden,
            policy.decide(keyboard) { FocusedField(isPassword = true) },
        )
        assertEquals(
            OverlayDecision.Hidden,
            policy.decide(keyboard) { FocusedField(isSensitive = true) },
        )
    }
}
