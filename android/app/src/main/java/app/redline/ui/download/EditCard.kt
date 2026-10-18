package app.redline.ui.download

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.WarningAmber
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RangeSlider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import app.redline.core.DownloadOptionsRecord
import app.redline.core.fmtDuration
import app.redline.core.fmtTime
import app.redline.ui.components.Chips
import app.redline.ui.components.InputField
import app.redline.ui.components.OptionRow
import app.redline.ui.components.SectionCard
import app.redline.ui.components.ToggleRow
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import kotlin.math.ceil

/** Trim and clean-up applied while downloading (download.rs `edit_card`). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun EditCard(model: DownloadModel, o: DownloadOptionsRecord) {
    val t = model.trim
    val info = model.info
    val duration = info?.durationSecs
    SectionCard(title = "Edit") {
        if (model.isPlaylist) {
            Text("Trimming works on single videos, not playlists.", color = RedlineColors.Muted)
        } else {
            ToggleRow("Trim", t.on, { t.on = it })
            if (t.on) {
                val total = duration?.takeIf { it >= 2.0 }?.let { ceil(it) }
                if (total != null) {
                    val accent = LocalAccent.current
                    RangeSlider(
                        value = t.start.toFloat()..(t.end ?: total).toFloat(),
                        onValueChange = { r -> t.setRange(Math.round(r.start).toDouble(), Math.round(r.endInclusive).toDouble()) },
                        valueRange = 0f..total.toFloat(),
                        colors = SliderDefaults.colors(
                            thumbColor = RedlineColors.Text,
                            activeTrackColor = accent.base,
                            inactiveTrackColor = RedlineColors.Track,
                        ),
                    )
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                        Text("0:00", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Subtle)
                        Text(fmtDuration(total), style = MaterialTheme.typography.bodySmall, color = RedlineColors.Subtle)
                    }
                }
                val end = t.end
                OptionRow("Keep", hint = end?.takeIf { it > t.start }?.let { "${fmtTime(it - t.start)} long" }) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        val numbers = KeyboardOptions(keyboardType = KeyboardType.Text)
                        InputField(t.startText, t::editStart, Modifier.weight(1f), placeholder = "0:00", keyboardOptions = numbers)
                        Text("  to  ", color = RedlineColors.Muted)
                        InputField(
                            t.endText,
                            t::editEnd,
                            Modifier.weight(1f),
                            placeholder = if (duration != null) "" else "end",
                            keyboardOptions = numbers,
                        )
                    }
                }
                OptionRow(
                    "Cut",
                    hint = if (t.precise) "Re-encodes around the cuts so they land exactly" else "Cuts may snap to the nearest keyframe",
                ) {
                    Chips(t.precise, listOf(false to "Fast", true to "Precise"), { t.precise = it })
                }
                model.trimProblem()?.let { problem ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Icon(Icons.Rounded.WarningAmber, null, tint = RedlineColors.Muted)
                        Spacer(Modifier.width(8.dp))
                        Text(problem, color = RedlineColors.Muted)
                    }
                }
            }
        }

        HorizontalDivider(color = RedlineColors.Divider)
        ToggleRow(
            "Remove sponsor segments",
            o.removeSponsors,
            { v -> model.updateOptions { it.copy(removeSponsors = v) } },
            hint = "YouTube only · via SponsorBlock",
        )
        if (!model.isPlaylist && !t.on) {
            val hint = when (val chapters = info?.chapters) {
                null -> "Only for videos with chapters"
                0u -> "This video has no chapters"
                else -> "$chapters chapters · saved in a folder next to the full file"
            }
            ToggleRow("Split into chapter files", o.splitChapters, { v -> model.updateOptions { it.copy(splitChapters = v) } }, hint = hint)
        }
    }
}

/** Audio tags, prefilled from the site (download.rs `tags_card`). */
@Composable
fun TagsCard(model: DownloadModel) {
    SectionCard(title = "Tags", subtitle = "Written into the audio file. Edit to fix names from the site.") {
        val tags = model.tags
        OptionRow("Title") { InputField(tags.title, { model.tags = tags.copy(title = it) }) }
        OptionRow("Artist") { InputField(tags.artist, { model.tags = tags.copy(artist = it) }) }
        OptionRow("Album") { InputField(tags.album, { model.tags = tags.copy(album = it) }) }
    }
}
