package app.redline.ui.download

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.ExpandLess
import androidx.compose.material.icons.rounded.ExpandMore
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import app.redline.core.DownloadChoiceListsRecord
import app.redline.core.DownloadMode
import app.redline.core.DownloadOptionsRecord
import app.redline.core.FormatRecord
import app.redline.core.HeightChoiceRecord
import app.redline.core.MediaInfoRecord
import app.redline.core.SubtitleMode
import app.redline.core.fmtBytes
import app.redline.core.isLosslessAudio
import app.redline.ui.components.Chips
import app.redline.ui.components.Dropdown
import app.redline.ui.components.Eyebrow
import app.redline.ui.components.InputField
import app.redline.ui.components.OptionRow
import app.redline.ui.components.SectionCard
import app.redline.ui.components.ToggleRow
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors

/** Video or audio, quality, exact formats, embeds and subtitles (download.rs `format_card`). */
@Composable
fun FormatCard(model: DownloadModel, o: DownloadOptionsRecord, lists: DownloadChoiceListsRecord) {
    val info = model.info
    val update = model::updateOptions
    val formats = info?.takeIf { !it.isPlaylist }?.formats.orEmpty()
    val hasFormats = formats.isNotEmpty()
    SectionCard(title = "Format") {
        Chips(o.mode, listOf(DownloadMode.VIDEO to "Video", DownloadMode.AUDIO to "Audio only"), { m -> update { it.copy(mode = m) } })
        val picked = when (o.mode) {
            DownloadMode.VIDEO -> model.pickVideo != null
            DownloadMode.AUDIO -> model.pickAudio != null
        }
        when (o.mode) {
            DownloadMode.VIDEO -> {
                OptionRow("Quality") {
                    when {
                        picked -> Text("Exact format picked below", color = RedlineColors.Muted)
                        hasFormats && info != null -> HeightPicker(o, info.heights) { h -> update { it.copy(maxHeight = h) } }
                        else -> Dropdown(
                            current = lists.videoQualities.firstOrNull { it.maxHeight == o.maxHeight }?.label ?: "Custom",
                            options = lists.videoQualities,
                            label = { it.label },
                            onSelect = { q -> update { it.copy(maxHeight = q.maxHeight) } },
                        )
                    }
                }
                OptionRow("Container") {
                    Chips(o.container, lists.containers.map { it to it }, { c -> update { it.copy(container = c) } })
                }
            }
            DownloadMode.AUDIO -> {
                OptionRow("Format") {
                    Chips(o.audioFormat, lists.audioFormats.map { it to it }, { f -> update { it.copy(audioFormat = f) } })
                }
                if (!isLosslessAudio(o.audioFormat)) {
                    OptionRow("Bitrate", hint = info?.bestAudioSize?.let { "source ≈ ${fmtBytes(it)}" }) {
                        Dropdown(
                            current = lists.audioBitrates.firstOrNull { it.value == o.audioBitrate }?.label ?: "Custom",
                            options = lists.audioBitrates,
                            label = { it.label },
                            onSelect = { b -> update { it.copy(audioBitrate = b.value) } },
                        )
                    }
                }
            }
        }

        if (hasFormats) ExactFormats(model, formats, o.mode)

        HorizontalDivider(color = RedlineColors.Divider)
        ToggleRow("Embed cover art", o.embedThumbnail, { v -> update { it.copy(embedThumbnail = v) } })
        if (o.mode == DownloadMode.AUDIO && o.embedThumbnail) {
            ToggleRow("Square cover", o.squareCover, { v -> update { it.copy(squareCover = v) } })
        }
        ToggleRow("Embed metadata", o.embedMetadata, { v -> update { it.copy(embedMetadata = v) } })
        ToggleRow("Subtitles", o.embedSubs, { v -> update { it.copy(embedSubs = v) } })
        if (model.isPlaylist) {
            ToggleRow("Download whole playlist", o.playlist, { v -> update { it.copy(playlist = v) } })
        }
        if (o.embedSubs) SubtitleOptions(o, info, lists.defaultSubLangs, update)
    }
}

/** The real heights the link offers, with sizes (download.rs `quality_combo`). */
@Composable
private fun HeightPicker(o: DownloadOptionsRecord, choices: List<HeightChoiceRecord>, onPick: (UInt) -> Unit) {
    fun label(c: HeightChoiceRecord) = buildString {
        append("${c.height}p")
        c.fps?.takeIf { it > 30.5f }?.let { append(Math.round(it)) }
        c.size?.let { append("  ·  ≈ ${fmtBytes(it)}") }
    }
    val current = if (o.maxHeight == 0u) {
        choices.firstOrNull()?.let { "Best (${label(it)})" } ?: "Best available"
    } else {
        // yt-dlp picks the tallest at or below the limit.
        choices.firstOrNull { it.height <= o.maxHeight }?.let(::label) ?: "Up to ${o.maxHeight}p"
    }
    val options = listOf<HeightChoiceRecord?>(null) + choices
    Dropdown(current, options, { it?.let(::label) ?: "Best available" }, { onPick(it?.height ?: 0u) })
}

/** "Pick exact formats": the link's formats, one tap to pick, again to unpick. */
@Composable
private fun ExactFormats(model: DownloadModel, formats: List<FormatRecord>, mode: DownloadMode) {
    var open by remember { mutableStateOf(false) }
    Row(
        Modifier
            .fillMaxWidth()
            .clickable { open = !open }
            .padding(vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("Pick exact formats", modifier = Modifier.weight(1f), color = RedlineColors.Muted)
        Icon(if (open) Icons.Rounded.ExpandLess else Icons.Rounded.ExpandMore, null, tint = RedlineColors.Muted)
    }
    if (!open) return
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(
            "Overrides the quality setting for this link only.",
            modifier = Modifier.weight(1f),
            style = MaterialTheme.typography.bodySmall,
            color = RedlineColors.Subtle,
        )
        if (model.pickVideo != null || model.pickAudio != null) {
            TextButton(onClick = {
                model.pickVideo = null
                model.pickAudio = null
            }) { Text("Back to automatic", color = LocalAccent.current.hover) }
        }
    }
    val byQuality = compareByDescending<FormatRecord> { it.height ?: 0u }.thenByDescending { it.tbr ?: 0.0 }
    if (mode == DownloadMode.VIDEO) {
        FormatList("Video", formats.filter { it.hasVideo }.sortedWith(byQuality), model.pickVideo) { model.pickVideo = it }
    }
    FormatList("Audio", formats.filter { it.hasAudio && !it.hasVideo }.sortedWith(byQuality), model.pickAudio) { model.pickAudio = it }
}

@Composable
private fun FormatList(title: String, rows: List<FormatRecord>, picked: String?, onPick: (String?) -> Unit) {
    Eyebrow(title)
    Column {
        for (f in rows) {
            val selected = picked == f.id
            Column(
                Modifier
                    .fillMaxWidth()
                    .background(if (selected) LocalAccent.current.base.copy(alpha = 0.35f) else RedlineColors.Card, MaterialTheme.shapes.small)
                    .clickable { onPick(if (selected) null else f.id) }
                    .padding(horizontal = 8.dp, vertical = 6.dp),
            ) {
                Text(
                    "${f.id}  ${f.qualityLabel}  ${f.ext}",
                    fontFamily = FontFamily.Monospace,
                    style = MaterialTheme.typography.bodySmall,
                    color = RedlineColors.Text,
                )
                val codec = listOfNotNull(f.vcodec, f.acodec).joinToString(" + ") { it.substringBefore('.') }
                Text(
                    "$codec  ${f.size?.let { fmtBytes(it) } ?: "?"}",
                    fontFamily = FontFamily.Monospace,
                    style = MaterialTheme.typography.bodySmall,
                    color = RedlineColors.Muted,
                )
            }
        }
    }
}

/** Languages, available tracks, auto captions and where they go (download.rs `subtitle_options`). */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun SubtitleOptions(
    o: DownloadOptionsRecord,
    info: MediaInfoRecord?,
    defaultLangs: String,
    update: ((DownloadOptionsRecord) -> DownloadOptionsRecord) -> Unit,
) {
    OptionRow("Languages", hint = "Comma separated · \"all\" for every language") {
        InputField(o.subLangs, { v -> update { it.copy(subLangs = v) } }, placeholder = defaultLangs)
    }
    val single = info?.takeIf { !it.isPlaylist }
    if (single != null && single.subtitleLangs.isNotEmpty()) {
        val langs = o.subLangs.split(',').map { it.trim() }.filter { it.isNotEmpty() }
        Text("Available", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Subtle)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            for (lang in single.subtitleLangs.take(24)) {
                val on = lang in langs
                Text(
                    lang,
                    modifier = Modifier
                        .background(if (on) RedlineColors.Text else RedlineColors.Elevated, MaterialTheme.shapes.small)
                        .clickable {
                            val next = if (on) langs - lang else langs + lang
                            update { it.copy(subLangs = next.joinToString(",")) }
                        }
                        .padding(horizontal = 10.dp, vertical = 4.dp),
                    style = MaterialTheme.typography.bodySmall,
                    color = if (on) RedlineColors.Black else RedlineColors.Text,
                )
            }
        }
    } else if (single != null) {
        Text(
            if (single.hasAutoCaptions) "No uploaded subtitles; turn on auto-generated captions below."
            else "This video has no subtitles.",
            style = MaterialTheme.typography.bodySmall,
            color = RedlineColors.Subtle,
        )
    }
    ToggleRow("Include auto-generated captions", o.autoSubs, { v -> update { it.copy(autoSubs = v) } })
    if (o.mode == DownloadMode.VIDEO) {
        OptionRow("Save as") {
            Chips(
                o.subsMode,
                listOf(SubtitleMode.EMBED to "Inside the video", SubtitleMode.SRT to ".srt files"),
                { m -> update { it.copy(subsMode = m) } },
            )
        }
    } else {
        Text("Saved as .srt files next to the audio", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Subtle)
    }
}
