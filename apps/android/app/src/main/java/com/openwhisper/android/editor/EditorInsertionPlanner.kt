package com.openwhisper.android.editor

data class EditorSnapshot(
    val text: String,
    val selectionStart: Int,
    val selectionEnd: Int,
    val isPassword: Boolean = false,
    val isSensitive: Boolean = false,
)

enum class EditorRejection {
    PASSWORD,
    SENSITIVE,
}

sealed interface InsertionPlan {
    data class SetText(val text: String, val cursor: Int) : InsertionPlan
    data class Rejected(val reason: EditorRejection) : InsertionPlan
    data object NoOp : InsertionPlan
}

class EditorInsertionPlanner {
    fun plan(snapshot: EditorSnapshot, transcript: String): InsertionPlan {
        if (snapshot.isPassword) return InsertionPlan.Rejected(EditorRejection.PASSWORD)
        if (snapshot.isSensitive) return InsertionPlan.Rejected(EditorRejection.SENSITIVE)
        if (transcript.isBlank()) return InsertionPlan.NoOp

        val selectionIsValid = snapshot.selectionStart in 0..snapshot.text.length &&
            snapshot.selectionEnd in 0..snapshot.text.length
        val start = if (selectionIsValid) {
            minOf(snapshot.selectionStart, snapshot.selectionEnd)
        } else {
            snapshot.text.length
        }
        val end = if (selectionIsValid) {
            maxOf(snapshot.selectionStart, snapshot.selectionEnd)
        } else {
            snapshot.text.length
        }
        val prefix = snapshot.text.substring(0, start)
        val suffix = snapshot.text.substring(end)
        var insertion = transcript.trim()
        if (prefix.lastOrNull()?.isLetterOrDigit() == true &&
            insertion.firstOrNull()?.isLetterOrDigit() == true
        ) {
            insertion = " $insertion"
        }
        if (insertion.lastOrNull()?.isLetterOrDigit() == true &&
            suffix.firstOrNull()?.isLetterOrDigit() == true
        ) {
            insertion += " "
        }
        val updated = prefix + insertion + suffix
        return InsertionPlan.SetText(updated, cursor = start + insertion.length)
    }
}
