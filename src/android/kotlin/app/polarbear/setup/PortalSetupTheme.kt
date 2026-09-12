package app.polarbear.setup

// SPIKE-ONLY (branch compose-setup-spike): Portal CONFIGURE screen tokens.
// Official palette: charcoal #191B1C, ivory #F1EBDD, orange #F07949.
// Dark is the primary target; light stays coherent. Appearance follows the
// Android system theme unless the user picks Dark/Light for preview.

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

enum class AppearanceMode { System, Dark, Light }

enum class InterfaceSize { Compact, Balanced, Large }

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
        textMuted = Color(0xA3A7A29C),
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
        selectionPill = Color(0x40F07949),
        selectionHighlight = Color(0x1EFFFFFF),
        // Charcoal on the light tint: white text would fail contrast here.
        selectionText = Color(0xFF222425),
        optionText = Color(0xFF7C766E),
        accent = Orange,
        accentSoft = Orange,
        buttonInterior = Color(0xFF1E2021),
        buttonOutline = Color(0x99F07949),
        glow = Color(0xFFF07949),
        scrim = Color(0x4D191B1C),
        arcIvory = Color(0x0A191B1C),
        arcOrange = Color(0x16F07949),
        // Exact official mark colours in both modes: no badge, no container.
        logoMain = Ivory,
        logoThreshold = Orange,
        isDark = false,
    )
}

@Composable
fun resolvePalette(mode: AppearanceMode): PortalPalette {
    val systemDark = isSystemInDarkTheme()
    return when (mode) {
        AppearanceMode.Dark -> PortalColors.dark()
        AppearanceMode.Light -> PortalColors.light()
        AppearanceMode.System -> if (systemDark) PortalColors.dark() else PortalColors.light()
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
}
