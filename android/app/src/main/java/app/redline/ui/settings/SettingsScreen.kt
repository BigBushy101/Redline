package app.redline.ui.settings

import android.content.Context
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.ErrorOutline
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import app.redline.core.DownloadChoiceListsRecord
import app.redline.core.DownloadMode
import app.redline.core.RedlineException
import app.redline.core.SettingsRecord
import app.redline.core.ToolsStatusRecord
import app.redline.core.isLosslessAudio
import app.redline.state.RedlineState
import app.redline.ui.components.Chips
import app.redline.ui.components.Dropdown
import app.redline.ui.components.GhostButton
import app.redline.ui.components.InputField
import app.redline.ui.components.OptionRow
import app.redline.ui.components.PageHeader
import app.redline.ui.components.PagePadding
import app.redline.ui.components.SectionCard
import app.redline.ui.components.ToggleRow
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

/**
 * Port of the download and convert parts of src/ui/settings.rs. Tool path overrides, the tools
 * installer and browser cookies are desktop-only or for later slices; the folders are read-only.
 */
@Composable
fun SettingsScreen(state: RedlineState, settings: SettingsRecord, tools: ToolsStatusRecord?, lists: DownloadChoiceListsRecord) {
    val update = state::updateSettings
    Column(
        Modifier
            .fillMaxWidth()
            .verticalScroll(rememberScrollState()),
    ) {
        PageHeader("Settings", "Settings", "Tools, folders and performance.")
        Column(Modifier.padding(horizontal = PagePadding), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            ToolsCard(state, tools)

            SectionCard(title = "Folders") {
                OptionRow("Downloads", hint = "Redline's own folder for now; saving to your Download folder comes later.") {
                    Text(settings.downloadDir, style = MaterialTheme.typography.bodySmall, color = RedlineColors.Text)
                }
                OptionRow("Conversions", hint = "Redline's own folder too: ffmpeg can only write there.") {
                    Text(settings.convert.convertDir ?: "Next to the original file", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Text)
                }
            }

            SectionCard(title = "Performance") {
                OptionRow("Simultaneous jobs: ${settings.concurrency}") {
                    val accent = LocalAccent.current
                    Slider(
                        value = settings.concurrency.toFloat(),
                        onValueChange = { v -> update { it.copy(concurrency = Math.round(v).coerceIn(1, 6).toUInt()) } },
                        valueRange = 1f..6f,
                        steps = 4,
                        colors = SliderDefaults.colors(
                            thumbColor = RedlineColors.Text,
                            activeTrackColor = accent.base,
                            inactiveTrackColor = RedlineColors.Track,
                            activeTickColor = RedlineColors.Text.copy(alpha = 0.5f),
                            inactiveTickColor = RedlineColors.Muted.copy(alpha = 0.5f),
                        ),
                    )
                }
                ToggleRow(
                    "Notify me when jobs finish",
                    settings.notify,
                    { v -> update { it.copy(notify = v) } },
                    hint = "Only while Redline is in the background · the phone stays awake while jobs run",
                )
            }

            SectionCard(title = "Downloading") {
                ToggleRow(
                    "Skip links already downloaded",
                    settings.skipDownloaded,
                    { v -> update { it.copy(skipDownloaded = v) } },
                    hint = "Great for playlists: only new items are fetched",
                )
                ToggleRow(
                    "Refresh saved collections on launch",
                    settings.refreshSavedOnStart,
                    { v -> update { it.copy(refreshSavedOnStart = v) } },
                    hint = "Ones not refreshed in the last 12 hours, one at a time in the background",
                )
            }

            DefaultsCard(settings, lists, update)
            NetworkCard(settings, update)

            Text(
                "Redline ${tools?.redlineVersion ?: ""} · yt-dlp ${tools?.ytdlpVersion ?: "not installed"}",
                style = MaterialTheme.typography.bodySmall,
                color = RedlineColors.Subtle,
            )
            Spacer(Modifier.height(24.dp))
        }
    }
}

/** Which tools Redline found, and Update yt-dlp (settings.rs `tools_card`). */
@Composable
private fun ToolsCard(state: RedlineState, tools: ToolsStatusRecord?) {
    val scope = rememberCoroutineScope()
    var updating by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<Result<String>?>(null) }
    SectionCard(title = "Tools", subtitle = "Redline runs these programs to do the actual work. They come with the app.") {
        if (tools == null) {
            Text("Checking…", color = RedlineColors.Muted)
        } else {
            ToolRow("yt-dlp", "Downloads media", tools.ytdlp?.let { p -> tools.ytdlpVersion?.let { "$it · $p" } ?: p })
            ToolRow("Python", "Runs yt-dlp", tools.ytdlpLauncher)
            ToolRow("ffmpeg", "Converts and merges video/audio", tools.ffmpeg)
            ToolRow("ffprobe", "Reads durations for progress", tools.ffprobe)
            ToolRow("QuickJS", "Lets yt-dlp solve YouTube's JavaScript checks", tools.jsRuntime)
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            GhostButton(
                "Update yt-dlp",
                {
                    updating = true
                    message = null
                    scope.launch {
                        message = try {
                            Result.success(state.onEngineSlow { it.updateYtdlp() })
                        } catch (e: CancellationException) {
                            throw e
                        } catch (e: RedlineException) {
                            Result.failure(e)
                        } catch (e: Exception) {
                            Result.failure(e)
                        }
                        updating = false
                        state.refreshTools() // the version changed
                    }
                },
                enabled = tools?.ytdlp != null && !updating,
            )
            if (updating) {
                Spacer(Modifier.width(12.dp))
                CircularProgressIndicator(Modifier.size(20.dp), color = LocalAccent.current.base, strokeWidth = 2.dp)
            }
        }
        message?.let { m ->
            m.fold(
                onSuccess = { Text(it, color = RedlineColors.Muted, style = MaterialTheme.typography.bodySmall) },
                onFailure = { Text(it.message ?: it.toString(), color = LocalAccent.current.hover, style = MaterialTheme.typography.bodySmall) },
            )
        }
    }
}

@Composable
private fun ToolRow(name: String, what: String, path: String?) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Icon(
            if (path != null) Icons.Rounded.CheckCircle else Icons.Rounded.ErrorOutline,
            null,
            tint = if (path != null) RedlineColors.Text else LocalAccent.current.base,
        )
        Spacer(Modifier.width(10.dp))
        Column {
            Text(name, style = MaterialTheme.typography.titleSmall, color = RedlineColors.Text)
            Text(what, style = MaterialTheme.typography.bodySmall, color = RedlineColors.Subtle)
            Text(path ?: "Not found", style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted, maxLines = 2)
        }
    }
}

/** The Download page's remembered choices: what a new link starts with. */
@Composable
private fun DefaultsCard(
    settings: SettingsRecord,
    lists: DownloadChoiceListsRecord,
    update: ((SettingsRecord) -> SettingsRecord) -> Unit,
) {
    val o = settings.download
    fun set(change: (app.redline.core.DownloadOptionsRecord) -> app.redline.core.DownloadOptionsRecord) =
        update { it.copy(download = change(it.download)) }
    SectionCard(title = "Download defaults", subtitle = "What the Download page starts with. It remembers your last choices.") {
        Chips(o.mode, listOf(DownloadMode.VIDEO to "Video", DownloadMode.AUDIO to "Audio only"), { m -> set { it.copy(mode = m) } })
        if (o.mode == DownloadMode.VIDEO) {
            OptionRow("Quality") {
                Dropdown(
                    current = lists.videoQualities.firstOrNull { it.maxHeight == o.maxHeight }?.label ?: "Up to ${o.maxHeight}p",
                    options = lists.videoQualities,
                    label = { it.label },
                    onSelect = { q -> set { it.copy(maxHeight = q.maxHeight) } },
                )
            }
            OptionRow("Container") {
                Chips(o.container, lists.containers.map { it to it }, { c -> set { it.copy(container = c) } })
            }
        } else {
            OptionRow("Format") {
                Chips(o.audioFormat, lists.audioFormats.map { it to it }, { f -> set { it.copy(audioFormat = f) } })
            }
            if (!isLosslessAudio(o.audioFormat)) {
                OptionRow("Bitrate") {
                    Dropdown(
                        current = lists.audioBitrates.firstOrNull { it.value == o.audioBitrate }?.label ?: "Custom",
                        options = lists.audioBitrates,
                        label = { it.label },
                        onSelect = { b -> set { it.copy(audioBitrate = b.value) } },
                    )
                }
            }
        }
        ToggleRow("Embed cover art", o.embedThumbnail, { v -> set { it.copy(embedThumbnail = v) } })
        ToggleRow("Embed metadata", o.embedMetadata, { v -> set { it.copy(embedMetadata = v) } })
        ToggleRow("Subtitles", o.embedSubs, { v -> set { it.copy(embedSubs = v) } })
        ToggleRow("Remove sponsor segments", o.removeSponsors, { v -> set { it.copy(removeSponsors = v) } }, hint = "YouTube only · via SponsorBlock")
    }
}

/** Cookies file, proxy, speed limit, retries (settings.rs `network_card`, minus browser cookies). */
@Composable
private fun NetworkCard(settings: SettingsRecord, update: ((SettingsRecord) -> SettingsRecord) -> Unit) {
    val net = settings.network
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var cookieError by remember { mutableStateOf<String?>(null) }
    // yt-dlp needs a real file path, so a picked cookies.txt is copied into the app's storage.
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        scope.launch {
            val path = runCatching { withContext(Dispatchers.IO) { copyCookies(context, uri) } }
            path.onSuccess { p ->
                cookieError = null
                update { it.copy(network = it.network.copy(cookiesFile = p)) }
            }.onFailure { cookieError = "Couldn't read that file: ${it.message}" }
        }
    }
    SectionCard(title = "Network & sign-in", subtitle = "A cookies.txt from a signed-in browser opens age-restricted, private or members-only videos.") {
        OptionRow("Cookies", hint = cookieError) {
            Text(
                if (net.cookiesFile != null) "cookies.txt file" else "Don't sign in",
                color = RedlineColors.Text,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                GhostButton("Choose cookies.txt…", { pick.launch(arrayOf("text/*", "application/octet-stream")) })
                if (net.cookiesFile != null) {
                    GhostButton("Don't sign in", { update { it.copy(network = it.network.copy(cookiesFile = null)) } })
                }
            }
        }
        OptionRow("Proxy") {
            InputField(net.proxy, { v -> update { it.copy(network = it.network.copy(proxy = v)) } }, placeholder = "none, e.g. socks5://127.0.0.1:1080")
        }
        OptionRow("Speed limit", hint = "e.g. 500K or 2M per second") {
            InputField(net.rateLimit, { v -> update { it.copy(network = it.network.copy(rateLimit = v)) } }, placeholder = "unlimited")
        }
        // Kept as text so the field can be empty while typing; empty means 0.
        var retries by remember { mutableStateOf(if (net.retries == 0u) "" else net.retries.toString()) }
        OptionRow("Retries", hint = "Empty or 0 uses yt-dlp's default (10)") {
            InputField(
                retries,
                { v ->
                    val digits = v.filter { it.isDigit() }.take(2)
                    retries = digits
                    val n = (digits.toIntOrNull() ?: 0).coerceIn(0, 50).toUInt()
                    update { it.copy(network = it.network.copy(retries = n)) }
                },
                placeholder = "0",
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
            )
        }
    }
}

private fun copyCookies(context: Context, uri: Uri): String {
    val out = File(context.filesDir, "cookies.txt")
    val input = context.contentResolver.openInputStream(uri) ?: error("it couldn't be opened")
    input.use { src -> out.outputStream().use { src.copyTo(it) } }
    return out.absolutePath
}
