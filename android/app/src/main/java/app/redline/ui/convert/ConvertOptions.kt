package app.redline.ui.convert

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Info
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import app.redline.core.ConvertChoiceListsRecord
import app.redline.core.ConvertOptionsRecord
import app.redline.core.ConvertSectionsRecord
import app.redline.core.CropChoice
import app.redline.core.GifOptionsRecord
import app.redline.core.GifPlayback
import app.redline.core.RotateChoice
import app.redline.core.codecLabel
import app.redline.core.defaultConvertOptions
import app.redline.core.encoderLabel
import app.redline.core.fmtTime
import app.redline.core.parseDuration
import app.redline.ui.components.Chips
import app.redline.ui.components.Dropdown
import app.redline.ui.components.Eyebrow
import app.redline.ui.components.InputField
import app.redline.ui.components.OptionRow
import app.redline.ui.components.ToggleRow
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import kotlin.math.ln
import kotlin.math.pow
import kotlin.math.roundToInt

/** Edits options by a change function, so quick edits in a row don't undo each other. */
typealias OptionsEdit = ((ConvertOptionsRecord) -> ConvertOptionsRecord) -> Unit

/**
 * The option groups that apply (convert.rs `option_sections`): video, GIF, audio, image and
 * adjust. Shared by the Options card and each file's own editor.
 */
@Composable
fun OptionSections(o: ConvertOptionsRecord, sec: ConvertSectionsRecord, lists: ConvertChoiceListsRecord, edit: OptionsEdit) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        if (sec.video) VideoOptions(o, sec, lists, edit)
        if (sec.gif) GifOptions(o.gif, lists) { change -> edit { it.copy(gif = change(it.gif)) } }
        if (sec.audio) {
            Eyebrow("Audio")
            OptionRow("Bitrate", hint = "Ignored for FLAC and WAV") {
                Dropdown("${o.audioBitrate} kbps", lists.audioBitrates, { "$it kbps" }, { b -> edit { it.copy(audioBitrate = b) } })
            }
        }
        if (sec.image) {
            Eyebrow("Image")
            OptionRow("Quality: ${o.imageQuality}", hint = "JPG and AVIF · WebP is saved lossless") {
                AccentSlider(o.imageQuality.toFloat(), 1f..100f, 98) { v -> edit { it.copy(imageQuality = v.roundToInt().coerceIn(1, 100).toUByte()) } }
            }
            OptionRow("Fit within", hint = "0 keeps original size") {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    WholeField(o.resizeWidth.toLong(), 0L..16384L, "px", Modifier.weight(1f)) { v -> edit { it.copy(resizeWidth = v.toUInt()) } }
                    Text("  ×  ", color = RedlineColors.Muted)
                    WholeField(o.resizeHeight.toLong(), 0L..16384L, "px", Modifier.weight(1f)) { v -> edit { it.copy(resizeHeight = v.toUInt()) } }
                }
            }
        }
        AdjustOptions(o, sec, lists, edit)
    }
}

@Composable
private fun VideoOptions(o: ConvertOptionsRecord, sec: ConvertSectionsRecord, lists: ConvertChoiceListsRecord, edit: OptionsEdit) {
    // Every file becomes WebM: H.264 and H.265 don't fit, so the choice moves to VP9.
    if (sec.codec != o.codec) {
        LaunchedEffect(sec.codec, o.codec) { edit { it.copy(codec = sec.codec) } }
    }
    Eyebrow("Video")
    OptionRow("Codec", hint = sec.codecHint) {
        Chips(sec.codec, sec.codecs.map { it to codecLabel(it) }, { c -> edit { it.copy(codec = c) } })
    }
    for (note in sec.codecNotes) Small(note)
    OptionRow("Encoder", hint = sec.encoderNote) {
        Chips(o.hardware, sec.encoders.map { it to encoderLabel(it) }, { h -> edit { it.copy(hardware = h) } })
    }
    val bySize = o.targetSizeMb > 0f
    OptionRow("Size") {
        Chips(bySize, listOf(false to "Set quality", true to "Fit a file size"), { on -> edit { it.copy(targetSizeMb = if (on) 10f else 0f) } })
    }
    if (bySize) {
        OptionRow("Target size", hint = "Each video aims just under this size. Lower the resolution for long videos.") {
            DecimalField(o.targetSizeMb.toDouble(), "MB") { v -> edit { it.copy(targetSizeMb = v.coerceIn(0.5, 10_000.0).toFloat()) } }
            Chips(o.targetSizeMb, listOf(8f, 10f, 25f, 50f, 100f).map { it to "${it.toInt()} MB" }, { mb -> edit { it.copy(targetSizeMb = mb) } })
        }
    } else {
        // The slider runs "smaller file" → "better quality", the inverse of CRF.
        OptionRow("Quality", hint = "CRF ${o.crf}") {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Smaller", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
                Spacer(Modifier.width(8.dp))
                AccentSlider((51 - o.crf.toInt()).toFloat(), 15f..37f, 21, Modifier.weight(1f)) { q ->
                    edit { it.copy(crf = (51 - q.roundToInt()).coerceIn(14, 36).toUByte()) }
                }
                Spacer(Modifier.width(8.dp))
                Text("Better", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
            }
        }
    }
    OptionRow("Encode speed") {
        Dropdown(o.preset, lists.encodeSpeeds, { it }, { p -> edit { it.copy(preset = p) } })
    }
    OptionRow("Max resolution") {
        Dropdown(
            lists.maxHeights.firstOrNull { it.value == o.maxHeight }?.label ?: "Custom",
            lists.maxHeights,
            { it.label },
            { h -> edit { it.copy(maxHeight = h.value) } },
        )
    }
}

/** Video → GIF settings (convert.rs `gif_options`). */
@Composable
private fun GifOptions(g: GifOptionsRecord, lists: ConvertChoiceListsRecord, edit: ((GifOptionsRecord) -> GifOptionsRecord) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Eyebrow("GIF", Modifier.weight(1f))
        TextButton(onClick = { edit { defaultConvertOptions().gif } }) { Text("Reset", color = RedlineColors.Subtle) }
    }
    OptionRow("Clip", hint = "Type 1:30 or seconds") {
        Row(verticalAlignment = Alignment.CenterVertically) {
            TimeField(g.startSecs, "Start 0:00", Modifier.weight(1f)) { v -> edit { it.copy(startSecs = v) } }
            Spacer(Modifier.width(8.dp))
            TimeField(g.lengthSecs, "Length: whole video", Modifier.weight(1f)) { v -> edit { it.copy(lengthSecs = v) } }
        }
    }
    OptionRow("Width", hint = "Height follows the aspect ratio") {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Dropdown(
                lists.gifWidths.firstOrNull { it.value == g.width }?.label ?: "${g.width} px",
                lists.gifWidths,
                { it.label },
                { w -> edit { it.copy(width = w.value) } },
                Modifier.weight(1f),
            )
            Spacer(Modifier.width(8.dp))
            WholeField(g.width.toLong(), 0L..3840L, "px", Modifier.weight(1f)) { v -> edit { it.copy(width = v.toUInt()) } }
        }
    }
    OptionRow("Frame rate", hint = "Lower is smaller") {
        Dropdown("${g.fps} fps", lists.gifFps, { "$it fps" }, { f -> edit { it.copy(fps = f) } })
    }
    OptionRow("Colors: ${g.colors}", hint = "Fewer colors, smaller file") {
        // Logarithmic like the desktop's slider: 2 … 256.
        val pos = (ln(g.colors.toDouble().coerceIn(2.0, 256.0) / 2.0) / ln(128.0)).toFloat()
        AccentSlider(pos, 0f..1f, 0) { p ->
            val colors = (2.0 * 128.0.pow(p.toDouble())).roundToInt().coerceIn(2, 256)
            edit { it.copy(colors = colors.toUInt()) }
        }
    }
    OptionRow("Dithering") {
        Chips(g.dither, lists.gifDithers.map { it.value to it.label }, { d -> edit { it.copy(dither = d) } })
    }
    OptionRow("Speed") {
        Chips(g.speed, lists.gifSpeeds.map { it.value to it.label }, { s -> edit { it.copy(speed = s) } })
    }
    OptionRow("Playback") {
        Chips(
            g.direction,
            listOf(GifPlayback.FORWARD to "Forward", GifPlayback.REVERSE to "Reverse", GifPlayback.BOOMERANG to "Boomerang"),
            { d -> edit { it.copy(direction = d) } },
        )
    }
    OptionRow("Loop") {
        Chips(g.plays, listOf(0u to "Forever", 1u to "Once", 2u to "Twice", 3u to "3 times"), { n -> edit { it.copy(plays = n) } })
    }
    ToggleRow(
        "Only redraw what changes",
        g.diffOnly,
        { v -> edit { it.copy(diffOnly = v) } },
        hint = "Smaller files for clips with a still background",
    )
    if (g.direction != GifPlayback.FORWARD && g.lengthSecs == 0.0) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Rounded.Info, null, tint = RedlineColors.Muted, modifier = Modifier.width(16.dp))
            Spacer(Modifier.width(6.dp))
            Text(
                "Reverse and Boomerang hold the whole clip in memory. Set a length for long videos.",
                style = MaterialTheme.typography.bodySmall,
                color = RedlineColors.Muted,
            )
        }
    }
}

/** Rotate, crop, speed and loudness (convert.rs `adjust_options`). */
@Composable
private fun AdjustOptions(o: ConvertOptionsRecord, sec: ConvertSectionsRecord, lists: ConvertChoiceListsRecord, edit: OptionsEdit) {
    if (!sec.picture && !sec.timed) return
    val f = o.filters
    val reset = defaultConvertOptions().filters
    Row(verticalAlignment = Alignment.CenterVertically) {
        Eyebrow("Adjust", Modifier.weight(1f))
        if (f != reset) {
            TextButton(onClick = { edit { it.copy(filters = reset) } }) { Text("Reset", color = RedlineColors.Subtle) }
        }
    }
    if (sec.picture) {
        OptionRow("Rotate") {
            Chips(
                f.rotate,
                listOf(RotateChoice.NONE to "None", RotateChoice.RIGHT to "90° right", RotateChoice.LEFT to "90° left", RotateChoice.HALF to "180°"),
                { r -> edit { it.copy(filters = it.filters.copy(rotate = r)) } },
            )
        }
        ToggleRow("Mirror", f.flip, { v -> edit { it.copy(filters = it.filters.copy(flip = v)) } })
        OptionRow("Crop", hint = if (f.crop != CropChoice.NONE) "Keeps the center" else null) {
            Chips(
                f.crop,
                listOf(
                    CropChoice.NONE to "None",
                    CropChoice.WIDE to "16:9",
                    CropChoice.TALL to "9:16",
                    CropChoice.SQUARE to "1:1",
                    CropChoice.PORTRAIT to "4:5",
                ),
                { c -> edit { it.copy(filters = it.filters.copy(crop = c)) } },
            )
        }
    }
    if (sec.timed) {
        OptionRow("Speed") {
            Chips(f.speed, lists.speeds.map { it.value to it.label }, { s -> edit { it.copy(filters = it.filters.copy(speed = s)) } })
        }
        ToggleRow(
            "Normalize loudness",
            f.normalize,
            { v -> edit { it.copy(filters = it.filters.copy(normalize = v)) } },
            hint = "Normalize evens out quiet and loud files",
        )
        val gain = if (f.gainDb == 0f) "0 dB" else "%+.1f dB".format(f.gainDb)
        OptionRow("Volume: $gain") {
            AccentSlider(f.gainDb, -30f..30f, 119) { v ->
                val db = (v * 2).roundToInt() / 2f
                edit { it.copy(filters = it.filters.copy(gainDb = db)) }
            }
        }
    }
}

@Composable
private fun Small(text: String) {
    Text(text, style = MaterialTheme.typography.bodySmall, color = RedlineColors.Subtle)
}

/** The app's slider: white thumb, accent track (as in Settings). */
@Composable
fun AccentSlider(value: Float, range: ClosedFloatingPointRange<Float>, steps: Int, modifier: Modifier = Modifier, onChange: (Float) -> Unit) {
    val accent = LocalAccent.current
    Slider(
        value = value.coerceIn(range),
        onValueChange = onChange,
        valueRange = range,
        steps = steps,
        modifier = modifier.fillMaxWidth(),
        colors = SliderDefaults.colors(
            thumbColor = RedlineColors.Text,
            activeTrackColor = accent.base,
            inactiveTrackColor = RedlineColors.Track,
            activeTickColor = androidx.compose.ui.graphics.Color.Transparent,
            inactiveTickColor = androidx.compose.ui.graphics.Color.Transparent,
        ),
    )
}

/**
 * A text field for a number that keeps what's typed while it doesn't parse (an empty field,
 * "1.") and takes outside changes (a chip, a preset) when they differ from it.
 */
@Composable
private fun ParsedField(
    value: Double,
    format: (Double) -> String,
    parse: (String) -> Double?,
    onValue: (Double) -> Unit,
    modifier: Modifier = Modifier,
    placeholder: String = "",
    suffix: String? = null,
    keyboard: KeyboardType = KeyboardType.Decimal,
) {
    var text by remember { mutableStateOf(format(value)) }
    LaunchedEffect(value) {
        if (parse(text) != value) text = format(value)
    }
    InputField(
        text,
        { t ->
            text = t
            parse(t)?.let(onValue)
        },
        modifier,
        placeholder = placeholder,
        trailing = suffix?.let { { Text(it, color = RedlineColors.Muted, modifier = Modifier.padding(end = 12.dp)) } },
        keyboardOptions = KeyboardOptions(keyboardType = keyboard),
    )
}

@Composable
private fun DecimalField(value: Double, suffix: String, onValue: (Double) -> Unit) {
    ParsedField(
        value,
        format = { v -> if (v == v.toLong().toDouble()) v.toLong().toString() else v.toString() },
        parse = { it.trim().replace(',', '.').toDoubleOrNull() },
        onValue = onValue,
        suffix = suffix,
    )
}

@Composable
private fun WholeField(value: Long, range: LongRange, suffix: String, modifier: Modifier = Modifier, onValue: (Long) -> Unit) {
    ParsedField(
        value.toDouble(),
        format = { it.toLong().toString() },
        parse = { t -> if (t.isBlank()) 0.0 else t.trim().toLongOrNull()?.coerceIn(range)?.toDouble() },
        onValue = { onValue(it.toLong()) },
        modifier = modifier,
        suffix = suffix,
        keyboard = KeyboardType.Number,
    )
}

/** m:ss, or seconds; blank (or words) is 0, which the placeholder names, as on the desktop. */
@Composable
private fun TimeField(secs: Double, placeholder: String, modifier: Modifier = Modifier, onValue: (Double) -> Unit) {
    ParsedField(
        secs,
        format = { if (it <= 0.0) "" else fmtTime(it) },
        parse = { t ->
            val s = t.trim()
            when {
                s.isEmpty() || s.any { it.isLetter() } -> 0.0
                else -> parseDuration(s)?.coerceIn(0.0, 86_400.0)
            }
        },
        onValue = onValue,
        modifier = modifier,
        placeholder = placeholder,
        keyboard = KeyboardType.Text,
    )
}
