package app.redline.ui.playlists

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.PlaylistPlay
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material3.AlertDialog
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
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.redline.core.PlaylistSummaryRecord
import app.redline.ui.components.ArtTile
import app.redline.ui.components.Cover
import app.redline.ui.components.InputField
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors

/** Asks for a name: creating a playlist ("New playlist"), or renaming one or a saved collection. */
@Composable
fun NameDialog(
    title: String,
    initial: String,
    confirm: String,
    onConfirm: (String) -> Unit,
    onDismiss: () -> Unit,
    placeholder: String = "Playlist name",
) {
    var name by rememberSaveable { mutableStateOf(initial) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    val done = {
        onConfirm(name)
        onDismiss()
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = RedlineColors.Elevated,
        title = { Text(title) },
        text = {
            InputField(
                name,
                { name = it },
                modifier = Modifier.focusRequester(focus),
                placeholder = placeholder,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { done() }),
            )
        },
        confirmButton = { TextButton(onClick = done) { Text(confirm, color = LocalAccent.current.hover) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = RedlineColors.Muted) } },
    )
}

/**
 * "Add to playlist…" (the desktop's add menu): pick one of the playlists, or make a new one
 * with [onNew], which gets the name.
 */
@Composable
fun AddToPlaylistDialog(
    playlists: List<PlaylistSummaryRecord>,
    onPick: (ULong) -> Unit,
    onNew: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    var naming by rememberSaveable { mutableStateOf(false) }
    if (naming) {
        NameDialog("New playlist", "", "Create", onConfirm = onNew, onDismiss = onDismiss)
        return
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = RedlineColors.Elevated,
        title = { Text("Add to playlist") },
        text = {
            LazyColumn(Modifier.heightIn(max = 360.dp)) {
                item {
                    PickRow(onClick = { naming = true }) {
                        ArtTile(Icons.Rounded.Add, LocalAccent.current.base, size = 40.dp)
                        Spacer(Modifier.width(12.dp))
                        Text("New playlist", style = MaterialTheme.typography.titleSmall, color = RedlineColors.Text)
                    }
                }
                items(playlists, key = { it.id.toLong() }) { p ->
                    PickRow(onClick = {
                        onPick(p.id)
                        onDismiss()
                    }) {
                        Cover(p.thumbnail, Icons.AutoMirrored.Rounded.PlaylistPlay, size = 40.dp)
                        Spacer(Modifier.width(12.dp))
                        Column(Modifier.weight(1f)) {
                            Text(p.name, style = MaterialTheme.typography.titleSmall, color = RedlineColors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            Text(trackCount(p.trackCount.toInt()), style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
                        }
                    }
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = RedlineColors.Muted) } },
    )
}

@Composable
private fun PickRow(onClick: () -> Unit, content: @Composable RowScope.() -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.Start,
    ) { content() }
}

/** "1 track", "12 tracks". */
fun trackCount(n: Int) = "$n track${if (n == 1) "" else "s"}"
