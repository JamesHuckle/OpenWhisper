package com.openwhisper.android

import androidx.core.util.AtomicFile
import java.io.File
import java.io.IOException
import java.nio.file.Files
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class AtomicFileWriteTest {
    @Test
    fun aWriteReplacesTheFile() {
        val folder = Files.createTempDirectory("atomic-write").toFile()
        try {
            val file = AtomicFile(File(folder, "details.json"))
            file.writeAll("old".encodeToByteArray())
            file.writeAll("new".encodeToByteArray())

            assertArrayEquals("new".encodeToByteArray(), file.readFully())
        } finally {
            folder.deleteRecursively()
        }
    }

    @Test
    fun aRenameThatFailsIsRethrown() {
        val folder = Files.createTempDirectory("atomic-write").toFile()
        try {
            // A folder that is not empty cannot be replaced by the new file.
            val target = File(folder, "details.json").apply { mkdirs() }
            File(target, "inside").writeText("kept")

            assertThrows(IOException::class.java) {
                AtomicFile(target).writeAll("new".encodeToByteArray())
            }
        } finally {
            folder.deleteRecursively()
        }
    }
}
