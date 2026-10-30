package app.redline.ui.batch

import android.app.Application
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import app.redline.RedlineApp
import app.redline.core.DownloadChoicesRecord
import app.redline.core.RedlineException
import app.redline.util.linksFromText
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

enum class BatchTarget { Download, Save }

sealed interface BatchState {
    data object Waiting : BatchState
    data object Looking : BatchState
    data class Done(val text: String) : BatchState
    data class Failed(val text: String) : BatchState
}

data class BatchItem(val url: String, val state: BatchState)

/**
 * Batch inbox: looks up a list of links one at a time, then queues each download or saves each
 * link. Kept in a ViewModel so rotation or switching tabs doesn't stop the run.
 */
class BatchModel(app: Application) : AndroidViewModel(app) {
    private val state = (app as RedlineApp).state

    var open by mutableStateOf(false)
    var input by mutableStateOf("")
    var target by mutableStateOf(BatchTarget.Download)
    var items by mutableStateOf<List<BatchItem>>(emptyList())
        private set
    var running by mutableStateOf(false)
        private set
    var error by mutableStateOf<String?>(null)

    private var job: Job? = null

    fun start() {
        if (running) return
        val links = linksFromText(input)
        if (links.isEmpty()) {
            error = "No http(s) links in there."
            return
        }
        val tools = state.tools.value
        val ready = when (target) {
            BatchTarget.Download -> tools?.readyForDownload == true
            BatchTarget.Save -> tools?.ytdlp != null
        }
        if (!ready) {
            error = if (target == BatchTarget.Download) {
                "Downloading needs yt-dlp and ffmpeg. Set them up in Settings."
            } else {
                "Saving needs yt-dlp. Set it up in Settings."
            }
            return
        }
        error = null
        items = links.map { BatchItem(it, BatchState.Waiting) }
        running = true
        job = viewModelScope.launch {
            try {
                for ((index, url) in links.withIndex()) {
                    if (!isActive) break
                    update(index, BatchState.Looking)
                    val result = try {
                        if (target == BatchTarget.Download) downloadOne(url) else saveOne(url)
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: RedlineException) {
                        BatchState.Failed(e.message ?: "Couldn't handle that link.")
                    } catch (e: Exception) {
                        BatchState.Failed(e.message ?: e.toString())
                    }
                    update(index, result)
                }
            } finally {
                running = false
            }
        }
    }

    fun stop() {
        job?.cancel()
        job = null
        running = false
        val index = items.indexOfFirst { it.state == BatchState.Looking }
        if (index >= 0) update(index, BatchState.Failed("Stopped"))
    }

    fun clear() {
        stop()
        input = ""
        items = emptyList()
        error = null
    }

    private fun update(index: Int, next: BatchState) {
        items = items.toMutableList().also { it[index] = it[index].copy(state = next) }
    }

    private suspend fun downloadOne(url: String): BatchState {
        val info = state.onEngineSlow { it.fetchInfo(url) }
        val settings = state.settings.value ?: state.settings.filterNotNull().first()
        val options = settings.download.copy(playlist = info.isPlaylist)
        val choices = DownloadChoicesRecord(
            url = url,
            options = options,
            pickVideo = null,
            pickAudio = null,
            tags = null,
            trim = null,
            notBefore = null,
        )
        state.onEngine { it.queueDownload(choices) }
        state.act { } // refresh the queue now
        return BatchState.Done("Queued ${info.title}")
    }

    private suspend fun saveOne(url: String): BatchState {
        val music = state.music ?: return BatchState.Failed("The player isn't ready yet.")
        val out = music.onMusicSlow { it.saveLink(url) }
        music.act { } // Saved changed
        state.act { } // new offline songs may be queued
        return BatchState.Done(out.message)
    }
}
