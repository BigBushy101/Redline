package app.redline.ui.saved

import android.app.Application
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.viewModelScope
import app.redline.RedlineApp
import app.redline.core.CollectionRecord
import app.redline.core.Platform
import app.redline.core.RedlineException
import app.redline.core.SavedPlatformPageRecord
import app.redline.core.SavedTrackRecord
import app.redline.core.TrackRecord
import app.redline.state.MusicState
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch

/** What the Saved pages (and the Save buttons elsewhere) tell the user. */
sealed interface SavedMessage {
    data class Info(val text: String) : SavedMessage
    data class Error(val text: String) : SavedMessage
    /** A link was saved; "View" opens where it went. */
    data class Saved(val text: String, val platform: Platform, val collection: ULong?) : SavedMessage
    /** Downloads for offline were queued. */
    data class Downloading(val text: String) : SavedMessage
}

/**
 * The Saved pages (the desktop's `SavedPage`): which platform, collection or "All songs" is
 * open, the links being looked up to save, and the edits, which go to the core through
 * [MusicState]. Refreshes run in the core; their progress comes back in the records.
 */
class SavedModel(app: Application, private val saved: SavedStateHandle) : AndroidViewModel(app) {
    private val state = (app as RedlineApp).state
    private val music: MusicState? get() = state.music

    /** The platform page shown, or null for the overview. */
    var platform by mutableStateOf(saved.get<String>(KEY_PLATFORM)?.let { n -> Platform.entries.firstOrNull { it.name == n } })
        private set
    /** The collection shown on top of the platform page. */
    var collectionId by mutableStateOf(saved.get<Long>(KEY_COLLECTION)?.toULong())
        private set
    /** The platform's "All songs" shown on top of its page. */
    var allSongs by mutableStateOf(saved.get<Boolean>(KEY_ALL) ?: false)
        private set

    /** The open platform's page, collection and "All songs", as last read. */
    var page by mutableStateOf<SavedPlatformPageRecord?>(null)
        private set
    var collection by mutableStateOf<CollectionRecord?>(null)
        private set
    var all by mutableStateOf<List<SavedTrackRecord>?>(null)
        private set

    /** Links being looked up to save. */
    var saving by mutableStateOf<List<String>>(emptyList())
        private set

    private val _messages = Channel<SavedMessage>(Channel.BUFFERED)
    val messages = _messages.receiveAsFlow()

    // ---- Where we are -------------------------------------------------------------------

    fun openPlatform(p: Platform) {
        if (platform != p) page = null
        platform = p
        saved[KEY_PLATFORM] = p.name
        closeCollection()
        closeAllSongs()
        reload()
    }

    fun openCollection(id: ULong) {
        if (collectionId != id) collection = null
        collectionId = id
        saved[KEY_COLLECTION] = id.toLong()
        reload()
    }

    fun openAllSongs() {
        all = null
        allSongs = true
        saved[KEY_ALL] = true
        reload()
    }

    /** One level up: a collection or "All songs" to the platform, the platform to the overview. */
    fun back() {
        when {
            collectionId != null -> closeCollection()
            allSongs -> closeAllSongs()
            else -> closeAll()
        }
    }

    /** Back to the overview. */
    private fun closeAll() {
        closeCollection()
        closeAllSongs()
        platform = null
        page = null
        saved.remove<String>(KEY_PLATFORM)
    }

    private fun closeCollection() {
        collectionId = null
        collection = null
        saved.remove<Long>(KEY_COLLECTION)
    }

    private fun closeAllSongs() {
        allSongs = false
        all = null
        saved.remove<Boolean>(KEY_ALL)
    }

    /** Reads what's open again (after Saved changed, or History changed what's offline). */
    fun reload() {
        val p = platform ?: return
        val music = music ?: return
        val id = collectionId
        val wantAll = allSongs
        viewModelScope.launch {
            val read = music.onMusic { m ->
                Triple(
                    m.savedPlatform(p),
                    id?.let { m.savedCollection(it) },
                    if (wantAll) m.savedAllSongs(p) else null,
                )
            }
            if (platform != p) return@launch
            val (pageRead, collectionRead, allRead) = read
            // The last item went: back to the overview, as the desktop does.
            if (pageRead.collections.isEmpty() && pageRead.songs.isEmpty()) {
                closeAll()
                return@launch
            }
            page = pageRead
            if (id != null && collectionId == id) {
                // Removed elsewhere: back to the platform.
                if (collectionRead == null) closeCollection() else collection = collectionRead
            }
            if (wantAll && allSongs) all = allRead
        }
    }

    // ---- Saving -------------------------------------------------------------------------

    fun isSaving(url: String) = url.trim() in saving

    /** Looks a link up and saves it: a playlist as a collection, anything else as a song. */
    fun saveLink(url: String) {
        val link = url.trim()
        if (link.isEmpty() || link in saving) return
        val music = music ?: return
        saving = saving + link
        viewModelScope.launch {
            try {
                val out = music.onMusicSlow { it.saveLink(link) }
                music.act { } // Saved changed
                state.act { } // and new songs of a kept-offline collection may be queued
                _messages.send(SavedMessage.Saved(out.message, out.platform, out.collectionId))
            } catch (e: RedlineException) {
                _messages.send(SavedMessage.Error(e.message ?: "Couldn't save that link."))
            } finally {
                saving = saving - link
            }
        }
    }

    /**
     * Saves a link being downloaded, so it's in Saved under its platform too. Nothing is said
     * unless saving fails; saving it again is a no-op.
     */
    fun saveQuietly(url: String) {
        val link = url.trim()
        if (link.isEmpty() || link in saving) return
        val music = music ?: return
        saving = saving + link
        viewModelScope.launch {
            try {
                music.onMusicSlow { it.saveLink(link) }
                music.act { } // Saved changed
            } catch (e: RedlineException) {
                _messages.send(SavedMessage.Error("Couldn't save it to Saved: ${e.message ?: "the lookup failed"}"))
            } finally {
                saving = saving - link
            }
        }
    }

    // ---- Collections --------------------------------------------------------------------

    fun refresh(id: ULong) = refreshing { it.refreshCollection(id) }

    fun refreshAll(p: Platform) = refreshing { it.refreshPlatform(p) }

    private fun refreshing(call: (app.redline.core.Music) -> Unit) {
        val music = music ?: return
        viewModelScope.launch {
            try {
                music.onMusic(call)
                music.act { }
            } catch (e: RedlineException) {
                _messages.send(SavedMessage.Error(e.message ?: "Couldn't refresh."))
            }
        }
    }

    fun rename(id: ULong, name: String) {
        if (name.isBlank()) return
        music?.act { it.renameCollection(id, name.trim()) }
    }

    fun remove(id: ULong) {
        if (collectionId == id) closeCollection()
        music?.act { it.removeCollection(id) }
    }

    fun clearRemoved(id: ULong) {
        music?.act { it.clearRemoved(id) }
    }

    /** Turning it on downloads everything not downloaded yet, straight away. */
    fun setKeepOffline(id: ULong, on: Boolean) {
        val music = music ?: return
        viewModelScope.launch {
            try {
                val n = music.onMusic { it.setKeepOffline(id, on) }.toInt()
                music.act { }
                if (!on) return@launch
                state.act { } // show the jobs in the queue now
                _messages.send(
                    if (n == 0) {
                        SavedMessage.Info("Everything here is downloaded or on its way")
                    } else {
                        SavedMessage.Downloading("Downloading ${songs(n)} for offline")
                    },
                )
            } catch (e: RedlineException) {
                _messages.send(SavedMessage.Error(e.message ?: "Couldn't keep it offline."))
            }
        }
    }

    /** "Copy to playlist…": into [playlist], or a new one called [newName]. */
    fun copyToPlaylist(id: ULong, playlist: ULong?, newName: String? = null) {
        val music = music ?: return
        viewModelScope.launch {
            val target = music.editPlaylistsNow { m ->
                val into = playlist ?: newName?.trim()?.takeIf { it.isNotEmpty() }?.let { m.createPlaylist(it) }
                m.copyCollectionToPlaylist(id, into)
            }
            val name = music.playlists.value.firstOrNull { it.id == target }?.name ?: "the playlist"
            _messages.send(SavedMessage.Info("Copied to $name"))
        }
    }

    // ---- Playing and songs --------------------------------------------------------------

    fun playCollection(id: ULong, index: Int = 0, shuffle: Boolean = false) {
        music?.act { it.playCollection(id, index.toUInt(), shuffle) }
    }

    fun playAll(p: Platform, index: Int = 0, shuffle: Boolean = false) {
        music?.act { it.playSavedSongs(p, index.toUInt(), shuffle) }
    }

    /** The platform's single songs, from [index]. */
    fun playSongs(songs: List<TrackRecord>, index: Int) {
        music?.act { it.playTracks(songs, index.toUInt()) }
    }

    fun playNext(track: TrackRecord) {
        music?.act { it.playNext(listOf(track)) }
        viewModelScope.launch { _messages.send(SavedMessage.Info("“${track.title}” plays next")) }
    }

    fun queue(track: TrackRecord) {
        music?.act { it.queueTracks(listOf(track)) }
        viewModelScope.launch { _messages.send(SavedMessage.Info("Added “${track.title}” to the queue")) }
    }

    fun removeSong(track: TrackRecord) {
        music?.act { it.removeSavedSong(track.location) }
    }

    fun downloadForOffline(track: TrackRecord) {
        val music = music ?: return
        viewModelScope.launch {
            try {
                music.onMusic { it.downloadForOffline(track) }
                state.act { }
                _messages.send(SavedMessage.Downloading("Downloading “${track.title}” for offline"))
            } catch (e: RedlineException) {
                _messages.send(SavedMessage.Error(e.message ?: "Couldn't queue that download."))
            }
        }
    }

    private companion object {
        const val KEY_PLATFORM = "saved-platform"
        const val KEY_COLLECTION = "saved-collection"
        const val KEY_ALL = "saved-all-songs"
    }
}

/** "1 song", "12 songs". */
fun songs(n: Int) = "$n song${if (n == 1) "" else "s"}"
