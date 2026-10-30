package app.redline.ui

import android.Manifest
import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.QueueMusic
import androidx.compose.material.icons.rounded.Download
import androidx.compose.material.icons.rounded.ErrorOutline
import androidx.compose.material.icons.rounded.History
import androidx.compose.material.icons.rounded.LibraryMusic
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material.icons.rounded.SwapHoriz
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.redline.NavModel
import app.redline.core.PlayFailureRecord
import app.redline.core.TrackRecord
import app.redline.state.EngineStatus
import app.redline.state.MusicState
import app.redline.state.RedlineState
import app.redline.ui.components.LocalOpenSearch
import app.redline.ui.components.LocalOpenSettings
import app.redline.ui.convert.ConvertMessage
import app.redline.ui.components.LocalTrackActions
import app.redline.ui.components.ReplacementDialog
import app.redline.ui.components.TrackActions
import app.redline.ui.convert.ConvertModel
import app.redline.ui.convert.ConvertScreen
import app.redline.ui.download.DownloadMessage
import app.redline.ui.download.DownloadModel
import app.redline.ui.download.DownloadScreen
import app.redline.ui.history.HistoryAction
import app.redline.ui.history.HistoryScreen
import app.redline.ui.library.LibraryScreen
import app.redline.ui.batch.BatchModel
import app.redline.ui.library.LibrarySection
import app.redline.ui.local.LocalMessage
import app.redline.ui.local.LocalModel
import app.redline.ui.player.FullPlayer
import app.redline.ui.player.MiniPlayer
import app.redline.ui.playlists.AddToPlaylistDialog
import app.redline.ui.playlists.PlaylistsMessage
import app.redline.ui.playlists.PlaylistsModel
import app.redline.ui.search.SearchScreen
import app.redline.ui.queue.QueueAction
import app.redline.ui.queue.QueueScreen
import app.redline.ui.saved.SavedMessage
import app.redline.ui.saved.SavedModel
import app.redline.ui.settings.SettingsScreen
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import app.redline.util.openFile
import app.redline.work.Notifications
import kotlinx.coroutines.launch

/**
 * The pages: the desktop sidebar's entries, in its order. Five fit a phone's bottom bar, so
 * Saved and Playlists share Library, and Settings is reached from the gear in each page's
 * header instead.
 */
enum class Tab(val label: String, val icon: ImageVector, val inBar: Boolean = true) {
    Download("Download", Icons.Rounded.Download),
    Convert("Convert", Icons.Rounded.SwapHoriz),
    Library("Library", Icons.Rounded.LibraryMusic),
    Queue("Queue", Icons.AutoMirrored.Rounded.QueueMusic),
    History("History", Icons.Rounded.History),
    Settings("Settings", Icons.Rounded.Settings, inBar = false),
}

@Composable
fun RedlineRoot(
    state: RedlineState,
    download: DownloadModel,
    batch: BatchModel,
    convert: ConvertModel,
    playlists: PlaylistsModel,
    saved: SavedModel,
    local: LocalModel,
    nav: NavModel,
) {
    val status by state.status.collectAsStateWithLifecycle()
    when (val s = status) {
        EngineStatus.Initializing -> Starting()
        is EngineStatus.Failed -> Failed(s.message)
        is EngineStatus.Ready -> {
            LaunchedEffect(Unit) {
                download.onEngineReady()
                saved.onMusicReady()
            }
            AskForNotifications()
            val music = state.music
            if (music == null) Failed("The player didn't start.") else Pages(state, music, download, batch, convert, playlists, saved, local, nav)
        }
    }
}

/** Asks once for notification permission (Android 13+), which the progress and done notices need. */
@Composable
private fun AskForNotifications() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
    val context = LocalContext.current
    var asked by rememberSaveable { mutableStateOf(false) }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { }
    LaunchedEffect(Unit) {
        if (!asked && !Notifications.canPost(context)) {
            asked = true
            launcher.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
}

@Composable
private fun Pages(
    state: RedlineState,
    music: MusicState,
    download: DownloadModel,
    batch: BatchModel,
    convert: ConvertModel,
    playlistsModel: PlaylistsModel,
    savedModel: SavedModel,
    localModel: LocalModel,
    nav: NavModel,
) {
    val tab = nav.tab
    var searchOpen by rememberSaveable { mutableStateOf(false) }
    val selectTab: (Tab) -> Unit = { nav.tab = it }
    val context = LocalContext.current
    val playlists by music.playlists.collectAsStateWithLifecycle()
    // A History or Library row's track waiting for "Add to playlist…" to pick where.
    var toAdd by remember { mutableStateOf<TrackRecord?>(null) }
    // A song that failed to play, with "Find replacement…" open for it.
    var replacingFailed by remember { mutableStateOf<PlayFailureRecord?>(null) }
    val queue by state.queue.collectAsStateWithLifecycle()
    val active by state.activeCount.collectAsStateWithLifecycle()
    val history by state.history.collectAsStateWithLifecycle()
    val settings by state.settings.collectAsStateWithLifecycle()
    val tools by state.tools.collectAsStateWithLifecycle()
    val encoders by state.encoders.collectAsStateWithLifecycle()
    val lists = state.choiceLists
    val convertLists = state.convertLists
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()

    fun say(text: String) {
        scope.launch { snackbar.showSnackbar(text, withDismissAction = true) }
    }

    LaunchedEffect(download) {
        download.messages.collect { m ->
            when (m) {
                is DownloadMessage.Error -> say(m.text)
                is DownloadMessage.Queued -> scope.launch {
                    savedModel.saveQuietly(m.url)
                    val r = snackbar.showSnackbar(
                        "Added “${m.title}” to your queue",
                        actionLabel = "View queue",
                        duration = SnackbarDuration.Short,
                    )
                    if (r == SnackbarResult.ActionPerformed) selectTab(Tab.Queue)
                }
            }
        }
    }

    LaunchedEffect(playlistsModel) {
        playlistsModel.messages.collect { m ->
            when (m) {
                is PlaylistsMessage.Info -> say(m.text)
                is PlaylistsMessage.Error -> say(m.text)
                is PlaylistsMessage.Downloading -> scope.launch {
                    val r = snackbar.showSnackbar(
                        "Downloading “${m.title}” for offline",
                        actionLabel = "View queue",
                        duration = SnackbarDuration.Short,
                    )
                    if (r == SnackbarResult.ActionPerformed) selectTab(Tab.Queue)
                }
            }
        }
    }

    LaunchedEffect(savedModel) {
        savedModel.messages.collect { m ->
            when (m) {
                is SavedMessage.Info -> say(m.text)
                is SavedMessage.Error -> say(m.text)
                is SavedMessage.Downloading -> scope.launch {
                    val r = snackbar.showSnackbar(m.text, actionLabel = "View queue", duration = SnackbarDuration.Short)
                    if (r == SnackbarResult.ActionPerformed) selectTab(Tab.Queue)
                }
                // Saved from anywhere (History, Download, the Saved pages): View opens where it went.
                is SavedMessage.Saved -> scope.launch {
                    val r = snackbar.showSnackbar(m.text, actionLabel = "View", duration = SnackbarDuration.Short)
                    if (r == SnackbarResult.ActionPerformed) {
                        nav.librarySection = LibrarySection.Saved
                        selectTab(Tab.Library)
                        savedModel.openPlatform(m.platform)
                        m.collection?.let(savedModel::openCollection)
                    }
                }
            }
        }
    }

    // A link that failed to play (the desktop's notice above the player bar): why, and Find
    // replacement where there's a playlist or Saved copy to put one. The core has already
    // marked its rows and moved on; a newer failure takes the place of the one showing.
    LaunchedEffect(music) {
        music.failures.collect { f ->
            snackbar.currentSnackbarData?.dismiss()
            scope.launch {
                val reason = f.error.lineSequence().firstOrNull { it.isNotBlank() }.orEmpty()
                val r = snackbar.showSnackbar(
                    "Couldn't play “${f.track.title}”. $reason",
                    actionLabel = if (f.playlistId != null || f.saved) "Find replacement" else null,
                    withDismissAction = true,
                    duration = SnackbarDuration.Long,
                )
                if (r == SnackbarResult.ActionPerformed) replacingFailed = f
            }
        }
    }

    LaunchedEffect(localModel) {
        localModel.messages.collect { m ->
            when (m) {
                is LocalMessage.Info -> say(m.text)
                is LocalMessage.Error -> say(m.text)
            }
        }
    }

    LaunchedEffect(convert) {
        convert.messages.collect { m ->
            when (m) {
                is ConvertMessage.Error -> say(m.text)
                // As on the desktop, converting moves to the queue.
                ConvertMessage.Queued -> selectTab(Tab.Queue)
            }
        }
    }

    /** "Convert" on a finished download or conversion: it's in Redline's folders already. */
    fun convertFile(path: String) {
        convert.addPaths(listOf(path))
        selectTab(Tab.Convert)
    }

    // What Library rows send songs to: Convert (a song's file, already in the Library, so not
    // added to Local again) and the "Add to playlist…" dialog below.
    val trackActions = remember(music) {
        TrackActions(
            convert = { t ->
                scope.launch {
                    val path = music.onMusic { it.localFile(t) }
                    if (path == null) {
                        say("That song isn't a file on this phone.")
                    } else {
                        convert.addPaths(listOf(path), keepInLocal = false)
                        nav.tab = Tab.Convert
                    }
                }
            },
            addToPlaylist = { toAdd = it },
        )
    }

    Box(Modifier.fillMaxSize()) {
        Scaffold(
            containerColor = RedlineColors.Bg,
            // Over the full player instead while it's open (below).
            snackbarHost = { if (!nav.playerOpen) SnackbarHost(snackbar) },
            bottomBar = {
                Column(Modifier.background(RedlineColors.Black)) {
                    MiniPlayer(music) { nav.playerOpen = true }
                    BottomBar(tab, active, selectTab)
                }
            },
        ) { padding ->
            Box(Modifier.padding(padding).fillMaxSize()) {
                val s = settings
                if (s == null || lists == null || convertLists == null) {
                    Starting()
                    return@Box
                }
                val openSettings = { selectTab(Tab.Settings) }
                CompositionLocalProvider(
                    LocalOpenSettings provides openSettings.takeIf { tab != Tab.Settings },
                    LocalOpenSearch provides { searchOpen = true },
                ) {
                    when (tab) {
                        Tab.Download -> DownloadScreen(
                            download,
                            batch,
                            s,
                            lists,
                            tools,
                            history,
                            openSettings = openSettings,
                            onSave = savedModel::saveLink,
                            isSaving = savedModel::isSaving,
                            onOpenQueue = { selectTab(Tab.Queue) },
                            onOpenSaved = {
                                savedModel.showOverview()
                                nav.librarySection = LibrarySection.Saved
                                selectTab(Tab.Library)
                            },
                        )
                        Tab.Convert -> ConvertScreen(convert, s, convertLists, encoders, tools, openSettings = openSettings)
                        Tab.Library -> CompositionLocalProvider(LocalTrackActions provides trackActions) {
                            LibraryScreen(
                                section = nav.librarySection,
                                onSection = { nav.librarySection = it },
                                saved = savedModel,
                                playlistsModel = playlistsModel,
                                local = localModel,
                                music = music,
                                playlists = playlists,
                                history = history,
                            )
                        }
                        Tab.Queue -> QueueScreen(
                            queue,
                            onAction = { a ->
                                when (a) {
                                    is QueueAction.Cancel -> state.act { it.cancel(a.id) }
                                    is QueueAction.Retry -> state.act { it.retry(a.id) }
                                    is QueueAction.RetryAnyway -> state.act { it.retryIgnoringArchive(a.id) }
                                    is QueueAction.Remove -> state.act { it.remove(a.id) }
                                    QueueAction.ClearFinished -> state.act { it.clearFinished() }
                                    QueueAction.CancelAll -> state.act { it.cancelAll() }
                                    is QueueAction.Open -> openFile(context, a.path)?.let(::say)
                                    is QueueAction.Convert -> convertFile(a.path)
                                }
                            },
                            findSomething = { selectTab(Tab.Download) },
                        )
                        Tab.History -> HistoryScreen(history) { a ->
                            when (a) {
                                is HistoryAction.Open -> openFile(context, a.path)?.let(::say)
                                is HistoryAction.Convert -> convertFile(a.path)
                                is HistoryAction.Remove -> state.act(touchesHistory = true) { it.removeHistory(a.index) }
                                HistoryAction.Clear -> state.act(touchesHistory = true) { it.clearHistory() }
                                is HistoryAction.Again -> scope.launch {
                                    try {
                                        state.onEngine { it.downloadAgain(a.index) }
                                        state.act { }
                                        selectTab(Tab.Queue)
                                    } catch (e: app.redline.core.RedlineException) {
                                        say(e.message ?: "Couldn't queue that again.")
                                    }
                                }
                                is HistoryAction.Play -> scope.launch {
                                    try {
                                        music.onMusic { it.playHistory(a.index) }
                                        music.act { }
                                    } catch (e: app.redline.core.RedlineException) {
                                        say(e.message ?: "Couldn't play that.")
                                    }
                                }
                                is HistoryAction.Save -> savedModel.saveLink(a.url)
                                is HistoryAction.AddToPlaylist -> scope.launch {
                                    val track = music.onMusic { it.trackForHistory(a.index) }
                                    if (track == null) say("There's nothing to play there any more.") else toAdd = track
                                }
                            }
                        }
                        Tab.Settings -> {
                            // Not in the bottom bar: Back returns to the page it was opened from.
                            BackHandler { selectTab(nav.backFromSettings) }
                            SettingsScreen(state, s, tools, lists)
                        }
                    }
                }
            }
        }
        // The full player covers everything, the bottom bar included.
        AnimatedVisibility(
            visible = nav.playerOpen,
            enter = slideInVertically { it },
            exit = slideOutVertically { it },
        ) {
            FullPlayer(music) { nav.playerOpen = false }
        }
        // Messages (a song that failed to play, say) show over the full player too.
        if (nav.playerOpen) SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter).navigationBarsPadding())
        if (searchOpen) {
            SearchScreen(
                music,
                savedModel,
                playlistsModel,
                localModel.tracks,
                onBack = { searchOpen = false },
                onOpenSaved = {
                    nav.librarySection = LibrarySection.Saved
                    selectTab(Tab.Library)
                },
                onOpenPlaylist = {
                    nav.librarySection = LibrarySection.Playlists
                    selectTab(Tab.Library)
                },
                onGo = selectTab,
            )
        }
    }
    toAdd?.let { track ->
        AddToPlaylistDialog(
            playlists,
            onPick = { id ->
                music.editPlaylists { it.addTracks(id, listOf(track)) }
                say("Added to ${playlists.firstOrNull { it.id == id }?.name ?: "the playlist"}")
            },
            onNew = { name ->
                scope.launch {
                    music.editPlaylistsNow { m -> m.addTracks(m.createPlaylist(name.trim()), listOf(track)) }
                    say("Added to a new playlist")
                }
            },
            onDismiss = { toAdd = null },
        )
    }
    replacingFailed?.let { f ->
        ReplacementDialog(
            f.track,
            search = { playlistsModel.findReplacements(f.track) },
            onPick = { pick -> playlistsModel.replaceFailed(f, pick) },
            onDismiss = { replacingFailed = null },
        )
    }
}

/** The desktop sidebar as a phone's bottom bar, with the Queue's active-count badge. */
@Composable
private fun BottomBar(tab: Tab, active: Int, selectTab: (Tab) -> Unit) {
    val accent = LocalAccent.current
    NavigationBar(containerColor = RedlineColors.Black) {
        for (t in Tab.entries.filter { it.inBar }) {
            NavigationBarItem(
                selected = t == tab,
                onClick = { selectTab(t) },
                icon = {
                    if (t == Tab.Queue && active > 0) {
                        BadgedBox(badge = { Badge(containerColor = accent.base, contentColor = RedlineColors.Text) { Text("$active") } }) {
                            Icon(t.icon, contentDescription = t.label)
                        }
                    } else {
                        Icon(t.icon, contentDescription = t.label)
                    }
                },
                label = { Text(t.label, style = MaterialTheme.typography.labelMedium, maxLines = 1, softWrap = false, overflow = TextOverflow.Visible) },
                colors = NavigationBarItemDefaults.colors(
                    selectedIconColor = RedlineColors.Text,
                    selectedTextColor = RedlineColors.Text,
                    indicatorColor = RedlineColors.Elevated,
                    unselectedIconColor = RedlineColors.Muted,
                    unselectedTextColor = RedlineColors.Muted,
                ),
            )
        }
    }
}

/** While youtubedl-android unpacks Python (seconds on the first run) and the core loads. */
@Composable
private fun Starting() {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
            CircularProgressIndicator(color = LocalAccent.current.base)
            Text("Getting Redline ready…", color = RedlineColors.Muted)
        }
    }
}

@Composable
private fun Failed(message: String) {
    Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(Icons.Rounded.ErrorOutline, null, tint = LocalAccent.current.base, modifier = Modifier.size(48.dp))
            Text("Redline couldn't start", style = MaterialTheme.typography.titleLarge, color = RedlineColors.Text)
            Text(message, color = RedlineColors.Muted, textAlign = TextAlign.Center)
        }
    }
}
