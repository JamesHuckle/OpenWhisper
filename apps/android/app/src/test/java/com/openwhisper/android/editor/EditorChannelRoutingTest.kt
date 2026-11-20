package com.openwhisper.android.editor

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Pins how the service picks between the accessibility editor and the input connection. */
class EditorChannelRoutingTest {
    private var accessibilityEditor: EditableNode? = null
    private var inputConnectionEditor: InputConnectionEditor? = null

    private fun controller(): EditorSessionPort {
        val accessibility = EditorSessionController({ accessibilityEditor })
        return InputConnectionEditorSessionController(
            provider = { if (accessibilityEditor == null) inputConnectionEditor else null },
        ).let { RoutingEditorSessionPort(it, accessibility) }
    }

    @Test
    fun prefersTheAccessibilityEditorWhenBothAreAvailable() {
        val node = PinnedEditableNode("node", EditorSnapshot("Hi", 2, 2))
        val input = PinnedInputConnectionEditor("input", EditorSnapshot("Hi", 2, 2))
        accessibilityEditor = node
        inputConnectionEditor = input
        val controller = controller()

        val session = requireNotNull(controller.begin())
        assertEquals(EditorSessionChannel.Accessibility, session.channel)
        assertEquals(EditorCommitResult.Committed, controller.commit(session, "there"))
        assertEquals("Hi there", node.setTextValue)
        assertNull(input.committedText)
    }

    @Test
    fun usesTheInputConnectionOnlyWithoutAnAccessibilityEditor() {
        val input = PinnedInputConnectionEditor("input", EditorSnapshot("Hi", 2, 2))
        inputConnectionEditor = input
        val controller = controller()

        val session = requireNotNull(controller.begin())
        assertEquals(EditorSessionChannel.InputConnection, session.channel)
        assertEquals(EditorCommitResult.Committed, controller.commit(session, "there"))
        assertEquals(" there", input.committedText)
    }

    @Test
    fun refusesAnInputConnectionCommitOnceAnAccessibilityEditorAppears() {
        val input = PinnedInputConnectionEditor("input", EditorSnapshot("Hi", 2, 2))
        inputConnectionEditor = input
        val controller = controller()
        val session = requireNotNull(controller.begin())

        val node = PinnedEditableNode("node", EditorSnapshot("Hi", 2, 2))
        accessibilityEditor = node
        assertEquals(EditorCommitResult.FocusChanged, controller.commit(session, "there"))
        assertNull(input.committedText)
        assertNull(node.setTextValue)
    }

    @Test
    fun refusesAnAccessibilitySessionWhenOnlyTheInputConnectionRemains() {
        val node = PinnedEditableNode("node", EditorSnapshot("Hi", 2, 2))
        val input = PinnedInputConnectionEditor("input", EditorSnapshot("Hi", 2, 2))
        accessibilityEditor = node
        inputConnectionEditor = input
        val controller = controller()
        val session = requireNotNull(controller.begin())

        accessibilityEditor = null
        assertEquals(EditorCommitResult.FocusChanged, controller.commit(session, "there"))
        assertNull(input.committedText)
    }
}

private class PinnedEditableNode(
    override val stableId: String,
    private val value: EditorSnapshot,
) : EditableNode {
    var setTextValue: String? = null

    override val availability: EditorAvailability
        get() = EditorAvailability(value.isPassword, value.isSensitive)

    override fun snapshot(): EditorSnapshot = value

    override fun setText(text: String): Boolean {
        setTextValue = text
        return true
    }

    override fun setSelection(cursor: Int): Boolean = true
}

private class PinnedInputConnectionEditor(
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
