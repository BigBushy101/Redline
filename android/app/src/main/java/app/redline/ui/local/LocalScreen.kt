package app.redline.ui.local

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.PlaylistAdd
import androidx.compose.material.icons.automirrored.rounded.QueueMusic
import androidx.compose.material.icons.rounded.AudioFile
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Folder
import androidx.compose.material.icons.rounded.LibraryMusic
import androidx.compose.material.icons.rounded.LockOpen
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.QueuePlayNext
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.RemoveCircleOutline
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material3.CircularProgressIndicator
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
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.redline.core.PlaylistSummaryRecord
import app.redline.core.TrackRecord
import app.redline.core.fmtDuration
import app.redline.state.MusicState
import app.redline.ui.components.EmptyState
import app.redline.ui.components.GhostButton
import app.redline.ui.components.InputField
import app.redline.ui.components.LibraryMenuItems
import app.redline.ui.components.MenuItem
import app.redline.ui.components.PageHeader
import app.redline.ui.components.PagePadding
import app.redline.ui.components.PillButton
import app.redline.ui.components.TrackRow
import app.redline.ui.playlists.AddToPlaylistDialog
import app.redline.ui.playlists.ShuffleButton
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors

/**
 * Port of src/ui/local.rs: the songs on this phone, from the Music folder or the folders picked
 * here, with Play / Shuffle and each song's menu.
 */
@Composable
fun LocalScreen(model: LocalModel, music: MusicState, playlists: List<PlaylistSummaryRecord>) {
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { model.permissionChanged() }
    val pickFolder = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri -> uri?.let(model::addFolder) }
    // Songs may have come or gone (or the permission changed in system settings) while away.
    LifecycleResumeEffect(Unit) {
        model.permissionChanged()
        onPauseOrDispose { }
    }
    LaunchedEffect(Unit) { model.rescan() }

    val snapshot by music.snapshot.collectAsStateWithLifecycle()
    val queue by music.queue.collectAsStateWithLifecycle()
    val current = queue.firstOrNull { it.isCurrent }?.track?.location.takeIf { snapshot != null }
    var search by rememberSaveable { mutableStateOf("") }
    var adding by remember { mutableStateOf<TrackRecord?>(null) }

    val tracks = model.tracks
    val shown = remember(tracks, search) {
        val q = search.trim().lowercase()
        tracks.withIndex().filter { (_, t) ->
            q.isEmpty() || t.title.lowercase().contains(q) || t.artist?.lowercase()?.contains(q) == true
        }
    }
    val secs = tracks.sumOf { it.durationSecs ?: 0.0 }
    val meta = "${tracks.size} song${if (tracks.size == 1) "" else "s"}" + if (secs > 0) " · ${fmtDuration(secs)}" else ""

    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item { PageHeader("Local", "On this phone", meta) }
        if (!model.allowed) {
            item {
                EmptyState(
                    Icons.Rounded.LockOpen,
                    "Let Redline see your music",
                    "To list the songs on this phone and play them, Redline needs access to your audio files.",
                ) { PillButton("Allow access", { ask.launch(LocalModel.permission) }) }
            }
            return@LazyColumn
        }
        item { Folders(model) { pickFolder.launch(null) } }
        item {
            Row(
                Modifier.padding(horizontal = PagePadding).padding(bottom = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                PillButton("Play", { model.play() }, icon = Icons.Rounded.PlayArrow, enabled = tracks.isNotEmpty())
                ShuffleButton(enabled = tracks.isNotEmpty()) { model.play(shuffle = true) }
                Spacer(Modifier.weight(1f))
                if (model.scanning) {
                    CircularProgressIndicator(Modifier.size(20.dp), color = LocalAccent.current.base, strokeWidth = 2.dp)
                } else {
                    IconButton(onClick = model::rescan) { Icon(Icons.Rounded.Refresh, "Look again", tint = RedlineColors.Muted) }
                }
            }
        }
        if (tracks.size > 8) {
            item {
                Box(Modifier.padding(horizontal = PagePadding).padding(bottom = 8.dp)) {
                    InputField(search, { search = it }, placeholder = "Find a song", leading = Icons.Rounded.Search, shapeRound = true)
                }
            }
        }
        if (tracks.isEmpty() && !model.scanning) {
            item {
                if (model.folders.isEmpty()) {
                    EmptyState(Icons.Rounded.Folder, "Pick a folder", "Add the folders your music is in, and their songs show up here.")
                } else {
                    EmptyState(Icons.Rounded.LibraryMusic, "No songs here yet", "Songs in these folders (MP3, FLAC, M4A, WAV, Ogg and more) show up here.")
                }
            }
        }
        itemsIndexed(shown, key = { _, (_, t) -> t.location }) { _, (i, t) ->
            TrackRow(i, t, playing = t.location == current, onClick = { model.play(start = i) }) { close ->
                MenuItem("Play", Icons.Rounded.PlayArrow) {
                    close()
                    model.play(start = i)
                }
                MenuItem("Play next", Icons.Rounded.QueuePlayNext) {
                    close()
                    model.playNext(t)
                }
                MenuItem("Add to queue", Icons.AutoMirrored.Rounded.QueueMusic) {
                    close()
                    model.queue(t)
                }
                MenuItem("Add to playlist…", Icons.AutoMirrored.Rounded.PlaylistAdd) {
                    close()
                    adding = t
                }
                LibraryMenuItems(t, close, playlist = false)
                if (model.isAdded(t.location)) {
                    MenuItem("Remove from Local", Icons.Rounded.RemoveCircleOutline) {
                        close()
                        model.removeFile(t.location)
                    }
                }
            }
        }
    }
    adding?.let { t ->
        AddToPlaylistDialog(
            playlists,
            onPick = { id -> model.addToPlaylist(t, id) },
            onNew = { name -> model.addToPlaylist(t, null, name) },
            onDismiss = { adding = null },
        )
    }
}

/** The folders searched, each removable, the files added on Convert, then Add folder (and back to Music). */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Folders(model: LocalModel, addFolder: () -> Unit) {
    Column(Modifier.padding(horizontal = PagePadding).padding(bottom = 12.dp)) {
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            for (dir in model.folders) {
                Row(
                    Modifier.background(RedlineColors.Card, CircleShape).padding(start = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(Icons.Rounded.Folder, null, tint = RedlineColors.Muted, modifier = Modifier.size(16.dp))
                    Spacer(Modifier.width(6.dp))
                    Text(
                        dir.trimEnd('/').substringAfterLast('/').ifEmpty { dir },
                        style = MaterialTheme.typography.labelLarge,
                        color = RedlineColors.Text,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f, fill = false),
                    )
                    IconButton(onClick = { model.removeFolder(dir) }, modifier = Modifier.size(36.dp)) {
                        Icon(Icons.Rounded.Close, "Stop listing $dir", tint = RedlineColors.Muted, modifier = Modifier.size(16.dp))
                    }
                }
            }
            val added = model.files.size
            if (added > 0) {
                Row(
                    Modifier.background(RedlineColors.Card, CircleShape).padding(start = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(Icons.Rounded.AudioFile, null, tint = RedlineColors.Muted, modifier = Modifier.size(16.dp))
                    Spacer(Modifier.width(6.dp))
                    Text("Added files: $added", style = MaterialTheme.typography.labelLarge, color = RedlineColors.Text, maxLines = 1)
                    IconButton(onClick = model::clearFiles, modifier = Modifier.size(36.dp)) {
                        Icon(Icons.Rounded.Close, "Clear the files added on Convert", tint = RedlineColors.Muted, modifier = Modifier.size(16.dp))
                    }
                }
            }
        }
        Spacer(Modifier.size(10.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            GhostButton("Add folder", addFolder)
            if (!model.isDefault) {
                TextButton(onClick = model::resetFolders) { Text("Reset to Music", color = RedlineColors.Subtle) }
            }
        }
    }
}
