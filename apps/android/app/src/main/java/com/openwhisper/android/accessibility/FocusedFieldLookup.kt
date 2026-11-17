package com.openwhisper.android.accessibility

import com.openwhisper.android.editor.EditableNode
import com.openwhisper.android.editor.EditorAvailability
import com.openwhisper.android.editor.InputConnectionEditor
import com.openwhisper.android.overlay.FocusedField

/**
 * Tells the overlay whether a field has focus and whether it is a password or
 * sensitive field. It reads only those flags, never the field's text, and
 * remembers the answer while the same node keeps input focus, so refreshes while
 * the user types cost one focus lookup instead of a walk of the other app's tree.
 * Call [forget] when focus or the windows change.
 */
internal class FocusedFieldLookup<N : Any>(
    /** The node with input focus: one call into the other app. */
    private val focusedNode: () -> N?,
    /** Stays the same while the same field keeps focus. */
    private val focusKey: (N) -> String,
    /** The usable editor for [focusedNode]'s result; may walk every window. */
    private val editorFor: (focused: N?) -> EditableNode?,
    private val inputConnectionEditor: () -> InputConnectionEditor?,
) {
    private var remembered: Remembered? = null

    fun focusedField(): FocusedField? {
        val focused = focusedNode()
        val key = focused?.let(focusKey)
        val accessibility = remembered?.takeIf { it.key == key }
            ?: Remembered(key, editorFor(focused)?.availability?.toFocusedField())
                .also { remembered = it }
        accessibility.field?.let { return it }
        // Rich-text editors without an editable node may still hold the input connection.
        return inputConnectionEditor()?.availability?.toFocusedField()
    }

    fun forget() {
        remembered = null
    }

    private class Remembered(val key: String?, val field: FocusedField?)

    private fun EditorAvailability.toFocusedField() =
        FocusedField(isPassword = isPassword, isSensitive = isSensitive)
}
