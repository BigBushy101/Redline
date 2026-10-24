package app.redline.ui.playlists

import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.PlaylistAdd
import androidx.compose.material.icons.automirrored.rounded.PlaylistPlay
import androidx.compose.material.icons.automirrored.rounded.QueueMusic
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.BookmarkBorder
import androidx.compose.material.icons.rounded.Delete
import androidx.compose.material.icons.rounded.Download
import androidx.compose.material.icons.rounded.Edit
import androidx.compose.material.icons.rounded.FindReplace
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.material.icons.rounded.KeyboardArrowUp
import androidx.compose.material.icons.rounded.Link
import androidx.compose.material.icons.rounded.MoreVert
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.QueuePlayNext
import androidx.compose.material.icons.rounded.RemoveCircleOutline
import androidx.compose.material.icons.rounded.Shuffle
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.redline.core.HistoryRecord
import app.redline.core.PlayerSnapshotRecord
import app.redline.core.PlaylistRecord
import app.redline.core.PlaylistSummaryRecord
import app.redline.core.QueueTrackRecord
import app.redline.core.TrackKind
import app.redline.core.TrackRecord
import app.redline.core.fmtDuration
import app.redline.state.MusicState
import app.redline.ui.components.Cover
import app.redline.ui.components.EmptyState
import app.redline.ui.components.Eyebrow
import app.redline.ui.components.GhostButton
import app.redline.ui.components.InputField
import app.redline.ui.components.LibraryMenuItems
import app.redline.ui.components.MenuItem
import app.redline.ui.components.PageHeader
import app.redline.ui.components.PagePadding
import app.redline.ui.components.PillButton
import app.redline.ui.components.ReplacementDialog
import app.redline.ui.components.ReviewDialog
import app.redline.ui.components.SectionCard
import app.redline.ui.components.TrackRow
import app.redline.ui.components.UnavailableBar
import app.redline.ui.components.dead
import app.redline.ui.components.unavailable
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors

/** What the picker offers for "Add files": songs, and videos (their sound plays). */
private val PICK_TYPES = arrayOf("audio/*", "video/*")

/**
 * Port of src/ui/playlists.rs for a phone: the list of playlists, and one playlist at a time
 * with Play / Shuffle, its tracks and the ways to add more (a link, or files from the picker).
 */
@Composable
fun PlaylistsScreen(
    model: PlaylistsModel,
    music: MusicState,
    playlists: List<PlaylistSummaryRecord>,
    history: List<HistoryRecord>,
) {
    val version by music.playlistsVersion.collectAsStateWithLifecycle()
    val id = model.openId
    val savedVersion by music.savedVersion.collectAsStateWithLifecycle()
    val linkVersion by music.linkCheckVersion.collectAsStateWithLifecycle()
    // Re-read after edits, when History changes which links have a downloaded copy, when
    // Saved changes which are saved, and when link checks find dead ones.
    LaunchedEffect(id, version, history, savedVersion, linkVersion) { model.reload() }
    if (id == null) {
        PlaylistList(model, playlists)
    } else {
        BackHandler { model.close() }
        val snapshot by music.snapshot.collectAsStateWithLifecycle()
        val queue by music.queue.collectAsStateWithLifecycle()
        val detail = model.detail
        if (detail == null || detail.id != id) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(color = LocalAccent.current.base)
            }
        } else {
            PlaylistDetail(model, detail, snapshot, queue)
        }
    }
}

// ---- The list ---------------------------------------------------------------------------

@Composable
private fun PlaylistList(model: PlaylistsModel, playlists: List<PlaylistSummaryRecord>) {
    var creating by rememberSaveable { mutableStateOf(false) }
    val total = playlists.sumOf { it.trackCount.toInt() }
    val n = playlists.size
    val subtitle = "$n playlist${if (n == 1) "" else "s"} · ${trackCount(total)}"
    if (playlists.isEmpty()) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            PageHeader("Playlists", "Your playlists", subtitle)
            EmptyState(
                Icons.AutoMirrored.Rounded.QueueMusic,
                "Make your first playlist",
                "Mix links from YouTube, SoundCloud and more with songs on this phone. Links stream when you play them.",
            ) { PillButton("New playlist", { creating = true }, icon = Icons.Rounded.Add) }
        }
    } else {
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
            item { PageHeader("Playlists", "Your playlists", subtitle) }
            item {
                Box(Modifier.padding(horizontal = PagePadding).padding(bottom = 12.dp)) {
                    GhostButton("New playlist", { creating = true })
                }
            }
            items(playlists, key = { it.id.toLong() }) { p ->
                PlaylistRow(p) { model.open(p.id) }
                Spacer(Modifier.height(4.dp))
            }
        }
    }
    if (creating) {
        NameDialog("New playlist", "", "Create", onConfirm = { model.create(it) }, onDismiss = { creating = false })
    }
}

@Composable
private fun PlaylistRow(p: PlaylistSummaryRecord, open: () -> Unit) {
    Row(
        Modifier
            .padding(horizontal = PagePadding)
            .fillMaxWidth()
            .background(RedlineColors.Card, MaterialTheme.shapes.small)
            .clickable(onClick = open)
            .padding(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Cover(p.thumbnail, Icons.AutoMirrored.Rounded.PlaylistPlay, size = 56.dp, tint = LocalAccent.current.base)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(p.name, style = MaterialTheme.typography.titleMedium, color = RedlineColors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(meta(p.trackCount.toInt(), p.durationSecs), style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
        }
    }
}

/** "12 tracks · 43:10". */
private fun meta(count: Int, secs: Double) =
    trackCount(count) + if (secs > 0) " · ${fmtDuration(secs)}" else ""

// ---- One playlist -----------------------------------------------------------------------

@Composable
private fun PlaylistDetail(model: PlaylistsModel, p: PlaylistRecord, snapshot: PlayerSnapshotRecord?, queue: List<QueueTrackRecord>) {
    var renaming by rememberSaveable { mutableStateOf(false) }
    var deleting by rememberSaveable { mutableStateOf(false) }
    // Only the unavailable songs, while the chip is on and there are some.
    var onlyUnavailable by rememberSaveable(p.id) { mutableStateOf(false) }
    // The song "Find replacement…" is open for, and its place.
    var replacing by remember(p.id) { mutableStateOf<Pair<Int, TrackRecord>?>(null) }
    val unavailable = p.tracks.count { it.unavailable }
    val filtering = onlyUnavailable && unavailable > 0
    val rows = p.tracks.withIndex().filter { !filtering || it.value.unavailable }
    // The row playing: this playlist's queue, at this track's place.
    val current = queue.firstOrNull { it.isCurrent }?.track?.location?.takeIf { snapshot?.playlistId == p.id }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item {
            DetailHeader(
                p,
                back = { model.close() },
                play = { model.play(p.id) },
                shuffle = { model.play(p.id, shuffle = true) },
                rename = { renaming = true },
                delete = { deleting = true },
            )
        }
        if (unavailable > 0) {
            item {
                UnavailableBar(
                    count = unavailable,
                    filtered = filtering,
                    onFilter = { onlyUnavailable = !filtering },
                    canFix = p.tracks.any { it.dead },
                    fixing = model.fixing == p.id,
                    onFix = { model.fixUnavailable(p.id) },
                )
            }
        }
        item { AddBar(model, p.id) }
        if (p.tracks.isEmpty()) {
            item {
                Text(
                    "Add songs with a link or from your files.",
                    Modifier.fillMaxWidth().padding(horizontal = PagePadding, vertical = 16.dp),
                    color = RedlineColors.Muted,
                    textAlign = TextAlign.Center,
                )
            }
        }
        items(rows, key = { (i, t) -> "$i:${t.location}" }) { (i, t) ->
            PlaylistTrackRow(
                index = i,
                track = t,
                count = p.tracks.size,
                playing = t.location == current,
                saved = t.location in model.savedLinks,
                onAction = { a ->
                    when (a) {
                        TrackAction.Play -> model.play(p.id, start = i)
                        TrackAction.Next -> model.playNext(t)
                        TrackAction.Queue -> model.queue(t)
                        TrackAction.Offline -> model.downloadForOffline(t)
                        TrackAction.Save -> model.saveTrack(t)
                        TrackAction.Replace -> replacing = i to t
                        TrackAction.Up -> model.move(p.id, i, i - 1)
                        TrackAction.Down -> model.move(p.id, i, i + 1)
                        TrackAction.Remove -> model.remove(p.id, i)
                    }
                },
            )
        }
    }
    replacing?.let { (i, t) ->
        ReplacementDialog(
            t,
            search = { model.findReplacements(t) },
            onPick = { pick -> model.replace(p.id, i, pick) },
            onDismiss = { replacing = null },
        )
    }
    model.review?.takeIf { it.first == p.id }?.let { (_, items) ->
        ReviewDialog(items, onPick = { item, pick -> model.replace(p.id, item.index.toInt(), pick) }, onDismiss = model::closeReview)
    }
    if (renaming) {
        NameDialog("Rename playlist", p.name, "Rename", onConfirm = { model.rename(p.id, it) }, onDismiss = { renaming = false })
    }
    if (deleting) {
        AlertDialog(
            onDismissRequest = { deleting = false },
            containerColor = RedlineColors.Elevated,
            title = { Text("Delete “${p.name}”?") },
            text = { Text("The playlist goes; the songs' files and downloads stay where they are.", color = RedlineColors.Muted) },
            confirmButton = {
                TextButton(onClick = {
                    deleting = false
                    model.delete(p.id)
                }) { Text("Delete", color = LocalAccent.current.hover) }
            },
            dismissButton = { TextButton(onClick = { deleting = false }) { Text("Cancel", color = RedlineColors.Muted) } },
        )
    }
}

/** The desktop's playlist card: art, name, count · length, Play and Shuffle, the menu. */
@Composable
private fun DetailHeader(
    p: PlaylistRecord,
    back: () -> Unit,
    play: () -> Unit,
    shuffle: () -> Unit,
    rename: () -> Unit,
    delete: () -> Unit,
) {
    val accent = LocalAccent.current
    var menu by remember { mutableStateOf(false) }
    Column(
        Modifier
            .fillMaxWidth()
            .background(Brush.verticalGradient(listOf(accent.headerTint, RedlineColors.Bg)))
            .padding(bottom = 16.dp),
    ) {
        Row(Modifier.fillMaxWidth().padding(4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = back) { Icon(Icons.AutoMirrored.Rounded.ArrowBack, "Back to playlists", tint = RedlineColors.Text) }
            Spacer(Modifier.weight(1f))
            Box {
                IconButton(onClick = { menu = true }) { Icon(Icons.Rounded.MoreVert, "Playlist options", tint = RedlineColors.Text) }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, containerColor = RedlineColors.Elevated) {
                    MenuItem("Rename", Icons.Rounded.Edit) {
                        menu = false
                        rename()
                    }
                    MenuItem("Delete playlist", Icons.Rounded.Delete) {
                        menu = false
                        delete()
                    }
                }
            }
        }
        Row(Modifier.padding(horizontal = PagePadding), verticalAlignment = Alignment.CenterVertically) {
            Cover(p.tracks.firstNotNullOfOrNull { it.thumbnail }, Icons.AutoMirrored.Rounded.PlaylistPlay, size = 112.dp, tint = accent.base)
            Spacer(Modifier.width(16.dp))
            Column(Modifier.weight(1f)) {
                Eyebrow("Playlist")
                Text(p.name, style = MaterialTheme.typography.headlineSmall, color = RedlineColors.Text, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(meta(p.tracks.size, p.durationSecs), style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
            }
        }
        Spacer(Modifier.height(16.dp))
        Row(Modifier.padding(horizontal = PagePadding), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
            PillButton("Play", play, icon = Icons.Rounded.PlayArrow, enabled = p.tracks.isNotEmpty())
            ShuffleButton(enabled = p.tracks.isNotEmpty(), onClick = shuffle)
        }
    }
}

/** Outlined Shuffle pill beside Play (also on Saved's collections). */
@Composable
fun ShuffleButton(enabled: Boolean, onClick: () -> Unit) {
    androidx.compose.material3.OutlinedButton(
        onClick = onClick,
        enabled = enabled,
        modifier = Modifier.height(48.dp),
        shape = androidx.compose.foundation.shape.CircleShape,
        border = androidx.compose.foundation.BorderStroke(1.dp, if (enabled) RedlineColors.Subtle else RedlineColors.Divider),
        colors = androidx.compose.material3.ButtonDefaults.outlinedButtonColors(
            contentColor = RedlineColors.Text,
            disabledContentColor = RedlineColors.Subtle,
        ),
    ) {
        Icon(Icons.Rounded.Shuffle, null, modifier = Modifier.size(20.dp))
        Spacer(Modifier.width(8.dp))
        Text("Shuffle", style = MaterialTheme.typography.labelLarge)
    }
}

/** Paste a link (a playlist link adds every entry) or pick files (the desktop's add bar). */
@Composable
private fun AddBar(model: PlaylistsModel, id: ULong) {
    val focus = LocalFocusManager.current
    var link by rememberSaveable { mutableStateOf("") }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris -> model.addUris(id, uris) }
    val add = {
        focus.clearFocus()
        model.addLink(id, link)
        link = ""
    }
    Column(Modifier.padding(horizontal = PagePadding).padding(bottom = 12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        InputField(
            link,
            { link = it },
            placeholder = "Paste a link to add",
            leading = Icons.Rounded.Link,
            shapeRound = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { add() }),
            trailing = if (link.isNotBlank()) {
                { IconButton(onClick = add) { Icon(Icons.AutoMirrored.Rounded.PlaylistAdd, "Add link", tint = LocalAccent.current.base) } }
            } else {
                null
            },
        )
        GhostButton("Add files from this phone", { pick.launch(PICK_TYPES) })
        for (a in model.adding.filter { it.playlist == id }) {
            SectionCard {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(Modifier.size(20.dp), color = LocalAccent.current.base, strokeWidth = 2.dp)
                    Spacer(Modifier.width(12.dp))
                    Text(if (a.what == "link") "Looking up link…" else "Adding ${a.what}…", color = RedlineColors.Muted)
                }
            }
        }
    }
}

private enum class TrackAction { Play, Next, Queue, Offline, Save, Replace, Up, Down, Remove }

/** One track, with the playlist's menu (playlists.rs's row buttons). */
@Composable
private fun PlaylistTrackRow(index: Int, track: TrackRecord, count: Int, playing: Boolean, saved: Boolean, onAction: (TrackAction) -> Unit) {
    TrackRow(index, track, playing, onClick = { onAction(TrackAction.Play) }) { close ->
        fun pick(a: TrackAction) {
            close()
            onAction(a)
        }
        // Any link can look for another upload; it's the first thing offered for one that won't play.
        if (track.unavailable) MenuItem("Find replacement…", Icons.Rounded.FindReplace) { pick(TrackAction.Replace) }
        MenuItem("Play", Icons.Rounded.PlayArrow) { pick(TrackAction.Play) }
        MenuItem("Play next", Icons.Rounded.QueuePlayNext) { pick(TrackAction.Next) }
        MenuItem("Add to queue", Icons.AutoMirrored.Rounded.QueueMusic) { pick(TrackAction.Queue) }
        if (track.canGoOffline) MenuItem("Download for offline", Icons.Rounded.Download) { pick(TrackAction.Offline) }
        if (track.kind == TrackKind.LINK && !saved) MenuItem("Save to Saved", Icons.Rounded.BookmarkBorder) { pick(TrackAction.Save) }
        if (track.kind == TrackKind.LINK && !track.unavailable) MenuItem("Find replacement…", Icons.Rounded.FindReplace) { pick(TrackAction.Replace) }
        LibraryMenuItems(track, close, playlist = false)
        if (index > 0) MenuItem("Move up", Icons.Rounded.KeyboardArrowUp) { pick(TrackAction.Up) }
        if (index + 1 < count) MenuItem("Move down", Icons.Rounded.KeyboardArrowDown) { pick(TrackAction.Down) }
        MenuItem("Remove from playlist", Icons.Rounded.RemoveCircleOutline) { pick(TrackAction.Remove) }
    }
}
