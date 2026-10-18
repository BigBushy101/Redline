package app.redline.ui.theme

import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Album
import androidx.compose.material.icons.rounded.Cloud
import androidx.compose.material.icons.rounded.Headphones
import androidx.compose.material.icons.rounded.Public
import androidx.compose.material.icons.rounded.SmartDisplay
import androidx.compose.material.icons.rounded.Videocam
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import app.redline.core.Platform
import app.redline.core.platformLabel

/*
 * How each platform looks on its Saved pages (theme.rs `accent_for` and `platform_glyph`).
 * Material icons stand in for the sites' logos: no official artwork is bundled.
 */

/** "YouTube", "SoundCloud"... */
val Platform.label: String get() = platformLabel(this)

/** The platform's brand color, lightened where it wouldn't read on the dark background. */
val Platform.color: Color
    get() = when (this) {
        Platform.YOU_TUBE -> Color(0xFFFF0033)
        Platform.SOUND_CLOUD -> Color(0xFFFF5500)
        Platform.BANDCAMP -> Color(0xFF1DA0C3)
        // Lightened from the brand purple, as on the desktop.
        Platform.MIXCLOUD -> Color(0xFF7B5CFF)
        Platform.VIMEO -> Color(0xFF1AB7EA)
        Platform.OTHER -> Color(0xFF9A9A9A)
    }

/** The accent its pages are painted with ([WithAccent]). */
val Platform.accent: Accent get() = Accent.fromBase(color)

val Platform.icon: ImageVector
    get() = when (this) {
        Platform.YOU_TUBE -> Icons.Rounded.SmartDisplay
        Platform.SOUND_CLOUD -> Icons.Rounded.Cloud
        Platform.BANDCAMP -> Icons.Rounded.Album
        Platform.MIXCLOUD -> Icons.Rounded.Headphones
        Platform.VIMEO -> Icons.Rounded.Videocam
        Platform.OTHER -> Icons.Rounded.Public
    }
