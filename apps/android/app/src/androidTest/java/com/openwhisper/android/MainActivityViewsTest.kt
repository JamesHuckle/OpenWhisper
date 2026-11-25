package com.openwhisper.android

import android.graphics.Color
import android.util.TypedValue
import android.view.Gravity
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.Switch
import android.widget.TextView
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.openwhisper.android.recordings.StoredRecording
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/** The main screen's labels, buttons and saved-recording rows, as built in code. */
@RunWith(AndroidJUnit4::class)
class MainActivityViewsTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private lateinit var recording: StoredRecording

    @Before
    fun saveOneRecording() {
        // Keeps the automatic update check from opening a dialog over the screen.
        context.getSharedPreferences("openwhisper_updates", 0)
            .edit()
            .putLong("last_check_epoch_ms", System.currentTimeMillis())
            .commit()
        recording = OpenWhisperApplication.recordings(context)
            .import("pin-test.wav", "audio/wav", "wav", ByteArray(10).inputStream())
    }

    @After
    fun deleteRecording() {
        OpenWhisperApplication.recordings(context).file(recording).parentFile?.deleteRecursively()
    }

    @Test
    fun mainScreenKeepsItsLabelsButtonsAndRecordingRows() {
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val scroll = activity.findViewById<ViewGroup>(android.R.id.content).getChildAt(0) as ScrollView
                val content = scroll.getChildAt(0) as LinearLayout
                val views = (0 until content.childCount).map(content::getChildAt)
                val defaultSize = TextView(activity).textSize
                fun sp(value: Float) =
                    TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, value, activity.resources.displayMetrics)
                val title = Color.rgb(24, 24, 32)
                val heading = Color.rgb(35, 35, 45)
                val secondary = Color.rgb(82, 82, 96)

                fun assertLabel(index: Int, text: String?, size: Float, color: Int, top: Int, bottom: Int) {
                    val label = views[index] as TextView
                    assertFalse(label is Button || label is EditText)
                    if (text != null) assertEquals(text, label.text.toString())
                    assertEquals(size, label.textSize, 0.01f)
                    assertEquals(color, label.currentTextColor)
                    assertEquals(listOf(0, activity.dp(top), 0, activity.dp(bottom)),
                        listOf(label.paddingLeft, label.paddingTop, label.paddingRight, label.paddingBottom))
                }
                fun assertStatusRow(index: Int, labelRes: Int, top: Int) {
                    assertLabel(index, null, sp(17f), heading, top, 8)
                    assertTrue((views[index] as TextView).text.startsWith(activity.getString(labelRes)))
                }
                fun assertButton(button: Button, textRes: Int) {
                    assertEquals(activity.getString(textRes), button.text.toString())
                    assertFalse(button.isAllCaps)
                }

                assertEquals(17, views.size)
                assertLabel(0, activity.getString(R.string.app_name), sp(30f), title, 0, 0)
                assertLabel(1, activity.getString(R.string.main_tagline), sp(16f), secondary, 8, 24)
                assertStatusRow(2, R.string.accessibility_service_label, 0)
                assertButton(views[3] as Button, R.string.enable_accessibility)
                assertStatusRow(4, R.string.microphone_permission_label, 22)
                assertButton(views[5] as Button, R.string.grant_microphone)
                assertStatusRow(6, R.string.api_key_label, 22)
                assertTrue(views[7] is EditText)
                val keyButtons = views[8] as LinearLayout
                assertEquals(Gravity.END, keyButtons.gravity and Gravity.RELATIVE_HORIZONTAL_GRAVITY_MASK)
                assertButton(keyButtons.getChildAt(0) as Button, R.string.clear)
                assertButton(keyButtons.getChildAt(1) as Button, R.string.save_securely)
                assertEquals(activity.getString(R.string.demo_mode), (views[9] as Switch).text.toString())
                assertLabel(10, activity.getString(R.string.demo_mode_explanation), defaultSize, secondary, 0, 0)
                val demoButton = views[11] as Button
                assertButton(demoButton, R.string.open_demo)
                assertEquals(listOf(0, activity.dp(20), 0, 0),
                    listOf(demoButton.paddingLeft, demoButton.paddingTop, demoButton.paddingRight, demoButton.paddingBottom))
                assertLabel(12, activity.getString(R.string.audio_and_recovery), sp(19f), heading, 26, 8)
                assertButton(views[13] as Button, R.string.transcribe_audio_file)
                assertLabel(14, "", defaultSize, secondary, 7, 7)
                assertButton(views[16] as Button, R.string.check_for_updates)

                val rows = views[15] as LinearLayout
                val row = (0 until rows.childCount).map { rows.getChildAt(it) as LinearLayout }
                    .single { (it.getChildAt(0) as Button).text.startsWith("pin-test.wav\n") }
                val openButton = row.getChildAt(0) as Button
                assertEquals(
                    activity.getString(R.string.saved_recording_format, "pin-test.wav", "saved", "1 KB"),
                    openButton.text.toString(),
                )
                assertFalse(openButton.isAllCaps)
                assertEquals(Gravity.START or Gravity.CENTER_VERTICAL, openButton.gravity)
                assertEquals(activity.getString(R.string.open_raw_recording, "pin-test.wav"), openButton.contentDescription)
                val transcribeButton = row.getChildAt(1) as Button
                assertButton(transcribeButton, R.string.quick_transcribe)
                assertEquals(
                    activity.getString(R.string.retranscribe_recording, "pin-test.wav"),
                    transcribeButton.contentDescription,
                )
                assertTrue(transcribeButton.isEnabled)
            }
        }
    }
}
