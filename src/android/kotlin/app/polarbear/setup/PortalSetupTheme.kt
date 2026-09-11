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
    val surfaceFill: Color,
    val surfaceBorder: Color,
    val surfaceShadow: Color,
    val textPrimary: Color,
    val textSecondary: Color,
    val textMuted: Color,
    val trackFill: Color,
    val selectionPill: Color,
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
        surfaceFill = Color(0x14F1EBDD),
        surfaceBorder = Color(0x1FF1EBDD),
        surfaceShadow = Color(0x66000000),
        textPrimary = Ivory,
        textSecondary = Color(0xBFF1EBDD),
        textMuted = Color(0x8AA7A29C),
        trackFill = Color(0x0FF1EBDD),
        selectionPill = Color(0x59C97B57),
        selectionText = Ivory,
        optionText = Color(0x99CFC6B8),
        accent = Orange,
        accentSoft = Color(0xFFF0A37E),
        buttonInterior = Color(0xFF232627),
        buttonOutline = Color(0x80F07949),
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
        surfaceFill = Color(0x8CFFFFFF),
        surfaceBorder = Color(0x14191B1C),
        surfaceShadow = Color(0x1F191B1C),
        textPrimary = Color(0xFF222425),
        textSecondary = Color(0xFF4C4E4E),
        textMuted = Color(0xFF8A8681),
        trackFill = Color(0x0F191B1C),
        selectionPill = Color(0x40D9734F),
        selectionText = Color(0xFFFFFFFF),
        optionText = Color(0xFF7C766E),
        accent = Color(0xFFD96A3C),
        accentSoft = Color(0xFFB85A33),
        buttonInterior = Color(0xFF1E2021),
        buttonOutline = Color(0x99D96A3C),
        glow = Color(0xFFF07949),
        scrim = Color(0x4D191B1C),
        arcIvory = Color(0x0A191B1C),
        arcOrange = Color(0x16D96A3C),
        logoMain = Color(0xFF232627),
        logoThreshold = Color(0xFFD96A3C),
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
    val SurfaceMaxWidth: Dp = 760.dp
    val SurfaceCorner: Dp = 32.dp
    val SurfacePaddingH: Dp = 44.dp
    val SurfacePaddingV: Dp = 40.dp
    val SectionSpacing: Dp = 26.dp
    val LogoSize: Dp = 46.dp
    val TitleSize = 30.sp
    val PickerMaxWidth: Dp = 560.dp
    val BeginMaxWidth: Dp = 310.dp
    val BeginHeight: Dp = 52.dp
}
