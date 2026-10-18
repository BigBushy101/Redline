package app.redline.ui.convert

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.NoteAdd
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.ErrorOutline
import androidx.compose.material.icons.rounded.Folder
import androidx.compose.material.icons.rounded.Image
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.material.icons.rounded.KeyboardArrowUp
import androidx.compose.material.icons.rounded.Movie
import androidx.compose.material.icons.rounded.MusicNote
import androidx.compose.material.icons.rounded.Tune
import androidx.compose.material.icons.rounded.SwapHoriz
import androidx.compose.material.icons.rounded.Warning
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.redline.core.ConvertChoiceListsRecord
import app.redline.core.ConvertItemRecord
import app.redline.core.ConvertOptionsRecord
import app.redline.core.ConvertPresetRecord
import app.redline.core.EncodersRecord
import app.redline.core.MediaCategory
import app.redline.core.SettingsRecord
import app.redline.core.SubtitleBurn
import app.redline.core.ToolsStatusRecord
import app.redline.core.convertItemMakesVideo
import app.redline.core.convertJoinTarget
import app.redline.core.convertPresetMatches
import app.redline.core.convertSections
import app.redline.core.fmtBytes
import app.redline.core.fmtDuration
import app.redline.core.renameExample
import app.redline.ui.components.ArtTile
import app.redline.ui.components.Chips
import app.redline.ui.components.Dropdown
import app.redline.ui.components.Eyebrow
import app.redline.ui.components.GhostButton
import app.redline.ui.components.InputField
import app.redline.ui.components.OptionRow
import app.redline.ui.components.PageHeader
import app.redline.ui.components.PagePadding
import app.redline.ui.components.PillButton
import app.redline.ui.components.SectionCard
import app.redline.ui.components.ToggleRow
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import coil3.compose.AsyncImage
import java.io.File

/** The kinds of file the picker offers, as the desktop's file dialog filter does. */
private val PICK_TYPES = arrayOf("audio/*", "video/*", "image/*")

/**
 * Port of src/ui/convert.rs: add files, the shared options with presets, the file list with
 * per-file formats and editors, then Convert or Join. Files come from the system picker or
 * another app's Share; the desktop's drop zone has no phone equivalent.
 */
@Composable
fun ConvertScreen(
    model: ConvertModel,
    settings: SettingsRecord,
    lists: ConvertChoiceListsRecord,
    encoders: EncodersRecord,
    tools: ToolsStatusRecord?,
    openSettings: () -> Unit,
) {
    val focus = LocalFocusManager.current
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris -> model.addUris(uris) }
    val items = model.items
    Column(
        Modifier
            .fillMaxWidth()
            .verticalScroll(rememberScrollState()),
    ) {
        PageHeader("Convert", "Change formats", "Turn videos, songs and images into the format you need.")
        Column(Modifier.padding(horizontal = PagePadding), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            val hasAv = items.any { it.category != MediaCategory.IMAGE }
            val noFfmpeg = tools?.readyForConvert == false
            if (noFfmpeg && (hasAv || items.isEmpty())) ToolsBanner(openSettings)

            AddFiles(compact = items.isNotEmpty()) { pick.launch(PICK_TYPES) }
            for (import in model.imports) ImportRow(import)
            if (model.skipped.isNotEmpty()) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(Icons.Rounded.Warning, null, tint = RedlineColors.Muted, modifier = Modifier.size(16.dp))
                    Spacer(Modifier.width(6.dp))
                    Text("Skipped unsupported files: ${model.skipped.joinToString(", ")}", color = RedlineColors.Muted, style = MaterialTheme.typography.bodySmall)
                }
            }
            if (items.isEmpty()) {
                Spacer(Modifier.height(24.dp))
                return@Column
            }

            OptionsCard(model, settings, lists, encoders)
            FileList(model, settings.convert.options, lists, encoders)

            val n = items.size
            val blocked = hasAv && noFfmpeg
            PillButton(
                "Convert $n file${if (n == 1) "" else "s"}",
                onClick = {
                    focus.clearFocus()
                    model.convert()
                },
                icon = Icons.Rounded.SwapHoriz,
                enabled = !blocked && !model.queueing,
                modifier = Modifier.fillMaxWidth(),
            )
            val joinTarget = remember(items) { convertJoinTarget(items) }
            if (joinTarget != null) {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    GhostButton("Join into one ${joinTarget.uppercase()}", { model.join() }, enabled = !blocked && !model.queueing)
                    Text(
                        "Plays the files one after another as a single ${joinTarget.uppercase()}, in list order",
                        style = MaterialTheme.typography.bodySmall,
                        color = RedlineColors.Subtle,
                    )
                }
            }
            GhostButton("Clear all", { model.clear() })
            if (blocked) Text("Converting video and audio needs ffmpeg.", color = RedlineColors.Muted)
            Spacer(Modifier.height(24.dp))
        }
    }
}

/** The tools card's warning (src/ui/mod.rs `tools_banner`). */
@Composable
private fun ToolsBanner(openSettings: () -> Unit) {
    SectionCard {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Rounded.ErrorOutline, null, tint = LocalAccent.current.base)
            Spacer(Modifier.width(10.dp))
            Text(
                "Video and audio conversion needs ffmpeg. Images work without it.",
                modifier = Modifier.weight(1f),
                color = RedlineColors.Text,
            )
        }
        GhostButton("Open Settings", openSettings)
    }
}

/** The drop zone's stand-in: big while the list is empty, a single button after. */
@Composable
private fun AddFiles(compact: Boolean, pick: () -> Unit) {
    if (compact) {
        GhostButton("Add more files", pick)
        return
    }
    SectionCard {
        Column(Modifier.fillMaxWidth().padding(vertical = 12.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Icon(Icons.AutoMirrored.Rounded.NoteAdd, null, tint = RedlineColors.Muted, modifier = Modifier.size(44.dp))
            Text("Pick images, videos or audio", style = MaterialTheme.typography.titleMedium, color = RedlineColors.Text)
            Text("or share them to Redline from another app", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
            Spacer(Modifier.height(6.dp))
            PillButton("Choose files", pick)
        }
    }
}

@Composable
private fun ImportRow(import: Import) {
    val total = import.total
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        val progress = total?.takeIf { it > 0 }?.let { " · ${fmtBytes(import.copied.toULong())} of ${fmtBytes(it.toULong())}" } ?: ""
        Text("Copying ${import.name}$progress", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        val accent = LocalAccent.current
        if (total != null && total > 0) {
            LinearProgressIndicator(
                progress = { (import.copied.toFloat() / total).coerceIn(0f, 1f) },
                modifier = Modifier.fillMaxWidth().height(4.dp),
                color = accent.base,
                trackColor = RedlineColors.Track,
                strokeCap = StrokeCap.Round,
                gapSize = 0.dp,
                drawStopIndicator = {},
            )
        } else {
            LinearProgressIndicator(Modifier.fillMaxWidth().height(4.dp), color = accent.base, trackColor = RedlineColors.Track, strokeCap = StrokeCap.Round, gapSize = 0.dp)
        }
    }
}

/** Presets, the shared options, where files go and how they're named (convert.rs `options_card`). */
@Composable
private fun OptionsCard(model: ConvertModel, settings: SettingsRecord, lists: ConvertChoiceListsRecord, encoders: EncodersRecord) {
    val c = settings.convert
    val o = c.options
    val items = model.items
    SectionCard(title = "Options") {
        PresetRow(c.presets, o, model)
        val customized = items.count { it.options != null }
        if (customized > 0) {
            Text(
                if (customized == 1) "1 file uses its own settings" else "$customized files use their own settings",
                style = MaterialTheme.typography.bodySmall,
                color = RedlineColors.Muted,
            )
        }
        val followers = items.filter { it.options == null }
        val sec = remember(followers, o, encoders) { convertSections(followers, o, encoders) }
        OptionSections(o, sec, lists, model::updateShared)

        HorizontalDivider(color = RedlineColors.Divider)
        OptionRow("Save to") {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Rounded.Folder, null, tint = RedlineColors.Muted, modifier = Modifier.size(18.dp))
                Spacer(Modifier.width(8.dp))
                // Read-only, like the downloads folder: ffmpeg needs the app's own storage.
                Text(c.convertDir ?: "Next to the original", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Text)
            }
        }
        OptionRow("File name", hint = "{name} original name · {n} number in list · {date} today · {target} new format") {
            InputField(c.renameTemplate, model::setTemplate, placeholder = "{name}")
            items.firstOrNull()?.let { first ->
                Text("→ ${renameExample(c.renameTemplate, first)}", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
            }
        }
    }
}

/** The presets row (src/ui/presets.rs): tap to apply, long-press to rename or delete, + to save. */
@OptIn(ExperimentalLayoutApi::class, ExperimentalFoundationApi::class)
@Composable
private fun PresetRow(presets: List<ConvertPresetRecord>, o: ConvertOptionsRecord, model: ConvertModel) {
    var menuFor by remember { mutableStateOf<Int?>(null) }
    var naming by rememberSaveable { mutableStateOf<String?>(null) }
    var renaming by remember { mutableStateOf<Pair<Int, String>?>(null) }
    Eyebrow("Presets")
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        presets.forEachIndexed { i, p ->
            val active = remember(p, o) { convertPresetMatches(p, o) }
            Box {
                Text(
                    p.name,
                    modifier = Modifier
                        .clip(CircleShape)
                        .background(if (active) RedlineColors.Text else RedlineColors.Elevated)
                        .combinedClickable(onClick = { model.applyPreset(p) }, onLongClick = { menuFor = i })
                        .padding(horizontal = 14.dp, vertical = 7.dp),
                    style = MaterialTheme.typography.bodyMedium,
                    color = if (active) RedlineColors.Black else RedlineColors.Text,
                )
                DropdownMenu(expanded = menuFor == i, onDismissRequest = { menuFor = null }, containerColor = RedlineColors.Elevated) {
                    DropdownMenuItem(text = { Text("Rename") }, onClick = {
                        menuFor = null
                        renaming = i to p.name
                    })
                    DropdownMenuItem(text = { Text("Delete") }, onClick = {
                        menuFor = null
                        model.deletePreset(i)
                    })
                }
            }
        }
        Row(
            Modifier
                .clip(CircleShape)
                .combinedClickable(onClick = { naming = "" })
                .padding(horizontal = 10.dp, vertical = 7.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Rounded.Add, null, tint = RedlineColors.Muted, modifier = Modifier.size(16.dp))
            Spacer(Modifier.width(4.dp))
            Text("Save current", style = MaterialTheme.typography.bodyMedium, color = RedlineColors.Muted)
        }
    }
    Text("Tap to apply · hold to rename or delete", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Subtle)
    naming?.let { text ->
        NameDialog("Save preset", text, { naming = it }, onDone = {
            model.savePreset(text)
            naming = null
        }, onCancel = { naming = null })
    }
    renaming?.let { (i, text) ->
        NameDialog("Rename preset", text, { renaming = i to it }, onDone = {
            model.renamePreset(i, text)
            renaming = null
        }, onCancel = { renaming = null })
    }
}

@Composable
private fun NameDialog(title: String, text: String, onText: (String) -> Unit, onDone: () -> Unit, onCancel: () -> Unit) {
    val accent = LocalAccent.current
    AlertDialog(
        onDismissRequest = onCancel,
        containerColor = RedlineColors.Elevated,
        title = { Text(title) },
        text = { InputField(text, onText, placeholder = "Preset name") },
        confirmButton = {
            TextButton(onClick = onDone, enabled = text.isNotBlank()) { Text("Save", color = if (text.isNotBlank()) accent.hover else RedlineColors.Subtle) }
        },
        dismissButton = { TextButton(onClick = onCancel) { Text("Cancel", color = RedlineColors.Muted) } },
    )
}

/** The bulk "set all" controls and one row per file (convert.rs `file_list`). */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun FileList(model: ConvertModel, shared: ConvertOptionsRecord, lists: ConvertChoiceListsRecord, encoders: EncodersRecord) {
    val items = model.items
    var subsFor by remember { mutableStateOf<String?>(null) }
    val pickSubs = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        val path = subsFor
        subsFor = null
        val index = model.items.indexOfFirst { it.path == path }
        if (uri != null && index >= 0) model.pickSubtitles(index, uri)
    }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("Files · ${items.size}", style = MaterialTheme.typography.titleLarge, color = RedlineColors.Text)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            for (cat in listOf(MediaCategory.VIDEO, MediaCategory.AUDIO, MediaCategory.IMAGE)) {
                if (items.none { it.category == cat }) continue
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("All ${label(cat).lowercase()}s →", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
                    Spacer(Modifier.width(6.dp))
                    Dropdown("set…", targets(lists, cat), { it.uppercase() }, { t -> model.setAll(cat, t) }, Modifier.width(96.dp))
                }
            }
        }
        if (items.any { it.category == MediaCategory.VIDEO }) {
            GhostButton("Extract audio (every video to MP3)", { model.extractAudio() })
        }
        items.forEachIndexed { i, item ->
            FileRow(
                index = i,
                item = item,
                count = items.size,
                expanded = item.path in model.expanded,
                model = model,
                lists = lists,
                shared = shared,
                encoders = encoders,
                pickSubtitles = {
                    subsFor = item.path
                    pickSubs.launch(arrayOf("*/*"))
                },
            )
        }
    }
}

private fun label(cat: MediaCategory) = when (cat) {
    MediaCategory.IMAGE -> "Image"
    MediaCategory.VIDEO -> "Video"
    MediaCategory.AUDIO -> "Audio"
}

private fun icon(cat: MediaCategory): ImageVector = when (cat) {
    MediaCategory.IMAGE -> Icons.Rounded.Image
    MediaCategory.VIDEO -> Icons.Rounded.Movie
    MediaCategory.AUDIO -> Icons.Rounded.MusicNote
}

private fun targets(lists: ConvertChoiceListsRecord, cat: MediaCategory) = when (cat) {
    MediaCategory.IMAGE -> lists.imageTargets
    MediaCategory.VIDEO -> lists.videoTargets
    MediaCategory.AUDIO -> lists.audioTargets
}

@Composable
private fun FileRow(
    index: Int,
    item: ConvertItemRecord,
    count: Int,
    expanded: Boolean,
    model: ConvertModel,
    lists: ConvertChoiceListsRecord,
    shared: ConvertOptionsRecord,
    encoders: EncodersRecord,
    pickSubtitles: () -> Unit,
) {
    val accent = LocalAccent.current
    val makesVideo = remember(item) { convertItemMakesVideo(item) }
    Column(
        Modifier
            .fillMaxWidth()
            .background(if (expanded) RedlineColors.CardHover else RedlineColors.Card, MaterialTheme.shapes.small)
            .padding(start = 12.dp, top = 8.dp, bottom = 8.dp, end = 4.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("${index + 1}", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted, modifier = Modifier.width(22.dp))
            if (item.category == MediaCategory.IMAGE) {
                AsyncImage(
                    model = File(item.path),
                    contentDescription = null,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.size(40.dp).clip(MaterialTheme.shapes.extraSmall).background(RedlineColors.Elevated),
                )
            } else {
                ArtTile(icon(item.category), RedlineColors.Muted, size = 40.dp)
            }
            Spacer(Modifier.width(10.dp))
            Column(Modifier.weight(1f)) {
                Text(item.name, style = MaterialTheme.typography.titleSmall, color = RedlineColors.Text, maxLines = 2, overflow = TextOverflow.Ellipsis)
                val meta = buildString {
                    append(label(item.category))
                    append(" · ")
                    append(fmtBytes(item.size))
                    item.durationSecs?.let { append(" · ${fmtDuration(it)}") }
                    if (item.options != null) append(" · custom settings")
                    if (item.burnSubs != null && makesVideo) append(" · subtitles burned in")
                }
                Text(meta, style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
            IconButton(onClick = { model.remove(index) }) {
                Icon(Icons.Rounded.Close, "Remove", tint = RedlineColors.Muted)
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("Convert to", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
            Spacer(Modifier.width(8.dp))
            Dropdown(item.target.uppercase(), targets(lists, item.category), { it.uppercase() }, { t -> model.update(index) { it.copy(target = t) } }, Modifier.weight(1f))
            IconButton(onClick = { model.move(index, -1) }, enabled = index > 0) {
                Icon(Icons.Rounded.KeyboardArrowUp, "Move up", tint = if (index > 0) RedlineColors.Muted else RedlineColors.Divider)
            }
            IconButton(onClick = { model.move(index, 1) }, enabled = index + 1 < count) {
                Icon(Icons.Rounded.KeyboardArrowDown, "Move down", tint = if (index + 1 < count) RedlineColors.Muted else RedlineColors.Divider)
            }
            val customized = item.options != null || item.burnSubs != null
            IconButton(onClick = { model.toggleEditor(item) }) {
                Icon(Icons.Rounded.Tune, "Settings for this file", tint = if (customized) accent.base else RedlineColors.Muted)
            }
        }
        if (expanded) ItemEditor(index, item, makesVideo, model, lists, shared, encoders, pickSubtitles)
    }
}

/** Under a file: its own settings and subtitle burn-in (convert.rs `item_editor`). */
@Composable
private fun ItemEditor(
    index: Int,
    item: ConvertItemRecord,
    makesVideo: Boolean,
    model: ConvertModel,
    lists: ConvertChoiceListsRecord,
    shared: ConvertOptionsRecord,
    encoders: EncodersRecord,
    pickSubtitles: () -> Unit,
) {
    Column(
        Modifier
            .fillMaxWidth()
            .padding(end = 8.dp)
            .background(RedlineColors.Bg, MaterialTheme.shapes.small)
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        val own = item.options
        ToggleRow(
            "Use its own settings",
            own != null,
            { on -> model.setCustom(index, on, shared) },
            hint = if (own != null) "Changes here only affect this file" else "Follows the shared options",
        )
        if (own != null) {
            val sec = remember(item, encoders) { convertSections(listOf(item), own, encoders) }
            OptionSections(own, sec, lists) { change -> model.update(index) { it.copy(options = change(it.options ?: shared)) } }
        }
        if (makesVideo) {
            Eyebrow("Subtitles")
            val mode = when (item.burnSubs) {
                null -> 0
                is SubtitleBurn.Embedded -> 1
                is SubtitleBurn.File -> 2
            }
            OptionRow("Burn in", hint = "Draws the subtitles into the picture, so they show in every player") {
                Chips(mode, listOf(0 to "Off", 1 to "Built-in track", 2 to "From a subtitle file"), { m ->
                    when (m) {
                        0 -> model.update(index) { it.copy(burnSubs = null) }
                        1 -> model.update(index) { it.copy(burnSubs = SubtitleBurn.Embedded) }
                        else -> pickSubtitles()
                    }
                })
                (item.burnSubs as? SubtitleBurn.File)?.let {
                    Text(File(it.path).name, style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted)
                }
            }
        }
    }
}
