package app.polarbear.setup.components

// SPIKE-ONLY: Portal bloom — genuine emitted-light glow shaped like the
// target rounded rect, rendered with Android framework Paint +
// BlurMaskFilter (OUTER halo + NORMAL broad falloff), the same technique as
// the Apache-2.0 StarkDroid/compose-ShadowGlow project, reimplemented here
// in a small Portal-specific form with no extra dependency.
//
// Only the drawn alpha breathes; blur radius, geometry and position never
// animate. The passes draw UNDER the button interior, so the center can
// never become a filled orange shape.

import android.graphics.BlurMaskFilter
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Paint
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

@Composable
fun Modifier.portalBloom(
    glow: androidx.compose.ui.graphics.Color,
    cornerRadius: Dp,
    intensity: Float,
    tightBlur: Dp = 12.dp,
    broadBlur: Dp = 28.dp,
    tightAlpha: Float = 0.55f,
    broadAlpha: Float = 0.16f,
): Modifier {
    val density = LocalDensity.current
    val tightPx = with(density) { tightBlur.toPx() }
    val broadPx = with(density) { broadBlur.toPx() }
    val cornerPx = with(density) { cornerRadius.toPx() }
    // Framework-backed paints are remembered: radius/geometry are fixed,
    // only the per-frame alpha changes inside drawBehind.
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
        drawIntoCanvas { canvas ->
            broadPaint.alpha = broadAlpha * intensity
            canvas.drawRoundRect(
                0f, 0f, size.width, size.height, cornerPx, cornerPx, broadPaint,
            )
            tightPaint.alpha = tightAlpha * intensity
            canvas.drawRoundRect(
                0f, 0f, size.width, size.height, cornerPx, cornerPx, tightPaint,
            )
        }
    }
}
