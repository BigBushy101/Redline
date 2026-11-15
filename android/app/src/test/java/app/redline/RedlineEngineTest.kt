package app.redline

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

class RedlineEngineTest {
    private val own = listOf(File("/storage/emulated/0/Android/data/app.redline/files"), File("/data/user/0/app.redline/files"), null)

    @Test
    fun oldFoldersAreTheAppsOwn() {
        assertTrue(RedlineEngine.isOwnFolder("/storage/emulated/0/Android/data/app.redline/files/Download/Redline", own))
        assertTrue(RedlineEngine.isOwnFolder("/storage/emulated/0/Android/data/app.redline/files/Converted", own))
        assertTrue(RedlineEngine.isOwnFolder("/data/user/0/app.redline/files/Redline", own))
    }

    @Test
    fun otherFoldersAreNot() {
        assertFalse(RedlineEngine.isOwnFolder("/storage/emulated/0/Download/Redline", own))
        assertFalse(RedlineEngine.isOwnFolder("/storage/emulated/0/Android/data/app.redline/filesX", own))
        assertFalse(RedlineEngine.isOwnFolder(null, own))
    }
}
