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
        val coordinator = DictationCoordinator(backend, editor, states::add)

        coordinator.onMicPressed()
        assertEquals(1, backend.startCount)
        assertEquals(DictationState.Listening(""), states.last())
        assertEquals(DictationState.Listening(""), coordinator.currentState)

        backend.listener?.onPartial("Hello")
        assertEquals(DictationState.Listening("Hello"), states.last())

        coordinator.onMicPressed()
        assertEquals(1, backend.finishCount)
        assertEquals(DictationState.Finalizing("Hello"), states.last())

        backend.listener?.onFinal("Hello world")
        assertEquals("Hello world", editor.committedText)
        assertEquals(DictationState.Idle, states.last())
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
        val coordinator = DictationCoordinator(backend, editor, states::add)

        coordinator.onMicPressed()
        coordinator.onMicPressed()
        backend.listener?.onFinal("private words")

        assertEquals(1, editor.commitCount)
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
