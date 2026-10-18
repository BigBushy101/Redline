package app.redline.state

import app.redline.core.Music
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.File

/**
 * The copies of files picked for playlists (ffmpeg can't read a content:// link), or added on
 * the Convert page from somewhere Local can't read, in the app's own Music folder [dir]. A copy
 * stays while a playlist, Now playing or History uses it (the core knows which), or while
 * [keep] lists it (Local's added files). It's also *held* from the moment its name is claimed until its tracks are in
 * the playlist. [sweep] deletes the rest, and only ever files right in [dir].
 *
 * Claiming, releasing and sweeping take the same lock, so a sweep never sees a copy between its
 * creation and its hold, like [ConvertInputs].
 */
class ImportedMusic(val dir: File, private val keep: () -> Collection<String> = { emptyList() }) {
    private val lock = Mutex()
    private val held = mutableSetOf<String>()

    /** A new empty file in [dir] named [name], numbered if that's taken; held until [release]. */
    suspend fun newFile(name: String): File = lock.withLock {
        withContext(Dispatchers.IO) {
            dir.mkdirs()
            val base = name.substringBeforeLast('.', name)
            val ext = name.substringAfterLast('.', "").let { if (it.isEmpty()) "" else ".$it" }
            var file = File(dir, name)
            var n = 1
            // createNewFile claims the name, so two copies at once can't pick the same one.
            while (!file.createNewFile()) {
                n++
                file = File(dir, "$base ($n)$ext")
            }
            held += file.absolutePath
            file
        }
    }

    /** Lets [sweep] delete these copies once nothing plays them. Runs even when cancelled. */
    suspend fun release(files: Collection<File>) {
        if (files.isEmpty()) return
        withContext(NonCancellable) { lock.withLock { held -= files.map { it.absolutePath }.toSet() } }
    }

    /** Deletes the copies nothing needs any more; returns how many. */
    suspend fun sweep(music: Music): Int = withContext(NonCancellable) {
        lock.withLock {
            withContext(Dispatchers.IO) { music.sweepImported(dir.absolutePath, held.toList() + keep()).toInt() }
        }
    }
}
