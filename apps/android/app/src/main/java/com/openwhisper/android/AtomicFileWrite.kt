package com.openwhisper.android

import androidx.core.util.AtomicFile
import java.io.IOException

/**
 * Replaces the file with [bytes] in one step: a failed write leaves the old
 * file as it was and is rethrown.
 */
internal fun AtomicFile.writeAll(bytes: ByteArray) {
    val output = startWrite()
    try {
        output.write(bytes)
        // finishWrite only logs a failed sync or rename, so both are checked here.
        output.fd.sync()
        finishWrite(output)
        if (!readFully().contentEquals(bytes)) throw IOException("Could not save $baseFile")
    } catch (failure: Throwable) {
        failWrite(output)
        throw failure
    }
}
