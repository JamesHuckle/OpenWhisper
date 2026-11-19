package com.openwhisper.android.dictation

sealed interface DictationState {
    data object Idle : DictationState
    data class Listening(val partialText: String) : DictationState
    data class Finalizing(val partialText: String) : DictationState
    data class ReadyToInsert(val transcript: String) : DictationState
    data class Failed(val message: String) : DictationState
}
