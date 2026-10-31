package com.openwhisper.android.overlay

import org.junit.Assert.assertEquals
import org.junit.Test

class OverlayPolicyTest {
    private val policy = OverlayPolicy()

    @Test
    fun hidesWhenKeyboardIsNotVisible() {
        assertEquals(
            OverlayDecision.Hidden,
            policy.decide(OverlayContext(keyboardBounds = null, hasEditableFocus = true)),
        )
    }

    @Test
    fun hidesWithoutAnEditableField() {
        assertEquals(
            OverlayDecision.Hidden,
            policy.decide(
                OverlayContext(
                    keyboardBounds = ScreenRect(0, 1400, 1080, 2400),
                    hasEditableFocus = false,
                ),
            ),
        )
    }

    @Test
    fun hidesForPasswordsAndSensitiveEditors() {
        val keyboard = ScreenRect(0, 1400, 1080, 2400)
        assertEquals(
            OverlayDecision.Hidden,
            policy.decide(OverlayContext(keyboard, true, isPassword = true)),
        )
        assertEquals(
            OverlayDecision.Hidden,
            policy.decide(OverlayContext(keyboard, true, isSensitive = true)),
        )
    }

    @Test
    fun positionsMicInLeftGutterBesideHomeRow() {
        val decision = policy.decide(
            OverlayContext(
                keyboardBounds = ScreenRect(0, 1400, 1080, 2400),
                hasEditableFocus = true,
                displayBounds = ScreenRect(0, 0, 1080, 2400),
                controlWidthPx = 64,
                controlHeightPx = 128,
                marginPx = 24,
            ),
        )

        assertEquals(OverlayDecision.Show(ScreenPoint(24, 1736)), decision)
    }

    @Test
    fun clampsMicOnNarrowOrOffsetDisplays() {
        val decision = policy.decide(
            OverlayContext(
                keyboardBounds = ScreenRect(-500, 40, -300, 500),
                hasEditableFocus = true,
                displayBounds = ScreenRect(-500, 0, -300, 500),
                controlWidthPx = 64,
                controlHeightPx = 128,
                marginPx = 24,
            ),
        )

        assertEquals(OverlayDecision.Show(ScreenPoint(-476, 160)), decision)
    }

    @Test
    fun floatsWhileDictatingWithTheKeyboardClosed() {
        assertEquals(
            OverlayDecision.Float,
            policy.decide(
                OverlayContext(keyboardBounds = null, hasEditableFocus = false, dictationActive = true),
            ),
        )
    }

    @Test
    fun floatsWhileDictatingOverAPasswordKeyboard() {
        assertEquals(
            OverlayDecision.Float,
            policy.decide(
                OverlayContext(
                    keyboardBounds = ScreenRect(0, 1400, 1080, 2400),
                    hasEditableFocus = true,
                    isPassword = true,
                    dictationActive = true,
                ),
            ),
        )
    }

    @Test
    fun docksBesideTheKeyboardWhenItReopensDuringDictation() {
        val decision = policy.decide(
            OverlayContext(
                keyboardBounds = ScreenRect(0, 1400, 1080, 2400),
                hasEditableFocus = true,
                controlWidthPx = 64,
                controlHeightPx = 128,
                marginPx = 24,
                dictationActive = true,
            ),
        )

        assertEquals(OverlayDecision.Show(ScreenPoint(24, 1736)), decision)
    }
}
