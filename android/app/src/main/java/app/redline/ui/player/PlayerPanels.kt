package app.redline.ui.player

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.width
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.redline.core.EqRecord
import app.redline.core.PlayerChoicesRecord
import app.redline.core.PlayerSettingsRecord
import app.redline.state.MusicState
import app.redline.ui.components.Chips
import app.redline.ui.components.ToggleRow
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import kotlinx.coroutines.withTimeoutOrNull
import kotlin.math.abs
import kotlin.math.roundToInt

/** player_panels.rs `speed`: the chips, a fine slider (double-tap or hold for 1×), reset per track. */
@Composable
internal fun SpeedCard(p: PlayerSettingsRecord, choices: PlayerChoicesRecord, music: MusicState) {
    PlayerCard("Speed · ${speedLabel(p.speed)}") {
        Chips(
            value = choices.speeds.firstOrNull { abs(it - p.speed) < 0.001f },
            options = choices.speeds.map { it to speedLabel(it) },
            onSelect = { it?.let(music::setSpeed) },
        )
        val steps = ((choices.maxSpeed - choices.minSpeed) / SPEED_STEP).roundToInt() - 1
        Slider(
            value = p.speed,
            onValueChange = { v ->
                val snapped = (choices.minSpeed + ((v - choices.minSpeed) / SPEED_STEP).roundToInt() * SPEED_STEP)
                    .coerceIn(choices.minSpeed, choices.maxSpeed)
                if (abs(snapped - p.speed) > 0.001f) music.setSpeed(snapped)
            },
            valueRange = choices.minSpeed..choices.maxSpeed,
            steps = steps.coerceAtLeast(0),
            colors = sliderColors(),
            modifier = Modifier.fillMaxWidth().resetGestures { music.setSpeed(1f) },
        )
        Text("Double-tap or hold the slider for 1×", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Subtle)
        ToggleRow("Back to 1× for each new track", p.resetSpeed, music::setResetSpeed)
        Text("Pitch rises and falls with speed, like a record player.", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Subtle)
    }
}

/** player_panels.rs `equalizer`: on/off, the presets, and the preamp plus ten bands. */
@Composable
internal fun EqCard(p: PlayerSettingsRecord, choices: PlayerChoicesRecord, set: (EqRecord) -> Unit) {
    val eq = p.eq
    PlayerCard {
        ToggleRow("Equalizer", eq.enabled, { set(eq.copy(enabled = it)) })
        Chips(
            value = p.eqPreset,
            options = choices.eqPresets.map { it.name to it.name },
            onSelect = { name ->
                choices.eqPresets.firstOrNull { it.name == name }?.let { preset ->
                    set(EqRecord(enabled = true, preampDb = preset.preampDb, bandsDb = preset.bandsDb))
                }
            },
        )
        val dim = !eq.enabled
        Row(Modifier.fillMaxWidth()) {
            Band("Pre", eq.preampDb, choices.preampMinDb..choices.preampMaxDb, dim) { set(eq.copy(preampDb = it)) }
            for ((i, freq) in choices.eqFreqs.withIndex()) {
                val value = eq.bandsDb.getOrElse(i) { 0f }
                Band(bandLabel(freq), value, -choices.bandDb..choices.bandDb, dim) { v ->
                    val bands = choices.eqFreqs.indices.map { j -> if (j == i) v else eq.bandsDb.getOrElse(j) { 0f } }
                    set(eq.copy(bandsDb = bands))
                }
            }
        }
        Text(
            if (dim) "Turned off: you hear the track as it is." else "Double-tap a slider for 0 dB.",
            style = MaterialTheme.typography.bodySmall,
            color = RedlineColors.Subtle,
        )
    }
}

/** One EQ column: its gain, the slider, its frequency. */
@Composable
private fun RowScope.Band(label: String, value: Float, range: ClosedFloatingPointRange<Float>, dim: Boolean, onChange: (Float) -> Unit) {
    val small = MaterialTheme.typography.labelMedium.copy(fontSize = 10.sp)
    Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally) {
        Text(dbLabel(value), style = small, color = if (dim) RedlineColors.Subtle else RedlineColors.Text, textAlign = TextAlign.Center, maxLines = 1, softWrap = false)
        VerticalSlider(value, range, dim, onChange)
        Text(label, style = small, color = RedlineColors.Muted, textAlign = TextAlign.Center, maxLines = 1, softWrap = false)
    }
}

/**
 * A slim vertical slider in half-dB steps, filled from 0 dB towards the thumb. Drag or tap to
 * set; double-tap for 0 dB. It takes vertical drags before the page's scrolling does.
 */
@Composable
private fun VerticalSlider(value: Float, range: ClosedFloatingPointRange<Float>, dim: Boolean, onChange: (Float) -> Unit) {
    val accent = LocalAccent.current
    val change by rememberUpdatedState(onChange)
    val current by rememberUpdatedState(value)
    val span = range.endInclusive - range.start
    fun valueAt(y: Float, height: Float): Float {
        val frac = 1f - (y / height).coerceIn(0f, 1f)
        val raw = range.start + frac * span
        return ((raw * 2).roundToInt() / 2f).coerceIn(range.start, range.endInclusive)
    }
    fun send(v: Float) {
        if (abs(v - current) > 0.01f) change(v)
    }
    Canvas(
        Modifier
            .width(26.dp)
            .height(150.dp)
            .pointerInput(range) {
                detectTapGestures(
                    onDoubleTap = { send(0f.coerceIn(range.start, range.endInclusive)) },
                    onTap = { send(valueAt(it.y, size.height.toFloat())) },
                )
            }
            .pointerInput(range) {
                detectVerticalDragGestures { c, _ ->
                    c.consume()
                    send(valueAt(c.position.y, size.height.toFloat()))
                }
            },
    ) {
        val x = size.width / 2
        val pad = 8.dp.toPx()
        val h = size.height - 2 * pad
        fun yOf(v: Float) = pad + (1f - (v - range.start) / span) * h
        val track = 4.dp.toPx()
        drawLine(RedlineColors.Track, Offset(x, pad), Offset(x, pad + h), track, StrokeCap.Round)
        val color = if (dim) RedlineColors.Subtle else accent.base
        drawLine(color, Offset(x, yOf(0f.coerceIn(range.start, range.endInclusive))), Offset(x, yOf(value)), track, StrokeCap.Round)
        drawCircle(if (dim) RedlineColors.Muted else RedlineColors.Text, 7.dp.toPx(), Offset(x, yOf(value)))
    }
}

/**
 * Calls [onReset] on a double-tap, or a press held still, without taking the events from the
 * control inside (it sees them after the control, so the control's own tap lands first).
 * [onReset] is read once, so it shouldn't capture anything that changes.
 */
private fun Modifier.resetGestures(onReset: () -> Unit): Modifier = pointerInput(Unit) {
    var lastTapUp = 0L
    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false)
        val start = down.position
        var moved = false
        val up = withTimeoutOrNull(viewConfiguration.longPressTimeoutMillis) {
            var result = -1L
            while (true) {
                val event = awaitPointerEvent()
                val c = event.changes.firstOrNull { it.id == down.id } ?: break
                if ((c.position - start).getDistance() > viewConfiguration.touchSlop) moved = true
                if (!c.pressed) {
                    result = c.uptimeMillis
                    break
                }
            }
            result
        }
        when {
            // Held without moving.
            up == null -> {
                lastTapUp = 0L
                if (!moved) onReset()
            }
            up > 0 && !moved -> {
                if (down.uptimeMillis - lastTapUp <= viewConfiguration.doubleTapTimeoutMillis) {
                    lastTapUp = 0L
                    onReset()
                } else {
                    lastTapUp = up
                }
            }
            else -> lastTapUp = 0L
        }
    }
}

private const val SPEED_STEP = 0.05f

/** "1.25×", trimmed (player_panels.rs `speed_label`). */
fun speedLabel(speed: Float): String {
    val s = "%.2f".format(java.util.Locale.ROOT, speed).trimEnd('0').trimEnd('.')
    return "$s×"
}

/** "62", "1k", "16k". */
private fun bandLabel(freq: Float): String =
    if (freq >= 1000f) {
        val k = freq / 1000f
        (if (k == k.toInt().toFloat()) "${k.toInt()}" else "%.1f".format(java.util.Locale.ROOT, k)) + "k"
    } else {
        "${freq.roundToInt()}"
    }

/** "0", "+3.5", "-2" (narrow: eleven columns share a phone's width). */
private fun dbLabel(db: Float): String = when {
    abs(db) < 0.05f -> "0"
    abs(db - db.roundToInt()) < 0.05f -> "%+d".format(java.util.Locale.ROOT, db.roundToInt())
    else -> "%+.1f".format(java.util.Locale.ROOT, db)
}
