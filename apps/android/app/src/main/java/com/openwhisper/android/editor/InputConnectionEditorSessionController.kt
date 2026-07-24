package com.openwhisper.android.editor

data class EditorAvailability(
    val isPassword: Boolean = false,
    val isSensitive: Boolean = false,
)

interface InputConnectionEditor {
    val stableId: String
    val availability: EditorAvailability
    fun snapshot(): EditorSnapshot?
    fun commitText(text: String): Boolean
}

fun interface InputConnectionEditorProvider {
    fun currentEditor(): InputConnectionEditor?
}

class InputConnectionEditorSessionController(
    private val provider: InputConnectionEditorProvider,
    private val planner: EditorInsertionPlanner = EditorInsertionPlanner(),
) : EditorSessionPort {
    fun currentAvailability(): EditorAvailability? =
        provider.currentEditor()?.availability

    override fun begin(): EditorSession? {
        val editor = provider.currentEditor() ?: return null
        if (editor.availability.isPassword || editor.availability.isSensitive) return null
        val snapshot = editor.snapshot() ?: return null
        if (snapshot.isPassword || snapshot.isSensitive || !snapshot.hasValidSelection()) return null
        return EditorSession(
            stableId = editor.stableId,
            snapshot = snapshot,
            channel = EditorSessionChannel.InputConnection,
        )
    }

    override fun commit(session: EditorSession, transcript: String): EditorCommitResult {
        if (session.channel != EditorSessionChannel.InputConnection) {
            return EditorCommitResult.Unsupported
        }
        val editor = provider.currentEditor() ?: return EditorCommitResult.FocusChanged
        if (editor.stableId != session.stableId) return EditorCommitResult.FocusChanged
        val captured = session.snapshot ?: return EditorCommitResult.Unsupported
        val current = editor.snapshot() ?: return EditorCommitResult.Unsupported
        if (current != captured) return EditorCommitResult.ContentChanged

        return when (val plan = planner.plan(captured, transcript)) {
            is InsertionPlan.SetText -> {
                val start = minOf(captured.selectionStart, captured.selectionEnd)
                val insertion = plan.text.substring(start, plan.cursor)
                if (editor.commitText(insertion)) {
                    EditorCommitResult.Committed
                } else {
                    EditorCommitResult.Unsupported
                }
            }
            is InsertionPlan.NoOp -> EditorCommitResult.Committed
            is InsertionPlan.Rejected -> EditorCommitResult.Unsupported
        }
    }

    private fun EditorSnapshot.hasValidSelection(): Boolean =
        selectionStart in 0..text.length && selectionEnd in 0..text.length
}
