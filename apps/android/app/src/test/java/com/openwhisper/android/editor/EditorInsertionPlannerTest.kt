package com.openwhisper.android.editor

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class EditorInsertionPlannerTest {
    private val planner = EditorInsertionPlanner()

    @Test
    fun insertsAtCollapsedCursorWithoutDestroyingExistingText() {
        val result = planner.plan(
            EditorSnapshot(text = "Hello world", selectionStart = 5, selectionEnd = 5),
            transcript = " brave new",
        )

        assertEquals(InsertionPlan.SetText("Hello brave new world", cursor = 15), result)
    }

    @Test
    fun replacesOnlyTheSelectedRange() {
        val result = planner.plan(
            EditorSnapshot(text = "Hello old world", selectionStart = 6, selectionEnd = 9),
            transcript = "new",
        )

        assertEquals(InsertionPlan.SetText("Hello new world", cursor = 9), result)
    }

    @Test
    fun addsNaturalSpacesAtWordBoundaries() {
        assertEquals(
            InsertionPlan.SetText("Hello dictated text", cursor = 19),
            planner.plan(EditorSnapshot("Hello", 5, 5), "dictated text"),
        )
        assertEquals(
            InsertionPlan.SetText("Hello brave world", cursor = 12),
            planner.plan(EditorSnapshot("Hello world", 6, 6), "brave"),
        )
    }

    @Test
    fun doesNotAddSpaceBeforeExistingPunctuation() {
        assertEquals(
            InsertionPlan.SetText("Hello world.", cursor = 11),
            planner.plan(EditorSnapshot("Hello.", 5, 5), "world"),
        )
    }

    @Test
    fun clampsBrokenSelectionMetadataFromThirdPartyEditors() {
        val result = planner.plan(
            EditorSnapshot(text = "Hi", selectionStart = -1, selectionEnd = 99),
            transcript = " there",
        )

        assertEquals(InsertionPlan.SetText("Hi there", cursor = 8), result)
    }

    @Test
    fun rejectsPasswordFields() {
        val result = planner.plan(
            EditorSnapshot(text = "secret", selectionStart = 6, selectionEnd = 6, isPassword = true),
            transcript = "do not insert",
        )

        assertEquals(InsertionPlan.Rejected(EditorRejection.PASSWORD), result)
    }

    @Test
    fun rejectsPlatformSensitiveFields() {
        val result = planner.plan(
            EditorSnapshot(text = "", selectionStart = 0, selectionEnd = 0, isSensitive = true),
            transcript = "do not insert",
        )

        assertEquals(InsertionPlan.Rejected(EditorRejection.SENSITIVE), result)
    }

    @Test
    fun emptyTranscriptsAreNoOps() {
        val result = planner.plan(EditorSnapshot("Hello", 5, 5), "  ")
        assertTrue(result is InsertionPlan.NoOp)
    }
}
