package app.polarbear.setup

// SPIKE-ONLY (branch compose-setup-spike): Portal CONFIGURE screen tokens.
// Official palette: charcoal #191B1C, ivory #F1EBDD, orange #F07949.
// Dark is the primary target; light stays coherent. Appearance follows the
// Android system theme unless the user picks Dark/Light for preview.

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import app.polarbear.setup.components.PortalEmphasized
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

@Immutable
data class PortalPalette(
    val background: Color,
    val surfaceTop: Color,
    val surfaceBottom: Color,
    val surfaceBorder: Color,
    val surfaceShadow: Color,
    val textPrimary: Color,
    val textSecondary: Color,
    val textMuted: Color,
    val trackFill: Color,
    val selectionPill: Color,
    val selectionHighlight: Color,
    val selectionText: Color,
    val optionText: Color,
    val accent: Color,
    val accentSoft: Color,
    val buttonInterior: Color,
    val buttonOutline: Color,
    val glow: Color,
    val scrim: Color,
    val arcIvory: Color,
    val arcOrange: Color,
    val logoMain: Color,
    val logoThreshold: Color,
    // Ambient drifting fragments: the aperture pieces take [fragmentInk],
    // thresholds stay orange. Gains lift the tuned dark-mode opacities so the
    // same pieces stay perceptible on a bright ground.
    val fragmentInk: Color,
    val fragmentGain: Float,
    val thresholdGain: Float,
    val pillShadow: Color,
    val isDark: Boolean,
)

object PortalColors {
    val Charcoal = Color(0xFF191B1C)
    val Ivory = Color(0xFFF1EBDD)
    val Orange = Color(0xFFF07949)

    fun dark(): PortalPalette = PortalPalette(
        background = Charcoal,
        surfaceTop = Color(0x16F1EBDD),
        surfaceBottom = Color(0x0BF1EBDD),
        surfaceBorder = Color(0x14F1EBDD),
        surfaceShadow = Color(0x55000000),
        textPrimary = Ivory,
        textSecondary = Color(0xBFF1EBDD),
        textMuted = Color(0xC4BDB7AA),
        trackFill = Color(0x08F1EBDD),
        selectionPill = Color(0xA8F07949),
        selectionHighlight = Color(0x14FFFFFF),
        selectionText = Ivory,
        optionText = Color(0x99CFC6B8),
        accent = Orange,
        accentSoft = Orange,
        buttonInterior = Color(0xFF191B1C),
        buttonOutline = Color(0x66F07949),
        glow = Orange,
        scrim = Color(0x73000000),
        arcIvory = Color(0x0AF1EBDD),
        arcOrange = Color(0x14F07949),
        logoMain = Ivory,
        logoThreshold = Orange,
        fragmentInk = Ivory,
        fragmentGain = 1f,
        thresholdGain = 1f,
        pillShadow = Color(0x40000000),
        isDark = true,
    )

    fun light(): PortalPalette = PortalPalette(
        background = Color(0xFFF4EEE1),
        surfaceTop = Color(0x99FFFFFF),
        surfaceBottom = Color(0x7FFDFBF4),
        surfaceBorder = Color(0x0F191B1C),
        surfaceShadow = Color(0x14191B1C),
        textPrimary = Color(0xFF222425),
        textSecondary = Color(0xFF4C4E4E),
        textMuted = Color(0xFF76716B),
        trackFill = Color(0x0A191B1C),
        // A sheet of paper lifted off the track: the orange tint read as
        // salmon on cream, and charcoal text stays crisp on white.
        selectionPill = Color(0xFFFFFDF8),
        selectionHighlight = Color(0x00FFFFFF),
        selectionText = Color(0xFF222425),
        optionText = Color(0xFF7C766E),
        accent = Orange,
        accentSoft = Orange,
        buttonInterior = Color(0xFF1E2021),
        buttonOutline = Color(0x5CF07949),
        glow = Color(0xFFF07949),
        scrim = Color(0x4D191B1C),
        arcIvory = Color(0x0A191B1C),
        arcOrange = Color(0x16F07949),
        // The official monochrome reading of the mark: an ivory aperture
        // vanishes on cream, so it inks in charcoal with the orange threshold.
        logoMain = Color(0xFF222425),
        logoThreshold = Orange,
        fragmentInk = Charcoal,
        fragmentGain = 1.5f,
        thresholdGain = 2.4f,
        pillShadow = Color(0x24191B1C),
        isDark = false,
    )
}

@Composable
fun resolvePalette(mode: AppearanceMode): PortalPalette {
    val systemDark = isSystemInDarkTheme()
    val target = when (mode) {
        AppearanceMode.Dark -> PortalColors.dark()
        AppearanceMode.Light -> PortalColors.light()
        AppearanceMode.System -> if (systemDark) PortalColors.dark() else PortalColors.light()
    }
    return target.copy(
        background = animateColorAsState(target.background, tween(360, easing = PortalEmphasized), label = "theme background").value,
        surfaceTop = animateColorAsState(target.surfaceTop, tween(360, easing = PortalEmphasized), label = "theme surfaceTop").value,
        surfaceBottom = animateColorAsState(target.surfaceBottom, tween(360, easing = PortalEmphasized), label = "theme surfaceBottom").value,
        surfaceBorder = animateColorAsState(target.surfaceBorder, tween(360, easing = PortalEmphasized), label = "theme surfaceBorder").value,
        surfaceShadow = animateColorAsState(target.surfaceShadow, tween(360, easing = PortalEmphasized), label = "theme surfaceShadow").value,
        textPrimary = animateColorAsState(target.textPrimary, tween(360, easing = PortalEmphasized), label = "theme textPrimary").value,
        textSecondary = animateColorAsState(target.textSecondary, tween(360, easing = PortalEmphasized), label = "theme textSecondary").value,
        textMuted = animateColorAsState(target.textMuted, tween(360, easing = PortalEmphasized), label = "theme textMuted").value,
        trackFill = animateColorAsState(target.trackFill, tween(360, easing = PortalEmphasized), label = "theme trackFill").value,
        selectionPill = animateColorAsState(target.selectionPill, tween(360, easing = PortalEmphasized), label = "theme selectionPill").value,
        selectionHighlight = animateColorAsState(target.selectionHighlight, tween(360, easing = PortalEmphasized), label = "theme selectionHighlight").value,
        selectionText = animateColorAsState(target.selectionText, tween(360, easing = PortalEmphasized), label = "theme selectionText").value,
        optionText = animateColorAsState(target.optionText, tween(360, easing = PortalEmphasized), label = "theme optionText").value,
        accent = animateColorAsState(target.accent, tween(360, easing = PortalEmphasized), label = "theme accent").value,
        accentSoft = animateColorAsState(target.accentSoft, tween(360, easing = PortalEmphasized), label = "theme accentSoft").value,
        buttonInterior = animateColorAsState(target.buttonInterior, tween(360, easing = PortalEmphasized), label = "theme buttonInterior").value,
        buttonOutline = animateColorAsState(target.buttonOutline, tween(360, easing = PortalEmphasized), label = "theme buttonOutline").value,
        glow = animateColorAsState(target.glow, tween(360, easing = PortalEmphasized), label = "theme glow").value,
        scrim = animateColorAsState(target.scrim, tween(360, easing = PortalEmphasized), label = "theme scrim").value,
        arcIvory = animateColorAsState(target.arcIvory, tween(360, easing = PortalEmphasized), label = "theme arcIvory").value,
        arcOrange = animateColorAsState(target.arcOrange, tween(360, easing = PortalEmphasized), label = "theme arcOrange").value,
        logoMain = animateColorAsState(target.logoMain, tween(360, easing = PortalEmphasized), label = "theme logoMain").value,
        logoThreshold = animateColorAsState(target.logoThreshold, tween(360, easing = PortalEmphasized), label = "theme logoThreshold").value,
        fragmentInk = animateColorAsState(target.fragmentInk, tween(360, easing = PortalEmphasized), label = "theme fragmentInk").value,
        pillShadow = animateColorAsState(target.pillShadow, tween(360, easing = PortalEmphasized), label = "theme pillShadow").value,
        fragmentGain = animateFloatAsState(target.fragmentGain, tween(360, easing = PortalEmphasized), label = "theme fragment gain").value,
        thresholdGain = animateFloatAsState(target.thresholdGain, tween(360, easing = PortalEmphasized), label = "theme threshold gain").value,
    )
}

/**
 * The one Compose root theme. Every Portal composition (setup/return overlay,
 * recovery, utility glass, game panels and previews) enters through this, so
 * a Material3 control can never fall back to the framework's purple scheme on
 * one screen while the rest of Portal is charcoal and orange. Text keeps its
 * pre-theme line-height behaviour: screens size their own type explicitly.
 */
@Composable
internal fun PortalTheme(
    mode: AppearanceMode = AppearanceMode.System,
    content: @Composable () -> Unit,
) {
    val palette = resolvePalette(mode)
    val colors = if (palette.isDark) {
        darkColorScheme(
            primary = palette.accent,
            onPrimary = palette.background,
            secondary = palette.accent,
            onSecondary = palette.background,
            background = palette.background,
            onBackground = palette.textPrimary,
            surface = palette.background,
            onSurface = palette.textPrimary,
            surfaceVariant = palette.background,
            onSurfaceVariant = palette.textMuted,
            outline = palette.surfaceBorder,
            error = Color(0xFFFF6B5E),
            onError = palette.background,
            scrim = Color.Black,
        )
    } else {
        lightColorScheme(
            primary = palette.accent,
            onPrimary = Color.White,
            secondary = palette.accent,
            onSecondary = palette.background,
            background = palette.background,
            onBackground = palette.textPrimary,
            surface = palette.background,
            onSurface = palette.textPrimary,
            surfaceVariant = palette.background,
            onSurfaceVariant = palette.textMuted,
            outline = palette.surfaceBorder,
            error = Color(0xFFB3261E),
            onError = Color.White,
            scrim = palette.background,
        )
    }
    MaterialTheme(colorScheme = colors) {
        CompositionLocalProvider(
            LocalContentColor provides palette.textPrimary,
            LocalTextStyle provides TextStyle.Default,
            content = content,
        )
    }
}

object PortalDimens {
    val SurfaceMaxWidth: Dp = 980.dp
    val SurfaceCorner: Dp = 32.dp
    val SurfacePaddingH: Dp = 34.dp
    val SurfacePaddingV: Dp = 30.dp
    val SectionSpacing: Dp = 18.dp
    val ColumnGutter: Dp = 32.dp
    val TwoColumnBreakpoint: Dp = 600.dp
    val LogoSize: Dp = 84.dp
    val TitleSize = 30.sp
    val PickerMaxWidth: Dp = 560.dp
    val BeginMaxWidth: Dp = 310.dp
    val BeginHeight: Dp = 52.dp

    // The onboarding stage keeps a phone's proportions on every screen: on a
    // tablet the card is this wide, then opens up when the install options
    // take over.
    val PrepareCardWidth: Dp = 480.dp

    // Phones in portrait: below this width the card fills the screen with a
    // small gutter and a tighter inset, and the header shrinks so the title
    // keeps to one line.
    val CompactBreakpoint: Dp = 480.dp
    val CompactScreenGutter: Dp = 12.dp
    val CompactSurfacePaddingH: Dp = 20.dp
    val CompactSurfacePaddingV: Dp = 24.dp
    val CompactLogoSize: Dp = 56.dp
    val CompactTitleSize = 22.sp
}
