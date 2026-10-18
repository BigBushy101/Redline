package app.redline.state

import android.app.Application
import android.util.Log
import app.redline.RedlineEngine
import app.redline.core.ConvertChoiceListsRecord
import app.redline.core.DownloadChoiceListsRecord
import app.redline.core.EncodersRecord
import app.redline.core.Engine
import app.redline.core.HistoryRecord
import app.redline.core.JobRecord
import app.redline.core.JobState
import app.redline.core.SettingsRecord
import app.redline.core.ToolsStatusRecord
import app.redline.core.convertChoiceLists
import app.redline.core.downloadChoiceLists
import app.redline.work.DownloadService
import app.redline.work.Notifications
import com.yausername.ffmpeg.FFmpeg
import com.yausername.youtubedl_android.YoutubeDL
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

/** Where the Rust core is in starting up. */
sealed interface EngineStatus {
    data object Initializing : EngineStatus
    data class Ready(val engine: Engine) : EngineStatus
    data class Failed(val message: String) : EngineStatus
}

/**
 * The app's copy of what the screens show, pulled from the [Engine] whenever it says something
 * changed. App-scoped (one per process, in [app.redline.RedlineApp]) so the foreground service
 * and the activity see the same queue.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class RedlineState(private val app: Application) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    /**
     * Quick engine calls (snapshots, cancel, settings) run here: one at a time, in order, and off
     * the main thread, since they wait for the core's lock, which its queue thread holds while it
     * saves to disk. Calls that run yt-dlp (lookups, updates, tool checks) use [Dispatchers.IO]
     * instead so they don't hold up a cancel; the engine is safe to call from any thread.
     */
    private val engineThread = Dispatchers.IO.limitedParallelism(1)

    private val _status = MutableStateFlow<EngineStatus>(EngineStatus.Initializing)
    val status: StateFlow<EngineStatus> = _status.asStateFlow()

    private val _queue = MutableStateFlow<List<JobRecord>>(emptyList())
    /** Newest first, as the desktop lists it. */
    val queue: StateFlow<List<JobRecord>> = _queue.asStateFlow()

    private val _activeCount = MutableStateFlow(0)
    /** Jobs queued or running. */
    val activeCount: StateFlow<Int> = _activeCount.asStateFlow()

    private val _history = MutableStateFlow<List<HistoryRecord>>(emptyList())
    val history: StateFlow<List<HistoryRecord>> = _history.asStateFlow()

    private val _settings = MutableStateFlow<SettingsRecord?>(null)
    /** Loaded once; from then on the app is its only writer (see [updateSettings]). */
    val settings: StateFlow<SettingsRecord?> = _settings.asStateFlow()

    private val _tools = MutableStateFlow<ToolsStatusRecord?>(null)
    val tools: StateFlow<ToolsStatusRecord?> = _tools.asStateFlow()

    /** What the Download page's pickers offer; fixed, so read once the core has loaded. */
    var choiceLists: DownloadChoiceListsRecord? = null
        private set

    /** The player and playlists; set just before [status] turns Ready. */
    var music: MusicState? = null
        private set
    @Volatile private var musicCore: app.redline.core.Music? = null

    /** What the Convert page's pickers offer; fixed like [choiceLists]. */
    var convertLists: ConvertChoiceListsRecord? = null
        private set

    private val _encoders = MutableStateFlow(EncodersRecord(emptyList()))
    /** Video encoders ffmpeg can use here; CPU only until detection (at start) finishes. */
    val encoders: StateFlow<EncodersRecord> = _encoders.asStateFlow()

    /** Copies of files picked for the Convert page. */
    val convertInputs = ConvertInputs(RedlineEngine.convertInputsDir(app))
    /** The copies queued, failed and cancelled jobs read, as of the last refresh. */
    private var heldByJobs: Set<String>? = null

    /** Set by the app's activity tracker; finished-job notifications only go out when false. */
    @Volatile var inForeground = false

    /** Nudges from the engine, collapsed: one refresh covers any number of them. */
    private val refreshes = Channel<Unit>(Channel.CONFLATED)
    /** History is only re-read when jobs finish or it's edited: each read checks every file. */
    @Volatile private var historyStale = true

    val engine: Engine?
        get() = (status.value as? EngineStatus.Ready)?.engine

    fun start() {
        if (status.value !is EngineStatus.Initializing || startedOnce) return
        startedOnce = true
        scope.launch {
            val result = withContext(Dispatchers.IO) {
                runCatching {
                    YoutubeDL.init(app)
                    FFmpeg.init(app)
                    val listener = Nudge(WeakReference(this@RedlineState))
                    val engine = RedlineEngine.start(app) { listener.changed() }
                    convertLists = convertChoiceLists()
                    // Loads the playlists and opens the audio output's thread; one per engine.
                    musicCore = engine.music(MusicNudge(WeakReference(this@RedlineState)))
                    engine to downloadChoiceLists()
                }
            }
            result.fold(
                onSuccess = { (engine, lists) ->
                    choiceLists = lists
                    music = musicCore?.let { MusicState(app, it) }
                    _status.value = EngineStatus.Ready(engine)
                    launch { refreshLoop() }
                    launch { _settings.value = onEngine { it.settings() } }
                    launch { _encoders.value = onEngineSlow { it.detectEncoders() } }
                    refreshTools()
                },
                onFailure = { e ->
                    // UnsatisfiedLinkError lands here too when libredline_mobile.so is missing.
                    Log.e(TAG, "Redline couldn't start", e)
                    _status.value = EngineStatus.Failed(e.message ?: e.toString())
                },
            )
        }
    }

    private var startedOnce = false

    /** Called on the engine's own thread; just queues a refresh. */
    internal fun requestRefresh() {
        refreshes.trySend(Unit)
    }

    private suspend fun refreshLoop() {
        for (unit in refreshes) refresh()
    }

    /** Pulls snapshots off the main thread, then publishes them on it. */
    private suspend fun refresh() {
        val engine = engine ?: return
        val stale = historyStale
        historyStale = false
        val snap = withContext(engineThread) {
            val batch = engine.drainFinished()
            Snapshot(
                queue = engine.queueSnapshot(),
                active = engine.activeCount().toInt(),
                history = if (stale || batch.jobs.isNotEmpty()) engine.history() else null,
                announcement = batch.announcement?.takeIf { batch.notify },
            )
        }
        _queue.value = snap.queue
        _activeCount.value = snap.active
        snap.history?.let { _history.value = it }
        snap.announcement?.let { if (!inForeground) Notifications.finished(app, it) }
        if (snap.active > 0) DownloadService.ensureRunning(app)
        // Copies of converted files go once their jobs are done (or removed from the list).
        val held = snap.queue.filter { it.state !in releasing }.flatMap { it.inputs }.toSet()
        if (held != heldByJobs) {
            heldByJobs = held
            scope.launch {
                // History says whether a conversion can run again, which needs its inputs.
                if (convertInputs.sweep(engine) > 0) {
                    historyStale = true
                    requestRefresh()
                }
            }
        }
    }

    private class Snapshot(
        val queue: List<JobRecord>,
        val active: Int,
        val history: List<HistoryRecord>?,
        val announcement: app.redline.core.AnnouncementRecord?,
    )

    /** Runs a quick call on the engine thread, then refreshes (the core pokes too, but late). */
    fun act(touchesHistory: Boolean = false, call: (Engine) -> Unit) {
        val engine = engine ?: return
        scope.launch {
            withContext(engineThread) { call(engine) }
            if (touchesHistory) historyStale = true
            requestRefresh()
        }
    }

    /** Runs [block] in the app's scope, for work that outlives a screen. */
    fun launchDetached(block: suspend () -> Unit) {
        scope.launch { block() }
    }

    /** A quick call whose result the caller needs. Throws if the core hasn't loaded. */
    suspend fun <T> onEngine(call: (Engine) -> T): T {
        val engine = engine ?: error("Redline is still starting.")
        return withContext(engineThread) { call(engine) }
    }

    /** A call that runs yt-dlp and blocks for seconds. */
    suspend fun <T> onEngineSlow(call: (Engine) -> T): T {
        val engine = engine ?: error("Redline is still starting.")
        return withContext(Dispatchers.IO) { call(engine) }
    }

    /**
     * Changes the settings at once in the UI and writes them back in order behind it. The
     * engine isn't re-read afterwards: nothing else changes these fields in this slice, and a
     * re-read racing a text field's next keystroke would jump its cursor.
     */
    fun updateSettings(change: (SettingsRecord) -> SettingsRecord) {
        val current = _settings.value ?: return
        val next = change(current)
        if (next == current) return
        _settings.value = next
        act { it.setSettings(next) }
    }

    /** Re-checks the tools (runs `yt-dlp --version`). */
    fun refreshTools() {
        scope.launch {
            _tools.value = runCatching { onEngineSlow { it.toolsStatus() } }.getOrNull()
        }
    }

    private companion object {
        const val TAG = "Redline"
        /** Jobs in these states don't read their inputs again. */
        val releasing = setOf(JobState.DONE, JobState.SKIPPED)
    }
}

/**
 * The engine's listener. It holds the state only weakly: the state holds the engine, whose Rust
 * side holds this listener, and a strong reference back would make a cycle through the FFI
 * handle map that neither garbage collector can see.
 */
private class Nudge(private val state: WeakReference<RedlineState>) {
    fun changed() {
        state.get()?.requestRefresh()
    }
}
