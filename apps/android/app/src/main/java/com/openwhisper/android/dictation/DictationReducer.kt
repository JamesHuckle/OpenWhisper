package com.openwhisper.android.dictation

sealed interface DictationState {
    data object Idle : DictationState
    data class Listening(val partialText: String) : DictationState
    data class Finalizing(val partialText: String) : DictationState
    data class ReadyToInsert(val transcript: String) : DictationState
    data class Failed(val message: String) : DictationState
}

sealed interface DictationEvent {
    data object MicPressed : DictationEvent
    data class PartialTranscript(val text: String) : DictationEvent
    data class FinalTranscript(val text: String) : DictationEvent
    data object Inserted : DictationEvent
    data object Cancelled : DictationEvent
    data class Failed(val message: String) : DictationEvent
}

object DictationReducer {
    fun reduce(state: DictationState, event: DictationEvent): DictationState = when (event) {
        DictationEvent.Cancelled -> DictationState.Idle
        DictationEvent.MicPressed -> when (state) {
            DictationState.Idle,
            is DictationState.Failed,
            -> DictationState.Listening(partialText = "")
            is DictationState.Listening -> DictationState.Finalizing(state.partialText)
            is DictationState.Finalizing,
            is DictationState.ReadyToInsert,
            -> state
        }
        is DictationEvent.PartialTranscript -> when (state) {
            is DictationState.Listening -> state.copy(partialText = event.text)
            else -> state
        }
        is DictationEvent.FinalTranscript -> when (state) {
            is DictationState.Finalizing -> DictationState.ReadyToInsert(event.text)
            else -> state
        }
        DictationEvent.Inserted -> when (state) {
            is DictationState.ReadyToInsert -> DictationState.Idle
            else -> state
        }
        is DictationEvent.Failed -> when (state) {
            DictationState.Idle -> state
            else -> DictationState.Failed(event.message)
        }
    }
}
