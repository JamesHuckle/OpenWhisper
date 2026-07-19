package com.openwhisper.android

import android.app.Instrumentation
import android.app.UiAutomation
import android.content.Intent
import android.os.SystemClock
import android.view.InputDevice
import android.view.MotionEvent
import android.view.inputmethod.InputMethodManager
import android.widget.EditText
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.openwhisper.android.demo.DemoTargetActivity
import com.openwhisper.android.overlay.OverlayKeyGeometry
import java.io.FileInputStream
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class OpenWhisperAccessibilityE2ETest {
    private lateinit var instrumentation: Instrumentation
    private lateinit var automation: UiAutomation
    private lateinit var activity: DemoTargetActivity

    @Before
    fun enableServiceAndLaunchDemo() {
        instrumentation = InstrumentationRegistry.getInstrumentation()
        automation = instrumentation.getUiAutomation(
            UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES or
                UiAutomation.FLAG_DONT_USE_ACCESSIBILITY,
        )
        shell("settings delete secure enabled_accessibility_services")
        shell("settings put secure accessibility_enabled 0")
        SystemClock.sleep(250)
        shell("settings put secure enabled_accessibility_services " +
            "com.openwhisper.android.debug/com.openwhisper.android.accessibility.OpenWhisperAccessibilityService")
        shell("settings put secure accessibility_enabled 1")
        assertTrue("The accessibility service did not bind", eventually(5_000) {
            shell("dumpsys accessibility").contains(
                "Bound services:{Service[label=OpenWhisper dictation",
            )
        })

        val intent = Intent().apply {
            setClassName(
                instrumentation.targetContext,
                "com.openwhisper.android.demo.DemoTargetActivity",
            )
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
        }
        activity = instrumentation.startActivitySync(intent) as DemoTargetActivity
    }

    @After
    fun disableService() {
        shell("settings delete secure enabled_accessibility_services")
        shell("settings put secure accessibility_enabled 0")
        if (::activity.isInitialized) activity.finish()
    }

    @Test
    fun demoDictationInsertsIntoFocusedFieldAndNeverShowsForPassword() {
        focusEditor(R.id.demo_editor)

        var overlay = waitForOverlay()
        assertNotNull("The accessibility microphone overlay did not appear", overlay)
        overlay = requireNotNull(overlay)
        val density = instrumentation.targetContext.resources.displayMetrics.density
        assertEquals(dp(OverlayKeyGeometry.WIDTH_DP, density), overlay.width)
        assertEquals(dp(OverlayKeyGeometry.HEIGHT_DP, density), overlay.height)
        assertEquals(overlay.height, overlay.width * 2)
        assertEquals(dp(OverlayKeyGeometry.GUTTER_MARGIN_DP, density), overlay.x)
        tap(overlay.centerX, overlay.centerY)
        SystemClock.sleep(300)
        overlay = requireNotNull(waitForOverlay())
        tap(overlay.centerX, overlay.centerY)

        assertTrue("The transcript was not inserted into the focused editor", eventually(5_000) {
            editorText(R.id.demo_editor).contains("OpenWhisper dictation works perfectly.")
        })

        focusEditor(R.id.demo_password)
        assertTrue("The microphone remained visible over a password editor", eventually(2_000) {
            findOverlay() == null
        })
        assertFalse(hasOverlayWindow())
    }

    private fun focusEditor(id: Int) {
        instrumentation.waitForIdleSync()
        assertTrue("The software keyboard did not become visible", eventually(5_000) {
            instrumentation.runOnMainSync {
                val editor = activity.findViewById<EditText>(id)
                editor.requestFocus()
                editor.setSelection(editor.text.length)
                activity.getSystemService(InputMethodManager::class.java)
                    .showSoftInput(editor, InputMethodManager.SHOW_IMPLICIT)
            }
            shell("dumpsys window").contains("mImeShowing=true")
        })
    }

    private fun editorText(id: Int): String {
        var text = ""
        instrumentation.runOnMainSync {
            text = activity.findViewById<EditText>(id).text.toString()
        }
        return text
    }

    private fun waitForOverlay(): OverlayBounds? {
        var result: OverlayBounds? = null
        var previous: OverlayBounds? = null
        var stableSamples = 0
        eventually(5_000) {
            result = findOverlay()
            stableSamples = if (result != null && result == previous) stableSamples + 1 else 0
            previous = result
            stableSamples >= 3
        }
        return result
    }

    private fun hasOverlayWindow(): Boolean = findOverlay() != null

    private fun findOverlay(): OverlayBounds? {
        val lines = shell("dumpsys window windows").lineSequence().toList()
        lines.indices.forEach { index ->
            val header = lines[index]
            if (!header.contains("Window #") || !header.contains("com.openwhisper.android.debug")) return@forEach
            val block = lines.subList(index, minOf(index + 32, lines.size)).joinToString("\n")
            if (!block.contains("ty=ACCESSIBILITY_OVERLAY")) return@forEach
            if (!block.contains("mHasSurface=true") ||
                !block.contains("isOnScreen=true") ||
                !block.contains("isVisible=true")
            ) return@forEach
            val frame = OVERLAY_FRAME.find(block) ?: return@forEach
            val left = frame.groupValues[1].toInt()
            val top = frame.groupValues[2].toInt()
            val right = frame.groupValues[3].toInt()
            val bottom = frame.groupValues[4].toInt()
            if (right <= left || bottom <= top) return@forEach
            return OverlayBounds(
                x = left,
                y = top,
                width = right - left,
                height = bottom - top,
            )
        }
        return null
    }

    private fun eventually(timeoutMs: Long, condition: () -> Boolean): Boolean {
        val deadline = SystemClock.elapsedRealtime() + timeoutMs
        do {
            if (condition()) return true
            SystemClock.sleep(100)
        } while (SystemClock.elapsedRealtime() < deadline)
        return condition()
    }

    private fun tap(x: Int, y: Int) {
        val downTime = SystemClock.uptimeMillis()
        val down = MotionEvent.obtain(
            downTime,
            downTime,
            MotionEvent.ACTION_DOWN,
            x.toFloat(),
            y.toFloat(),
            0,
        ).apply { source = InputDevice.SOURCE_TOUCHSCREEN }
        val up = MotionEvent.obtain(
            downTime,
            downTime + 50,
            MotionEvent.ACTION_UP,
            x.toFloat(),
            y.toFloat(),
            0,
        ).apply { source = InputDevice.SOURCE_TOUCHSCREEN }
        try {
            assertTrue(automation.injectInputEvent(down, true))
            assertTrue(automation.injectInputEvent(up, true))
        } finally {
            down.recycle()
            up.recycle()
        }
    }

    private fun shell(command: String): String =
        automation.executeShellCommand(command).use { descriptor ->
            FileInputStream(descriptor.fileDescriptor).bufferedReader().use { it.readText() }
        }

    private fun dp(value: Int, density: Float): Int = (value * density).toInt()

    private data class OverlayBounds(
        val x: Int,
        val y: Int,
        val width: Int,
        val height: Int,
    ) {
        val centerX get() = x + width / 2
        val centerY get() = y + height / 2
    }

    private companion object {
        val OVERLAY_FRAME = Regex(
            """Frames:.*?frame=\[(-?\d+),(-?\d+)\]\[(-?\d+),(-?\d+)\]""",
        )
    }
}
