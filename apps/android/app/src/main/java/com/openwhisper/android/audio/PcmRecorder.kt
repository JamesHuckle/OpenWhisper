package com.openwhisper.android.audio

data class PcmRecording(
    val bytes: ByteArray,
    val sampleRate: Int,
)

interface PcmRecorder {
    fun start()
    fun stop(): PcmRecording
    fun cancel()
}
