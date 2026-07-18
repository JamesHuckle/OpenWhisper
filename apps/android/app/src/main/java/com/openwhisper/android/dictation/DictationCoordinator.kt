package com.openwhisper.android.dictation

import com.openwhisper.android.editor.EditorCommitResult
import com.openwhisper.android.editor.EditorSession
import com.openwhisper.android.editor.EditorSessionPort

interface DictationBackend {
    interface Listener {
        fun onPartial(text: String)
        fun onFinal(text: String)
        fun onError(message: String)
    }

    fun start(listener: Listener)
    fun finish()
    fun cancel()
}

class DictationCoordinator(
    private val backend: DictationBackend,
    private val editor: EditorSessionPort,
    private val render: (DictationState) -> Unit,
) : DictationBackend.Listener {
    private var state: DictationState = DictationState.Idle
    private var session: EditorSession? = null

    val currentState: DictationState
        get() = state

    fun onMicPressed() {
        when (state) {
            DictationState.Idle,
            is DictationState.Failed,
            -> start()
            is DictationState.Listening -> finish()
            is DictationState.Finalizing,
            is DictationState.ReadyToInsert,
            -> Unit
        }
    }

    fun cancel() {
        if (state !is DictationState.Idle) backend.cancel()
        session = null
        transition(DictationState.Idle)
    }

    override fun onPartial(text: String) {
        val listening = state as? DictationState.Listening ?: return
        transition(listening.copy(partialText = text))
    }

    override fun onFinal(text: String) {
        if (state !is DictationState.Finalizing) return
        transition(DictationState.ReadyToInsert(text))
        val captured = session
        if (captured == null) {
            transition(DictationState.Failed("Captured editor session was lost"))
            return
        }
        when (editor.commit(captured, text)) {
            EditorCommitResult.Committed -> {
                session = null
                transition(DictationState.Idle)
            }
            EditorCommitResult.FocusChanged -> failCommit(
                "Focused field changed; transcript was not inserted",
            )
            EditorCommitResult.ContentChanged -> failCommit(
                "Text changed while recording; transcript was not inserted",
            )
            EditorCommitResult.Unsupported -> failCommit(
                "This field does not support safe text insertion",
            )
        }
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
        transition(DictationState.Listening(partialText = ""))
        backend.start(this)
    }

    private fun finish() {
        val listening = state as DictationState.Listening
        transition(DictationState.Finalizing(listening.partialText))
        backend.finish()
    }

    private fun failCommit(message: String) {
        session = null
        transition(DictationState.Failed(message))
    }

    private fun transition(next: DictationState) {
        state = next
        render(next)
    }
}
