package app.redline.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material3.IconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors
import app.redline.util.artModel
import coil3.compose.AsyncImage

/** Side padding of every page. */
val PagePadding = 16.dp

/**
 * The desktop's page title block: an uppercase eyebrow, a big bold title and a muted line, over
 * the accent-to-background gradient the main panel starts with.
 */
@Composable
fun PageHeader(eyebrow: String, title: String, subtitle: String, modifier: Modifier = Modifier) {
    val tint = LocalAccent.current.headerTint
    val openSettings = LocalOpenSettings.current
    val openSearch = LocalOpenSearch.current
    Box(
        modifier
            .fillMaxWidth()
            .background(Brush.verticalGradient(listOf(tint, RedlineColors.Bg))),
    ) {
        Column(Modifier.padding(start = PagePadding, end = PagePadding, top = 28.dp, bottom = 16.dp)) {
            Eyebrow(eyebrow)
            Text(title, style = MaterialTheme.typography.displaySmall, color = RedlineColors.Text)
            Spacer(Modifier.height(4.dp))
            Text(subtitle, style = MaterialTheme.typography.bodyMedium, color = RedlineColors.Muted)
        }
        if (openSearch != null) {
            IconButton(
                onClick = openSearch,
                modifier = Modifier
                    .align(Alignment.TopEnd)
                    .padding(top = 4.dp, end = if (openSettings != null) 52.dp else 4.dp),
            ) {
                Icon(Icons.Rounded.Search, contentDescription = "Search", tint = RedlineColors.Text)
            }
        }
        // Settings left the bottom bar for this gear, in the top corner of every page but its own.
        if (openSettings != null) {
            IconButton(onClick = openSettings, modifier = Modifier.align(Alignment.TopEnd).padding(top = 4.dp, end = 4.dp)) {
                Icon(Icons.Rounded.Settings, contentDescription = "Settings", tint = RedlineColors.Text)
            }
        }
    }
}

/** Opens Settings from a page header's gear; null hides the gear (on Settings itself). */
val LocalOpenSettings = compositionLocalOf<(() -> Unit)?> { null }

/** Opens global search from a page header's magnifier. */
val LocalOpenSearch = compositionLocalOf<(() -> Unit)?> { null }

/** Small bold uppercase label (theme.rs `eyebrow`). */
@Composable
fun Eyebrow(text: String, modifier: Modifier = Modifier, color: Color = RedlineColors.Muted) {
    Text(text.uppercase(), modifier = modifier, style = MaterialTheme.typography.labelSmall, color = color)
}

/** A rounded content card with an optional heading and muted line (theme.rs `card`). */
@Composable
fun SectionCard(
    modifier: Modifier = Modifier,
    title: String? = null,
    subtitle: String? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    Surface(
        modifier = modifier.fillMaxWidth(),
        color = RedlineColors.Card,
        shape = MaterialTheme.shapes.medium,
    ) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            if (title != null || subtitle != null) {
                Column {
                    title?.let { Text(it, style = MaterialTheme.typography.titleLarge, color = RedlineColors.Text) }
                    subtitle?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = RedlineColors.Muted) }
                }
            }
            content()
        }
    }
}

/** Square icon tile standing in for artwork (theme.rs `art_tile`). */
@Composable
fun ArtTile(icon: ImageVector, tint: Color, size: Dp = 44.dp) {
    Box(
        Modifier
            .size(size)
            .background(RedlineColors.Elevated, RoundedCornerShape(4.dp)),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(size * 0.5f))
    }
}

/**
 * A track's or playlist's picture (theme.rs `cover`): the image when there is one and it loads,
 * else the [ArtTile] with [icon].
 */
@Composable
fun Cover(thumbnail: String?, icon: ImageVector, size: Dp = 44.dp, tint: Color = RedlineColors.Muted) {
    val model = remember(thumbnail) { artModel(thumbnail) }
    var failed by remember(thumbnail) { mutableStateOf(false) }
    if (model == null || failed) {
        ArtTile(icon, tint, size)
        return
    }
    Box(Modifier.size(size).background(RedlineColors.Elevated, RoundedCornerShape(4.dp))) {
        AsyncImage(
            model = model,
            contentDescription = null,
            contentScale = ContentScale.Crop,
            onError = { failed = true },
            modifier = Modifier.size(size).clip(RoundedCornerShape(4.dp)),
        )
    }
}

/** Big faded icon, heading, line and an optional action, for empty lists. */
@Composable
fun EmptyState(icon: ImageVector, title: String, body: String, action: (@Composable () -> Unit)? = null) {
    Column(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = PagePadding, vertical = 40.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Icon(icon, contentDescription = null, tint = RedlineColors.Subtle, modifier = Modifier.size(64.dp))
        Text(title, style = MaterialTheme.typography.titleLarge, color = RedlineColors.Text, textAlign = TextAlign.Center)
        Text(body, style = MaterialTheme.typography.bodyMedium, color = RedlineColors.Muted, textAlign = TextAlign.Center)
        if (action != null) {
            Spacer(Modifier.height(10.dp))
            action()
        }
    }
}
