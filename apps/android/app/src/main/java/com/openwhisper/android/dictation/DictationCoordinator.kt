package com.openwhisper.android.dictation

import com.openwhisper.android.editor.EditorCommitResult
import com.openwhisper.android.editor.EditorSession
import com.openwhisper.android.editor.EditorSessionPort

interface DictationBackend {
    interface Listener {
        fun onFinal(text: String)
        fun onError(message: String)
    }

    fun start(listener: Listener)
    fun finish()
    fun cancel()
}

/** [selectBackend] picks the backend at each start; that recording keeps it until it ends. */
class DictationCoordinator(
    private val selectBackend: () -> DictationBackend,
    private val editor: EditorSessionPort,
    private val render: (DictationState) -> Unit,
    private val onTranscriptReady: (String) -> Unit = {},
) : DictationBackend.Listener {
    private var state: DictationState = DictationState.Idle
    private var session: EditorSession? = null
    private var backend: DictationBackend? = null

    val currentState: DictationState
        get() = state

    fun onMicPressed() {
        when (state) {
            DictationState.Idle,
            is DictationState.Failed,
            -> start()
            DictationState.Listening -> finish()
            DictationState.Finalizing -> Unit
        }
    }

    fun cancel() {
        if (state !is DictationState.Idle) {
            backend?.cancel()
            backend = null
        }
        session = null
        transition(DictationState.Idle)
    }

    override fun onFinal(text: String) {
        if (state !is DictationState.Finalizing) return
        onTranscriptReady(text)
        // The transcript callback may have cancelled the session; then nothing is inserted.
        if (state !is DictationState.Finalizing) return
        val captured = session ?: return onError("Captured editor session was lost")
        val failure = when (editor.commit(captured, text)) {
            EditorCommitResult.Committed -> null
            EditorCommitResult.FocusChanged -> "Focused field changed; transcript was not inserted"
            EditorCommitResult.ContentChanged -> "Text changed while recording; transcript was not inserted"
            EditorCommitResult.Unsupported -> "This field does not support safe text insertion"
        }
        if (failure != null) return onError(failure)
        session = null
        transition(DictationState.Idle)
    }

    override fun onError(message: String) {
        session = null
        transition(DictationState.Failed(message))
    }

    private fun start() {
        val captured = editor.begin()
        if (captured == null) {
            transition(DictationState.Failed("No safe editable field is focused"))
            return
        }
        session = captured
        transition(DictationState.Listening)
        backend?.cancel()
        backend = selectBackend().also { it.start(this) }
    }

    private fun finish() {
        transition(DictationState.Finalizing)
        backend?.finish()
    }

    private fun transition(next: DictationState) {
        state = next
        render(next)
    }
}
