package com.openwhisper.android

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context

/** Puts [transcript] on the clipboard under OpenWhisper's transcript label. */
internal fun Context.copyTranscript(transcript: String) {
    getSystemService(ClipboardManager::class.java)
        .setPrimaryClip(ClipData.newPlainText(getString(R.string.clipboard_transcript_label), transcript))
}
