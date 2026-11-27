package com.openwhisper.android.transcription

import java.io.File
import java.nio.file.Files
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Test

class SavedPartsTest {
    @Test
    fun aSavedPartIsReadBackForTheSameKeyOnly() {
        val folder = Files.createTempDirectory("saved-parts").toFile()
        try {
            val file = File(folder, "audio.parts.json")
            SavedParts(file, "key").add(0, SavedPart(start = 0, end = 10, text = "Hello"))

            assertEquals("Hello", SavedParts(file, "key").get(0, 0, 10)?.text)
            assertNull(SavedParts(file, "key").get(0, 0, 11))
            assertNull(SavedParts(file, "other key").get(0, 0, 10))
        } finally {
            folder.deleteRecursively()
        }
    }

    @Test
    fun aPartThatCannotBeSavedIsKeptForThisRunAndSkippedOnDisk() {
        val folder = Files.createTempDirectory("saved-parts").toFile()
        try {
            // A regular file as its folder makes every save fail, on any system
            // and for any user.
            val notAFolder = File(folder, "recording").apply { writeText("") }
            val file = File(notAFolder, "audio.parts.json")
            val parts = SavedParts(file, "key")
            parts.add(0, SavedPart(start = 0, end = 10, text = "Hello"))

            assertEquals("Hello", parts.get(0, 0, 10)?.text)
            assertFalse(file.exists())
            assertNull(SavedParts(file, "key").get(0, 0, 10))
        } finally {
            folder.deleteRecursively()
        }
    }
}
