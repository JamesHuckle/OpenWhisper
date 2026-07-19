package com.openwhisper.android.overlay

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class OverlayKeyGeometryTest {
    @Test
    fun micKeyIsHalfTheWidthOfAReferenceLetterKey() {
        assertEquals(
            OverlayKeyGeometry.REFERENCE_LETTER_KEY_WIDTH_DP,
            OverlayKeyGeometry.WIDTH_DP * 2,
        )
    }

    @Test
    fun micKeyIsAKeyShapedPortraitRectangle() {
        assertEquals(
            OverlayKeyGeometry.REFERENCE_LETTER_KEY_WIDTH_DP,
            OverlayKeyGeometry.HEIGHT_DP,
        )
        assertTrue(OverlayKeyGeometry.HEIGHT_DP > OverlayKeyGeometry.WIDTH_DP)
    }

    @Test
    fun microphoneIconIsSmallerThanTheCompactKey() {
        assertTrue(OverlayKeyGeometry.ICON_SIZE_DP < OverlayKeyGeometry.WIDTH_DP)
        assertTrue(OverlayKeyGeometry.ICON_SIZE_DP < OverlayKeyGeometry.HEIGHT_DP)
    }
}
