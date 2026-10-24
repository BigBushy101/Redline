package app.redline.ui.local

import android.Manifest
import android.app.Application
import android.content.Context
import android.content.SharedPreferences
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.content.ContextCompat
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import app.redline.RedlineApp
import app.redline.core.LinkStatusRecord
import app.redline.core.TrackKind
import app.redline.core.TrackRecord
import app.redline.state.LocalFiles
import app.redline.state.MusicState
import app.redline.util.isAudioPath
import app.redline.util.treePath
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

/** What the Local page tells the user. */
sealed interface LocalMessage {
    data class Info(val text: String) : LocalMessage
    data class Error(val text: String) : LocalMessage
}

/**
 * The Library's Local section (the desktop's `LocalPage`): the songs in the folders picked for
 * it, the phone's Music folder until others are. Android's media index already knows every song
 * on the phone with its tags, so a scan is a query of it rather than ffprobe on each file; only
 * audio files the player takes are listed. The songs play from where they are, which needs
 * permission to read audio ([permission]).
 */
class LocalModel(app: Application) : AndroidViewModel(app) {
    private val state = (app as RedlineApp).state
    private val music: MusicState? get() = state.music
    private val prefs = app.getSharedPreferences("local", Context.MODE_PRIVATE)

    /** Folders searched, as paths. */
    var folders by mutableStateOf(loadFolders())
        private set
    /** Songs added on the Convert page, listed whatever folder they're in ([LocalFiles]). */
    var files by mutableStateOf(LocalFiles.load(app))
        private set
    var tracks by mutableStateOf<List<TrackRecord>>(emptyList())
        private set
    var scanning by mutableStateOf(false)
        private set
    /** Whether the songs can be read; asked for on the page. */
    var allowed by mutableStateOf(hasPermission(app))
        private set

    private val _messages = Channel<LocalMessage>(Channel.BUFFERED)
    val messages = _messages.receiveAsFlow()

    private var scan: Job? = null

    /** The Convert page adds files straight to [LocalFiles]; this picks them up. */
    private val filesWatch = SharedPreferences.OnSharedPreferenceChangeListener { _, key ->
        if (key == LocalFiles.KEY_FILES) {
            val now = LocalFiles.load(getApplication())
            if (now != files) {
                files = now
                rescan()
            }
        }
    }

    init {
        // Held weakly by the preferences, so the model keeps it.
        LocalFiles.prefs(app).registerOnSharedPreferenceChangeListener(filesWatch)
    }

    override fun onCleared() {
        LocalFiles.prefs(getApplication()).unregisterOnSharedPreferenceChangeListener(filesWatch)
    }

    /** After the permission prompt, or coming back to the page. */
    fun permissionChanged() {
        val now = hasPermission(getApplication())
        if (now != allowed) {
            allowed = now
            rescan()
        }
    }

    fun rescan() {
        if (!allowed) {
            tracks = emptyList()
            return
        }
        scan?.cancel()
        val dirs = folders
        val added = files
        scanning = true
        scan = viewModelScope.launch {
            val indexed = withContext(Dispatchers.IO) { query(dirs, added) }
            // Added files the media index doesn't know (app-private copies, folders it skips):
            // ffprobe reads their tags.
            val known = indexed.map { it.location }.toSet()
            val rest = added.filter { it !in known && File(it).isFile }
            val probed = if (rest.isEmpty()) emptyList() else music?.onMusicSlow { it.scanFiles(rest) }.orEmpty()
            tracks = indexed + probed
            scanning = false
        }
    }

    /** Whether [path] is listed because it was added, rather than for its folder. */
    fun isAdded(path: String): Boolean {
        val roots = folders.map { it.trimEnd('/') + "/" }
        return path in files && roots.none { path.startsWith(it) }
    }

    fun removeFile(path: String) = LocalFiles.remove(getApplication(), path)

    fun clearFiles() = LocalFiles.clear(getApplication())

    /** A folder from the system picker; refused if it isn't on the phone's own storage. */
    fun addFolder(tree: Uri) {
        val path = treePath(tree)
        if (path == null) {
            viewModelScope.launch { _messages.send(LocalMessage.Error("Pick a folder on this phone's storage or SD card.")) }
            return
        }
        if (folders.any { it == path }) return
        saveFolders(folders + path)
    }

    fun removeFolder(path: String) = saveFolders(folders - path)

    fun resetFolders() = saveFolders(listOf(defaultFolder()))

    val isDefault: Boolean get() = folders == listOf(defaultFolder())

    private fun saveFolders(list: List<String>) {
        folders = list
        prefs.edit().putString(KEY_FOLDERS, list.joinToString("\n")).apply()
        rescan()
    }

    private fun loadFolders(): List<String> =
        prefs.getString(KEY_FOLDERS, null)?.split('\n')?.filter { it.isNotEmpty() } ?: listOf(defaultFolder())

    // ---- Playing ------------------------------------------------------------------------

    fun play(start: Int = 0, shuffle: Boolean = false) {
        val list = tracks
        if (list.isEmpty()) return
        val from = if (shuffle) list.indices.random() else start
        music?.act { m ->
            if (shuffle) m.setShuffle(true)
            m.playTracks(list, from.toUInt())
        }
    }

    fun playNext(track: TrackRecord) {
        music?.act { it.playNext(listOf(track)) }
        viewModelScope.launch { _messages.send(LocalMessage.Info("“${track.title}” plays next")) }
    }

    fun queue(track: TrackRecord) {
        music?.act { it.queueTracks(listOf(track)) }
        viewModelScope.launch { _messages.send(LocalMessage.Info("Added “${track.title}” to the queue")) }
    }

    /** Into a playlist, or a new one named [newName] when [playlist] is null. */
    fun addToPlaylist(track: TrackRecord, playlist: ULong?, newName: String = "") {
        val music = music ?: return
        viewModelScope.launch {
            music.editPlaylistsNow { m ->
                val id = playlist ?: m.createPlaylist(newName.trim())
                m.addTracks(id, listOf(track))
            }
            _messages.send(LocalMessage.Info("Added “${track.title}” to the playlist"))
        }
    }

    // ---- The media index ----------------------------------------------------------------

    @Suppress("DEPRECATION") // DATA is still the file's path, and still readable.
    private fun query(dirs: List<String>, added: List<String>): List<TrackRecord> {
        val roots = dirs.map { it.trimEnd('/') + "/" }
        val addedSet = added.toSet()
        if (roots.isEmpty() && addedSet.isEmpty()) return emptyList()
        val cols = arrayOf(
            MediaStore.Audio.Media.DATA,
            MediaStore.Audio.Media.TITLE,
            MediaStore.Audio.Media.ARTIST,
            MediaStore.Audio.Media.DURATION,
            MediaStore.Audio.Media.DATE_ADDED,
        )
        // Ringtones, alarms and notification sounds aren't songs.
        val where = "${MediaStore.Audio.Media.IS_RINGTONE} = 0 AND ${MediaStore.Audio.Media.IS_ALARM} = 0 AND " +
            "${MediaStore.Audio.Media.IS_NOTIFICATION} = 0"
        val out = mutableListOf<TrackRecord>()
        val seen = mutableSetOf<String>()
        val resolver = getApplication<Application>().contentResolver
        resolver.query(MediaStore.Audio.Media.EXTERNAL_CONTENT_URI, cols, where, null, "${MediaStore.Audio.Media.DATA} ASC")?.use { c ->
            while (c.moveToNext()) {
                val path = c.getString(0) ?: continue
                if ((roots.none { path.startsWith(it) } && path !in addedSet) || !seen.add(path)) continue
                if (!isAudioPath(path)) continue
                if (!File(path).isFile) continue
                val name = path.substringAfterLast('/').substringBeforeLast('.')
                out += TrackRecord(
                    kind = TrackKind.FILE,
                    location = path,
                    title = c.getString(1)?.takeIf { it.isNotBlank() } ?: name,
                    artist = c.getString(2)?.takeIf { it.isNotBlank() && it != MediaStore.UNKNOWN_STRING },
                    durationSecs = c.getLong(3).takeIf { it > 0 }?.let { it / 1000.0 },
                    thumbnail = null,
                    addedAt = c.getLong(4),
                    platform = null,
                    offline = false,
                    linkStatus = LinkStatusRecord.UNKNOWN,
                    replacedFrom = null,
                )
            }
        }
        return out
    }

    companion object {
        private const val KEY_FOLDERS = "folders"

        fun defaultFolder(): String =
            Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_MUSIC).absolutePath

        /** The permission that lets the app list and read songs. */
        val permission: String =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) Manifest.permission.READ_MEDIA_AUDIO else Manifest.permission.READ_EXTERNAL_STORAGE

        fun hasPermission(context: Context) =
            ContextCompat.checkSelfPermission(context, permission) == PackageManager.PERMISSION_GRANTED
    }
}
