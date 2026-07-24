package com.openwhisper.android.accessibility

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class FocusedEditorSearchTest {
    @Test
    fun findsFocusedEditorNestedInAnEmbeddedHierarchy() {
        val editor = TestNode("editor", matches = true)
        val root = TestNode(
            "root",
            children = listOf(TestNode("toolbar"), TestNode("embedded", listOf(editor))),
        )

        val result = FocusedEditorSearch.find(
            roots = listOf(root),
            children = TestNode::children,
            matches = TestNode::matches,
            maxVisited = 100,
        )

        assertEquals(editor, result)
    }

    @Test
    fun stopsAtDefensiveTraversalLimit() {
        val editor = TestNode("editor", matches = true)
        val root = TestNode("root", listOf(TestNode("container", listOf(editor))))

        val result = FocusedEditorSearch.find(
            roots = listOf(root),
            children = TestNode::children,
            matches = TestNode::matches,
            maxVisited = 2,
        )

        assertNull(result)
    }

    @Test
    fun acceptsCustomSetTextEditorsButStillRequiresEnabledInputFocus() {
        assertTrue(
            EditorNodeEligibility.isUsable(
                isEnabled = true,
                isInputFocused = true,
                isEditable = false,
                supportsSetText = true,
            ),
        )
        assertFalse(
            EditorNodeEligibility.isUsable(
                isEnabled = false,
                isInputFocused = true,
                isEditable = true,
                supportsSetText = true,
            ),
        )
        assertFalse(
            EditorNodeEligibility.isUsable(
                isEnabled = true,
                isInputFocused = false,
                isEditable = true,
                supportsSetText = true,
            ),
        )
    }

    private data class TestNode(
        val name: String,
        val children: List<TestNode> = emptyList(),
        val matches: Boolean = false,
    )
}
