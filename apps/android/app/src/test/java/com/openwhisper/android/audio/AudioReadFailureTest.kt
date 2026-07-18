package com.openwhisper.android.audio

import android.media.AudioRecord
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AudioReadFailureTest {
    @Test
    fun readErrorAfterStopIsNormalShutdown() {
        assertNull(audioReadFailure(AudioRecord.ERROR_INVALID_OPERATION, isRecording = false))
    }

    @Test
    fun readErrorsWhileRecordingRemainActionable() {
        assertEquals(
            "Microphone disconnected while recording",
            audioReadFailure(AudioRecord.ERROR_DEAD_OBJECT, isRecording = true),
        )
        assertEquals(
            "Microphone read failed (${AudioRecord.ERROR_BAD_VALUE})",
            audioReadFailure(AudioRecord.ERROR_BAD_VALUE, isRecording = true),
        )
    }

    @Test
    fun successfulAndEmptyReadsAreNotFailures() {
        assertNull(audioReadFailure(512, isRecording = true))
        assertNull(audioReadFailure(0, isRecording = true))
    }
}
