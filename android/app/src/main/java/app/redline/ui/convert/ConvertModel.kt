package app.redline.ui.convert

import android.app.Application
import android.content.ContentResolver
import android.net.Uri
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import app.redline.RedlineApp
import app.redline.core.ConvertItemRecord
import app.redline.core.ConvertOptionsRecord
import app.redline.core.ConvertPresetRecord
import app.redline.core.ConvertSettingsRecord
import app.redline.core.MediaCategory
import app.redline.core.RedlineException
import app.redline.core.SubtitleBurn
import app.redline.core.applyConvertPreset
import app.redline.core.classifyFileName
import app.redline.core.newConvertPreset
import app.redline.state.EngineStatus
import app.redline.state.LocalFiles
import app.redline.util.documentPath
import app.redline.util.isAudioPath
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.io.IOException

/** What the Convert page tells the user. */
sealed interface ConvertMessage {
    /** Jobs were queued; the page moves to the Queue, as the desktop does. */
    data object Queued : ConvertMessage
    data class Error(val text: String) : ConvertMessage
}

/** A picked file being copied into the app's storage so ffmpeg can read it. */
class Import(val name: String, val total: Long?) {
    /** Bytes so far; written from the copying thread. */
    var copied by mutableLongStateOf(0L)
}

/**
 * The Convert page's file list (the desktop's `ConvertPage`): which files, what each becomes,
 * their own settings, which editors are open. The shared options and presets are settings, as
 * on the desktop; edits go straight to them through [app.redline.state.RedlineState].
 *
 * Picked and shared files are copied into [app.redline.state.ConvertInputs]' folders first;
 * a folder is held while its file is listed here, then the queue keeps it until the job is done.
 */
class ConvertModel(app: Application) : AndroidViewModel(app) {
    private val state = (app as RedlineApp).state
    private val inputs = state.convertInputs

    var items by mutableStateOf<List<ConvertItemRecord>>(emptyList())
        private set
    /** Paths of the files whose own editor is open. */
    var expanded by mutableStateOf<Set<String>>(emptySet())
        private set
    /** Files that weren't added, by name (the desktop's "Skipped unsupported files"). */
    var skipped by mutableStateOf<List<String>>(emptyList())
        private set
    var imports by mutableStateOf<List<Import>>(emptyList())
        private set
    var queueing by mutableStateOf(false)
        private set

    /** The copy folders each listed file holds (its own and its subtitle file's). */
    private val held = mutableMapOf<String, MutableList<File>>()

    private val _messages = Channel<ConvertMessage>(Channel.BUFFERED)
    val messages = _messages.receiveAsFlow()

    private val resolver: ContentResolver get() = getApplication<Application>().contentResolver

    // ---- Adding files -------------------------------------------------------------------

    /** Files from the picker or another app's "Share": copied, then read and listed. */
    fun addUris(uris: List<Uri>) {
        if (uris.isEmpty()) return
        viewModelScope.launch {
            skipped = emptyList()
            val copies = mutableListOf<Pair<File, File>>() // folder, file
            val picked = mutableMapOf<String, Uri>() // copy's path, where it came from
            val skippedNow = mutableListOf<String>()
            for (uri in uris) {
                val (name, size, named) = withContext(Dispatchers.IO) { describe(uri) }
                if (!named && classifyFileName(name) == null) {
                    // The app that shared it didn't let Redline read it.
                    _messages.send(ConvertMessage.Error("Couldn't read a shared file."))
                    continue
                }
                if (classifyFileName(name) == null) {
                    skippedNow += name
                    continue
                }
                val dir = inputs.newFolder()
                val file = File(dir, name)
                val import = Import(name, size)
                imports = imports + import
                val ok = try {
                    withContext(Dispatchers.IO) { copy(uri, file) { import.copied = it } }
                    true
                } catch (e: CancellationException) {
                    inputs.release(listOf(dir))
                    throw e
                } catch (e: Exception) {
                    _messages.send(ConvertMessage.Error("Couldn't copy $name: ${e.message ?: e}"))
                    false
                } finally {
                    imports = imports.filter { it !== import }
                }
                if (ok) {
                    copies += dir to file
                    picked[file.absolutePath] = uri
                } else {
                    inputs.release(listOf(dir))
                }
            }
            skipped = skippedNow
            if (copies.isNotEmpty()) {
                val added = try {
                    read(copies.map { it.second.absolutePath })
                } catch (e: CancellationException) {
                    inputs.release(copies.map { it.first })
                    throw e
                } catch (e: Exception) {
                    _messages.send(ConvertMessage.Error(e.message ?: e.toString()))
                    emptyList()
                }
                for ((dir, file) in copies) {
                    if (added.any { it.path == file.absolutePath }) {
                        held[file.absolutePath] = mutableListOf(dir)
                    } else {
                        inputs.release(listOf(dir))
                    }
                }
                for (item in added) {
                    val uri = picked[item.path] ?: continue
                    if (isAudioPath(item.path)) keepInLocal(uri, File(item.path))
                }
            }
            sweep()
        }
    }

    /**
     * Files already in Redline's folders (finished downloads and conversions): no copy needed.
     * The songs among them are kept in Local too, unless they came from the Library
     * ([keepInLocal] false), where they are already.
     */
    fun addPaths(paths: List<String>, keepInLocal: Boolean = true) {
        val fresh = paths.filter { p -> items.none { it.path == p } }
        if (fresh.isEmpty()) return
        viewModelScope.launch {
            skipped = emptyList()
            try {
                val added = read(fresh)
                if (keepInLocal) LocalFiles.add(getApplication(), added.map { it.path })
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _messages.send(ConvertMessage.Error(e.message ?: e.toString()))
            }
        }
    }

    /**
     * Keeps a picked song in the Library's Local section: where it really is when that's a
     * readable file, else a lasting copy in [app.redline.state.ImportedMusic] (the Convert
     * page's own [copy] goes once the job is done).
     */
    private suspend fun keepInLocal(uri: Uri, copy: File) {
        val app = getApplication<Application>()
        val original = withContext(Dispatchers.IO) { documentPath(app, uri) }
        if (original != null) {
            LocalFiles.add(app, listOf(original))
            return
        }
        val imported = state.music?.imported ?: return
        val dest = imported.newFile(copy.name)
        try {
            withContext(Dispatchers.IO) { copy.copyTo(dest, overwrite = true) }
            LocalFiles.add(app, listOf(dest.absolutePath))
        } catch (e: CancellationException) {
            dest.delete()
            throw e
        } catch (e: Exception) {
            dest.delete()
            _messages.send(ConvertMessage.Error("Couldn't keep ${copy.name} in Local: ${e.message ?: e}"))
        } finally {
            imported.release(listOf(dest))
        }
    }

    /** Lists the files the engine can convert; returns those it added. */
    private suspend fun read(paths: List<String>): List<ConvertItemRecord> {
        // A file shared while Redline starts waits for the core.
        state.status.first { it is EngineStatus.Ready }
        val found = state.onEngineSlow { it.convertFiles(paths) }
        items = items + found.items
        skipped = skipped + found.skipped
        return found.items
    }

    /**
     * The file's display name (with an extension, which the core classifies by), its size, and
     * whether the provider answered at all.
     */
    private fun describe(uri: Uri, media: Boolean = true): Triple<String, Long?, Boolean> {
        var name: String? = null
        var size: Long? = null
        runCatching {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
                if (c.moveToFirst()) {
                    val n = c.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                    val s = c.getColumnIndex(OpenableColumns.SIZE)
                    if (n >= 0 && !c.isNull(n)) name = c.getString(n)
                    if (s >= 0 && !c.isNull(s)) size = c.getLong(s)
                }
            }
        }
        var file = (name ?: uri.lastPathSegment?.substringAfterLast('/') ?: "file")
            .replace(Regex("""[/\\:*?"<>|\u0000-\u001f]"""), "_")
            .trim()
            .ifEmpty { "file" }
        if (media && classifyFileName(file) == null) {
            // Some apps share "IMG_1234" without an extension; the MIME type knows it.
            val ext = runCatching { resolver.getType(uri) }.getOrNull()?.let { MimeTypeMap.getSingleton().getExtensionFromMimeType(it) }
            if (ext != null) file = "$file.$ext"
        }
        return Triple(file, size, name != null)
    }

    private fun copy(uri: Uri, to: File, progress: (Long) -> Unit) {
        val input = resolver.openInputStream(uri) ?: throw IOException("the file can't be opened")
        input.use { src ->
            to.outputStream().use { dst ->
                val buf = ByteArray(256 * 1024)
                var total = 0L
                var reported = 0L
                while (true) {
                    val n = src.read(buf)
                    if (n < 0) break
                    dst.write(buf, 0, n)
                    total += n
                    if (total - reported >= 4L * 1024 * 1024) {
                        reported = total
                        progress(total)
                    }
                }
            }
        }
    }

    // ---- The list -----------------------------------------------------------------------

    fun update(index: Int, change: (ConvertItemRecord) -> ConvertItemRecord) {
        items = items.mapIndexed { i, item -> if (i == index) change(item) else item }
    }

    fun toggleEditor(item: ConvertItemRecord) {
        expanded = if (item.path in expanded) expanded - item.path else expanded + item.path
    }

    fun move(index: Int, by: Int) {
        val to = index + by
        if (index !in items.indices || to !in items.indices) return
        items = items.toMutableList().also { it.add(to, it.removeAt(index)) }
    }

    fun remove(index: Int) {
        val item = items.getOrNull(index) ?: return
        items = items.filterIndexed { i, _ -> i != index }
        expanded = expanded - item.path
        releaseAndSweep(listOf(item))
    }

    fun clear() {
        val gone = items
        items = emptyList()
        expanded = emptySet()
        skipped = emptyList()
        releaseAndSweep(gone)
    }

    /** "All videos → MKV". */
    fun setAll(category: MediaCategory, target: String) {
        items = items.map { if (it.category == category) it.copy(target = target) else it }
    }

    /** "Extract audio": every video becomes an MP3. */
    fun extractAudio() = setAll(MediaCategory.VIDEO, "mp3")

    /** The desktop's "Use its own settings" toggle: starts from the shared options. */
    fun setCustom(index: Int, on: Boolean, shared: ConvertOptionsRecord) =
        update(index) { it.copy(options = if (on) shared else null) }

    /** Burns a picked subtitle file in; it's copied next to the other copies. */
    fun pickSubtitles(index: Int, uri: Uri) {
        val item = items.getOrNull(index) ?: return
        viewModelScope.launch {
            val name = withContext(Dispatchers.IO) { describe(uri, media = false).first }
            if (name.substringAfterLast('.', "").lowercase() !in SUBTITLE_TYPES) {
                _messages.send(ConvertMessage.Error("Pick a .srt, .ass, .ssa or .vtt file."))
                return@launch
            }
            val dir = inputs.newFolder()
            val file = File(dir, name)
            try {
                withContext(Dispatchers.IO) { copy(uri, file) {} }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                inputs.release(listOf(dir))
                _messages.send(ConvertMessage.Error("Couldn't copy $name: ${e.message ?: e}"))
                return@launch
            }
            val i = items.indexOfFirst { it.path == item.path }
            if (i < 0) {
                inputs.release(listOf(dir))
                sweep()
                return@launch
            }
            // The previous subtitle file's folder can go.
            val old = (items[i].burnSubs as? SubtitleBurn.File)?.path?.let { File(it).parentFile }
            val list = held.getOrPut(item.path) { mutableListOf() }
            list += dir
            update(i) { it.copy(burnSubs = SubtitleBurn.File(file.absolutePath)) }
            if (old != null && list.remove(old)) {
                inputs.release(listOf(old))
                sweep()
            }
        }
    }

    // ---- Presets ------------------------------------------------------------------------

    private fun updateConvert(change: (ConvertSettingsRecord) -> ConvertSettingsRecord) {
        state.updateSettings { it.copy(convert = change(it.convert)) }
    }

    fun updateShared(change: (ConvertOptionsRecord) -> ConvertOptionsRecord) =
        updateConvert { it.copy(options = change(it.options)) }

    fun setTemplate(template: String) = updateConvert { it.copy(renameTemplate = template) }

    fun applyPreset(preset: ConvertPresetRecord) {
        val shared = state.settings.value?.convert?.options ?: return
        val applied = applyConvertPreset(preset, shared, items)
        items = applied.items
        updateShared { applied.options }
    }

    fun savePreset(name: String) {
        if (name.isBlank()) return
        updateConvert { c -> c.copy(presets = c.presets + newConvertPreset(name, c.options, items)) }
    }

    fun renamePreset(index: Int, name: String) {
        if (name.isBlank()) return
        updateConvert { c -> c.copy(presets = c.presets.mapIndexed { i, p -> if (i == index) p.copy(name = name.trim()) else p }) }
    }

    fun deletePreset(index: Int) = updateConvert { c -> c.copy(presets = c.presets.filterIndexed { i, _ -> i != index }) }

    // ---- Queueing -----------------------------------------------------------------------

    fun convert() = queue(join = false)

    fun join() = queue(join = true)

    private fun queue(join: Boolean) {
        val files = items
        if (files.isEmpty() || queueing) return
        queueing = true
        viewModelScope.launch {
            try {
                if (join) state.onEngine { it.queueJoin(files) } else state.onEngine { it.queueConversions(files) }
                state.act { } // refresh the queue now rather than at the next nudge
                // Like the desktop, the files leave the page for the queue.
                items = items.filter { it !in files }
                expanded = emptySet()
                releaseAndSweep(files)
                _messages.send(ConvertMessage.Queued)
            } catch (e: CancellationException) {
                throw e
            } catch (e: RedlineException) {
                _messages.send(ConvertMessage.Error(e.message ?: "Couldn't queue that."))
            } catch (e: Exception) {
                _messages.send(ConvertMessage.Error(e.message ?: e.toString()))
            } finally {
                queueing = false
            }
        }
    }

    /** Lets the files' copies go: the queue holds the ones its jobs still need. */
    private fun releaseAndSweep(gone: List<ConvertItemRecord>) {
        val dirs = gone.flatMap { held.remove(it.path).orEmpty() }
        viewModelScope.launch {
            inputs.release(dirs)
            sweep()
        }
    }

    private suspend fun sweep() {
        val engine = state.engine ?: return
        inputs.sweep(engine)
    }

    private companion object {
        /** What the desktop's subtitle file dialog accepts. */
        val SUBTITLE_TYPES = setOf("srt", "ass", "ssa", "vtt")
    }

    override fun onCleared() {
        // The activity is gone for good: nothing lists these files any more.
        val dirs = held.values.flatten()
        held.clear()
        val engine = state.engine ?: return
        state.launchDetached {
            inputs.release(dirs)
            inputs.sweep(engine)
        }
    }
}
