package com.openwhisper.android.audio

typealias PcmRecording = com.openwhisper.android.recordings.PcmRecording

interface PcmRecorder {
    /** Starts a recording; only the returned handle stops or cancels it. */
    fun start(): ActiveRecording
}

/** One recording in progress. Whichever of [stop] or [cancel] comes first ends it. */
interface ActiveRecording {
    /** Stops and saves this recording; throws if it has already ended. */
    fun stop(): PcmRecording

    /** Discards this recording; does nothing if it has already ended. */
    fun cancel()
}
