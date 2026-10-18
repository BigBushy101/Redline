package app.redline.ui.saved

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.PlaylistAdd
import androidx.compose.material.icons.automirrored.rounded.PlaylistPlay
import androidx.compose.material.icons.automirrored.rounded.QueueMusic
import androidx.compose.material.icons.rounded.BookmarkAdd
import androidx.compose.material.icons.rounded.Bookmarks
import androidx.compose.material.icons.rounded.ChevronRight
import androidx.compose.material.icons.rounded.ClearAll
import androidx.compose.material.icons.rounded.Delete
import androidx.compose.material.icons.rounded.Download
import androidx.compose.material.icons.rounded.DownloadForOffline
import androidx.compose.material.icons.rounded.Edit
import androidx.compose.material.icons.rounded.ErrorOutline
import androidx.compose.material.icons.rounded.LibraryMusic
import androidx.compose.material.icons.rounded.Link
import androidx.compose.material.icons.rounded.MoreVert
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.QueuePlayNext
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.RemoveCircleOutline
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
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.redline.core.CollectionRecord
import app.redline.core.CollectionSummaryRecord
import app.redline.core.HistoryRecord
import app.redline.core.Platform
import app.redline.core.PlaylistSummaryRecord
import app.redline.core.RefreshState
import app.redline.core.SavedOverviewRecord
import app.redline.core.SavedPlatformPageRecord
import app.redline.core.SavedPlatformRecord
import app.redline.core.SavedTrackRecord
import app.redline.core.TrackRecord
import app.redline.core.fmtAgo
import app.redline.core.fmtDuration
import app.redline.state.MusicState
import app.redline.ui.components.ArtTile
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
import app.redline.ui.components.SectionCard
import app.redline.ui.components.ToggleRow
import app.redline.ui.components.TrackRow
import app.redline.ui.playlists.AddToPlaylistDialog
import app.redline.ui.playlists.NameDialog
import app.redline.ui.playlists.ShuffleButton
import app.redline.ui.playlists.canGoOffline
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import app.redline.ui.theme.WithAccent
import app.redline.ui.theme.accent
import app.redline.ui.theme.color
import app.redline.ui.theme.icon
import app.redline.ui.theme.label

/**
 * Port of src/ui/saved.rs for a phone: the overview (one card per platform), a platform's page
 * (its collections, single songs and "All songs"), and one collection or "All songs" at a time.
 * A platform's pages take its color; the mini player and bottom bar, outside, stay red.
 */
@Composable
fun SavedScreen(model: SavedModel, music: MusicState, history: List<HistoryRecord>, playlists: List<PlaylistSummaryRecord>) {
    val version by music.savedVersion.collectAsStateWithLifecycle()
    val overview by music.savedOverview.collectAsStateWithLifecycle()
    val queue by music.queue.collectAsStateWithLifecycle()
    val current = queue.firstOrNull { it.isCurrent }?.track?.location
    val p = model.platform
    // Re-read after any change to Saved (refreshes included), and when History changes which
    // links have a downloaded copy.
    LaunchedEffect(p, model.collectionId, model.allSongs, version, history) { model.reload() }
    if (p == null) {
        Overview(model, overview)
        return
    }
    BackHandler { model.back() }
    WithAccent(p.accent) {
        val id = model.collectionId
        val collection = model.collection
        val all = model.all
        val page = model.page
        when {
            id != null -> if (collection == null || collection.summary.id != id) Loading() else CollectionPage(model, collection, current, playlists)
            model.allSongs -> if (all == null) Loading() else AllSongsPage(model, p, all, current)
            else -> if (page == null || page.platform != p) Loading() else PlatformPage(model, page, current)
        }
    }
}

@Composable
private fun Loading() {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        CircularProgressIndicator(color = LocalAccent.current.base)
    }
}

/** "1 collection", "3 collections". */
private fun count(n: Int, word: String) = "$n $word${if (n == 1) "" else "s"}"

// ---- The overview -----------------------------------------------------------------------

@Composable
private fun Overview(model: SavedModel, overview: SavedOverviewRecord?) {
    val platforms = overview?.platforms.orEmpty()
    val subtitle = if (overview == null || platforms.isEmpty()) {
        "Save playlists and songs from YouTube, SoundCloud, Bandcamp and more."
    } else {
        "${count(overview.collectionCount.toInt(), "collection")} · ${songs(overview.songCount.toInt())} · ${count(platforms.size, "platform")}"
    }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item { PageHeader("Saved", "Saved music", subtitle) }
        item { SaveBox(model) }
        if (platforms.isEmpty()) {
            item {
                EmptyState(
                    Icons.Rounded.Bookmarks,
                    "Save music from anywhere",
                    "Paste a link above. Playlists, sets and albums stay in step with the original; each platform gets its own page.",
                )
            }
        } else {
            item { SectionTitle("Platforms") }
            items(platforms, key = { it.platform.name }) { p ->
                PlatformCard(p) { model.openPlatform(p.platform) }
                Spacer(Modifier.height(8.dp))
            }
        }
    }
}

/** A card tinted with the platform's color: glyph, name, counts and last refresh. */
@Composable
private fun PlatformCard(p: SavedPlatformRecord, open: () -> Unit) {
    val color = p.platform.color
    Row(
        Modifier
            .padding(horizontal = PagePadding)
            .fillMaxWidth()
            .background(lerp(RedlineColors.Card, color, 0.14f), MaterialTheme.shapes.medium)
            .clickable(onClick = open)
            .padding(16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(p.platform.icon, contentDescription = null, tint = color, modifier = Modifier.size(40.dp))
        Spacer(Modifier.width(16.dp))
        Column(Modifier.weight(1f)) {
            Text(p.label, style = MaterialTheme.typography.titleLarge, color = RedlineColors.Text)
            Text(
                "${count(p.collectionCount.toInt(), "collection")} · ${songs(p.songCount.toInt())}",
                style = MaterialTheme.typography.bodyMedium,
                color = RedlineColors.Muted,
            )
            val status = when {
                p.refreshing > 0u -> "Refreshing ${count(p.refreshing.toInt(), "collection")}…"
                else -> p.syncedAt?.let { "Synced ${fmtAgo(it)}" }
            }
            status?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = RedlineColors.Subtle) }
        }
        Icon(Icons.Rounded.ChevronRight, contentDescription = null, tint = RedlineColors.Muted)
    }
}

/** "Paste a link" with Save, and the links being looked up (saved.rs `save_box`). */
@Composable
private fun SaveBox(model: SavedModel) {
    val focus = LocalFocusManager.current
    var link by rememberSaveable { mutableStateOf("") }
    val save = {
        focus.clearFocus()
        model.saveLink(link)
        link = ""
    }
    Column(Modifier.padding(horizontal = PagePadding).padding(bottom = 12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        InputField(
            link,
            { link = it },
            placeholder = "Paste a playlist or song link to save",
            leading = Icons.Rounded.Link,
            shapeRound = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { save() }),
            trailing = if (link.isNotBlank()) {
                { IconButton(onClick = save) { Icon(Icons.Rounded.BookmarkAdd, "Save", tint = LocalAccent.current.base) } }
            } else {
                null
            },
        )
        for (url in model.saving) {
            SectionCard {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(Modifier.size(20.dp), color = LocalAccent.current.base, strokeWidth = 2.dp)
                    Spacer(Modifier.width(12.dp))
                    Text("Looking up $url…", color = RedlineColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
        }
    }
}

@Composable
private fun SectionTitle(text: String) {
    Text(
        text,
        Modifier.padding(horizontal = PagePadding).padding(top = 8.dp, bottom = 8.dp),
        style = MaterialTheme.typography.titleLarge,
        color = RedlineColors.Text,
    )
}

/** The accent-tinted top of a platform's pages, with Back and room for actions. */
@Composable
private fun TintedHeader(
    back: () -> Unit,
    backLabel: String,
    actions: @Composable RowScope.() -> Unit = {},
    content: @Composable ColumnScope.() -> Unit,
) {
    val accent = LocalAccent.current
    Column(
        Modifier
            .fillMaxWidth()
            .background(Brush.verticalGradient(listOf(accent.headerTint, RedlineColors.Bg)))
            .padding(bottom = 16.dp),
    ) {
        Row(Modifier.fillMaxWidth().padding(4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = back) { Icon(Icons.AutoMirrored.Rounded.ArrowBack, backLabel, tint = RedlineColors.Text) }
            Spacer(Modifier.weight(1f))
            actions()
        }
        content()
    }
}

// ---- A platform -------------------------------------------------------------------------

@Composable
private fun PlatformPage(model: SavedModel, page: SavedPlatformPageRecord, current: String?) {
    val accent = LocalAccent.current
    val collections = page.collections
    val refreshing = page.refreshing.toInt()
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item {
            TintedHeader(back = model::back, backLabel = "Back to Saved") {
                Row(Modifier.padding(horizontal = PagePadding), verticalAlignment = Alignment.CenterVertically) {
                    Icon(page.platform.icon, contentDescription = null, tint = accent.base, modifier = Modifier.size(52.dp))
                    Spacer(Modifier.width(12.dp))
                    Column {
                        Eyebrow("Saved")
                        Text(page.label, style = MaterialTheme.typography.displaySmall, color = RedlineColors.Text)
                    }
                }
                Spacer(Modifier.height(4.dp))
                Text(
                    "${count(collections.size, "collection")} · ${songs(page.songs.size)} saved on their own · ${songs(page.allSongCount.toInt())} in all",
                    Modifier.padding(horizontal = PagePadding),
                    style = MaterialTheme.typography.bodyMedium,
                    color = RedlineColors.Muted,
                )
                Spacer(Modifier.height(12.dp))
                Row(
                    Modifier.padding(horizontal = PagePadding),
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    GhostButton(
                        "Refresh all",
                        { model.refreshAll(page.platform) },
                        enabled = collections.isNotEmpty() && refreshing < collections.size,
                    )
                    if (refreshing > 0) {
                        CircularProgressIndicator(Modifier.size(16.dp), color = accent.base, strokeWidth = 2.dp)
                        Text("Refreshing ${count(refreshing, "collection")}…", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
                    }
                }
            }
        }
        item { SaveBox(model) }
        item { AllSongsRow(page.allSongCount.toInt()) { model.openAllSongs() } }
        item { SectionTitle("Collections") }
        if (collections.isEmpty()) {
            item {
                Text(
                    "No playlists, sets or albums saved from here yet. Paste one above.",
                    Modifier.padding(horizontal = PagePadding, vertical = 4.dp),
                    color = RedlineColors.Muted,
                )
            }
        }
        items(collections, key = { it.id.toLong() }) { c ->
            CollectionRow(c) { model.openCollection(c.id) }
            Spacer(Modifier.height(4.dp))
        }
        if (page.songs.isNotEmpty()) {
            item { SectionTitle("Songs") }
            itemsIndexed(page.songs, key = { i, t -> "song:$i:${t.location}" }) { i, t ->
                TrackRow(i, t, playing = t.location == current, onClick = { model.playSongs(page.songs, i) }) { close ->
                    MenuItem("Play", Icons.Rounded.PlayArrow) {
                        close()
                        model.playSongs(page.songs, i)
                    }
                    MenuItem("Play next", Icons.Rounded.QueuePlayNext) {
                        close()
                        model.playNext(t)
                    }
                    MenuItem("Add to queue", Icons.AutoMirrored.Rounded.QueueMusic) {
                        close()
                        model.queue(t)
                    }
                    LibraryMenuItems(t, close)
                    if (t.canGoOffline) {
                        MenuItem("Download for offline", Icons.Rounded.Download) {
                            close()
                            model.downloadForOffline(t)
                        }
                    }
                    MenuItem("Remove from Saved", Icons.Rounded.RemoveCircleOutline) {
                        close()
                        model.removeSong(t)
                    }
                }
            }
        }
    }
}

/** The way into "All songs": every song saved from the platform, once each. */
@Composable
private fun AllSongsRow(n: Int, open: () -> Unit) {
    Row(
        Modifier
            .padding(horizontal = PagePadding)
            .fillMaxWidth()
            .background(RedlineColors.Card, MaterialTheme.shapes.small)
            .clickable(onClick = open)
            .padding(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ArtTile(Icons.Rounded.LibraryMusic, LocalAccent.current.base, size = 56.dp)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text("All songs", style = MaterialTheme.typography.titleMedium, color = RedlineColors.Text)
            Text("${songs(n)}, from every collection and on their own", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
        }
        Icon(Icons.Rounded.ChevronRight, contentDescription = null, tint = RedlineColors.Muted)
    }
}

/** The status line under a collection's name: its refresh, or when it was last refreshed. */
private fun refreshStatus(c: CollectionSummaryRecord): String? = when (c.refresh) {
    RefreshState.RUNNING -> "Refreshing…"
    RefreshState.WAITING -> "Waiting to refresh"
    RefreshState.IDLE -> if (c.syncError != null) "Couldn't refresh" else c.syncedAt?.let { "Synced ${fmtAgo(it)}" }
}

@Composable
private fun CollectionRow(c: CollectionSummaryRecord, open: () -> Unit) {
    val accent = LocalAccent.current
    Row(
        Modifier
            .padding(horizontal = PagePadding)
            .fillMaxWidth()
            .background(RedlineColors.Card, MaterialTheme.shapes.small)
            .clickable(onClick = open)
            .padding(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Cover(c.thumbnail, Icons.AutoMirrored.Rounded.PlaylistPlay, size = 56.dp, tint = accent.base)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(c.name, style = MaterialTheme.typography.titleMedium, color = RedlineColors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis)
            val removed = c.removedCount.toInt()
            Text(
                songs(c.songCount.toInt()) + if (removed > 0) " · $removed removed" else "",
                style = MaterialTheme.typography.bodySmall,
                color = RedlineColors.Muted,
            )
            refreshStatus(c)?.let {
                Text(it, style = MaterialTheme.typography.bodySmall, color = if (c.syncError != null && c.refresh == RefreshState.IDLE) accent.hover else RedlineColors.Subtle)
            }
        }
        when {
            c.refresh == RefreshState.RUNNING ->
                CircularProgressIndicator(Modifier.padding(end = 4.dp).size(18.dp), color = accent.base, strokeWidth = 2.dp)
            c.keepOffline -> Icon(Icons.Rounded.DownloadForOffline, "Kept offline", tint = accent.base, modifier = Modifier.size(20.dp))
        }
    }
}

// ---- A collection -----------------------------------------------------------------------

@Composable
private fun CollectionPage(model: SavedModel, c: CollectionRecord, current: String?, playlists: List<PlaylistSummaryRecord>) {
    val s = c.summary
    val label = s.platform.label
    val accent = LocalAccent.current
    var renaming by rememberSaveable { mutableStateOf(false) }
    var removing by rememberSaveable { mutableStateOf(false) }
    var copying by rememberSaveable { mutableStateOf(false) }
    var menu by remember { mutableStateOf(false) }
    val removed = s.removedCount.toInt()
    val playable = s.songCount > 0u
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item {
            TintedHeader(
                back = model::back,
                backLabel = "Back to $label",
                actions = {
                    Box {
                        IconButton(onClick = { menu = true }) { Icon(Icons.Rounded.MoreVert, "Collection options", tint = RedlineColors.Text) }
                        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, containerColor = RedlineColors.Elevated) {
                            MenuItem("Rename", Icons.Rounded.Edit) {
                                menu = false
                                renaming = true
                            }
                            MenuItem("Copy to playlist…", Icons.AutoMirrored.Rounded.PlaylistAdd) {
                                menu = false
                                copying = true
                            }
                            if (removed > 0) {
                                MenuItem("Clear removed ($removed)", Icons.Rounded.ClearAll) {
                                    menu = false
                                    model.clearRemoved(s.id)
                                }
                            }
                            MenuItem("Remove from Saved", Icons.Rounded.Delete) {
                                menu = false
                                removing = true
                            }
                        }
                    }
                },
            ) {
                Row(Modifier.padding(horizontal = PagePadding), verticalAlignment = Alignment.CenterVertically) {
                    Cover(s.thumbnail, Icons.AutoMirrored.Rounded.PlaylistPlay, size = 112.dp, tint = accent.base)
                    Spacer(Modifier.width(16.dp))
                    Column(Modifier.weight(1f)) {
                        Eyebrow("$label collection")
                        Text(s.name, style = MaterialTheme.typography.headlineSmall, color = RedlineColors.Text, maxLines = 2, overflow = TextOverflow.Ellipsis)
                        val meta = buildList {
                            add(songs(s.songCount.toInt()))
                            if (c.durationSecs > 0) add(fmtDuration(c.durationSecs))
                            s.syncedAt?.let { add("synced ${fmtAgo(it)}") }
                        }
                        Text(meta.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
                    }
                }
                Spacer(Modifier.height(16.dp))
                Row(
                    Modifier.padding(horizontal = PagePadding),
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    PillButton("Play", { model.playCollection(s.id) }, icon = Icons.Rounded.PlayArrow, enabled = playable)
                    ShuffleButton(enabled = playable, onClick = { model.playCollection(s.id, shuffle = true) })
                    if (s.refresh == RefreshState.IDLE) {
                        IconButton(onClick = { model.refresh(s.id) }) { Icon(Icons.Rounded.Refresh, "Refresh from $label", tint = RedlineColors.Text) }
                    } else {
                        Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) {
                            CircularProgressIndicator(Modifier.size(22.dp), color = accent.base, strokeWidth = 2.dp)
                        }
                    }
                }
                if (s.refresh != RefreshState.IDLE) {
                    Text(
                        if (s.refresh == RefreshState.RUNNING) "Refreshing from $label…" else "Waiting for another refresh to finish…",
                        Modifier.padding(horizontal = PagePadding).padding(top = 8.dp),
                        style = MaterialTheme.typography.bodySmall,
                        color = RedlineColors.Muted,
                    )
                }
            }
        }
        item {
            Column(Modifier.padding(horizontal = PagePadding).padding(bottom = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                ToggleRow(
                    "Keep offline",
                    s.keepOffline,
                    { model.setKeepOffline(s.id, it) },
                    hint = "Download every song, and new ones after each refresh",
                )
                s.syncError?.let { e ->
                    SectionCard {
                        Row {
                            Icon(Icons.Rounded.ErrorOutline, null, tint = accent.base)
                            Spacer(Modifier.width(10.dp))
                            Text("Couldn't refresh: $e", color = RedlineColors.Text)
                        }
                    }
                }
                if (removed > 0) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text("${songs(removed)} no longer on $label.", Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium, color = RedlineColors.Muted)
                        TextButton(onClick = { model.clearRemoved(s.id) }) { Text("Clear them", color = accent.hover) }
                    }
                }
            }
        }
        savedTracks(c.tracks, label, current, play = { i -> model.playCollection(s.id, i) }, model)
    }
    if (renaming) {
        NameDialog("Rename collection", s.name, "Rename", onConfirm = { model.rename(s.id, it) }, onDismiss = { renaming = false }, placeholder = "Name")
    }
    if (copying) {
        AddToPlaylistDialog(
            playlists,
            onPick = { pid -> model.copyToPlaylist(s.id, pid) },
            onNew = { name -> model.copyToPlaylist(s.id, null, name) },
            onDismiss = { copying = false },
        )
    }
    if (removing) {
        AlertDialog(
            onDismissRequest = { removing = false },
            containerColor = RedlineColors.Elevated,
            title = { Text("Remove “${s.name}”?") },
            text = { Text("It's taken out of Saved; downloads and your playlists stay as they are.", color = RedlineColors.Muted) },
            confirmButton = {
                TextButton(onClick = {
                    removing = false
                    model.remove(s.id)
                }) { Text("Remove", color = accent.hover) }
            },
            dismissButton = { TextButton(onClick = { removing = false }) { Text("Cancel", color = RedlineColors.Muted) } },
        )
    }
}

/** Rows of saved tracks, the ones gone from the platform marked (saved.rs `saved_table`). */
private fun androidx.compose.foundation.lazy.LazyListScope.savedTracks(
    tracks: List<SavedTrackRecord>,
    label: String,
    current: String?,
    play: (Int) -> Unit,
    model: SavedModel,
) {
    if (tracks.isEmpty()) {
        item { Text("Nothing here yet.", Modifier.padding(horizontal = PagePadding), color = RedlineColors.Muted) }
    }
    itemsIndexed(tracks, key = { i, t -> "$i:${t.track.location}" }) { i, t ->
        val track: TrackRecord = t.track
        TrackRow(
            i,
            track,
            playing = track.location == current,
            onClick = { play(i) },
            tag = if (t.removed) "Removed from $label" else null,
        ) { close ->
            MenuItem("Play", Icons.Rounded.PlayArrow) {
                close()
                play(i)
            }
            if (!t.removed) {
                MenuItem("Play next", Icons.Rounded.QueuePlayNext) {
                    close()
                    model.playNext(track)
                }
                MenuItem("Add to queue", Icons.AutoMirrored.Rounded.QueueMusic) {
                    close()
                    model.queue(track)
                }
                LibraryMenuItems(track, close)
            }
            if (track.canGoOffline) {
                MenuItem("Download for offline", Icons.Rounded.Download) {
                    close()
                    model.downloadForOffline(track)
                }
            }
        }
    }
}

// ---- All songs --------------------------------------------------------------------------

@Composable
private fun AllSongsPage(model: SavedModel, p: Platform, all: List<SavedTrackRecord>, current: String?) {
    val label = p.label
    val playable = all.any { !it.removed }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item {
            TintedHeader(back = model::back, backLabel = "Back to $label") {
                Row(Modifier.padding(horizontal = PagePadding), verticalAlignment = Alignment.CenterVertically) {
                    ArtTile(Icons.Rounded.LibraryMusic, LocalAccent.current.base, size = 112.dp)
                    Spacer(Modifier.width(16.dp))
                    Column(Modifier.weight(1f)) {
                        Eyebrow(label)
                        Text("All songs", style = MaterialTheme.typography.headlineSmall, color = RedlineColors.Text)
                        Text("${songs(all.size)} · newest first", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
                    }
                }
                Spacer(Modifier.height(16.dp))
                Row(Modifier.padding(horizontal = PagePadding), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    PillButton("Play all", { model.playAll(p) }, icon = Icons.Rounded.PlayArrow, enabled = playable)
                    ShuffleButton(enabled = playable, onClick = { model.playAll(p, shuffle = true) })
                }
            }
        }
        savedTracks(all, label, current, play = { i -> model.playAll(p, i) }, model)
    }
}
