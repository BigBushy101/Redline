package app.redline.ui.batch

import android.content.ClipboardManager
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
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
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.ErrorOutline
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.redline.ui.components.Chips
import app.redline.ui.components.GhostButton
import app.redline.ui.components.InputField
import app.redline.ui.components.PageHeader
import app.redline.ui.components.PagePadding
import app.redline.ui.components.PillButton
import app.redline.ui.components.SectionCard
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import app.redline.util.linksFromText

/** Batch inbox: paste many links, then queue downloads or save them all. */
@Composable
fun BatchScreen(
    model: BatchModel,
    onBack: () -> Unit,
    onOpenQueue: () -> Unit,
    onOpenSaved: () -> Unit,
) {
    BackHandler(onBack = onBack)
    val context = LocalContext.current
    val links = remember(model.input) { linksFromText(model.input) }
    val done = model.items.count { it.state is BatchState.Done || it.state is BatchState.Failed }

    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 32.dp)) {
        item {
            Row(
                Modifier
                    .fillMaxWidth()
                    .background(LocalAccent.current.headerTint)
                    .padding(start = 4.dp, top = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(onClick = onBack) {
                    Icon(Icons.AutoMirrored.Rounded.ArrowBack, contentDescription = "Back to Download")
                }
                Text("Download", style = MaterialTheme.typography.bodyMedium, color = RedlineColors.Muted)
            }
            PageHeader(
                "Batch",
                "Many links at once",
                "Paste one link per line. Redline looks each one up in turn.",
            )
        }
        item { Spacer(Modifier.height(16.dp)) }
        item {
            Column(Modifier.padding(horizontal = PagePadding)) {
                SectionCard(
                    title = "Links",
                    subtitle = if (links.isEmpty()) {
                        "No links yet"
                    } else {
                        "${links.size} link${if (links.size == 1) "" else "s"} found"
                    },
                ) {
                    InputField(
                        value = model.input,
                        onChange = { model.input = it },
                        placeholder = "https://www.youtube.com/watch?v=…\nhttps://soundcloud.com/…",
                        singleLine = false,
                        minLines = 5,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        GhostButton("Paste", {
                            val text = context.getSystemService(ClipboardManager::class.java)
                                ?.primaryClip
                                ?.takeIf { it.itemCount > 0 }
                                ?.getItemAt(0)
                                ?.coerceToText(context)
                                ?.toString()
                            if (!text.isNullOrBlank()) {
                                model.input = if (model.input.isBlank()) text else model.input.trimEnd() + "\n" + text
                            }
                        }, enabled = !model.running)
                        GhostButton("Clear", model::clear, enabled = !model.running)
                    }
                }
            }
        }
        item { Spacer(Modifier.height(16.dp)) }
        item {
            Column(Modifier.padding(horizontal = PagePadding)) {
                SectionCard(title = "Do this with each link") {
                    if (model.running) {
                        val label = when (model.target) {
                            BatchTarget.Download -> "Download"
                            BatchTarget.Save -> "Save to Saved"
                        }
                        Text("Running: $label", color = RedlineColors.Muted)
                    } else {
                        Chips(
                            model.target,
                            listOf(BatchTarget.Download to "Download", BatchTarget.Save to "Save to Saved"),
                            { model.target = it },
                        )
                    }
                }
            }
        }
        item { Spacer(Modifier.height(16.dp)) }
        item {
            Column(Modifier.padding(horizontal = PagePadding), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                model.error?.let { Text(it, color = LocalAccent.current.base) }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    PillButton(
                        if (model.running) "Running…" else "Start",
                        { model.start() },
                        modifier = Modifier.weight(1f),
                        enabled = !model.running && links.isNotEmpty(),
                    )
                    if (model.running) {
                        GhostButton("Stop", model::stop)
                    }
                }
                if (model.items.isNotEmpty()) {
                    Text(
                        "$done of ${model.items.size} done",
                        style = MaterialTheme.typography.bodySmall,
                        color = RedlineColors.Muted,
                    )
                }
            }
        }
        if (model.items.isNotEmpty()) {
            item {
                Text(
                    "Results",
                    modifier = Modifier.padding(start = PagePadding, end = PagePadding, top = 20.dp, bottom = 4.dp),
                    style = MaterialTheme.typography.titleLarge,
                    color = RedlineColors.Text,
                )
            }
            items(model.items, key = { it.url }) { item -> BatchRow(item) }
            item {
                Row(
                    Modifier.padding(horizontal = PagePadding, vertical = 12.dp),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    if (model.items.any { it.state is BatchState.Done }) {
                        when (model.target) {
                            BatchTarget.Download -> TextButton(onClick = { model.open = false; onOpenQueue() }) {
                                Text("View queue", color = RedlineColors.Muted)
                            }
                            BatchTarget.Save -> TextButton(onClick = { model.open = false; onOpenSaved() }) {
                                Text("View Saved", color = RedlineColors.Muted)
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun BatchRow(item: BatchItem) {
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = PagePadding, vertical = 6.dp),
        verticalAlignment = Alignment.Top,
    ) {
        when (item.state) {
            BatchState.Waiting -> Text("•", color = RedlineColors.Subtle, modifier = Modifier.width(22.dp))
            BatchState.Looking -> CircularProgressIndicator(
                modifier = Modifier.size(16.dp),
                color = LocalAccent.current.base,
                strokeWidth = 2.dp,
            )
            is BatchState.Done -> Icon(
                Icons.Rounded.CheckCircle,
                contentDescription = null,
                tint = LocalAccent.current.base,
                modifier = Modifier.size(18.dp),
            )
            is BatchState.Failed -> Icon(
                Icons.Rounded.ErrorOutline,
                contentDescription = null,
                tint = LocalAccent.current.base,
                modifier = Modifier.size(18.dp),
            )
        }
        Spacer(Modifier.width(8.dp))
        Column {
            val label = when (val state = item.state) {
                BatchState.Waiting -> "Waiting"
                BatchState.Looking -> "Looking up…"
                is BatchState.Done -> state.text
                is BatchState.Failed -> "Failed — ${state.text}"
            }
            Text(
                label,
                color = if (item.state is BatchState.Failed) LocalAccent.current.base else RedlineColors.Text,
                style = MaterialTheme.typography.bodyMedium,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                item.url,
                color = RedlineColors.Subtle,
                style = MaterialTheme.typography.bodySmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}
