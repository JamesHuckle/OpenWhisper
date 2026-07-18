package com.openwhisper.android.dictation

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class DictationReducerTest {
    @Test
    fun completeHappyPathIsDeterministic() {
        var state: DictationState = DictationState.Idle
        state = DictationReducer.reduce(state, DictationEvent.MicPressed)
        assertEquals(DictationState.Listening(partialText = ""), state)

        state = DictationReducer.reduce(state, DictationEvent.PartialTranscript("Hello"))
        assertEquals(DictationState.Listening(partialText = "Hello"), state)

        state = DictationReducer.reduce(state, DictationEvent.MicPressed)
        assertEquals(DictationState.Finalizing(partialText = "Hello"), state)

        state = DictationReducer.reduce(state, DictationEvent.FinalTranscript("Hello world."))
        assertEquals(DictationState.ReadyToInsert("Hello world."), state)

        state = DictationReducer.reduce(state, DictationEvent.Inserted)
        assertEquals(DictationState.Idle, state)
    }

    @Test
    fun cancellationAlwaysReturnsToIdle() {
        val states = listOf<DictationState>(
            DictationState.Listening("partial"),
            DictationState.Finalizing("partial"),
            DictationState.ReadyToInsert("final"),
            DictationState.Failed("network"),
        )

        states.forEach { state ->
            assertEquals(DictationState.Idle, DictationReducer.reduce(state, DictationEvent.Cancelled))
        }
    }

    @Test
    fun staleEventsCannotCorruptIdleState() {
        assertEquals(
            DictationState.Idle,
            DictationReducer.reduce(DictationState.Idle, DictationEvent.FinalTranscript("stale")),
        )
    }

    @Test
    fun errorsBecomeRecoverableFailures() {
        val failed = DictationReducer.reduce(
            DictationState.Listening(""),
            DictationEvent.Failed("Microphone unavailable"),
        )

        assertTrue(failed is DictationState.Failed)
        assertEquals(
            DictationState.Listening(""),
            DictationReducer.reduce(failed, DictationEvent.MicPressed),
        )
    }
}
