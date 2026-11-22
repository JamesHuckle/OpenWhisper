package com.openwhisper.android.dictation

import com.openwhisper.android.editor.EditorCommitResult
import com.openwhisper.android.editor.EditorSession
import com.openwhisper.android.editor.EditorSessionPort
import org.junit.Assert.assertEquals
import org.junit.Test

class DictationCoordinatorTest {
    @Test
    fun recordsFinalizesAndCommitsThroughInjectedPorts() {
        val backend = FakeDictationBackend()
        val editor = FakeEditorSessionPort()
        val states = mutableListOf<DictationState>()
        val copiedTranscripts = mutableListOf<String>()
        val coordinator = DictationCoordinator(
            backend,
            editor,
            states::add,
            copiedTranscripts::add,
        )

        coordinator.onMicPressed()
        assertEquals(1, backend.startCount)
        assertEquals(DictationState.Listening, states.last())
        assertEquals(DictationState.Listening, coordinator.currentState)

        coordinator.onMicPressed()
        assertEquals(1, backend.finishCount)
        assertEquals(DictationState.Finalizing, states.last())

        // Pressing again while finalizing changes nothing.
        coordinator.onMicPressed()
        assertEquals(1, backend.finishCount)
        assertEquals(DictationState.Finalizing, coordinator.currentState)

        backend.listener?.onFinal("Hello world")
        assertEquals("Hello world", editor.committedText)
        assertEquals(listOf("Hello world"), copiedTranscripts)
        assertEquals(
            listOf(
                DictationState.Listening,
                DictationState.Finalizing,
                DictationState.ReadyToInsert,
                DictationState.Idle,
            ),
            states,
        )
    }

    @Test
    fun doesNotRecordWithoutSafeEditorSession() {
        val backend = FakeDictationBackend()
        val editor = FakeEditorSessionPort(session = null)
        val states = mutableListOf<DictationState>()
        val coordinator = DictationCoordinator(backend, editor, states::add)

        coordinator.onMicPressed()

        assertEquals(0, backend.startCount)
        assertEquals(DictationState.Failed("No safe editable field is focused"), states.last())
    }

    @Test
    fun focusChangeAtCommitIsReportedAndTranscriptIsNotRetriedElsewhere() {
        val backend = FakeDictationBackend()
        val editor = FakeEditorSessionPort(commitResult = EditorCommitResult.FocusChanged)
        val states = mutableListOf<DictationState>()
        val copiedTranscripts = mutableListOf<String>()
        val coordinator = DictationCoordinator(
            backend,
            editor,
            states::add,
            copiedTranscripts::add,
        )

        coordinator.onMicPressed()
        coordinator.onMicPressed()
        backend.listener?.onFinal("private words")

        assertEquals(1, editor.commitCount)
        assertEquals(listOf("private words"), copiedTranscripts)
        assertEquals(DictationState.Failed("Focused field changed; transcript was not inserted"), states.last())
    }

    @Test
    fun cancellationStopsBackendAndClearsCapturedSession() {
        val backend = FakeDictationBackend()
        val editor = FakeEditorSessionPort()
        val states = mutableListOf<DictationState>()
        val coordinator = DictationCoordinator(backend, editor, states::add)

        coordinator.onMicPressed()
        coordinator.cancel()

        assertEquals(1, backend.cancelCount)
        assertEquals(DictationState.Idle, states.last())
        assertEquals(DictationState.Idle, coordinator.currentState)
    }

    @Test
    fun cancellationReturnsToIdleFromFinalizingAndFailed() {
        val backend = FakeDictationBackend()
        val editor = FakeEditorSessionPort()
        val coordinator = DictationCoordinator(backend, editor, {})

        coordinator.onMicPressed()
        coordinator.onMicPressed()
        assertEquals(DictationState.Finalizing, coordinator.currentState)
        coordinator.cancel()
        assertEquals(DictationState.Idle, coordinator.currentState)

        coordinator.onMicPressed()
        backend.listener?.onError("network")
        assertEquals(DictationState.Failed("network"), coordinator.currentState)
        coordinator.cancel()
        assertEquals(DictationState.Idle, coordinator.currentState)
        assertEquals(0, editor.commitCount)
    }

    @Test
    fun cancellingWhileReadyToInsertDoesNotCommit() {
        val backend = FakeDictationBackend()
        val editor = FakeEditorSessionPort()
        val states = mutableListOf<DictationState>()
        lateinit var coordinator: DictationCoordinator
        coordinator = DictationCoordinator(backend, editor, { state ->
            states += state
            if (state is DictationState.ReadyToInsert) coordinator.cancel()
        })

        coordinator.onMicPressed()
        coordinator.onMicPressed()
        backend.listener?.onFinal("cancelled words")

        assertEquals(0, editor.commitCount)
        assertEquals(DictationState.Idle, states.last())
        assertEquals(DictationState.Idle, coordinator.currentState)
    }

    @Test
    fun lateFinalTranscriptCannotCorruptIdleState() {
        val backend = FakeDictationBackend()
        val editor = FakeEditorSessionPort()
        val states = mutableListOf<DictationState>()
        val copiedTranscripts = mutableListOf<String>()
        val coordinator = DictationCoordinator(
            backend,
            editor,
            states::add,
            copiedTranscripts::add,
        )

        coordinator.onMicPressed()
        coordinator.onMicPressed()
        val staleListener = backend.listener
        coordinator.cancel()
        staleListener?.onFinal("stale")

        assertEquals(DictationState.Idle, states.last())
        assertEquals(DictationState.Idle, coordinator.currentState)
        assertEquals(0, editor.commitCount)
        assertEquals(emptyList<String>(), copiedTranscripts)
    }

    @Test
    fun errorsBecomeRecoverableFailures() {
        val backend = FakeDictationBackend()
        val editor = FakeEditorSessionPort()
        val states = mutableListOf<DictationState>()
        val coordinator = DictationCoordinator(backend, editor, states::add)

        coordinator.onMicPressed()
        backend.listener?.onError("Microphone unavailable")
        assertEquals(DictationState.Failed("Microphone unavailable"), states.last())

        coordinator.onMicPressed()
        assertEquals(2, backend.startCount)
        assertEquals(DictationState.Listening, states.last())
    }
}

private class FakeDictationBackend : DictationBackend {
    var startCount = 0
    var finishCount = 0
    var cancelCount = 0
    var listener: DictationBackend.Listener? = null

    override fun start(listener: DictationBackend.Listener) {
        startCount += 1
        this.listener = listener
    }

    override fun finish() {
        finishCount += 1
    }

    override fun cancel() {
        cancelCount += 1
    }
}

private class FakeEditorSessionPort(
    private val session: EditorSession? = EditorSession("field-1"),
    private val commitResult: EditorCommitResult = EditorCommitResult.Committed,
) : EditorSessionPort {
    var committedText: String? = null
    var commitCount = 0

    override fun begin(): EditorSession? = session

    override fun commit(session: EditorSession, transcript: String): EditorCommitResult {
        commitCount += 1
        committedText = transcript
        return commitResult
    }
}
