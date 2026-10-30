package app.redline.ui.download

import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.PlaylistPlay
import androidx.compose.material.icons.rounded.ContentPaste
import androidx.compose.material.icons.rounded.Download
import androidx.compose.material.icons.rounded.ErrorOutline
import androidx.compose.material.icons.rounded.Folder
import androidx.compose.material.icons.rounded.History
import androidx.compose.material.icons.rounded.Link
import androidx.compose.material.icons.rounded.Movie
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TimeInput
import androidx.compose.material3.TimePickerDefaults
import androidx.compose.material3.rememberTimePickerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.redline.core.DownloadChoiceListsRecord
import app.redline.core.HistoryRecord
import app.redline.core.MediaInfoRecord
import app.redline.core.SettingsRecord
import app.redline.core.ToolsStatusRecord
import app.redline.core.fmtDuration
import app.redline.ui.batch.BatchModel
import app.redline.ui.batch.BatchScreen
import app.redline.ui.components.Chips
import app.redline.ui.components.Eyebrow
import app.redline.ui.components.GhostButton
import app.redline.ui.components.InputField
import app.redline.ui.components.PageHeader
import app.redline.ui.components.PagePadding
import app.redline.ui.components.PillButton
import app.redline.ui.components.SectionCard
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import app.redline.util.clockText
import app.redline.util.formatTimestamp
import app.redline.util.nextAt
import app.redline.util.startsAt
import coil3.compose.AsyncImage

/** Port of src/ui/download.rs: link, lookup card, format/edit/tag cards, destination, Download. */
@Composable
fun DownloadScreen(
    model: DownloadModel,
    batch: BatchModel,
    settings: SettingsRecord,
    lists: DownloadChoiceListsRecord,
    tools: ToolsStatusRecord?,
    history: List<HistoryRecord>,
    openSettings: () -> Unit,
    onSave: (String) -> Unit,
    isSaving: (String) -> Boolean,
    onOpenQueue: () -> Unit,
    onOpenSaved: () -> Unit,
) {
    if (batch.open) {
        BatchScreen(
            batch,
            onBack = { batch.open = false },
            onOpenQueue = onOpenQueue,
            onOpenSaved = onOpenSaved,
        )
        return
    }
    val focus = LocalFocusManager.current
    Column(
        Modifier
            .fillMaxWidth()
            .verticalScroll(rememberScrollState()),
    ) {
        PageHeader(
            "Download",
            "Grab anything",
            "Paste a link from YouTube, SoundCloud, Bandcamp, Vimeo and over a thousand other sites.",
        )
        Column(
            Modifier.padding(horizontal = PagePadding),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            if (tools != null && !tools.readyForDownload) {
                ToolsBanner(openSettings)
            }
            UrlBar(model) {
                focus.clearFocus()
                model.lookUp()
            }
            GhostButton(
                if (batch.running) "Batch — running" else "Batch links…",
                { batch.open = true },
                modifier = Modifier.fillMaxWidth(),
            )

            val info = model.info
            when {
                model.fetching -> SectionCard {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        CircularProgressIndicator(Modifier.size(20.dp), color = LocalAccent.current.base, strokeWidth = 2.dp)
                        Spacer(Modifier.width(12.dp))
                        Text("Looking up link…", color = RedlineColors.Muted)
                    }
                }
                model.error != null -> SectionCard {
                    Row {
                        Icon(Icons.Rounded.ErrorOutline, null, tint = LocalAccent.current.base)
                        Spacer(Modifier.width(10.dp))
                        Text(model.error.orEmpty(), color = RedlineColors.Text)
                    }
                }
                info != null -> MediaCard(info, history.firstOrNull { it.url == info.url }?.finishedAt)
            }

            FormatCard(model, settings.download, lists)
            EditCard(model, settings.download)
            if (info != null && !info.isPlaylist && settings.download.mode == app.redline.core.DownloadMode.AUDIO) {
                TagsCard(model)
            }

            SectionCard {
                Text("Save to", style = MaterialTheme.typography.bodyMedium, color = RedlineColors.Muted)
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(Icons.Rounded.Folder, null, tint = RedlineColors.Muted, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(8.dp))
                    // Read-only for now: the app's own folder. Saving to shared storage comes later.
                    Text(settings.downloadDir, style = MaterialTheme.typography.bodySmall, color = RedlineColors.Text)
                }
            }

            ScheduleRow(model)

            val problem = model.trimProblem()
            val ready = tools?.readyForDownload != false && model.url.isNotBlank() && problem == null && !model.queueing
            PillButton(
                "Download",
                onClick = {
                    focus.clearFocus()
                    model.download()
                },
                icon = Icons.Rounded.Download,
                enabled = ready,
                modifier = Modifier.fillMaxWidth(),
            )
            // As on the desktop: once looked up, the link can go to Saved instead (a playlist
            // as a collection that stays in step with the original).
            if (info != null && !model.fetching) {
                val saving = isSaving(info.url)
                GhostButton(
                    when {
                        saving -> "Saving…"
                        info.isPlaylist -> "Save playlist to Saved"
                        else -> "Save to Saved"
                    },
                    { onSave(info.url) },
                    enabled = !saving,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            Spacer(Modifier.height(24.dp))
        }
    }
}

/** The tools card's warning, pointing at Settings (src/ui/mod.rs `tools_banner`). */
@Composable
private fun ToolsBanner(openSettings: () -> Unit) {
    SectionCard {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Rounded.ErrorOutline, null, tint = LocalAccent.current.base)
            Spacer(Modifier.width(10.dp))
            Text(
                "Downloading needs yt-dlp and ffmpeg, and Redline can't find them.",
                modifier = Modifier.weight(1f),
                color = RedlineColors.Text,
            )
        }
        GhostButton("Open Settings", openSettings)
    }
}

/** The round link field with paste and Look up. */
@Composable
private fun UrlBar(model: DownloadModel, lookUp: () -> Unit) {
    val context = LocalContext.current
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        InputField(
            value = model.url,
            onChange = model::onUrlChange,
            placeholder = "Paste a video, song or playlist link",
            leading = Icons.Rounded.Link,
            shapeRound = true,
            trailing = {
                IconButton(onClick = {
                    clipboardText(context)?.let {
                        model.onUrlChange(it.trim())
                        lookUp()
                    }
                }) {
                    Icon(Icons.Rounded.ContentPaste, contentDescription = "Paste", tint = RedlineColors.Muted)
                }
            },
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Search),
            keyboardActions = KeyboardActions(onSearch = { lookUp() }),
        )
        GhostButton("Look up", lookUp, enabled = model.url.isNotBlank() && !model.fetching)
    }
}

private fun clipboardText(context: Context): String? {
    val clip = context.getSystemService(ClipboardManager::class.java)?.primaryClip ?: return null
    if (clip.itemCount == 0) return null
    return clip.getItemAt(0).coerceToText(context)?.toString()?.takeIf { it.isNotBlank() }
}

/** The looked-up link, album-header style: art, kind, big title, meta line. */
@Composable
private fun MediaCard(info: MediaInfoRecord, downloadedAt: Long?) {
    SectionCard {
        val thumb = info.thumbnail
        val shape = MaterialTheme.shapes.small
        val artModifier = Modifier
            .fillMaxWidth()
            .aspectRatio(16f / 9f)
            .clip(shape)
            .background(RedlineColors.Elevated)
        if (thumb != null) {
            AsyncImage(model = thumb, contentDescription = null, modifier = artModifier, contentScale = ContentScale.Crop)
        } else {
            Row(artModifier, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.Center) {
                Icon(
                    if (info.isPlaylist) Icons.AutoMirrored.Rounded.PlaylistPlay else Icons.Rounded.Movie,
                    null,
                    tint = RedlineColors.Muted,
                    modifier = Modifier.size(48.dp),
                )
            }
        }
        val kind = if (info.isPlaylist) "Playlist" else "Video"
        Eyebrow(info.extractor?.let { "$kind · $it" } ?: kind)
        Text(info.title, style = MaterialTheme.typography.headlineSmall, color = RedlineColors.Text)
        val meta = buildList {
            info.uploader?.let { add(it) }
            if (info.isPlaylist) add("${info.entryCount} items")
            info.durationSecs?.let { add(fmtDuration(it)) }
        }
        if (meta.isNotEmpty()) {
            Text(meta.joinToString(" • "), color = RedlineColors.Muted, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
        if (downloadedAt != null) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Rounded.History, null, tint = LocalAccent.current.hover, modifier = Modifier.size(16.dp))
                Spacer(Modifier.width(6.dp))
                Text(
                    "You downloaded this on ${formatTimestamp(downloadedAt)}",
                    style = MaterialTheme.typography.bodySmall,
                    color = LocalAccent.current.hover,
                )
            }
        }
    }
}

/** Start now, or at the next hh:mm (the desktop's schedule row). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ScheduleRow(model: DownloadModel) {
    var picking by remember { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("Start", color = RedlineColors.Muted)
            Spacer(Modifier.width(12.dp))
            Chips(model.later, listOf(false to "Now", true to "Later"), { model.later = it })
            if (model.later) {
                Spacer(Modifier.width(8.dp))
                TextButton(onClick = { picking = true }) {
                    Text("at ${clockText(model.hour, model.minute)}", color = RedlineColors.Text)
                }
            }
        }
        if (model.later) {
            Text(
                "${startsAt(nextAt(model.hour, model.minute))} · Redline keeps running in the background",
                style = MaterialTheme.typography.bodySmall,
                color = RedlineColors.Subtle,
            )
        }
    }
    if (picking) {
        val picker = rememberTimePickerState(initialHour = model.hour, initialMinute = model.minute, is24Hour = true)
        val accent = LocalAccent.current
        AlertDialog(
            onDismissRequest = { picking = false },
            containerColor = RedlineColors.Elevated,
            title = { Text("Start at") },
            text = {
                TimeInput(
                    state = picker,
                    colors = TimePickerDefaults.colors(
                        timeSelectorSelectedContainerColor = accent.headerTint,
                        timeSelectorSelectedContentColor = RedlineColors.Text,
                    ),
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    model.hour = picker.hour
                    model.minute = picker.minute
                    picking = false
                }) { Text("Set", color = accent.hover) }
            },
            dismissButton = {
                TextButton(onClick = { picking = false }) { Text("Cancel", color = RedlineColors.Muted) }
            },
        )
    }
}
