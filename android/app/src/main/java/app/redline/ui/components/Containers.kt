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
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
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

/** Side padding of every page. */
val PagePadding = 16.dp

/**
 * The desktop's page title block: an uppercase eyebrow, a big bold title and a muted line, over
 * the accent-to-background gradient the main panel starts with.
 */
@Composable
fun PageHeader(eyebrow: String, title: String, subtitle: String, modifier: Modifier = Modifier) {
    val tint = LocalAccent.current.headerTint
    Column(
        modifier
            .fillMaxWidth()
            .background(Brush.verticalGradient(listOf(tint, RedlineColors.Bg)))
            .padding(start = PagePadding, end = PagePadding, top = 28.dp, bottom = 16.dp),
    ) {
        Eyebrow(eyebrow)
        Text(title, style = MaterialTheme.typography.displaySmall, color = RedlineColors.Text)
        Spacer(Modifier.height(4.dp))
        Text(subtitle, style = MaterialTheme.typography.bodyMedium, color = RedlineColors.Muted)
    }
}

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
