package app.redline.state

import app.redline.core.Engine
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.File
import java.util.UUID

/**
 * The copies of picked and shared files that ffmpeg reads (it needs real paths, and a picked
 * document is only a content:// link). Each copy gets a folder of its own under [root]. A folder
 * is *held* while the Convert page lists its file or a copy into it is under way; the engine
 * knows which ones queued, failed or cancelled jobs still read. [sweep] deletes the rest.
 *
 * Holding and sweeping take the same lock, so a sweep never sees a folder between its creation
 * and its hold, or a page's file between its release and its job being queued.
 */
class ConvertInputs(val root: File) {
    private val lock = Mutex()
    private val held = mutableSetOf<String>()

    /** A new, empty folder, held until [release]. */
    suspend fun newFolder(): File = lock.withLock {
        val dir = File(root, UUID.randomUUID().toString())
        withContext(Dispatchers.IO) { dir.mkdirs() }
        held += dir.absolutePath
        dir
    }

    /** Lets [sweep] delete these folders once no job reads from them. */
    suspend fun release(dirs: Collection<File>) {
        if (dirs.isEmpty()) return
        lock.withLock { held -= dirs.map { it.absolutePath }.toSet() }
    }

    /** Deletes the folders nothing needs any more; returns how many. */
    suspend fun sweep(engine: Engine): Int = lock.withLock {
        withContext(Dispatchers.IO) { engine.sweepConvertInputs(root.absolutePath, held.toList()).toInt() }
    }
}
