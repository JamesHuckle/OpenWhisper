package com.openwhisper.android.demo

import android.app.Activity
import android.graphics.Color
import android.os.Bundle
import android.text.InputType
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import com.openwhisper.android.R

class DemoTargetActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val content = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            isFocusableInTouchMode = true
            setPadding(dp(24), dp(36), dp(24), dp(24))
            setBackgroundColor(Color.WHITE)
        }
        content.addView(TextView(this).apply {
            text = getString(R.string.demo_title)
            textSize = 24f
            setTextColor(Color.rgb(25, 25, 32))
        })
        content.addView(TextView(this).apply {
            text = getString(R.string.demo_instructions)
            textSize = 15f
            setTextColor(Color.rgb(80, 80, 92))
            setPadding(0, dp(8), 0, dp(24))
        })
        content.addView(EditText(this).apply {
            id = R.id.demo_editor
            hint = getString(R.string.safe_dictation_field)
            setText(R.string.demo_initial_text)
            setSelection(text.length)
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE
            minLines = 4
            gravity = android.view.Gravity.TOP
            contentDescription = getString(R.string.safe_dictation_field)
        })
        content.addView(InputConnectionOnlyView(this).apply {
            id = R.id.demo_input_connection_only
        })
        content.addView(InputConnectionOnlyView(this).apply {
            id = R.id.demo_input_connection_password
            editorInputType =
                InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
        })
        content.addView(EditText(this).apply {
            id = R.id.demo_password
            hint = getString(R.string.password_field_hint)
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            contentDescription = getString(R.string.password_field)
            setPadding(0, dp(24), 0, 0)
        })

        setContentView(ScrollView(this).apply { addView(content) })
        content.requestFocus()
    }

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()
}
