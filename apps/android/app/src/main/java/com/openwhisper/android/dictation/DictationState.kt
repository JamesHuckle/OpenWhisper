package com.openwhisper.android.dictation

sealed interface DictationState {
    data object Idle : DictationState
    data object Listening : DictationState
    data object Finalizing : DictationState
    data object ReadyToInsert : DictationState
    data class Failed(val message: String) : DictationState
}
