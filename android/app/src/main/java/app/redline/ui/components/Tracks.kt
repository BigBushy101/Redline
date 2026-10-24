package app.redline.ui.components

import android.widget.Toast
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.PlaylistAdd
import androidx.compose.material.icons.automirrored.rounded.VolumeUp
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.Link
import androidx.compose.material.icons.rounded.LinkOff
import androidx.compose.material.icons.rounded.MoreVert
import androidx.compose.material.icons.rounded.MusicNote
import androidx.compose.material.icons.rounded.PhoneAndroid
import androidx.compose.material.icons.rounded.RemoveCircleOutline
import androidx.compose.material.icons.rounded.SwapHoriz
import androidx.compose.material.icons.rounded.WarningAmber
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.redline.core.LinkStatusRecord
import app.redline.core.TrackKind
import app.redline.core.TrackRecord
import app.redline.core.fmtDuration
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import app.redline.ui.theme.color
import app.redline.ui.theme.icon
import app.redline.ui.theme.label
import app.redline.util.art

/** The amber of a link that's private or blocked from playing here. */
val RestrictedColor = Color(0xFFFFB020)

/** What a long press on a dead or restricted row says (linkcheck.rs `LinkStatus::label`). */
fun linkLabel(status: LinkStatusRecord): String? = when (status) {
    LinkStatusRecord.DEAD -> "This video is no longer available"
    LinkStatusRecord.RESTRICTED -> "This video is private or can't be played here"
    else -> null
}

/** A link track whose link is gone. */
val TrackRecord.dead: Boolean get() = kind == TrackKind.LINK && linkStatus == LinkStatusRecord.DEAD

/** A link track whose link has stopped playing, or plays only for some. */
val TrackRecord.unavailable: Boolean
    get() = kind == TrackKind.LINK && (linkStatus == LinkStatusRecord.DEAD || linkStatus == LinkStatusRecord.RESTRICTED)

/**
 * One track (tracks.rs `track_row`): number, art, title, artist and where it plays from, length
 * and a menu, whose items [menu] adds (it gets the call that closes the menu). A [tag] (Saved's
 * "Removed from YouTube") takes the place of where it plays from, and greys the row; so does a
 * dead link's "No longer available". A long press on a dead or restricted row says why.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun TrackRow(
    index: Int,
    track: TrackRecord,
    playing: Boolean,
    onClick: () -> Unit,
    tag: String? = null,
    menu: @Composable (close: () -> Unit) -> Unit,
) {
    val accent = LocalAccent.current
    val context = LocalContext.current
    var open by remember { mutableStateOf(false) }
    val dead = track.kind == TrackKind.LINK && track.linkStatus == LinkStatusRecord.DEAD
    val label = if (track.kind == TrackKind.LINK) linkLabel(track.linkStatus) else null
    Row(
        Modifier
            .fillMaxWidth()
            .background(if (playing) RedlineColors.Elevated else RedlineColors.Bg)
            .combinedClickable(
                onClick = onClick,
                onLongClick = label?.let { { Toast.makeText(context, it, Toast.LENGTH_SHORT).show() } },
            )
            .padding(start = PagePadding, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.widthIn(min = 28.dp), contentAlignment = Alignment.Center) {
            if (playing) {
                Icon(Icons.AutoMirrored.Rounded.VolumeUp, "Playing", tint = accent.base, modifier = Modifier.size(18.dp))
            } else {
                Text("${index + 1}", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
            }
        }
        Spacer(Modifier.width(6.dp))
        Cover(track.art, if (track.kind == TrackKind.LINK) Icons.Rounded.Link else Icons.Rounded.MusicNote, size = 44.dp)
        Spacer(Modifier.width(10.dp))
        Column(Modifier.weight(1f)) {
            Text(
                track.title,
                style = MaterialTheme.typography.titleSmall,
                color = when {
                    playing -> accent.hover
                    tag != null || dead -> RedlineColors.Muted
                    else -> RedlineColors.Text
                },
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            when {
                tag != null -> TagLine(tag)
                dead -> TagLine("No longer available", Icons.Rounded.LinkOff)
                else -> SourceLine(track)
            }
        }
        track.durationSecs?.let {
            Spacer(Modifier.width(8.dp))
            Text(fmtDuration(it), style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
        }
        Box {
            IconButton(onClick = { open = true }) { Icon(Icons.Rounded.MoreVert, "Track options", tint = RedlineColors.Muted) }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }, containerColor = RedlineColors.Elevated) {
                menu { open = false }
            }
        }
    }
}

/**
 * "Artist · ✓ Downloaded", "Artist · ▶ YouTube", "Artist · On this phone". A stream shows its
 * platform's icon in its color, as the desktop's track rows do.
 */
@Composable
private fun SourceLine(track: TrackRecord) {
    val platform = track.platform
    val (icon, label) = when {
        track.kind == TrackKind.FILE -> Icons.Rounded.PhoneAndroid to "On this phone"
        track.offline -> Icons.Rounded.CheckCircle to "Downloaded"
        platform != null -> platform.icon to platform.label
        else -> Icons.Rounded.Link to "Stream"
    }
    val tint = when {
        track.offline -> LocalAccent.current.base
        platform != null -> platform.color
        else -> RedlineColors.Muted
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        if (track.kind == TrackKind.LINK && track.linkStatus == LinkStatusRecord.RESTRICTED) {
            Icon(Icons.Rounded.WarningAmber, "Private or blocked", tint = RestrictedColor, modifier = Modifier.size(12.dp))
            Spacer(Modifier.width(3.dp))
        }
        val artist = track.artist?.takeIf { it.isNotBlank() }
        if (artist != null) {
            Text(
                "$artist · ",
                style = MaterialTheme.typography.bodySmall,
                color = RedlineColors.Muted,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false),
            )
        }
        Icon(icon, null, tint = tint, modifier = Modifier.size(12.dp))
        Spacer(Modifier.width(3.dp))
        Text(label, style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted, maxLines = 1)
    }
}

/** The row's tag in place of its source, e.g. "⊖ Removed from YouTube". */
@Composable
private fun TagLine(tag: String, icon: ImageVector = Icons.Rounded.RemoveCircleOutline) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Icon(icon, null, tint = LocalAccent.current.hover, modifier = Modifier.size(12.dp))
        Spacer(Modifier.width(3.dp))
        Text(tag, style = MaterialTheme.typography.bodySmall, color = LocalAccent.current.hover, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** What Library rows can send a song to, provided by the app's root. */
class TrackActions(
    /** Opens Convert with the song's file. */
    val convert: (TrackRecord) -> Unit,
    /** Asks which playlist, then adds it. */
    val addToPlaylist: (TrackRecord) -> Unit,
)

val LocalTrackActions = staticCompositionLocalOf<TrackActions?> { null }

/**
 * A Library row's "Add to playlist…" (unless the page has its own, [playlist] false) and
 * "Convert", which only songs that are files get: file tracks, and links downloaded for offline.
 */
@Composable
fun LibraryMenuItems(track: TrackRecord, close: () -> Unit, playlist: Boolean = true) {
    val actions = LocalTrackActions.current ?: return
    if (playlist) {
        MenuItem("Add to playlist…", Icons.AutoMirrored.Rounded.PlaylistAdd) {
            close()
            actions.addToPlaylist(track)
        }
    }
    if (track.kind == TrackKind.FILE || track.offline) {
        MenuItem("Convert", Icons.Rounded.SwapHoriz) {
            close()
            actions.convert(track)
        }
    }
}

/** A menu entry with a muted icon. */
@Composable
fun MenuItem(text: String, icon: ImageVector, onClick: () -> Unit) {
    DropdownMenuItem(
        text = { Text(text, style = MaterialTheme.typography.bodyMedium) },
        leadingIcon = { Icon(icon, null, tint = RedlineColors.Muted) },
        onClick = onClick,
    )
}
