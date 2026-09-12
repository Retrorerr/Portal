package app.polarbear.setup.components

// SPIKE-ONLY: static Portal bloom — a faint constant full-silhouette glow
// rendered with Android framework Paint + BlurMaskFilter (OUTER halo +
// NORMAL broad falloff), the same technique family as the Apache-2.0
// StarkDroid/compose-ShadowGlow project. The animated atmospheric layer
// lives in PortalAgslGlow.kt; this layer never animates.

import android.graphics.BlurMaskFilter
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Paint
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlin.math.min

@Composable
fun Modifier.portalBloom(
    glow: Color,
    cornerRadius: Dp,
    intensity: Float,
    tightBlur: Dp = 12.dp,
    broadBlur: Dp = 28.dp,
    tightAlpha: Float = 0.5f,
    broadAlpha: Float = 0.14f,
): Modifier {
    val density = LocalDensity.current
    val tightPx = with(density) { tightBlur.toPx() }
    val broadPx = with(density) { broadBlur.toPx() }
    val cornerPx = with(density) { cornerRadius.toPx() }
    // Framework paints are remembered: radius/geometry are fixed, only the
    // per-frame alpha changes inside drawBehind.
    val tightPaint = remember(glow, tightPx) {
        Paint().apply {
            color = glow
            asFrameworkPaint().apply {
                isAntiAlias = true
                style = android.graphics.Paint.Style.FILL
                maskFilter = BlurMaskFilter(tightPx, BlurMaskFilter.Blur.OUTER)
            }
        }
    }
    val broadPaint = remember(glow, broadPx) {
        Paint().apply {
            color = glow
            asFrameworkPaint().apply {
                isAntiAlias = true
                style = android.graphics.Paint.Style.FILL
                maskFilter = BlurMaskFilter(broadPx, BlurMaskFilter.Blur.NORMAL)
            }
        }
    }
    return this.drawBehind {
        val w = size.width
        val h = size.height
        val r = cornerPx.coerceAtMost(min(w, h) / 2f)
        drawIntoCanvas { canvas ->
            broadPaint.alpha = broadAlpha * intensity
            canvas.drawRoundRect(0f, 0f, w, h, r, r, broadPaint)
            tightPaint.alpha = tightAlpha * intensity
            canvas.drawRoundRect(0f, 0f, w, h, r, r, tightPaint)
        }
    }
}
