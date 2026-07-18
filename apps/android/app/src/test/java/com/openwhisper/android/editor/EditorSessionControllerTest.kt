package com.openwhisper.android.editor

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class EditorSessionControllerTest {
    @Test
    fun capturesSafeFocusedEditorAndCommitsAtOriginalSelection() {
        val node = FakeEditableNode("field-1", EditorSnapshot("Hello world", 5, 5))
        val provider = FakeFocusedEditorProvider(node)
        val controller = EditorSessionController(provider)

        val session = controller.begin()
        val result = controller.commit(requireNotNull(session), " brave")

        assertEquals(EditorCommitResult.Committed, result)
        assertEquals("Hello brave world", node.setTextValue)
        assertEquals(11, node.selectionValue)
    }

    @Test
    fun refusesToLeakTranscriptWhenFocusMovesToAnotherField() {
        val original = FakeEditableNode("field-1", EditorSnapshot("One", 3, 3))
        val replacement = FakeEditableNode("field-2", EditorSnapshot("Two", 3, 3))
        val provider = FakeFocusedEditorProvider(original)
        val controller = EditorSessionController(provider)
        val session = requireNotNull(controller.begin())

        provider.focused = replacement
        val result = controller.commit(session, " secret")

        assertEquals(EditorCommitResult.FocusChanged, result)
        assertNull(original.setTextValue)
        assertNull(replacement.setTextValue)
    }

    @Test
    fun refusesToOverwriteTextEditedWhileDictationWasRunning() {
        val original = FakeEditableNode("field-1", EditorSnapshot("One", 3, 3))
        val provider = FakeFocusedEditorProvider(original)
        val controller = EditorSessionController(provider)
        val session = requireNotNull(controller.begin())

        provider.focused = FakeEditableNode("field-1", EditorSnapshot("One plus typing", 15, 15))
        val result = controller.commit(session, " dictated")

        assertEquals(EditorCommitResult.ContentChanged, result)
        assertNull(original.setTextValue)
    }

    @Test
    fun neverStartsSessionForPasswordOrSensitiveField() {
        val password = FakeEditableNode(
            "password",
            EditorSnapshot("", 0, 0, isPassword = true),
        )
        val provider = FakeFocusedEditorProvider(password)
        val controller = EditorSessionController(provider)
        assertNull(controller.begin())

        provider.focused = FakeEditableNode(
            "sensitive",
            EditorSnapshot("", 0, 0, isSensitive = true),
        )
        assertNull(controller.begin())
    }

    @Test
    fun reportsEditorsThatRejectAccessibilityActions() {
        val node = FakeEditableNode(
            "field-1",
            EditorSnapshot("Hello", 5, 5),
            acceptSetText = false,
        )
        val controller = EditorSessionController(FakeFocusedEditorProvider(node))

        val result = controller.commit(requireNotNull(controller.begin()), " world")
        assertEquals(EditorCommitResult.Unsupported, result)
    }
}

private class FakeFocusedEditorProvider(var focused: FakeEditableNode?) : FocusedEditorProvider {
    override fun focusedEditor(): EditableNode? = focused
}

private class FakeEditableNode(
    override val stableId: String,
    private val value: EditorSnapshot,
    private val acceptSetText: Boolean = true,
) : EditableNode {
    var setTextValue: String? = null
    var selectionValue: Int? = null

    override fun snapshot(): EditorSnapshot = value

    override fun setText(text: String): Boolean {
        if (!acceptSetText) return false
        setTextValue = text
        return true
    }

    override fun setSelection(cursor: Int): Boolean {
        selectionValue = cursor
        return true
    }
}
