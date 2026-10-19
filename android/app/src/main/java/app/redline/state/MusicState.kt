package app.redline.state

import android.app.Application
import app.redline.RedlineEngine
import app.redline.core.EqRecord
import app.redline.core.Music
import app.redline.core.MusicListener
import app.redline.core.PlayerChoicesRecord
import app.redline.core.PlayerSettingsRecord
import app.redline.core.PlayerSnapshotRecord
import app.redline.core.PlayerStatus
import app.redline.core.PlaylistSummaryRecord
import app.redline.core.QueueTrackRecord
import app.redline.core.RepeatMode
import app.redline.core.SavedOverviewRecord
import app.redline.core.playerChoices
import app.redline.work.PlaybackService
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.lang.ref.WeakReference

/**
 * The app's copy of the player, Now playing, the playlists and Saved, pulled from the core's [Music]
 * whenever it nudges (up to ten times a second while playing). App-scoped like [RedlineState],
 * which makes it once the core has loaded, so the playback service and the screens share it.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class MusicState(private val app: Application, val music: Music) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    /**
     * Quick calls (transport, snapshots, settings, playlist edits) run here, one at a time and
     * in order, off the main thread: they take the core's locks, which its threads hold while
     * saving. Lookups and scans use [Dispatchers.IO] instead.
     */
    private val musicThread = Dispatchers.IO.limitedParallelism(1)

    /** What the speed and EQ controls offer; fixed. */
    val choices: PlayerChoicesRecord = playerChoices()

    private val _snapshot = MutableStateFlow<PlayerSnapshotRecord?>(null)
    val snapshot: StateFlow<PlayerSnapshotRecord?> = _snapshot.asStateFlow()

    private val _queue = MutableStateFlow<List<QueueTrackRecord>>(emptyList())
    /** Now playing, in play order. */
    val queue: StateFlow<List<QueueTrackRecord>> = _queue.asStateFlow()

    private val _settings = MutableStateFlow<PlayerSettingsRecord?>(null)
    val settings: StateFlow<PlayerSettingsRecord?> = _settings.asStateFlow()

    private val _playlists = MutableStateFlow<List<PlaylistSummaryRecord>>(emptyList())
    val playlists: StateFlow<List<PlaylistSummaryRecord>> = _playlists.asStateFlow()

    private val _playlistsVersion = MutableStateFlow(0)
    /** Bumped after every playlist edit, so an open playlist knows to read itself again. */
    val playlistsVersion: StateFlow<Int> = _playlistsVersion.asStateFlow()

    private val _savedVersion = MutableStateFlow(0UL)
    /** Changes whenever Saved or one of its refreshes does, so open Saved pages read again. */
    val savedVersion: StateFlow<ULong> = _savedVersion.asStateFlow()

    private val _savedOverview = MutableStateFlow<SavedOverviewRecord?>(null)
    /** The platforms with something saved, for the Saved overview. */
    val savedOverview: StateFlow<SavedOverviewRecord?> = _savedOverview.asStateFlow()

    /** Copies of files picked for playlists. */
    val imported = ImportedMusic(RedlineEngine.importedMusicDir(app)) { LocalFiles.load(app) }

    private val refreshes = Channel<Unit>(Channel.CONFLATED)
    private var queueVersion: ULong? = null
    /** Counts the settings changes made here; a read that started before one is stale. */
    private var settingsEpoch = 0

    init {
        // The phone's volume buttons set the volume; there's no slider to undo a low one an
        // older version saved.
        act { it.setVolume(1f) }
        scope.launch { for (unit in refreshes) refresh() }
        requestRefresh()
        reloadPlaylists()
        // Copies an older version left behind, or whose tracks have left Now playing since.
        scope.launch { imported.sweep(music) }
    }

    /** Called on the core's thread; just queues a refresh. */
    internal fun requestRefresh() {
        refreshes.trySend(Unit)
    }

    private suspend fun refresh() {
        val known = queueVersion
        val epoch = settingsEpoch
        val savedKnown = _savedOverview.value?.version
        val pulled = withContext(musicThread) {
            val s = music.snapshot()
            Pulled(
                s,
                if (s.queueVersion != known) music.queue() else null,
                music.playerSettings(),
                // Saved is only read again when it changed (a refresh nudges as it starts and ends).
                if (music.savedVersion() != savedKnown) music.savedOverview() else null,
            )
        }
        val (snap, queue, settings, saved) = pulled
        if (saved != null) {
            _savedOverview.value = saved
            _savedVersion.value = saved.version
        }
        _snapshot.value = snap
        if (queue != null) {
            _queue.value = queue
            queueVersion = snap.queueVersion
        }
        // The speed goes back to 1x by itself for a new track; local edits win while in flight.
        if (epoch == settingsEpoch && settings != _settings.value) _settings.value = settings
        if (snap.isPlaying) PlaybackService.ensureRunning(app)
    }

    private data class Pulled(
        val snapshot: PlayerSnapshotRecord,
        val queue: List<QueueTrackRecord>?,
        val settings: PlayerSettingsRecord,
        val saved: SavedOverviewRecord?,
    )

    /** Runs a quick call in order behind the others, then refreshes. */
    fun act(call: (Music) -> Unit) {
        scope.launch {
            withContext(musicThread) { call(music) }
            requestRefresh()
        }
    }

    /** A quick call whose result the caller needs. */
    suspend fun <T> onMusic(call: (Music) -> T): T = withContext(musicThread) { call(music) }

    /** A call that blocks for seconds (yt-dlp lookups, ffprobe scans). */
    suspend fun <T> onMusicSlow(call: (Music) -> T): T = withContext(Dispatchers.IO) { call(music) }

    /**
     * Changes the playlists, then reads the list again and tells open playlists. An edit that
     * drops tracks ([sweepImports]) also deletes the copied files nothing plays any more.
     */
    fun editPlaylists(sweepImports: Boolean = false, call: (Music) -> Unit) {
        scope.launch {
            withContext(musicThread) { call(music) }
            reloadPlaylistsNow()
            if (sweepImports) imported.sweep(music)
        }
    }

    /** Deletes the copied files nothing plays any more (see [ImportedMusic]). */
    suspend fun sweepImports() {
        imported.sweep(music)
    }

    /** Like [editPlaylists], for a call whose result the caller needs. */
    suspend fun <T> editPlaylistsNow(call: (Music) -> T): T {
        val result = withContext(musicThread) { call(music) }
        reloadPlaylistsNow()
        return result
    }

    fun reloadPlaylists() {
        scope.launch { reloadPlaylistsNow() }
    }

    private suspend fun reloadPlaylistsNow() {
        _playlists.value = withContext(musicThread) { music.playlists() }
        _playlistsVersion.value++
        requestRefresh()
    }

    // ---- Transport ----------------------------------------------------------------------

    fun play() = act { it.play() }
    fun pause() = act { it.pause() }
    fun toggle() = act { it.toggle() }
    fun next() = act { it.next() }
    fun prev() = act { it.prev() }
    fun stop() = act { it.stop() }
    fun seek(secs: Double) = act { it.seek(secs) }
    fun setLoopA() = act { it.setLoopA() }
    fun setLoopB() = act { it.setLoopB() }
    fun clearLoop() = act { it.clearLoop() }
    fun jump(index: UInt) = act { it.jump(index) }
    fun removeFromQueue(index: UInt) = act { it.removeFromQueue(index) }
    fun setShuffle(on: Boolean) = act { it.setShuffle(on) }
    fun setRepeat(mode: RepeatMode) = act { it.setRepeat(mode) }

    // ---- Player settings: shown at once, written in order behind -----------------------

    private fun changeSettings(change: (PlayerSettingsRecord) -> PlayerSettingsRecord, call: (Music) -> Unit) {
        settingsEpoch++
        _settings.value?.let { _settings.value = change(it) }
        act(call)
    }

    fun setVolume(v: Float) = changeSettings({ it.copy(volume = v) }) { it.setVolume(v) }
    fun setSpeed(v: Float) = changeSettings({ it.copy(speed = v) }) { it.setSpeed(v) }
    fun setResetSpeed(on: Boolean) = changeSettings({ it.copy(resetSpeed = on) }) { it.setResetSpeed(on) }

    /** The preset name is the core's to work out; the next read fills it in. */
    fun setEq(eq: EqRecord) = changeSettings({ it.copy(eq = eq, eqPreset = null) }) { it.setEq(eq) }

    companion object {
        /** Whether a track is loaded: the mini player shows, and the service may run. */
        fun loaded(s: PlayerSnapshotRecord?): Boolean =
            s != null && (s.status != PlayerStatus.IDLE || s.queueLength > 0u)
    }
}

/**
 * The player's listener. Weak for the same reason as the engine's: the core holds this, and a
 * strong path back to the state would be a cycle no collector can see.
 */
internal class MusicNudge(private val state: WeakReference<RedlineState>) : MusicListener {
    override fun onChange() {
        state.get()?.music?.requestRefresh()
    }
}
