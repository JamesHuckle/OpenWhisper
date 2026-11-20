package com.openwhisper.android.editor

data class EditorAvailability(
    val isPassword: Boolean = false,
    val isSensitive: Boolean = false,
)

/** A focused field dictation can read and write, through either channel. */
interface DictationField {
    val stableId: String

    /** The field's password and sensitive flags, read without copying its text. */
    val availability: EditorAvailability

    /** The field's text and selection, or null when it cannot be read safely. */
    fun snapshot(): EditorSnapshot?

    /** Writes [plan], made from [captured], into the field. */
    fun write(captured: EditorSnapshot, plan: InsertionPlan.SetText): Boolean
}

/** A field found in the accessibility tree: it replaces the whole text, then moves the cursor. */
interface EditableNode : DictationField {
    override fun snapshot(): EditorSnapshot
    fun setText(text: String): Boolean
    fun setSelection(cursor: Int): Boolean

    override fun write(captured: EditorSnapshot, plan: InsertionPlan.SetText): Boolean {
        if (!setText(plan.text)) return false
        setSelection(plan.cursor)
        return true
    }
}

/** A field reached through the accessibility input method: it commits only the new text. */
interface InputConnectionEditor : DictationField {
    /** The text around the cursor, or null when the connection returns none. */
    fun surroundingText(): EditorSnapshot?
    fun commitText(text: String): Boolean

    /** Null when the selection is outside the text: commitText could not place the insertion. */
    override fun snapshot(): EditorSnapshot? = surroundingText()?.takeIf {
        it.selectionStart in 0..it.text.length && it.selectionEnd in 0..it.text.length
    }

    override fun write(captured: EditorSnapshot, plan: InsertionPlan.SetText): Boolean {
        val start = minOf(captured.selectionStart, captured.selectionEnd)
        return commitText(plan.text.substring(start, plan.cursor))
    }
}

fun interface FocusedEditorProvider {
    fun focusedEditor(): EditableNode?
}

fun interface InputConnectionEditorProvider {
    fun currentEditor(): InputConnectionEditor?
}

data class EditorSession(
    val stableId: String,
    val snapshot: EditorSnapshot? = null,
    val channel: EditorSessionChannel = EditorSessionChannel.Accessibility,
)

enum class EditorSessionChannel {
    Accessibility,
    InputConnection,
}

enum class EditorCommitResult {
    Committed,
    FocusChanged,
    ContentChanged,
    Unsupported,
}

interface EditorSessionPort {
    fun begin(): EditorSession?
    fun commit(session: EditorSession, transcript: String): EditorCommitResult
}

/**
 * Captures the focused field when dictation starts and writes the transcript
 * into it when dictation ends. The accessibility editor wins; the input
 * connection is used only when the accessibility tree has no usable editor.
 */
class EditorSessionController(
    private val accessibility: FocusedEditorProvider,
    private val inputConnection: InputConnectionEditorProvider = InputConnectionEditorProvider { null },
    private val planner: EditorInsertionPlanner = EditorInsertionPlanner(),
) : EditorSessionPort {
    override fun begin(): EditorSession? {
        val accessibilityEditor = accessibility.focusedEditor()
        val channel = if (accessibilityEditor != null) {
            EditorSessionChannel.Accessibility
        } else {
            EditorSessionChannel.InputConnection
        }
        val field = accessibilityEditor ?: inputConnection.currentEditor() ?: return null
        if (field.availability.isPassword || field.availability.isSensitive) return null
        val snapshot = field.snapshot() ?: return null
        if (snapshot.isPassword || snapshot.isSensitive) return null
        return EditorSession(field.stableId, snapshot, channel)
    }

    override fun commit(session: EditorSession, transcript: String): EditorCommitResult {
        val accessibilityEditor = accessibility.focusedEditor()
        val field = when (session.channel) {
            EditorSessionChannel.Accessibility -> accessibilityEditor
            // An accessibility editor that appeared since the start takes over the input.
            EditorSessionChannel.InputConnection ->
                if (accessibilityEditor == null) inputConnection.currentEditor() else null
        } ?: return EditorCommitResult.FocusChanged
        if (field.stableId != session.stableId) return EditorCommitResult.FocusChanged
        val captured = session.snapshot ?: return EditorCommitResult.Unsupported
        val current = field.snapshot() ?: return EditorCommitResult.Unsupported
        if (current != captured) return EditorCommitResult.ContentChanged

        return when (val plan = planner.plan(captured, transcript)) {
            is InsertionPlan.SetText ->
                if (field.write(captured, plan)) {
                    EditorCommitResult.Committed
                } else {
                    EditorCommitResult.Unsupported
                }
            is InsertionPlan.NoOp -> EditorCommitResult.Committed
        }
    }
}
