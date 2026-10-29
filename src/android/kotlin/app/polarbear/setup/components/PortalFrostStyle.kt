package app.polarbear.setup.components

import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.ColorMatrix

/** The veil and utility glass use the same desktop treatment. */
internal object PortalFrostStyle {
    const val SnapshotDivisor = 6
    const val StrongBlur = 11f
    const val LightBlur = 2.5f
    val vibrancy = ColorFilter.colorMatrix(ColorMatrix().apply { setToSaturation(1.35f) })
    fun tint(isDark: Boolean) = if (isDark) 0.64f else 0.70f
}
