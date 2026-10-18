package app.redline.ui.player

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.MusicNote
import androidx.compose.material.icons.rounded.Pause
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.SkipNext
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.redline.core.PlayerSnapshotRecord
import app.redline.core.PlayerStatus
import app.redline.state.MusicState
import app.redline.ui.components.Cover
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors

/**
 * The desktop's player bar, shrunk to Spotify's mini player: above the bottom bar whenever a
 * track is loaded; tap it for the full player.
 */
@Composable
fun MiniPlayer(music: MusicState, open: () -> Unit) {
    val snapshot by music.snapshot.collectAsStateWithLifecycle()
    val s = snapshot
    if (s == null || !MusicState.loaded(s)) return
    val accent = LocalAccent.current
    Surface(
        color = RedlineColors.Elevated,
        shape = MaterialTheme.shapes.small,
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 8.dp, vertical = 6.dp),
    ) {
        Column(Modifier.clickable(onClick = open)) {
            Row(Modifier.padding(start = 8.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                Cover(s.thumbnail, Icons.Rounded.MusicNote, size = 40.dp)
                Spacer(Modifier.width(10.dp))
                Column(Modifier.weight(1f)) {
                    Text(s.title ?: "Nothing playing", style = MaterialTheme.typography.titleSmall, color = RedlineColors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    val (line, error) = statusLine(s)
                    Text(
                        line,
                        style = MaterialTheme.typography.bodySmall,
                        color = if (error) accent.hover else RedlineColors.Muted,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) {
                    if (waiting(s)) {
                        CircularProgressIndicator(Modifier.size(22.dp), color = RedlineColors.Text, strokeWidth = 2.dp)
                    } else {
                        IconButton(onClick = { music.toggle() }) {
                            Icon(
                                if (s.isPlaying) Icons.Rounded.Pause else Icons.Rounded.PlayArrow,
                                if (s.isPlaying) "Pause" else "Play",
                                tint = RedlineColors.Text,
                            )
                        }
                    }
                }
                IconButton(onClick = { music.next() }, enabled = s.hasNext) {
                    Icon(Icons.Rounded.SkipNext, "Next", tint = if (s.hasNext) RedlineColors.Text else RedlineColors.Subtle)
                }
            }
            val duration = s.durationSecs
            if (duration != null && duration > 0) {
                LinearProgressIndicator(
                    progress = { (s.positionSecs / duration).toFloat().coerceIn(0f, 1f) },
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp).height(2.dp),
                    color = RedlineColors.Text,
                    trackColor = RedlineColors.Track,
                    strokeCap = StrokeCap.Round,
                    gapSize = 0.dp,
                    drawStopIndicator = {},
                )
            } else if (waiting(s)) {
                LinearProgressIndicator(
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp).height(2.dp),
                    color = accent.base,
                    trackColor = RedlineColors.Track,
                    gapSize = 0.dp,
                )
            } else {
                Spacer(Modifier.height(2.dp))
            }
        }
    }
}

/** Looking the track up, or waiting for sound. */
fun waiting(s: PlayerSnapshotRecord) = s.status == PlayerStatus.LOADING || (s.buffering && s.isPlaying)

/** The line under the title (now_bar.rs): what's going on, else the artist; and whether it's an error. */
fun statusLine(s: PlayerSnapshotRecord): Pair<String, Boolean> = when {
    s.status == PlayerStatus.ERROR -> (s.error?.lineSequence()?.firstOrNull() ?: "Couldn't play this") to true
    s.status == PlayerStatus.LOADING -> "Loading…" to false
    s.buffering && s.isPlaying -> "Buffering…" to false
    else -> (s.artist ?: "") to false
}
