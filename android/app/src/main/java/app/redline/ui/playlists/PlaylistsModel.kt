package app.redline.ui.playlists

import android.app.Application
import android.content.ContentResolver
import android.net.Uri
import android.os.Environment
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.viewModelScope
import app.redline.RedlineApp
import app.redline.core.PlaylistRecord
import app.redline.core.RedlineException
import app.redline.core.TrackKind
import app.redline.core.TrackRecord
import app.redline.state.MusicState
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.io.IOException

/** What the Playlists page tells the user. */
sealed interface PlaylistsMessage {
    data class Info(val text: String) : PlaylistsMessage
    /** A download for offline was queued. */
    data class Downloading(val title: String) : PlaylistsMessage
    data class Error(val text: String) : PlaylistsMessage
}

/** Tracks being looked up, copied or scanned for a playlist (the desktop's `Adding`). */
class Adding(val playlist: ULong, val what: String)

/**
 * The Playlists page (the desktop's `PlaylistsPage`): which playlist is open, what's being
 * added to it, and the edits, which go to the core through [MusicState].
 *
 * Picked files are copied into the app's Music folder first: ffmpeg can't read a content://
 * link. Unlike the Convert page's copies these stay, since the playlist keeps pointing at them.
 */
class PlaylistsModel(app: Application, private val saved: SavedStateHandle) : AndroidViewModel(app) {
    private val state = (app as RedlineApp).state
    private val music: MusicState? get() = state.music

    /** The playlist shown, or null for the list. */
    var openId by mutableStateOf(saved.get<Long>(KEY_OPEN)?.toULong())
        private set
    /** The open playlist as last read. */
    var detail by mutableStateOf<PlaylistRecord?>(null)
        private set
    var adding by mutableStateOf<List<Adding>>(emptyList())
        private set

    private val _messages = Channel<PlaylistsMessage>(Channel.BUFFERED)
    val messages = _messages.receiveAsFlow()

    private val resolver: ContentResolver get() = getApplication<Application>().contentResolver

    fun open(id: ULong) {
        if (openId != id) detail = null
        openId = id
        saved[KEY_OPEN] = id.toLong()
        reload()
    }

    fun close() {
        openId = null
        detail = null
        saved.remove<Long>(KEY_OPEN)
    }

    /** Reads the open playlist again (after an edit, or History changing what's offline). */
    fun reload() {
        val id = openId ?: return
        val music = music ?: return
        viewModelScope.launch {
            val p = music.onMusic { it.playlist(id) }
            if (openId != id) return@launch
            // Deleted elsewhere: back to the list.
            if (p == null) close() else detail = p
        }
    }

    // ---- Playlists ----------------------------------------------------------------------

    fun create(name: String, openIt: Boolean = true) {
        val music = music ?: return
        viewModelScope.launch {
            val id = music.editPlaylistsNow { it.createPlaylist(name.trim()) }
            if (openIt) open(id)
        }
    }

    fun rename(id: ULong, name: String) {
        if (name.isBlank()) return
        music?.editPlaylists { it.renamePlaylist(id, name.trim()) }
    }

    fun delete(id: ULong) {
        if (openId == id) close()
        music?.editPlaylists { it.deletePlaylist(id) }
    }

    fun play(id: ULong, start: Int = 0, shuffle: Boolean = false) {
        music?.act { it.playPlaylist(id, start.toUInt(), shuffle) }
    }

    // ---- Tracks -------------------------------------------------------------------------

    fun queue(track: TrackRecord) {
        music?.act { it.queueTracks(listOf(track)) }
        viewModelScope.launch { _messages.send(PlaylistsMessage.Info("Added “${track.title}” to the queue")) }
    }

    fun move(id: ULong, from: Int, to: Int) {
        music?.editPlaylists { it.moveTrack(id, from.toUInt(), to.toUInt()) }
    }

    fun remove(id: ULong, index: Int) {
        music?.editPlaylists { it.removeTrack(id, index.toUInt()) }
    }

    fun downloadForOffline(track: TrackRecord) {
        val music = music ?: return
        viewModelScope.launch {
            try {
                music.onMusic { it.downloadForOffline(track) }
                state.act { } // show the job in the queue now
                _messages.send(PlaylistsMessage.Downloading(track.title))
            } catch (e: RedlineException) {
                _messages.send(PlaylistsMessage.Error(e.message ?: "Couldn't queue that download."))
            }
        }
    }

    // ---- Adding -------------------------------------------------------------------------

    /** A link: every entry of a playlist link, or the one video. */
    fun addLink(id: ULong, url: String) {
        val link = url.trim()
        if (link.isEmpty()) return
        val music = music ?: return
        add(id, "link") {
            try {
                music.onMusicSlow { it.tracksForLink(link) }
            } catch (e: RedlineException) {
                _messages.send(PlaylistsMessage.Error(e.message ?: "Couldn't look that link up."))
                null
            }
        }
    }

    /** Files from the picker: copied into the app's Music folder, then read with ffprobe. */
    fun addUris(id: ULong, uris: List<Uri>) {
        if (uris.isEmpty()) return
        val music = music ?: return
        val what = if (uris.size == 1) "file" else "${uris.size} files"
        add(id, what) {
            val paths = mutableListOf<String>()
            for (uri in uris) {
                try {
                    paths += withContext(Dispatchers.IO) { copy(uri).absolutePath }
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    _messages.send(PlaylistsMessage.Error("Couldn't copy a file: ${e.message ?: e}"))
                }
            }
            if (paths.isEmpty()) null else music.onMusicSlow { it.scanFiles(paths) }
        }
    }

    private fun add(id: ULong, what: String, find: suspend () -> List<TrackRecord>?) {
        val music = music ?: return
        val entry = Adding(id, what)
        adding = adding + entry
        viewModelScope.launch {
            try {
                val tracks = find() ?: return@launch
                if (tracks.isEmpty()) {
                    _messages.send(PlaylistsMessage.Error("Nothing playable in that $what."))
                    return@launch
                }
                music.editPlaylistsNow { it.addTracks(id, tracks) }
                val n = tracks.size
                _messages.send(PlaylistsMessage.Info("Added $n track${if (n == 1) "" else "s"}"))
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _messages.send(PlaylistsMessage.Error(e.message ?: e.toString()))
            } finally {
                adding = adding.filter { it !== entry }
            }
        }
    }

    /** Copies a picked file to the Music folder under its own name, numbered if taken. */
    private fun copy(uri: Uri): File {
        val app = getApplication<Application>()
        val dir = app.getExternalFilesDir(Environment.DIRECTORY_MUSIC) ?: File(app.filesDir, "Music")
        dir.mkdirs()
        val name = displayName(uri)
        val base = name.substringBeforeLast('.', name)
        val ext = name.substringAfterLast('.', "").let { if (it.isEmpty()) "" else ".$it" }
        var file = File(dir, name)
        var n = 1
        // createNewFile claims the name, so two copies at once can't pick the same one.
        while (!file.createNewFile()) {
            n++
            file = File(dir, "$base ($n)$ext")
        }
        val input = resolver.openInputStream(uri) ?: throw IOException("the file can't be opened")
        try {
            input.use { src -> file.outputStream().use { dst -> src.copyTo(dst, 256 * 1024) } }
        } catch (e: Exception) {
            file.delete()
            throw e
        }
        return file
    }

    /** The file's display name, safe for a file system, with an extension ffprobe can go by. */
    private fun displayName(uri: Uri): String {
        var name: String? = null
        runCatching {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                if (c.moveToFirst() && !c.isNull(0)) name = c.getString(0)
            }
        }
        var file = (name ?: uri.lastPathSegment?.substringAfterLast('/') ?: "track")
            .replace(Regex("""[/\\:*?"<>|\u0000-\u001f]"""), "_")
            .trim()
            .ifEmpty { "track" }
        if (!file.contains('.')) {
            val ext = runCatching { resolver.getType(uri) }.getOrNull()?.let { MimeTypeMap.getSingleton().getExtensionFromMimeType(it) }
            if (ext != null) file = "$file.$ext"
        }
        return file
    }

    private companion object {
        const val KEY_OPEN = "open-playlist"
    }
}

/** A link track that isn't downloaded yet can be. */
val TrackRecord.canGoOffline: Boolean get() = kind == TrackKind.LINK && !offline
