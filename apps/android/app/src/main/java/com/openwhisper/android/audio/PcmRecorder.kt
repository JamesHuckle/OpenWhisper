package com.openwhisper.android.audio

typealias PcmRecording = com.openwhisper.android.recordings.PcmRecording

interface PcmRecorder {
    fun start()
    fun stop(): PcmRecording
    fun cancel()
}
