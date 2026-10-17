package app.redline

import android.app.Application
import android.os.Bundle
import android.os.Environment
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import com.yausername.ffmpeg.FFmpeg
import com.yausername.youtubedl_android.YoutubeDL
import com.yausername.youtubedl_android.YoutubeDLRequest
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

class MainActivity : ComponentActivity() {
    private val vm: DownloadViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent { RedlineTheme { DownloadScreen(vm) } }
    }
}

data class UiState(
    val ready: Boolean = false,
    val busy: Boolean = false,
    val progress: Float = 0f, // 0..1, or -1 while indeterminate
    val status: String = "Starting...",
    val log: String = "",
)

/**
 * Holds the yt-dlp work so a rotation doesn't kill a running download.
 * Everything here is temporary scaffolding until redline-core (Rust, via UniFFI) owns the logic.
 */
class DownloadViewModel(app: Application) : AndroidViewModel(app) {
    private val _state = MutableStateFlow(UiState())
    val state: StateFlow<UiState> = _state

    private val outDir: File
        get() = File(getApplication<Application>().getExternalFilesDir(Environment.DIRECTORY_MUSIC), "Redline")

    init {
        // init() unpacks python + yt-dlp on first run, which takes seconds: keep it off the main thread.
        viewModelScope.launch {
            runCatchingToLog("Init") {
                withContext(Dispatchers.IO) {
                    YoutubeDL.init(getApplication())
                    FFmpeg.init(getApplication())
                }
                val version = YoutubeDL.version(getApplication()) ?: "unknown"
                log("yt-dlp $version ready. Output: ${outDir.absolutePath}")
                _state.update { it.copy(ready = true, status = "Ready") }
            }
        }
    }

    fun download(url: String) = runJob("Downloading") {
        val dir = outDir.apply { mkdirs() }
        val startedAt = System.currentTimeMillis()
        val request = YoutubeDLRequest(url.trim()).apply {
            addOption("-v")
            addOption("-x")
            addOption("--audio-format", "mp3")
            addOption("--no-playlist")
            addOption("-o", "${dir.absolutePath}/%(title)s.%(ext)s")
        }
        val response = withContext(Dispatchers.IO) {
            YoutubeDL.execute(request, processId = null) { progress, eta, line ->
                // Called from the library's reader thread for every stdout line.
                _state.update {
                    it.copy(progress = progress / 100f, status = "Downloading ${progress.toInt()}%  (eta ${eta}s)")
                }
                if (line.isNotBlank()) log(line)
            }
        }
        if (response.err.isNotBlank()) log(response.err.trim())

        // Debug: logcat truncates long lines, so dump stdout/stderr in chunks.
        val command = response.command.joinToString(" ")
        log("Command: $command")
        logcat("command", command)
        logcat("stdout", response.out)
        logcat("stderr", response.err)
        log("--- last stdout lines ---")
        response.out.trim().lines().takeLast(30).forEach { log(it) }

        // yt-dlp renames the file after conversion, so find what it produced rather than guessing.
        val result = dir.listFiles()
            ?.filter { it.isFile && it.lastModified() >= startedAt - 2_000 }
            ?.maxByOrNull { it.lastModified() }
        if (result != null) {
            log("Saved: ${result.absolutePath} (${humanSize(result.length())})")
        } else {
            log("Finished (exit ${response.exitCode}) but no new file found in ${dir.absolutePath}")
        }
    }

    fun updateYtDlp() = runJob("Updating yt-dlp") {
        val status = withContext(Dispatchers.IO) {
            YoutubeDL.updateYoutubeDL(getApplication(), YoutubeDL.UpdateChannel.STABLE)
        }
        log("Update: $status, now ${YoutubeDL.version(getApplication())}")
    }

    /** Runs one job at a time with the busy flag + progress bar handled in one place. */
    private fun runJob(label: String, block: suspend () -> Unit) {
        if (_state.value.busy || !_state.value.ready) return
        _state.update { it.copy(busy = true, progress = -1f, status = "$label...") }
        viewModelScope.launch {
            val ok = runCatchingToLog(label, block)
            _state.update {
                it.copy(busy = false, progress = if (ok) 1f else 0f, status = if (ok) "Done" else "Failed")
            }
        }
    }

    // A prototype should report failures, not crash: everything (incl. yt-dlp's stderr) lands in the log.
    private suspend fun runCatchingToLog(label: String, block: suspend () -> Unit): Boolean =
        try {
            block(); true
        } catch (e: Exception) {
            log("$label failed: ${e.javaClass.simpleName}: ${e.message}")
            e.cause?.let { log("  caused by: ${it.javaClass.simpleName}: ${it.message}") }
            _state.update { it.copy(status = "$label failed") }
            false
        }

    private fun log(line: String) {
        // Cap the log so a long download's progress lines don't grow the string forever.
        _state.update { it.copy(log = (it.log + line + "\n").takeLast(20_000)) }
    }

    private fun logcat(name: String, text: String) {
        if (text.isEmpty()) { Log.d("Redline", "$name: <empty>"); return }
        text.chunked(3000).forEachIndexed { i, chunk -> Log.d("Redline", "$name[$i]: $chunk") }
    }

    private fun humanSize(bytes: Long): String = when {
        bytes >= 1 shl 20 -> "%.1f MB".format(bytes / 1048576.0)
        bytes >= 1 shl 10 -> "%.1f KB".format(bytes / 1024.0)
        else -> "$bytes B"
    }
}

@Composable
fun DownloadScreen(vm: DownloadViewModel) {
    val state by vm.state.collectAsStateWithLifecycle()
    var url by rememberSaveable { mutableStateOf("") }
    val logScroll = rememberScrollState()

    // Follow the tail of the log as lines arrive.
    LaunchedEffect(state.log.length) { logScroll.scrollTo(logScroll.maxValue) }

    Scaffold { padding ->
        Column(
            modifier = Modifier.fillMaxSize().padding(padding).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text("Redline", style = MaterialTheme.typography.headlineMedium)
            OutlinedTextField(
                value = url,
                onValueChange = { url = it },
                label = { Text("URL") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                modifier = Modifier.fillMaxWidth(),
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(
                    onClick = { vm.download(url) },
                    enabled = state.ready && !state.busy && url.isNotBlank(),
                ) { Text("Download") }
                OutlinedButton(
                    onClick = { vm.updateYtDlp() },
                    enabled = state.ready && !state.busy,
                ) { Text("Update yt-dlp") }
            }
            if (state.progress < 0f) {
                LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
            } else {
                LinearProgressIndicator(progress = { state.progress }, modifier = Modifier.fillMaxWidth())
            }
            Text(state.status, style = MaterialTheme.typography.bodyMedium)
            Surface(
                tonalElevation = 2.dp,
                shape = MaterialTheme.shapes.small,
                modifier = Modifier.fillMaxWidth().weight(1f),
            ) {
                Text(
                    text = state.log,
                    fontFamily = FontFamily.Monospace,
                    fontSize = 11.sp,
                    modifier = Modifier.verticalScroll(logScroll).padding(8.dp),
                )
            }
        }
    }
}

private val RedlineRed = Color(0xFFD32F2F)

@Composable
fun RedlineTheme(content: @Composable () -> Unit) {
    val scheme = if (isSystemInDarkTheme()) {
        darkColorScheme(primary = Color(0xFFFF6659))
    } else {
        lightColorScheme(primary = RedlineRed)
    }
    MaterialTheme(colorScheme = scheme, content = content)
}
