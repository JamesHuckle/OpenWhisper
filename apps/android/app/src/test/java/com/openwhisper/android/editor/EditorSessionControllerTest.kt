package com.openwhisper.android.editor

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class EditorSessionControllerTest {
    private var accessibilityEditor: FakeEditableNode? = null
    private var inputConnectionEditor: FakeInputConnectionEditor? = null
    private var accessibilityLookups = 0
    private var inputConnectionLookups = 0

    private val controller = EditorSessionController(
        accessibility = { accessibilityLookups += 1; accessibilityEditor },
        inputConnection = { inputConnectionLookups += 1; inputConnectionEditor },
    )

    // Accessibility channel

    @Test
    fun capturesSafeFocusedEditorAndCommitsAtOriginalSelection() {
        val node = FakeEditableNode("field-1", EditorSnapshot("Hello world", 5, 5))
        accessibilityEditor = node

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
        accessibilityEditor = original
        val session = requireNotNull(controller.begin())

        accessibilityEditor = replacement
        val result = controller.commit(session, " secret")

        assertEquals(EditorCommitResult.FocusChanged, result)
        assertNull(original.setTextValue)
        assertNull(replacement.setTextValue)
    }

    @Test
    fun refusesToOverwriteTextEditedWhileDictationWasRunning() {
        val original = FakeEditableNode("field-1", EditorSnapshot("One", 3, 3))
        accessibilityEditor = original
        val session = requireNotNull(controller.begin())

        accessibilityEditor = FakeEditableNode("field-1", EditorSnapshot("One plus typing", 15, 15))
        val result = controller.commit(session, " dictated")

        assertEquals(EditorCommitResult.ContentChanged, result)
        assertNull(original.setTextValue)
    }

    @Test
    fun neverStartsSessionForPasswordOrSensitiveField() {
        accessibilityEditor = FakeEditableNode(
            "password",
            EditorSnapshot("", 0, 0, isPassword = true),
        )
        assertNull(controller.begin())

        accessibilityEditor = FakeEditableNode(
            "sensitive",
            EditorSnapshot("", 0, 0, isSensitive = true),
        )
        assertNull(controller.begin())
    }

    @Test
    fun neverCommitsIntoAFieldThatBecamePasswordOrSensitive() {
        accessibilityEditor = FakeEditableNode("field-1", EditorSnapshot("One", 3, 3))
        val session = requireNotNull(controller.begin())

        val password = FakeEditableNode("field-1", EditorSnapshot("One", 3, 3, isPassword = true))
        accessibilityEditor = password
        assertEquals(EditorCommitResult.ContentChanged, controller.commit(session, " secret"))
        assertNull(password.setTextValue)

        val sensitive = FakeEditableNode("field-1", EditorSnapshot("One", 3, 3, isSensitive = true))
        accessibilityEditor = sensitive
        assertEquals(EditorCommitResult.ContentChanged, controller.commit(session, " secret"))
        assertNull(sensitive.setTextValue)
    }

    @Test
    fun reportsEditorsThatRejectAccessibilityActions() {
        accessibilityEditor = FakeEditableNode(
            "field-1",
            EditorSnapshot("Hello", 5, 5),
            acceptSetText = false,
        )

        val result = controller.commit(requireNotNull(controller.begin()), " world")
        assertEquals(EditorCommitResult.Unsupported, result)
    }

    // Input-connection channel

    @Test
    fun commitsOnlyThePlannedInsertionThroughTheInputConnection() {
        val editor = FakeInputConnectionEditor(
            stableId = "onenote-editor",
            value = EditorSnapshot("Hello world", 5, 5),
        )
        inputConnectionEditor = editor

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
        inputConnectionEditor = editor

        val result = controller.commit(requireNotNull(controller.begin()), "new")

        assertEquals(EditorCommitResult.Committed, result)
        assertEquals("new", editor.committedText)
    }

    @Test
    fun rejectsChangedFocusOrContentBeforeCommit() {
        inputConnectionEditor = FakeInputConnectionEditor("first", EditorSnapshot("One", 3, 3))
        val focusSession = requireNotNull(controller.begin())
        inputConnectionEditor = FakeInputConnectionEditor("second", EditorSnapshot("Two", 3, 3))
        assertEquals(
            EditorCommitResult.FocusChanged,
            controller.commit(focusSession, "secret"),
        )

        inputConnectionEditor = FakeInputConnectionEditor("same", EditorSnapshot("One", 3, 3))
        val contentSession = requireNotNull(controller.begin())
        val changed = FakeInputConnectionEditor("same", EditorSnapshot("One changed", 11, 11))
        inputConnectionEditor = changed
        assertEquals(
            EditorCommitResult.ContentChanged,
            controller.commit(contentSession, "secret"),
        )
        assertNull(changed.committedText)
    }

    @Test
    fun neverStartsForPasswordSensitiveOrBrokenEditors() {
        inputConnectionEditor = FakeInputConnectionEditor(
            "password",
            EditorSnapshot("", 0, 0),
            EditorAvailability(isPassword = true),
        )
        assertNull(controller.begin())

        inputConnectionEditor = FakeInputConnectionEditor(
            "sensitive",
            EditorSnapshot("", 0, 0),
            EditorAvailability(isSensitive = true),
        )
        assertNull(controller.begin())

        inputConnectionEditor = FakeInputConnectionEditor(
            "broken",
            EditorSnapshot("text", -1, -1),
        )
        assertNull(controller.begin())
    }

    // Choosing the channel

    @Test
    fun prefersTheAccessibilityEditorWhenBothAreAvailable() {
        val node = FakeEditableNode("node", EditorSnapshot("Hi", 2, 2))
        val input = FakeInputConnectionEditor("input", EditorSnapshot("Hi", 2, 2))
        accessibilityEditor = node
        inputConnectionEditor = input

        val session = requireNotNull(controller.begin())
        assertEquals(EditorSessionChannel.Accessibility, session.channel)
        assertEquals(EditorCommitResult.Committed, controller.commit(session, "there"))
        assertEquals("Hi there", node.setTextValue)
        assertNull(input.committedText)
    }

    @Test
    fun usesTheInputConnectionOnlyWithoutAnAccessibilityEditor() {
        val input = FakeInputConnectionEditor("input", EditorSnapshot("Hi", 2, 2))
        inputConnectionEditor = input

        val session = requireNotNull(controller.begin())
        assertEquals(EditorSessionChannel.InputConnection, session.channel)
        assertEquals(EditorCommitResult.Committed, controller.commit(session, "there"))
        assertEquals(" there", input.committedText)
    }

    @Test
    fun refusesAnInputConnectionCommitOnceAnAccessibilityEditorAppears() {
        val input = FakeInputConnectionEditor("input", EditorSnapshot("Hi", 2, 2))
        inputConnectionEditor = input
        val session = requireNotNull(controller.begin())

        val node = FakeEditableNode("node", EditorSnapshot("Hi", 2, 2))
        accessibilityEditor = node
        assertEquals(EditorCommitResult.FocusChanged, controller.commit(session, "there"))
        assertNull(input.committedText)
        assertNull(node.setTextValue)
    }

    @Test
    fun refusesAnAccessibilitySessionWhenOnlyTheInputConnectionRemains() {
        val node = FakeEditableNode("node", EditorSnapshot("Hi", 2, 2))
        val input = FakeInputConnectionEditor("input", EditorSnapshot("Hi", 2, 2))
        accessibilityEditor = node
        inputConnectionEditor = input
        val session = requireNotNull(controller.begin())

        accessibilityEditor = null
        assertEquals(EditorCommitResult.FocusChanged, controller.commit(session, "there"))
        assertNull(input.committedText)
    }

    @Test
    fun refusesAnInputConnectionWithTheSameIdAsTheCapturedAccessibilityEditor() {
        val node = FakeEditableNode("same", EditorSnapshot("Hi", 2, 2))
        val input = FakeInputConnectionEditor("same", EditorSnapshot("Hi", 2, 2))
        accessibilityEditor = node
        inputConnectionEditor = input
        val session = requireNotNull(controller.begin())

        accessibilityEditor = null
        assertEquals(EditorCommitResult.FocusChanged, controller.commit(session, "there"))
        assertNull(input.committedText)
        assertNull(node.setTextValue)
    }

    @Test
    fun refusesAnAccessibilityEditorWithTheSameIdAsTheCapturedInputConnection() {
        val input = FakeInputConnectionEditor("same", EditorSnapshot("Hi", 2, 2))
        inputConnectionEditor = input
        val session = requireNotNull(controller.begin())

        val node = FakeEditableNode("same", EditorSnapshot("Hi", 2, 2))
        accessibilityEditor = node
        assertEquals(EditorCommitResult.FocusChanged, controller.commit(session, "there"))
        assertNull(input.committedText)
        assertNull(node.setTextValue)
    }

    @Test
    fun looksUpEachChannelAtMostOnceAtStart() {
        accessibilityEditor = FakeEditableNode("node", EditorSnapshot("Hi", 2, 2))
        inputConnectionEditor = FakeInputConnectionEditor("input", EditorSnapshot("Hi", 2, 2))
        requireNotNull(controller.begin())
        assertEquals(1, accessibilityLookups)
        assertEquals(0, inputConnectionLookups)

        accessibilityEditor = null
        requireNotNull(controller.begin())
        assertEquals(2, accessibilityLookups)
        assertEquals(1, inputConnectionLookups)
    }
}

private class FakeEditableNode(
    override val stableId: String,
    private val value: EditorSnapshot,
    private val acceptSetText: Boolean = true,
) : EditableNode {
    var setTextValue: String? = null
    var selectionValue: Int? = null

    override val availability: EditorAvailability
        get() = EditorAvailability(value.isPassword, value.isSensitive)

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

private class FakeInputConnectionEditor(
    override val stableId: String,
    private val value: EditorSnapshot,
    override val availability: EditorAvailability = EditorAvailability(),
) : InputConnectionEditor {
    var committedText: String? = null

    override fun surroundingText(): EditorSnapshot = value

    override fun commitText(text: String): Boolean {
        committedText = text
        return true
    }
}
