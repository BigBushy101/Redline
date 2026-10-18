package app.redline.ui.queue

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.QueueMusic
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Delete
import androidx.compose.material.icons.rounded.Download
import androidx.compose.material.icons.rounded.ErrorOutline
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Schedule
import androidx.compose.material.icons.rounded.SwapHoriz
import androidx.compose.material3.AlertDialog
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
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.redline.core.JobRecord
import app.redline.core.JobState
import app.redline.ui.components.ArtTile
import app.redline.ui.components.GhostButton
import app.redline.ui.components.EmptyState
import app.redline.ui.components.PageHeader
import app.redline.ui.components.PagePadding
import app.redline.ui.components.PillButton
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import app.redline.util.startsAt
import java.io.File

/** What a queue row asks for; the caller runs it on the engine. */
sealed interface QueueAction {
    data class Cancel(val id: ULong) : QueueAction
    data class Retry(val id: ULong) : QueueAction
    data class RetryAnyway(val id: ULong) : QueueAction
    data class Remove(val id: ULong) : QueueAction
    data class Open(val path: String) : QueueAction
    /** Add a finished file to the Convert page. */
    data class Convert(val path: String) : QueueAction
    data object ClearFinished : QueueAction
    data object CancelAll : QueueAction
}

/** Port of src/ui/queue.rs: summary, bulk actions, one row per job (newest first). */
@Composable
fun QueueScreen(jobs: List<JobRecord>, onAction: (QueueAction) -> Unit, findSomething: () -> Unit) {
    val active = jobs.count { it.state.active() }
    val scheduled = jobs.count { it.state == JobState.QUEUED && it.notBefore != null }
    val finished = jobs.size - active
    val summary = buildString {
        append("${active - scheduled} in progress")
        if (scheduled > 0) append(" · $scheduled scheduled")
        append(" · $finished finished")
    }
    if (jobs.isEmpty()) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            PageHeader("Queue", "Your queue", summary)
            EmptyState(
                Icons.AutoMirrored.Rounded.QueueMusic,
                "Your queue is empty",
                "Downloads and conversions you start will show up here.",
            ) { PillButton("Find something to download", findSomething) }
        }
        return
    }
    var errorShown by remember { mutableStateOf<JobRecord?>(null) }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item { PageHeader("Queue", "Your queue", summary) }
        item {
            Row(
                Modifier.padding(horizontal = PagePadding).padding(bottom = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                GhostButton("Clear finished", { onAction(QueueAction.ClearFinished) }, enabled = finished > 0)
                GhostButton("Cancel all", { onAction(QueueAction.CancelAll) }, enabled = active > 0)
            }
        }
        items(jobs, key = { it.id.toLong() }) { job ->
            JobRow(job, onAction, showError = { errorShown = job })
            Spacer(Modifier.height(4.dp))
        }
    }
    errorShown?.let { job ->
        AlertDialog(
            onDismissRequest = { errorShown = null },
            containerColor = RedlineColors.Elevated,
            title = { Text(job.title, maxLines = 2, overflow = TextOverflow.Ellipsis) },
            text = { Text(job.error ?: job.detail, color = RedlineColors.Muted) },
            confirmButton = { TextButton(onClick = { errorShown = null }) { Text("Close", color = RedlineColors.Text) } },
        )
    }
}

private fun JobState.active() = this == JobState.QUEUED || this == JobState.RUNNING || this == JobState.PROCESSING

@Composable
private fun JobRow(job: JobRecord, onAction: (QueueAction) -> Unit, showError: () -> Unit) {
    val accent = LocalAccent.current
    val running = job.state == JobState.RUNNING || job.state == JobState.PROCESSING
    val output = job.outputPath
    // Tapping opens a finished file, or shows a failure's whole error.
    val tap: (() -> Unit)? = when {
        job.state == JobState.DONE && output != null -> { { onAction(QueueAction.Open(output)) } }
        job.state == JobState.FAILED && job.error != null -> showError
        else -> null
    }
    Row(
        Modifier
            .padding(horizontal = PagePadding)
            .fillMaxWidth()
            .background(RedlineColors.Card, MaterialTheme.shapes.small)
            .then(if (tap != null) Modifier.clickable(onClick = tap) else Modifier)
            .padding(start = 12.dp, top = 10.dp, bottom = 10.dp, end = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ArtTile(
            if (job.isDownload) Icons.Rounded.Download else Icons.Rounded.SwapHoriz,
            if (running) accent.base else RedlineColors.Muted,
        )
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(job.title, style = MaterialTheme.typography.titleSmall, color = RedlineColors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(job.subtitle, style = MaterialTheme.typography.bodySmall, color = RedlineColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Status(job)
        }
        Actions(job, onAction)
    }
}

/** The status column: what's happening, a progress line while running (queue.rs `status_cell`). */
@Composable
private fun Status(job: JobRecord) {
    val accent = LocalAccent.current
    @Composable
    fun line(text: String, color: androidx.compose.ui.graphics.Color, icon: ImageVector? = null) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (icon != null) {
                Icon(icon, null, tint = color, modifier = Modifier.size(14.dp))
                Spacer(Modifier.width(4.dp))
            }
            Text(text, style = MaterialTheme.typography.bodySmall, color = color, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
    when (job.state) {
        JobState.QUEUED -> {
            val at = job.notBefore
            if (at != null) line(startsAt(at), RedlineColors.Muted, Icons.Rounded.Schedule) else line("Waiting…", RedlineColors.Muted)
        }
        JobState.RUNNING, JobState.PROCESSING -> {
            line(job.detail, RedlineColors.Text)
            Spacer(Modifier.height(4.dp))
            val p = job.progress
            if (job.state == JobState.RUNNING && p != null) {
                LinearProgressIndicator(
                    progress = { p.coerceIn(0f, 1f) },
                    modifier = Modifier.fillMaxWidth().height(4.dp),
                    color = accent.base,
                    trackColor = RedlineColors.Track,
                    strokeCap = StrokeCap.Round,
                    gapSize = 0.dp,
                    drawStopIndicator = {},
                )
            } else {
                LinearProgressIndicator(
                    modifier = Modifier.fillMaxWidth().height(4.dp),
                    color = accent.base,
                    trackColor = RedlineColors.Track,
                    strokeCap = StrokeCap.Round,
                    gapSize = 0.dp,
                )
            }
        }
        JobState.DONE -> {
            line("Done", RedlineColors.Text, Icons.Rounded.CheckCircle)
            job.outputPath?.let { line(File(it).name, RedlineColors.Muted) }
        }
        JobState.FAILED -> {
            line("Failed", accent.hover, Icons.Rounded.ErrorOutline)
            line(job.error?.lineSequence()?.firstOrNull() ?: job.detail, RedlineColors.Muted)
        }
        JobState.SKIPPED -> line(job.detail, RedlineColors.Muted, Icons.Rounded.CheckCircle)
        JobState.CANCELLED -> line("Cancelled", RedlineColors.Muted)
    }
}

/** Cancel while active; retry / download anyway and remove once ended (queue.rs actions). */
@Composable
private fun Actions(job: JobRecord, onAction: (QueueAction) -> Unit) {
    @Composable
    fun button(icon: ImageVector, label: String, action: QueueAction) {
        IconButton(onClick = { onAction(action) }) {
            Icon(icon, contentDescription = label, tint = RedlineColors.Muted)
        }
    }
    Row {
        when (job.state) {
            JobState.QUEUED, JobState.RUNNING, JobState.PROCESSING -> button(Icons.Rounded.Close, "Cancel", QueueAction.Cancel(job.id))
            JobState.SKIPPED -> {
                button(Icons.Rounded.Refresh, "Download anyway", QueueAction.RetryAnyway(job.id))
                button(Icons.Rounded.Delete, "Remove from list", QueueAction.Remove(job.id))
            }
            JobState.FAILED, JobState.CANCELLED -> {
                button(Icons.Rounded.Refresh, "Retry", QueueAction.Retry(job.id))
                button(Icons.Rounded.Delete, "Remove from list", QueueAction.Remove(job.id))
            }
            JobState.DONE -> {
                job.outputPath?.takeIf { !File(it).isDirectory }?.let { button(Icons.Rounded.SwapHoriz, "Convert", QueueAction.Convert(it)) }
                button(Icons.Rounded.Delete, "Remove from list", QueueAction.Remove(job.id))
            }
        }
    }
}
