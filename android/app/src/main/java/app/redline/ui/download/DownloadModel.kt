package app.redline.ui.download

import android.app.Application
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import app.redline.RedlineApp
import app.redline.core.DownloadChoicesRecord
import app.redline.core.DownloadMode
import app.redline.core.DownloadOptionsRecord
import app.redline.core.MediaInfoRecord
import app.redline.core.RedlineException
import app.redline.core.TagsRecord
import app.redline.core.TrimRecord
import app.redline.core.fmtTime
import app.redline.core.parseDuration
import app.redline.core.trimProblem
import app.redline.util.nextAt
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlin.math.ceil
import kotlin.math.min

/** What the Download page tells the user in a snackbar. */
sealed interface DownloadMessage {
    /** The download of [url] is queued; it's saved to the Library too. */
    data class Queued(val title: String, val url: String) : DownloadMessage
    data class Error(val text: String) : DownloadMessage
}

/**
 * The Download page's state for the current link (the desktop's `DownloadPage` + `LinkChoices`),
 * kept in a ViewModel so a rotation or a tab switch doesn't lose a lookup. The format choices
 * themselves are the remembered settings, as on the desktop: edits go straight to them.
 */
class DownloadModel(app: Application) : AndroidViewModel(app) {
    private val state = (app as RedlineApp).state

    var url by mutableStateOf("")
        private set
    var fetching by mutableStateOf(false)
        private set
    var error by mutableStateOf<String?>(null)
        private set
    var info by mutableStateOf<MediaInfoRecord?>(null)
        private set
    /** Exact format ids from [info]; override the quality for this link only. */
    var pickVideo by mutableStateOf<String?>(null)
    var pickAudio by mutableStateOf<String?>(null)
    /** Starts as the site's tags; written into single audio downloads. */
    var tags by mutableStateOf(TagsRecord("", "", ""))
    val trim = TrimState()
    var later by mutableStateOf(false)
    var hour by mutableStateOf(2)
    var minute by mutableStateOf(0)
    var queueing by mutableStateOf(false)
        private set

    private val _messages = Channel<DownloadMessage>(Channel.BUFFERED)
    val messages = _messages.receiveAsFlow()

    private var lookup: Job? = null

    val isPlaylist: Boolean get() = info?.isPlaylist == true

    /** Typing a new link forgets everything tied to the old one. */
    fun onUrlChange(value: String) {
        url = value
        resetLink()
    }

    /** A link shared from another app: fill it in and look it up right away. */
    fun shared(text: String) {
        val link = Regex("""https?://\S+""").find(text)?.value ?: text.trim()
        if (link.isEmpty() || link == url.trim() && (info != null || fetching)) return
        onUrlChange(link)
        if (state.engine != null) lookUp() else pendingLookup = true
    }

    /** A link shared while the core was still starting gets looked up once it's ready. */
    private var pendingLookup = false

    fun onEngineReady() {
        if (pendingLookup) {
            pendingLookup = false
            lookUp()
        }
    }

    private fun resetLink() {
        lookup?.cancel() // the yt-dlp run finishes on its own; its answer is just dropped
        lookup = null
        fetching = false
        error = null
        info = null
        pickVideo = null
        pickAudio = null
        tags = TagsRecord("", "", "")
        trim.reset(null)
    }

    fun lookUp() {
        val link = url.trim()
        if (link.isEmpty() || state.engine == null) return
        resetLink()
        fetching = true
        lookup = viewModelScope.launch {
            try {
                val found = state.onEngineSlow { it.fetchInfo(link) }
                info = found
                tags = found.tags
                trim.reset(found.durationSecs)
                // Like the desktop: a playlist link turns "whole playlist" on, a single one off.
                updateOptions { it.copy(playlist = found.isPlaylist) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: RedlineException) {
                error = e.message
            } catch (e: Exception) {
                error = e.message ?: e.toString()
            } finally {
                // A cancelled lookup was replaced or reset; the new state isn't ours to touch.
                if (isActive) fetching = false
            }
        }
    }

    fun updateOptions(change: (DownloadOptionsRecord) -> DownloadOptionsRecord) {
        state.updateSettings { it.copy(download = change(it.download)) }
    }

    /** Why the trim can't be used, or null (also null when trimming is off or n/a). */
    fun trimProblem(): String? {
        if (!trim.on || isPlaylist) return null
        return trimProblem(trim.record(), info?.durationSecs)
    }

    fun download() {
        val link = url.trim()
        val options = state.settings.value?.download ?: return
        if (link.isEmpty() || queueing) return
        val current = info
        val audioSingle = current != null && !current.isPlaylist && options.mode == DownloadMode.AUDIO
        val choices = DownloadChoicesRecord(
            url = link,
            options = options,
            pickVideo = pickVideo.takeIf { current != null },
            pickAudio = pickAudio.takeIf { current != null },
            tags = tags.takeIf { audioSingle },
            trim = if (trim.on && !isPlaylist) trim.record() else null,
            notBefore = if (later) nextAt(hour, minute) else null,
        )
        queueing = true
        viewModelScope.launch {
            try {
                state.onEngine { it.queueDownload(choices) }
                state.act { } // refresh the queue now rather than at the next nudge
                _messages.send(DownloadMessage.Queued(current?.title ?: link, link))
                url = ""
                resetLink()
            } catch (e: CancellationException) {
                throw e
            } catch (e: RedlineException) {
                _messages.send(DownloadMessage.Error(e.message ?: "Couldn't queue that."))
            } catch (e: Exception) {
                _messages.send(DownloadMessage.Error(e.message ?: e.toString()))
            } finally {
                queueing = false
            }
        }
    }
}

/** Per-link trim selection, edited with the range slider or the time fields (desktop TrimState). */
class TrimState {
    var on by mutableStateOf(false)
    var precise by mutableStateOf(false)
    var start by mutableStateOf(0.0)
        private set
    /** null = to the end (only when the duration is unknown). */
    var end by mutableStateOf<Double?>(null)
        private set
    var startText by mutableStateOf("")
        private set
    var endText by mutableStateOf("")
        private set
    private var duration: Double? = null

    fun reset(duration: Double?) {
        this.duration = duration
        on = false
        precise = false
        start = 0.0
        end = duration?.let { ceil(it) }
        syncText()
    }

    /** From the slider. */
    fun setRange(from: Double, to: Double) {
        start = from
        end = to
        syncText()
    }

    fun editStart(text: String) {
        startText = text
        parseDuration(text)?.let { start = clamp(it) }
    }

    fun editEnd(text: String) {
        endText = text
        if (text.isBlank() && duration == null) {
            end = null
        } else {
            parseDuration(text)?.let { end = clamp(it) }
        }
    }

    fun record() = TrimRecord(startSecs = start, endSecs = end, precise = precise)

    private fun clamp(v: Double) = duration?.let { min(v, ceil(it)) } ?: v

    private fun syncText() {
        // 0 is spelled out so a reset before the core has loaded doesn't need it.
        startText = if (start == 0.0) "0:00" else fmtTime(start)
        endText = end?.let { fmtTime(it) } ?: ""
    }
}
