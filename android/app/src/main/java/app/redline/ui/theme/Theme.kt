package app.redline.ui.theme

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.redline.R

/** The desktop's palette (src/theme.rs): Spotify-style greys, white text. Dark only, as there. */
object RedlineColors {
    val Black = Color(0xFF000000)
    val Bg = Color(0xFF121212)
    val Card = Color(0xFF181818)
    val CardHover = Color(0xFF282828)
    val Input = Color(0xFF242424)
    val Elevated = Color(0xFF282828)
    val Text = Color(0xFFFFFFFF)
    val Muted = Color(0xFFB3B3B3)
    val Subtle = Color(0xFF6A6A6A)
    val Track = Color(0xFF4D4D4D)
    /** The faint lines between widgets (egui's noninteractive stroke). */
    val Divider = Color(0xFF2A2A2A)
}

/**
 * The highlight set: buttons, toggles, progress, the selected tab and the page header tint.
 * Redline red everywhere for now; the Saved pages will tint it per platform ([fromBase]).
 */
@Immutable
data class Accent(val base: Color, val hover: Color, val pressed: Color, val headerTint: Color) {
    companion object {
        val Redline = Accent(
            base = Color(0xFFE8263A),
            hover = Color(0xFFFF3B4E),
            pressed = Color(0xFFC41A2D),
            headerTint = Color(0xFF6E121D),
        )

        /** theme.rs `Accent::from_base`: lighter hover, darker pressed, header ~45% into the bg. */
        fun fromBase(base: Color) = Accent(
            base = base,
            hover = lerp(base, Color.White, 0.15f),
            pressed = lerp(base, RedlineColors.Black, 0.18f),
            headerTint = lerp(RedlineColors.Bg, base, 0.45f),
        )
    }
}

val LocalAccent = staticCompositionLocalOf { Accent.Redline }

/** Inter, as bundled with the desktop app. */
val Inter = FontFamily(
    Font(R.font.inter_regular, FontWeight.Normal),
    Font(R.font.inter_bold, FontWeight.Bold),
)

private fun colors(accent: Accent) = darkColorScheme(
    primary = accent.base,
    onPrimary = RedlineColors.Text,
    primaryContainer = accent.headerTint,
    onPrimaryContainer = RedlineColors.Text,
    secondary = RedlineColors.Muted,
    onSecondary = RedlineColors.Black,
    // The nav bar's selected-item pill and selected chips.
    secondaryContainer = RedlineColors.Elevated,
    onSecondaryContainer = RedlineColors.Text,
    tertiary = accent.hover,
    background = RedlineColors.Bg,
    onBackground = RedlineColors.Text,
    surface = RedlineColors.Bg,
    onSurface = RedlineColors.Text,
    surfaceVariant = RedlineColors.Input,
    onSurfaceVariant = RedlineColors.Muted,
    surfaceContainerLowest = RedlineColors.Black,
    surfaceContainerLow = RedlineColors.Card,
    surfaceContainer = RedlineColors.Card,
    surfaceContainerHigh = RedlineColors.Elevated,
    surfaceContainerHighest = RedlineColors.Elevated,
    inverseSurface = RedlineColors.Text,
    inverseOnSurface = RedlineColors.Black,
    inversePrimary = accent.pressed,
    outline = RedlineColors.Subtle,
    outlineVariant = RedlineColors.Divider,
    error = accent.hover,
    onError = RedlineColors.Text,
    scrim = RedlineColors.Black,
)

/** egui's text styles: 14 body, 12 small, bold headings (20 card heading, big page titles). */
private val typography = Typography().let { base ->
    fun TextStyle.inter() = copy(fontFamily = Inter)
    Typography(
        displaySmall = base.displaySmall.inter().copy(fontWeight = FontWeight.Bold, fontSize = 34.sp, lineHeight = 40.sp),
        headlineSmall = base.headlineSmall.inter().copy(fontWeight = FontWeight.Bold, fontSize = 24.sp),
        titleLarge = base.titleLarge.inter().copy(fontWeight = FontWeight.Bold, fontSize = 20.sp),
        titleMedium = base.titleMedium.inter().copy(fontWeight = FontWeight.Bold, fontSize = 15.sp),
        titleSmall = base.titleSmall.inter().copy(fontWeight = FontWeight.Bold, fontSize = 14.sp),
        bodyLarge = base.bodyLarge.inter().copy(fontSize = 16.sp),
        bodyMedium = base.bodyMedium.inter().copy(fontSize = 14.sp),
        bodySmall = base.bodySmall.inter().copy(fontSize = 12.sp),
        labelLarge = base.labelLarge.inter().copy(fontWeight = FontWeight.Bold, fontSize = 14.sp),
        labelMedium = base.labelMedium.inter().copy(fontSize = 12.sp),
        // The desktop's uppercase "eyebrow" labels.
        labelSmall = base.labelSmall.inter().copy(fontWeight = FontWeight.Bold, fontSize = 11.sp, letterSpacing = 0.8.sp),
    )
}

/** Cards 8, inputs and rows 6, as on the desktop; pills are fully round where they're drawn. */
private val shapes = Shapes(
    extraSmall = RoundedCornerShape(4.dp),
    small = RoundedCornerShape(6.dp),
    medium = RoundedCornerShape(8.dp),
    large = RoundedCornerShape(12.dp),
    extraLarge = RoundedCornerShape(16.dp),
)

@Composable
fun RedlineTheme(accent: Accent = Accent.Redline, content: @Composable () -> Unit) {
    CompositionLocalProvider(LocalAccent provides accent) {
        MaterialTheme(colorScheme = colors(accent), typography = typography, shapes = shapes, content = content)
    }
}
