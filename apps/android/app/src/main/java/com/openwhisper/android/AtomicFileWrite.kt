package com.openwhisper.android

import androidx.core.util.AtomicFile

/**
 * Replaces the file with [bytes] in one step: a failed write leaves the old
 * file as it was and is rethrown.
 */
internal fun AtomicFile.writeAll(bytes: ByteArray) {
    val output = startWrite()
    try {
        output.write(bytes)
        finishWrite(output)
    } catch (failure: Throwable) {
        failWrite(output)
        throw failure
    }
}
