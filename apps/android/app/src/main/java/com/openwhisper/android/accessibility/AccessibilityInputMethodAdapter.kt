package com.openwhisper.android.accessibility

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.InputMethod
import android.os.Build
import android.text.InputType
import android.view.inputmethod.EditorInfo
import androidx.annotation.RequiresApi
import com.openwhisper.android.editor.EditorAvailability
import com.openwhisper.android.editor.EditorSnapshot
import com.openwhisper.android.editor.InputConnectionEditor
import com.openwhisper.android.editor.InputConnectionEditorProvider

@RequiresApi(Build.VERSION_CODES.TIRAMISU)
class OpenWhisperInputMethod(
    service: AccessibilityService,
    private val onEditorStateChanged: () -> Unit,
) : InputMethod(service), InputConnectionEditorProvider {
    private var generation = 0L

    override fun onStartInput(attribute: EditorInfo, restarting: Boolean) {
        super.onStartInput(attribute, restarting)
        generation += 1
        onEditorStateChanged()
    }

    override fun onFinishInput() {
        generation += 1
        onEditorStateChanged()
        super.onFinishInput()
    }

    override fun onUpdateSelection(
        oldSelStart: Int,
        oldSelEnd: Int,
        newSelStart: Int,
        newSelEnd: Int,
        candidatesStart: Int,
        candidatesEnd: Int,
    ) {
        onEditorStateChanged()
    }

    override fun currentEditor(): InputConnectionEditor? {
        if (!currentInputStarted) return null
        val editorInfo = currentInputEditorInfo ?: return null
        if (editorInfo.inputType == InputType.TYPE_NULL) return null
        val connection = currentInputConnection ?: return null
        return AndroidInputConnectionEditor(
            stableId = listOf(
                "input",
                generation,
                editorInfo.packageName,
                editorInfo.fieldId,
                editorInfo.inputType,
            ).joinToString(":"),
            availability = EditorAvailability(
                isPassword = isPasswordInputType(editorInfo.inputType),
            ),
            connection = connection,
        )
    }

    private fun isPasswordInputType(inputType: Int): Boolean {
        val inputClass = inputType and InputType.TYPE_MASK_CLASS
        val variation = inputType and InputType.TYPE_MASK_VARIATION
        return when (inputClass) {
            InputType.TYPE_CLASS_TEXT ->
                variation == InputType.TYPE_TEXT_VARIATION_PASSWORD ||
                    variation == InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD ||
                    variation == InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD
            InputType.TYPE_CLASS_NUMBER ->
                variation == InputType.TYPE_NUMBER_VARIATION_PASSWORD
            else -> false
        }
    }

    private companion object {
        const val SURROUNDING_TEXT_LIMIT = 4_096
    }

    private class AndroidInputConnectionEditor(
        override val stableId: String,
        override val availability: EditorAvailability,
        private val connection: AccessibilityInputConnection,
    ) : InputConnectionEditor {
        override fun snapshot(): EditorSnapshot? {
            val surrounding = connection.getSurroundingText(
                SURROUNDING_TEXT_LIMIT,
                SURROUNDING_TEXT_LIMIT,
                0,
            ) ?: return null
            return EditorSnapshot(
                text = surrounding.text.toString(),
                selectionStart = surrounding.selectionStart,
                selectionEnd = surrounding.selectionEnd,
                isPassword = availability.isPassword,
                isSensitive = availability.isSensitive,
            )
        }

        override fun commitText(text: String): Boolean {
            connection.commitText(text, 1, null)
            return true
        }
    }
}
