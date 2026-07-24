package com.openwhisper.android.demo

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.os.Build
import android.text.Editable
import android.text.InputType
import android.text.Selection
import android.text.SpannableStringBuilder
import android.util.TypedValue
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager

/**
 * A deterministic stand-in for rich-text apps that accept keyboard input
 * through InputConnection but expose no editable AccessibilityNodeInfo.
 */
class InputConnectionOnlyView(context: Context) : View(context) {
    var editorInputType: Int =
        InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE
    private val content = SpannableStringBuilder("Existing note")
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.rgb(25, 25, 32)
        textSize = TypedValue.applyDimension(
            TypedValue.COMPLEX_UNIT_SP,
            18f,
            resources.displayMetrics,
        )
    }

    init {
        isFocusable = true
        isFocusableInTouchMode = true
        minimumHeight = (64 * resources.displayMetrics.density).toInt()
        setBackgroundColor(Color.rgb(245, 245, 248))
        Selection.setSelection(content, content.length)
    }

    override fun onCheckIsTextEditor(): Boolean = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        outAttrs.inputType = editorInputType
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI
        outAttrs.initialSelStart = Selection.getSelectionStart(content)
        outAttrs.initialSelEnd = Selection.getSelectionEnd(content)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            outAttrs.setInitialSurroundingText(content)
        }
        return object : BaseInputConnection(this, true) {
            override fun getEditable(): Editable = content

            override fun commitText(text: CharSequence, newCursorPosition: Int): Boolean {
                val committed = super.commitText(text, newCursorPosition)
                notifySelectionChanged()
                invalidate()
                return committed
            }

            override fun setSelection(start: Int, end: Int): Boolean {
                val selected = super.setSelection(start, end)
                notifySelectionChanged()
                return selected
            }
        }
    }

    fun editorText(): String = content.toString()

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        canvas.drawText(content.toString(), paddingLeft.toFloat(), paint.textSize + paddingTop, paint)
    }

    private fun notifySelectionChanged() {
        context.getSystemService(InputMethodManager::class.java).updateSelection(
            this,
            Selection.getSelectionStart(content),
            Selection.getSelectionEnd(content),
            -1,
            -1,
        )
    }
}
