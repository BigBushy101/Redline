package app.redline.ui.history

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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.OpenInNew
import androidx.compose.material.icons.automirrored.rounded.PlaylistAdd
import androidx.compose.material.icons.rounded.Delete
import androidx.compose.material.icons.rounded.Download
import androidx.compose.material.icons.rounded.History
import androidx.compose.material.icons.rounded.MoreVert
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material.icons.rounded.SwapHoriz
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.redline.core.HistoryRecord
import app.redline.core.MediaCategory
import app.redline.core.classifyFileName
import app.redline.ui.components.ArtTile
import app.redline.ui.components.EmptyState
import app.redline.ui.components.GhostButton
import app.redline.ui.components.InputField
import app.redline.ui.components.PageHeader
import app.redline.ui.components.PagePadding
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import app.redline.util.formatTimestamp

sealed interface HistoryAction {
    data class Open(val path: String) : HistoryAction
    /** Add the finished file to the Convert page. */
    data class Convert(val path: String) : HistoryAction
    data class Again(val index: UInt) : HistoryAction
    data class Remove(val index: UInt) : HistoryAction
    /** Its file while that's there, otherwise its link streamed. */
    data class Play(val index: UInt) : HistoryAction
    /** "Add to playlist…": the caller asks which one. */
    data class AddToPlaylist(val index: UInt) : HistoryAction
    data object Clear : HistoryAction
}

/** Port of src/ui/history.rs: search, clear, one row per finished job (newest first). */
@Composable
fun HistoryScreen(entries: List<HistoryRecord>, onAction: (HistoryAction) -> Unit) {
    val n = entries.size
    val subtitle = "$n item${if (n == 1) "" else "s"}"
    if (n == 0) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            PageHeader("History", "Recently finished", subtitle)
            EmptyState(Icons.Rounded.History, "Nothing here yet", "Finished downloads and conversions are kept here.")
        }
        return
    }
    var search by rememberSaveable { mutableStateOf("") }
    var confirmClear by rememberSaveable { mutableStateOf(false) }
    val query = search.trim()
    val shown = if (query.isEmpty()) entries else entries.filter { it.matches(query) }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item { PageHeader("History", "Recently finished", subtitle) }
        item {
            Column(
                Modifier.padding(horizontal = PagePadding).padding(bottom = 12.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                InputField(search, { search = it }, placeholder = "Search titles, links and files", leading = Icons.Rounded.Search, shapeRound = true)
                GhostButton("Clear history", { confirmClear = true })
            }
        }
        if (shown.isEmpty()) {
            item { Text("No matches.", Modifier.padding(horizontal = PagePadding), color = RedlineColors.Muted) }
        }
        items(shown, key = { "${it.index}:${it.finishedAt}:${it.title}" }) { entry ->
            EntryRow(entry, onAction)
            Spacer(Modifier.height(4.dp))
        }
    }
    if (confirmClear) {
        // One tap on a phone is easier to hit by accident than the desktop's button.
        AlertDialog(
            onDismissRequest = { confirmClear = false },
            containerColor = RedlineColors.Elevated,
            title = { Text("Clear history?") },
            text = { Text("The list is cleared; downloaded files stay where they are.", color = RedlineColors.Muted) },
            confirmButton = {
                TextButton(onClick = {
                    confirmClear = false
                    onAction(HistoryAction.Clear)
                }) { Text("Clear", color = LocalAccent.current.hover) }
            },
            dismissButton = { TextButton(onClick = { confirmClear = false }) { Text("Cancel", color = RedlineColors.Muted) } },
        )
    }
}

/** The desktop's `HistoryEntry::matches`: title, link or file name, ignoring case. */
private fun HistoryRecord.matches(query: String): Boolean =
    listOfNotNull(title, url, outputPath, subtitle).any { it.contains(query, ignoreCase = true) }

@Composable
private fun EntryRow(entry: HistoryRecord, onAction: (HistoryAction) -> Unit) {
    val output = entry.outputPath?.takeIf { entry.outputExists }
    val playable = entry.playable()
    var menu by remember { mutableStateOf(false) }
    Row(
        Modifier
            .padding(horizontal = PagePadding)
            .fillMaxWidth()
            .background(RedlineColors.Card, MaterialTheme.shapes.small)
            .then(if (output != null) Modifier.clickable { onAction(HistoryAction.Open(output)) } else Modifier)
            .padding(start = 12.dp, top = 8.dp, bottom = 8.dp, end = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ArtTile(if (entry.isDownload) Icons.Rounded.Download else Icons.Rounded.SwapHoriz, RedlineColors.Muted, size = 40.dp)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(entry.title, style = MaterialTheme.typography.titleSmall, color = RedlineColors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis)
            val sub = entry.subtitle + if (entry.outputPath != null && !entry.outputExists) " · file moved or deleted" else ""
            Text(sub, style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(formatTimestamp(entry.finishedAt), style = MaterialTheme.typography.bodySmall, color = RedlineColors.Subtle)
        }
        // The downloaded file if it's still there, otherwise the link streamed (history.rs).
        if (playable) {
            val local = output != null && isAudioOrVideo(output)
            IconButton(onClick = { onAction(HistoryAction.Play(entry.index)) }) {
                Icon(Icons.Rounded.PlayArrow, if (local) "Play" else "Stream", tint = RedlineColors.Text)
            }
        }
        if (output != null) {
            IconButton(onClick = { onAction(HistoryAction.Open(output)) }) {
                Icon(Icons.AutoMirrored.Rounded.OpenInNew, "Open in its app", tint = RedlineColors.Muted)
            }
        }
        // The rest in a menu: a phone row doesn't fit the desktop's six buttons.
        Box {
            IconButton(onClick = { menu = true }) { Icon(Icons.Rounded.MoreVert, "More", tint = RedlineColors.Muted) }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, containerColor = RedlineColors.Elevated) {
                @Composable
                fun item(text: String, icon: ImageVector, action: HistoryAction) {
                    DropdownMenuItem(
                        text = { Text(text, style = MaterialTheme.typography.bodyMedium) },
                        leadingIcon = { Icon(icon, null, tint = RedlineColors.Muted) },
                        onClick = {
                            menu = false
                            onAction(action)
                        },
                    )
                }
                if (playable) item("Add to playlist…", Icons.AutoMirrored.Rounded.PlaylistAdd, HistoryAction.AddToPlaylist(entry.index))
                if (output != null && !java.io.File(output).isDirectory) item("Convert", Icons.Rounded.SwapHoriz, HistoryAction.Convert(output))
                // A conversion of a picked file can't run again: its copy is gone once it's done.
                if (entry.canRepeat) {
                    item(if (entry.isDownload) "Download again" else "Convert again", Icons.Rounded.Refresh, HistoryAction.Again(entry.index))
                }
                item("Remove from history", Icons.Rounded.Delete, HistoryAction.Remove(entry.index))
            }
        }
    }
}

/** Has a link, or an audio or video file that's still there (history.rs `as_track`). */
private fun HistoryRecord.playable(): Boolean =
    url != null || outputPath?.takeIf { outputExists }?.let(::isAudioOrVideo) == true

private fun isAudioOrVideo(path: String): Boolean {
    val file = java.io.File(path)
    if (file.isDirectory) return false
    val c = classifyFileName(file.name)
    return c == MediaCategory.AUDIO || c == MediaCategory.VIDEO
}
