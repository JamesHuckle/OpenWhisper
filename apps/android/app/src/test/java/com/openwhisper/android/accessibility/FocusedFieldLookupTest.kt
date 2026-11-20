package com.openwhisper.android.accessibility

import com.openwhisper.android.editor.EditableNode
import com.openwhisper.android.editor.EditorAvailability
import com.openwhisper.android.editor.EditorSnapshot
import com.openwhisper.android.editor.InputConnectionEditor
import com.openwhisper.android.overlay.FocusedField
import com.openwhisper.android.overlay.OverlayContext
import com.openwhisper.android.overlay.OverlayDecision
import com.openwhisper.android.overlay.OverlayPolicy
import com.openwhisper.android.overlay.ScreenRect
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class FocusedFieldLookupTest {
    private val keyboard = OverlayContext(keyboardBounds = ScreenRect(0, 1600, 1080, 2400))

    @Test
    fun overlayDecisionReadsOnlyFlagsNeverTheFieldText() {
        val fields = TestFields(focused = "note")
        fields.editors["note"] = TextlessEditor("note")
        fields.editors["password"] = TextlessEditor(
            "password",
            EditorAvailability(isPassword = true),
        )
        fields.editors["sensitive"] = TextlessEditor(
            "sensitive",
            EditorAvailability(isSensitive = true),
        )
        val lookup = fields.lookup()

        assertTrue(OverlayPolicy().decide(keyboard, lookup::focusedField) is OverlayDecision.Show)

        fields.focused = "password"
        lookup.forget()
        assertEquals(OverlayDecision.Hidden, OverlayPolicy().decide(keyboard, lookup::focusedField))

        fields.focused = "sensitive"
        lookup.forget()
        assertEquals(OverlayDecision.Hidden, OverlayPolicy().decide(keyboard, lookup::focusedField))
    }

    @Test
    fun refreshWithUnchangedFocusDoesOneLookup() {
        val fields = TestFields(focused = "note")
        fields.editors["note"] = TextlessEditor("note")
        val lookup = fields.lookup()

        repeat(5) { assertEquals(FocusedField(), lookup.focusedField()) }

        assertEquals(1, fields.editorLookups)
        assertEquals(5, fields.focusLookups)
    }

    @Test
    fun remembersAFailedTreeWalkUntilFocusChanges() {
        val fields = TestFields(focused = "web-view")
        val lookup = fields.lookup()

        assertNull(lookup.focusedField())
        assertNull(lookup.focusedField())
        assertEquals(1, fields.editorLookups)

        lookup.forget()
        assertNull(lookup.focusedField())
        assertEquals(2, fields.editorLookups)
    }

    @Test
    fun looksAgainWhenFocusMovesOrIsForgotten() {
        val fields = TestFields(focused = "note")
        fields.editors["note"] = TextlessEditor("note")
        fields.editors["password"] = TextlessEditor(
            "password",
            EditorAvailability(isPassword = true),
        )
        val lookup = fields.lookup()
        lookup.focusedField()

        // Focus moved without an event yet: the focus key still catches it.
        fields.focused = "password"
        assertEquals(FocusedField(isPassword = true), lookup.focusedField())
        assertEquals(2, fields.editorLookups)

        // A focus or window event forgets the field even if the key is unchanged.
        lookup.forget()
        lookup.focusedField()
        assertEquals(3, fields.editorLookups)
    }

    @Test
    fun fallsBackToInputConnectionFlagsWithoutReadingItsText() {
        val fields = TestFields(focused = "rich-editor")
        fields.inputConnection = TextlessInputConnection(EditorAvailability(isPassword = true))
        val lookup = fields.lookup()

        assertEquals(FocusedField(isPassword = true), lookup.focusedField())
        assertEquals(FocusedField(isPassword = true), lookup.focusedField())
        assertEquals(1, fields.editorLookups)
    }

    private class TestFields(var focused: String?) {
        val editors = mutableMapOf<String, EditableNode>()
        var inputConnection: InputConnectionEditor? = null
        var focusLookups = 0
        var editorLookups = 0

        fun lookup() = FocusedFieldLookup(
            focusedNode = { focusLookups += 1; focused },
            focusKey = { it },
            editorFor = { node -> editorLookups += 1; node?.let(editors::get) },
            inputConnectionEditor = { inputConnection },
        )
    }

    private class TextlessEditor(
        override val stableId: String,
        override val availability: EditorAvailability = EditorAvailability(),
    ) : EditableNode {
        override fun snapshot(): EditorSnapshot =
            throw AssertionError("The overlay must not copy the field's text")

        override fun setText(text: String): Boolean = throw AssertionError("unexpected write")

        override fun setSelection(cursor: Int): Boolean = throw AssertionError("unexpected write")
    }

    private class TextlessInputConnection(
        override val availability: EditorAvailability,
    ) : InputConnectionEditor {
        override val stableId: String = "input"

        override fun surroundingText(): EditorSnapshot? =
            throw AssertionError("The overlay must not copy the field's text")

        override fun commitText(text: String): Boolean = throw AssertionError("unexpected write")
    }
}
