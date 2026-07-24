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
    private val windowRoots: () -> List<AccessibilityNodeInfo> = { emptyList() },
) : FocusedEditorProvider {
    override fun focusedEditor(): EditableNode? {
        val node = focusedNode()
            ?.takeIf(::isUsableEditor)
            ?: windowRoots()
                .asSequence()
                .mapNotNull(::findFocusedEditor)
                .firstOrNull()
            ?: return null
        return AccessibilityEditableNode(node)
    }

    private fun findFocusedEditor(root: AccessibilityNodeInfo): AccessibilityNodeInfo? {
        return FocusedEditorSearch.find(
            roots = listOf(root),
            children = { node ->
                (0 until node.childCount).mapNotNull(node::getChild)
            },
            matches = ::isUsableEditor,
            maxVisited = MAX_TRAVERSED_NODES,
        )
    }

    private fun isUsableEditor(node: AccessibilityNodeInfo): Boolean {
        val supportsSetText = node.actionList.any {
            it.id == AccessibilityNodeInfo.ACTION_SET_TEXT
        }
        return EditorNodeEligibility.isUsable(
            isEnabled = node.isEnabled,
            isInputFocused = node.isFocused,
            isEditable = node.isEditable,
            supportsSetText = supportsSetText,
        )
    }

    private companion object {
        // Accessibility trees are expected to be finite, but a defensive limit
        // keeps a malformed third-party virtual hierarchy off the main thread.
        const val MAX_TRAVERSED_NODES = 10_000
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
