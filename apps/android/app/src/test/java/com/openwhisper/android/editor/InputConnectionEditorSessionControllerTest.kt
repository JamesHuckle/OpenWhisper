package com.openwhisper.android.editor

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class InputConnectionEditorSessionControllerTest {
    @Test
    fun commitsOnlyThePlannedInsertionThroughTheInputConnection() {
        val editor = FakeInputConnectionEditor(
            stableId = "onenote-editor",
            value = EditorSnapshot("Hello world", 5, 5),
        )
        val controller = InputConnectionEditorSessionController(provider = { editor })

        val result = controller.commit(requireNotNull(controller.begin()), "brave")

        assertEquals(EditorCommitResult.Committed, result)
        assertEquals(" brave", editor.committedText)
    }

    @Test
    fun replacesTheCurrentSelectionThroughTheInputConnection() {
        val editor = FakeInputConnectionEditor(
            stableId = "onenote-editor",
            value = EditorSnapshot("Hello old world", 6, 9),
        )
        val controller = InputConnectionEditorSessionController(provider = { editor })

        val result = controller.commit(requireNotNull(controller.begin()), "new")

        assertEquals(EditorCommitResult.Committed, result)
        assertEquals("new", editor.committedText)
    }

    @Test
    fun rejectsChangedFocusOrContentBeforeCommit() {
        var editor = FakeInputConnectionEditor("first", EditorSnapshot("One", 3, 3))
        val controller = InputConnectionEditorSessionController(provider = { editor })
        val focusSession = requireNotNull(controller.begin())
        editor = FakeInputConnectionEditor("second", EditorSnapshot("Two", 3, 3))
        assertEquals(
            EditorCommitResult.FocusChanged,
            controller.commit(focusSession, "secret"),
        )

        editor = FakeInputConnectionEditor("same", EditorSnapshot("One", 3, 3))
        val contentSession = requireNotNull(controller.begin())
        editor = FakeInputConnectionEditor("same", EditorSnapshot("One changed", 11, 11))
        assertEquals(
            EditorCommitResult.ContentChanged,
            controller.commit(contentSession, "secret"),
        )
        assertNull(editor.committedText)
    }

    @Test
    fun neverStartsForPasswordSensitiveOrBrokenEditors() {
        var editor = FakeInputConnectionEditor(
            "password",
            EditorSnapshot("", 0, 0),
            EditorAvailability(isPassword = true),
        )
        val controller = InputConnectionEditorSessionController(provider = { editor })
        assertNull(controller.begin())

        editor = FakeInputConnectionEditor(
            "sensitive",
            EditorSnapshot("", 0, 0),
            EditorAvailability(isSensitive = true),
        )
        assertNull(controller.begin())

        editor = FakeInputConnectionEditor(
            "broken",
            EditorSnapshot("text", -1, -1),
        )
        assertNull(controller.begin())
    }
}

private class FakeInputConnectionEditor(
    override val stableId: String,
    private val value: EditorSnapshot,
    override val availability: EditorAvailability = EditorAvailability(),
) : InputConnectionEditor {
    var committedText: String? = null

    override fun snapshot(): EditorSnapshot = value

    override fun commitText(text: String): Boolean {
        committedText = text
        return true
    }
}
