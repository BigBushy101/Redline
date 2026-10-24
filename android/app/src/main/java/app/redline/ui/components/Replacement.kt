package app.redline.ui.components

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.Link
import androidx.compose.material.icons.rounded.LinkOff
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.redline.core.RedlineException
import app.redline.core.ReplacementRecord
import app.redline.core.ReviewRecord
import app.redline.core.TrackRecord
import app.redline.core.fmtDuration
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import kotlinx.coroutines.CancellationException
import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * Under a playlist's or collection's header when some of its songs are unavailable: the
 * "N unavailable" chip, which shows only those, and "Fix unavailable" while any are dead.
 */
@Composable
fun UnavailableBar(count: Int, filtered: Boolean, onFilter: () -> Unit, canFix: Boolean, fixing: Boolean, onFix: () -> Unit) {
    val accent = LocalAccent.current
    Row(
        Modifier.padding(horizontal = PagePadding).padding(bottom = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        FilterChip(
            selected = filtered,
            onClick = onFilter,
            label = { Text("$count unavailable") },
            leadingIcon = { Icon(Icons.Rounded.LinkOff, null, modifier = Modifier.size(16.dp)) },
            shape = CircleShape,
            colors = FilterChipDefaults.filterChipColors(
                containerColor = RedlineColors.Elevated,
                labelColor = RedlineColors.Text,
                iconColor = RedlineColors.Muted,
                selectedContainerColor = RedlineColors.Text,
                selectedLabelColor = RedlineColors.Black,
                selectedLeadingIconColor = RedlineColors.Black,
            ),
            border = null,
        )
        if (canFix) {
            if (fixing) {
                CircularProgressIndicator(Modifier.size(18.dp), color = accent.base, strokeWidth = 2.dp)
                Text("Looking for replacements…", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
            } else {
                GhostButton("Fix unavailable", onFix)
            }
        }
    }
}

/** Where a replacement search is. */
private sealed interface Search {
    data object Running : Search
    data class Found(val picks: List<ReplacementRecord>) : Search
    data class Failed(val text: String) : Search
}

/**
 * "Find replacement…" (the desktop's replace.rs): searches YouTube for other uploads of the
 * track's song with [search] (blocking, so the caller runs it on the IO dispatcher), then lists
 * them best first; tapping one hands it to [onPick] and closes.
 */
@Composable
fun ReplacementDialog(
    track: TrackRecord,
    search: suspend () -> List<ReplacementRecord>,
    onPick: (ReplacementRecord) -> Unit,
    onDismiss: () -> Unit,
) {
    val result by produceState<Search>(Search.Running, track.location) {
        value = try {
            Search.Found(search())
        } catch (e: CancellationException) {
            throw e
        } catch (e: RedlineException) {
            Search.Failed(e.message ?: "The search failed.")
        } catch (e: Exception) {
            Search.Failed(e.message ?: e.toString())
        }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = RedlineColors.Elevated,
        title = { Text("Replace “${track.title}”") },
        text = {
            when (val r = result) {
                Search.Running -> Row(verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(Modifier.size(20.dp), color = LocalAccent.current.base, strokeWidth = 2.dp)
                    Spacer(Modifier.width(12.dp))
                    Text("Searching YouTube…", color = RedlineColors.Muted)
                }
                is Search.Failed -> Text("Couldn't search: ${r.text}", color = RedlineColors.Muted)
                is Search.Found -> Picks(r.picks) { pick ->
                    onPick(pick)
                    onDismiss()
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = RedlineColors.Muted) } },
    )
}

/**
 * "Fix unavailable"'s review: the dead songs it found no clear match for, one at a time, each
 * with its picks and Skip. [onPick] gets the song and the pick; the dialog closes after the last.
 */
@Composable
fun ReviewDialog(items: List<ReviewRecord>, onPick: (ReviewRecord, ReplacementRecord) -> Unit, onDismiss: () -> Unit) {
    var at by remember(items) { mutableIntStateOf(0) }
    val item = items.getOrNull(at)
    if (item == null) {
        LaunchedEffect(Unit) { onDismiss() }
        return
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = RedlineColors.Elevated,
        title = {
            Column {
                Eyebrow(if (items.size > 1) "Unavailable · ${at + 1} of ${items.size}" else "Unavailable")
                Text("Replace “${item.track.title}”", maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
        },
        text = {
            Picks(item.candidates) { pick ->
                onPick(item, pick)
                at++
            }
        },
        confirmButton = { TextButton(onClick = { at++ }) { Text("Skip", color = LocalAccent.current.hover) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Done", color = RedlineColors.Muted) } },
    )
}

@Composable
private fun Picks(picks: List<ReplacementRecord>, onPick: (ReplacementRecord) -> Unit) {
    if (picks.isEmpty()) {
        Text("No matches found.", color = RedlineColors.Muted)
        return
    }
    LazyColumn(Modifier.heightIn(max = 380.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        items(picks, key = { it.url }) { p -> PickRow(p) { onPick(p) } }
    }
}

/** A pick: art, title, and "channel · 3:21 · +2s", a clear match ticked. */
@Composable
private fun PickRow(p: ReplacementRecord, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Cover(p.thumbnail, Icons.Rounded.Link, size = 44.dp)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(p.title, style = MaterialTheme.typography.titleSmall, color = RedlineColors.Text, maxLines = 2, overflow = TextOverflow.Ellipsis)
            val meta = listOfNotNull(p.uploader?.takeIf { it.isNotBlank() }, p.durationSecs?.let { fmtDuration(it) }, p.deltaSecs?.let(::delta))
            Text(meta.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (p.clearMatch) {
            Spacer(Modifier.width(8.dp))
            Box(contentAlignment = Alignment.Center) {
                Icon(Icons.Rounded.CheckCircle, "Close match", tint = LocalAccent.current.base, modifier = Modifier.size(18.dp))
            }
        }
    }
}

/** How much longer or shorter a pick is: "+2s", "−12s", "same length". */
private fun delta(secs: Double): String {
    val s = secs.roundToInt()
    return when {
        s == 0 -> "same length"
        s > 0 -> "+${s}s"
        else -> "−${abs(s)}s"
    }
}
