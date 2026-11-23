package com.openwhisper.android.audio

import com.openwhisper.android.recordings.StoredRecording

interface PcmRecorder {
    /** Starts a recording; only the returned handle stops or cancels it. */
    fun start(): ActiveRecording
}

/** One recording in progress. Whichever of [stop] or [cancel] comes first ends it. */
interface ActiveRecording {
    /** Stops and saves this recording; throws if it has already ended. */
    fun stop(): StoredRecording

    /** Discards this recording; does nothing if it has already ended. */
    fun cancel()
}
