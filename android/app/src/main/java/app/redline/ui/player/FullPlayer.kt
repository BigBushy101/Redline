package app.redline.ui.player

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.material.icons.rounded.Link
import androidx.compose.material.icons.rounded.MoreVert
import androidx.compose.material.icons.rounded.MusicNote
import androidx.compose.material.icons.rounded.Pause
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Repeat
import androidx.compose.material.icons.rounded.RepeatOne
import androidx.compose.material.icons.rounded.Shuffle
import androidx.compose.material.icons.rounded.SkipNext
import androidx.compose.material.icons.rounded.SkipPrevious
import androidx.compose.material.icons.rounded.Stop
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderColors
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.CancellationException
import app.redline.core.PlayerSnapshotRecord
import app.redline.core.PlayerStatus
import app.redline.core.QueueTrackRecord
import app.redline.core.RepeatMode
import app.redline.core.TrackRecord
import app.redline.core.TrackKind
import app.redline.core.fmtDuration
import app.redline.state.MusicState
import app.redline.ui.components.Cover
import app.redline.ui.components.Eyebrow
import app.redline.ui.components.PagePadding
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import app.redline.util.art

/**
 * The whole player over the app: the desktop's player bar, its speed and EQ popups and the
 * Now playing list in one scrolling page. Back or the chevron closes it.
 */
@Composable
fun FullPlayer(music: MusicState, close: () -> Unit) {
    BackHandler(onBack = close)
    val snapshot by music.snapshot.collectAsStateWithLifecycle()
    val queue by music.queue.collectAsStateWithLifecycle()
    val settings by music.settings.collectAsStateWithLifecycle()
    val s = snapshot
    // Nothing left to show (stopped, or the queue cleared): back to the page.
    if (s == null || !MusicState.loaded(s)) {
        LaunchedEffect(Unit) { close() }
        return
    }
    val accent = LocalAccent.current
    val current = queue.firstOrNull { it.isCurrent }
    val upNext = queue.dropWhile { !it.isCurrent }.drop(1)
    Box(
        Modifier
            .fillMaxSize()
            .background(RedlineColors.Bg)
            // The tint runs up behind the status bar...
            .background(Brush.verticalGradient(0f to accent.headerTint, 0.6f to RedlineColors.Bg))
            // Swallow taps so the pages underneath don't get them.
            .clickable(interactionSource = null, indication = null) {},
    ) {
        LazyColumn(
            Modifier
                .fillMaxSize()
                // ...but the page scrolls out of sight below it, not under the clock.
                .statusBarsPadding()
                .navigationBarsPadding(),
            contentPadding = PaddingValues(bottom = 32.dp),
        ) {
            item { TopBar(s, music, close) }
            item { NowPlaying(s, music, current, settings?.waveform == true) }
            item {
                Column(Modifier.padding(horizontal = PagePadding), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    settings?.let { p ->
                        SpeedCard(p, music.choices, music)
                        EqCard(p, music.choices, music::setEq)
                    }
                    Spacer(Modifier.height(4.dp))
                    Eyebrow(if (upNext.isEmpty()) "Nothing up next" else "Up next")
                }
            }
            items(upNext, key = { it.index.toLong() }) { q ->
                QueueRow(q, jump = { music.jump(q.index) }, remove = { music.removeFromQueue(q.index) })
            }
        }
    }
}

@Composable
private fun TopBar(s: PlayerSnapshotRecord, music: MusicState, close: () -> Unit) {
    var menu by remember { mutableStateOf(false) }
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 4.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(onClick = close) { Icon(Icons.Rounded.KeyboardArrowDown, "Close player", tint = RedlineColors.Text) }
        Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally) {
            Eyebrow("Now playing")
            val pos = s.queuePosition
            if (pos != null && s.queueLength > 1u) {
                Text("${pos + 1u} of ${s.queueLength}", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
            }
        }
        Box {
            IconButton(onClick = { menu = true }) { Icon(Icons.Rounded.MoreVert, "Player options", tint = RedlineColors.Text) }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, containerColor = RedlineColors.Elevated) {
                DropdownMenuItem(
                    text = { Text("Stop and clear the queue", style = MaterialTheme.typography.bodyMedium) },
                    leadingIcon = { Icon(Icons.Rounded.Stop, null, tint = RedlineColors.Muted) },
                    onClick = {
                        menu = false
                        music.stop()
                    },
                )
            }
        }
    }
}

/** Art, title, artist, the seek bar and the transport. */
@Composable
private fun NowPlaying(s: PlayerSnapshotRecord, music: MusicState, current: QueueTrackRecord?, showWaveform: Boolean) {
    val accent = LocalAccent.current
    Column(Modifier.padding(horizontal = PagePadding), horizontalAlignment = Alignment.CenterHorizontally) {
        BoxWithConstraints(Modifier.fillMaxWidth().padding(vertical = 16.dp), contentAlignment = Alignment.Center) {
            val side = if (maxWidth < 360.dp) maxWidth else 360.dp
            Cover(s.thumbnail, if (s.isLink) Icons.Rounded.Link else Icons.Rounded.MusicNote, size = side)
        }
        Text(
            s.title ?: "",
            style = MaterialTheme.typography.headlineSmall,
            color = RedlineColors.Text,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.fillMaxWidth(),
        )
        val (line, error) = statusLine(s)
        Text(
            listOfNotNull(line.takeIf { it.isNotEmpty() }, s.album?.takeIf { !error && it.isNotBlank() && line == s.artist }).joinToString(" · "),
            style = MaterialTheme.typography.bodyMedium,
            color = if (error) accent.hover else RedlineColors.Muted,
            maxLines = if (error) 4 else 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.fillMaxWidth(),
        )
        if (s.status == PlayerStatus.ERROR && (s.error?.lines()?.size ?: 0) > 1) {
            Text(s.error.orEmpty(), style = MaterialTheme.typography.bodySmall, color = RedlineColors.Subtle, modifier = Modifier.fillMaxWidth())
        }
        if (showWaveform && current != null) {
            Spacer(Modifier.height(12.dp))
            WaveformStrip(s, current.track, music)
        }
        Spacer(Modifier.height(12.dp))
        SeekBar(s, music::seek)
        LoopControls(s, music)
        Spacer(Modifier.height(8.dp))
        Transport(s, music)
        Spacer(Modifier.height(16.dp))
    }
}

/**
 * The optional waveform: cached peaks, the A-B span, and a playhead that follows the track.
 * Press and drag anywhere on it to seek on release.
 */
@Composable
private fun WaveformStrip(s: PlayerSnapshotRecord, track: TrackRecord, music: MusicState) {
    val duration = s.durationSecs?.takeIf { it > 0 } ?: return
    var peaks by remember(track.location, track.kind) { mutableStateOf<List<Float>?>(null) }
    var failed by remember(track.location, track.kind) { mutableStateOf(false) }
    LaunchedEffect(track.location, track.kind) {
        peaks = null
        failed = false
        try {
            peaks = music.waveformForTrack(track).peaks
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            failed = true
        }
    }
    val bars = peaks
    if (bars == null) {
        Box(Modifier.fillMaxWidth().height(80.dp), contentAlignment = Alignment.Center) {
            if (failed) {
                Text("Waveform unavailable", color = RedlineColors.Muted, style = MaterialTheme.typography.bodySmall)
            } else {
                CircularProgressIndicator(Modifier.size(18.dp), color = LocalAccent.current.base, strokeWidth = 2.dp)
            }
        }
        return
    }
    var dragging by remember { mutableStateOf<Float?>(null) }
    val accent = LocalAccent.current
    Canvas(
        Modifier
            .fillMaxWidth()
            .height(84.dp)
            .pointerInput(duration, bars) {
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    if (size.width <= 0) return@awaitEachGesture
                    fun at(x: Float): Float =
                        ((x / size.width) * duration).coerceIn(0.0, duration).toFloat()
                    var pos = at(down.position.x)
                    dragging = pos
                    down.consume()
                    while (true) {
                        val event = awaitPointerEvent()
                        val change = event.changes.firstOrNull { it.id == down.id } ?: break
                        if (!change.pressed) break
                        pos = at(change.position.x)
                        dragging = pos
                        change.consume()
                    }
                    music.seek(pos.toDouble())
                    dragging = null
                }
            },
    ) {
        val width = size.width
        val height = size.height
        val mid = height / 2f
        val play = (dragging ?: s.positionSecs.toFloat()).coerceIn(0f, duration.toFloat())
        val playFrac = (play / duration).toFloat()
        val a = s.loopASecs
        val b = s.loopBSecs
        if (a != null && b != null) {
            drawRect(
                color = accent.base.copy(alpha = 0.16f),
                topLeft = Offset((a / duration).toFloat() * width, 0f),
                size = Size(((b - a) / duration).toFloat() * width, height),
            )
        }
        val slots = (width / 3.dp.toPx()).toInt().coerceIn(1, bars.size)
        val step = width / slots
        for (i in 0 until slots) {
            val from = i * bars.size / slots
            val to = ((i + 1) * bars.size / slots).coerceAtLeast(from + 1)
            val peak = bars.subList(from, to).max()
            val x = (i + 0.5f) * step
            val amp = (peak * height * 0.42f).coerceAtLeast(0.5f)
            drawLine(
                color = if (x <= playFrac * width) accent.base else RedlineColors.Muted,
                start = Offset(x, mid - amp),
                end = Offset(x, mid + amp),
                strokeWidth = (step * 0.5f).coerceAtLeast(1f),
            )
        }
        for (mark in listOfNotNull(a, b)) {
            val x = (mark / duration).toFloat() * width
            drawLine(RedlineColors.Text, Offset(x, 0f), Offset(x, height), strokeWidth = 1.5.dp.toPx())
        }
        val playX = playFrac * width
        drawLine(RedlineColors.Text, Offset(playX, 0f), Offset(playX, height), strokeWidth = 2.dp.toPx())
    }
}

/**
 * Follows the track; dragging shows where you'll land, and it seeks on release. The A-B
 * loop's span is lit on the track, with white marks at A and B like the trim slider's handles.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SeekBar(s: PlayerSnapshotRecord, seek: (Double) -> Unit) {
    val duration = s.durationSecs?.takeIf { it > 0 }
    var dragging by remember { mutableStateOf<Float?>(null) }
    val position = dragging?.toDouble() ?: s.positionSecs
    val colors = sliderColors()
    val accent = LocalAccent.current
    val frac = { t: Double? -> if (t == null || duration == null) null else (t / duration).coerceIn(0.0, 1.0).toFloat() }
    val (a, b) = frac(s.loopASecs) to frac(s.loopBSecs)
    Slider(
        track = { state ->
            SliderDefaults.Track(
                sliderState = state,
                colors = colors,
                enabled = duration != null,
                drawStopIndicator = null,
                modifier = Modifier.drawWithContent {
                    drawContent()
                    if (a != null && b != null) {
                        drawRect(accent.hover.copy(alpha = 0.45f), Offset(size.width * a, 0f), Size(size.width * (b - a), size.height))
                    }
                    for (f in listOfNotNull(a, b)) {
                        val w = 3.dp.toPx()
                        drawRect(RedlineColors.Text, Offset(size.width * f - w / 2, -2.dp.toPx()), Size(w, size.height + 4.dp.toPx()))
                    }
                },
            )
        },
        value = (duration?.let { position.coerceIn(0.0, it) } ?: 0.0).toFloat(),
        onValueChange = { dragging = it },
        onValueChangeFinished = {
            dragging?.let { seek(it.toDouble()) }
            dragging = null
        },
        valueRange = 0f..(duration?.toFloat() ?: 1f),
        enabled = duration != null,
        colors = colors,
        modifier = Modifier.fillMaxWidth(),
    )
    Row(Modifier.fillMaxWidth()) {
        Text(fmtDuration(position), style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
        Spacer(Modifier.weight(1f))
        Text(duration?.let(::fmtDuration) ?: if (s.isLink) "Live" else "", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
    }
}

/** A-B loop: A and B mark where the track is now; reaching B goes back to A. */
@Composable
private fun LoopControls(s: PlayerSnapshotRecord, music: MusicState) {
    val accent = LocalAccent.current
    val can = s.durationSecs != null && s.status in setOf(PlayerStatus.PLAYING, PlayerStatus.PAUSED, PlayerStatus.ENDED)
    val (a, b) = s.loopASecs to s.loopBSecs
    // The core refuses a B less than half a second after A.
    val bOk = s.positionSecs >= (a ?: 0.0) + 0.5
    val label = { name: String, t: Double? -> if (t == null) "Set $name" else "$name ${fmtDuration(t)}" }
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Text("Loop", style = MaterialTheme.typography.labelLarge, color = RedlineColors.Muted)
        Spacer(Modifier.width(4.dp))
        TextButton(onClick = { music.setLoopA() }, enabled = can) {
            Text(label("A", a), color = if (a != null) accent.hover else if (can) RedlineColors.Text else RedlineColors.Subtle)
        }
        TextButton(onClick = { music.setLoopB() }, enabled = can && bOk) {
            Text(label("B", b), color = if (b != null) accent.hover else if (can && bOk) RedlineColors.Text else RedlineColors.Subtle)
        }
        Spacer(Modifier.weight(1f))
        if (a != null || b != null) {
            TextButton(onClick = { music.clearLoop() }) { Text("Clear", color = RedlineColors.Muted) }
        }
    }
}

@Composable
private fun Transport(s: PlayerSnapshotRecord, music: MusicState) {
    val accent = LocalAccent.current
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.SpaceBetween) {
        IconButton(onClick = { music.setShuffle(!s.shuffle) }) {
            Icon(Icons.Rounded.Shuffle, if (s.shuffle) "Shuffle off" else "Shuffle", tint = if (s.shuffle) accent.base else RedlineColors.Muted)
        }
        IconButton(onClick = { music.prev() }, modifier = Modifier.size(56.dp)) {
            Icon(Icons.Rounded.SkipPrevious, if (s.hasPrev) "Previous" else "Back to start", tint = RedlineColors.Text, modifier = Modifier.size(36.dp))
        }
        Box(
            Modifier
                .size(68.dp)
                .background(RedlineColors.Text, CircleShape)
                .clickable { music.toggle() },
            contentAlignment = Alignment.Center,
        ) {
            if (waiting(s)) {
                CircularProgressIndicator(Modifier.size(30.dp), color = RedlineColors.Black, strokeWidth = 3.dp)
            } else {
                Icon(
                    if (s.isPlaying) Icons.Rounded.Pause else Icons.Rounded.PlayArrow,
                    if (s.isPlaying) "Pause" else "Play",
                    tint = RedlineColors.Black,
                    modifier = Modifier.size(38.dp),
                )
            }
        }
        IconButton(onClick = { music.next() }, enabled = s.hasNext, modifier = Modifier.size(56.dp)) {
            Icon(Icons.Rounded.SkipNext, "Next", tint = if (s.hasNext) RedlineColors.Text else RedlineColors.Subtle, modifier = Modifier.size(36.dp))
        }
        val (icon, label, next) = when (s.repeat) {
            RepeatMode.OFF -> Triple(Icons.Rounded.Repeat, "Repeat all", RepeatMode.ALL)
            RepeatMode.ALL -> Triple(Icons.Rounded.Repeat, "Repeat one", RepeatMode.ONE)
            RepeatMode.ONE -> Triple(Icons.Rounded.RepeatOne, "Repeat off", RepeatMode.OFF)
        }
        IconButton(onClick = { music.setRepeat(next) }) {
            Icon(icon, label, tint = if (s.repeat == RepeatMode.OFF) RedlineColors.Muted else accent.base)
        }
    }
}

/** Up next (playlists.rs `now_playing`): tap to play from there, × to take it out. */
@Composable
private fun QueueRow(q: QueueTrackRecord, jump: () -> Unit, remove: () -> Unit) {
    val t = q.track
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(onClick = jump)
            .padding(start = PagePadding, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Cover(t.art, if (t.kind == TrackKind.LINK) Icons.Rounded.Link else Icons.Rounded.MusicNote, size = 40.dp)
        Spacer(Modifier.width(10.dp))
        Column(Modifier.weight(1f)) {
            Text(t.title, style = MaterialTheme.typography.titleSmall, color = RedlineColors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis)
            val sub = listOfNotNull(t.artist, t.durationSecs?.let(::fmtDuration)).joinToString(" · ")
            if (sub.isNotEmpty()) Text(sub, style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        IconButton(onClick = remove) { Icon(Icons.Rounded.Close, "Remove from queue", tint = RedlineColors.Muted) }
    }
}

@Composable
internal fun PlayerCard(title: String? = null, content: @Composable () -> Unit) {
    Column(
        Modifier
            .fillMaxWidth()
            .background(RedlineColors.Card, MaterialTheme.shapes.medium)
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        title?.let { Text(it, style = MaterialTheme.typography.titleMedium, color = RedlineColors.Text, textAlign = TextAlign.Start) }
        content()
    }
}

@Composable
internal fun sliderColors(): SliderColors {
    val accent = LocalAccent.current
    return SliderDefaults.colors(
        thumbColor = RedlineColors.Text,
        activeTrackColor = accent.base,
        inactiveTrackColor = RedlineColors.Track,
        disabledThumbColor = RedlineColors.Subtle,
        disabledActiveTrackColor = RedlineColors.Track,
        disabledInactiveTrackColor = RedlineColors.Track,
        activeTickColor = androidx.compose.ui.graphics.Color.Transparent,
        inactiveTickColor = androidx.compose.ui.graphics.Color.Transparent,
    )
}
