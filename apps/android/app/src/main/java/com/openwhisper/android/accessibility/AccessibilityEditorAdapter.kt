package com.openwhisper.android.accessibility

import android.graphics.Rect
import android.os.Build
import android.os.Bundle
import android.view.accessibility.AccessibilityNodeInfo
import com.openwhisper.android.editor.EditableNode
import com.openwhisper.android.editor.EditorSnapshot
import com.openwhisper.android.editor.FocusedEditorProvider

class AccessibilityEditorProvider(
    private val focusedNode: () -> AccessibilityNodeInfo?,
) : FocusedEditorProvider {
    override fun focusedEditor(): EditableNode? {
        val node = focusedNode() ?: return null
        if (!node.isEditable || !node.isEnabled || !node.isFocused) return null
        return AccessibilityEditableNode(node)
    }
}

private class AccessibilityEditableNode(
    private val node: AccessibilityNodeInfo,
) : EditableNode {
    override val stableId: String = buildStableId(node)

    override fun snapshot(): EditorSnapshot = EditorSnapshot(
        text = node.text?.toString().orEmpty(),
        selectionStart = node.textSelectionStart,
        selectionEnd = node.textSelectionEnd,
        isPassword = node.isPassword,
        isSensitive = Build.VERSION.SDK_INT >= 34 && node.isAccessibilityDataSensitive,
    )

    override fun setText(text: String): Boolean {
        val arguments = Bundle().apply {
            putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text)
        }
        return node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, arguments)
    }

    override fun setSelection(cursor: Int): Boolean {
        val arguments = Bundle().apply {
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, cursor)
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, cursor)
        }
        return node.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, arguments)
    }

    private companion object {
        fun buildStableId(node: AccessibilityNodeInfo): String {
            if (Build.VERSION.SDK_INT >= 33) {
                node.uniqueId?.takeIf { it.isNotBlank() }?.let { return "unique:$it" }
            }
            val bounds = Rect().also(node::getBoundsInScreen)
            return listOf(
                node.windowId,
                node.packageName,
                node.viewIdResourceName,
                node.className,
                bounds.flattenToString(),
            ).joinToString("|")
        }
    }
}
