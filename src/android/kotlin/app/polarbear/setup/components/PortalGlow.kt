package app.polarbear.setup.components

// SPIKE-ONLY: Portal bloom — persistent emitted light shaped like the
// target rounded rect, rendered with Android framework Paint +
// BlurMaskFilter (OUTER halo + NORMAL broad falloff), the same technique as
// the Apache-2.0 StarkDroid/compose-ShadowGlow project, reimplemented here
// in a small Portal-specific form with no extra dependency.
//
// The light has two parts, both drawn UNDER the button interior so the
// center can never become a filled orange shape:
//   - a low, constant diffuse bloom (never animated);
//   - 1-2 broad caustic lobes that slowly travel the rounded perimeter
//     (~7s and ~9s circulation). Each lobe is a heavily blurred radial
//     smear with a fainter trailer, so it stretches and softens as it
//     moves: no dots, no hard edges, no rotating gradient. Overall
//     luminosity stays stable; only the distribution morphs.
// Only the drawn alpha responds to `intensity` (press dip). Blur radius,
// geometry and position are never driven by brightness.

import android.graphics.BlurMaskFilter
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Paint
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.min
import kotlin.math.sin

private const val LOBE_PERIOD_MS_1 = 7000
private const val LOBE_PERIOD_MS_2 = 9000

/**
 * Point travelling clockwise around a rounded-rect perimeter, starting at
 * the top-left corner. [t] wraps at 1.
 */
private fun perimeterPoint(t: Float, w: Float, h: Float, r: Float): Offset {
    val straightW = (w - 2f * r).coerceAtLeast(0f)
    val straightH = (h - 2f * r).coerceAtLeast(0f)
    val arc = (PI.toFloat() / 2f) * r
    val total = 2f * straightW + 2f * straightH + 4f * arc
    if (total <= 0f) {
        return Offset(w / 2f, 0f)
    }
    var d = (((t % 1f) + 1f) % 1f) * total
    // Top edge, left to right.
    if (d < straightW) {
        return Offset(r + d, 0f)
    }
    d -= straightW
    // Top-right corner, -90° to 0°.
    if (d < arc) {
        val a = -PI.toFloat() / 2f + (d / arc) * (PI.toFloat() / 2f)
        return Offset(w - r + r * cos(a), r + r * sin(a))
    }
    d -= arc
    // Right edge, top to bottom.
    if (d < straightH) {
        return Offset(w, r + d)
    }
    d -= straightH
    // Bottom-right corner, 0° to 90°.
    if (d < arc) {
        val a = (d / arc) * (PI.toFloat() / 2f)
        return Offset(w - r + r * cos(a), h - r + r * sin(a))
    }
    d -= arc
    // Bottom edge, right to left.
    if (d < straightW) {
        return Offset(w - r - d, h)
    }
    d -= straightW
    // Bottom-left corner, 90° to 180°.
    if (d < arc) {
        val a = PI.toFloat() / 2f + (d / arc) * (PI.toFloat() / 2f)
        return Offset(r + r * cos(a), h - r + r * sin(a))
    }
    d -= arc
    // Left edge, bottom to top.
    if (d < straightH) {
        return Offset(0f, h - r - d)
    }
    d -= straightH
    // Top-left corner, 180° to 270°.
    val a = PI.toFloat() + (d / arc) * (PI.toFloat() / 2f)
    return Offset(r + r * cos(a), r + r * sin(a))
}

private fun DrawScope.drawCausticLobe(
    center: Offset,
    radiusPx: Float,
    glow: Color,
    peakAlpha: Float,
) {
    drawCircle(
        brush = Brush.radialGradient(
            colors = listOf(
                glow.copy(alpha = peakAlpha),
                glow.copy(alpha = 0f),
            ),
            center = center,
            radius = radiusPx,
        ),
        radius = radiusPx,
        center = center,
    )
}

@Composable
fun Modifier.portalBloom(
    glow: Color,
    cornerRadius: Dp,
    intensity: Float,
    tightBlur: Dp = 12.dp,
    broadBlur: Dp = 28.dp,
    tightAlpha: Float = 0.38f,
    broadAlpha: Float = 0.14f,
): Modifier {
    val density = LocalDensity.current
    val tightPx = with(density) { tightBlur.toPx() }
    val broadPx = with(density) { broadBlur.toPx() }
    val cornerPx = with(density) { cornerRadius.toPx() }
    val lobeRadius = with(density) { 44.dp.toPx() }
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
    // Two slow circulation phases. Values are read inside drawBehind so only
    // the glow redraws, not the button subtree.
    val travel = rememberInfiniteTransition(label = "caustic")
    val phase1 = travel.animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = LOBE_PERIOD_MS_1, easing = LinearEasing),
            repeatMode = RepeatMode.Restart,
        ),
        label = "caustic1",
    )
    val phase2 = travel.animateFloat(
        initialValue = 0.45f,
        targetValue = 1.45f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = LOBE_PERIOD_MS_2, easing = LinearEasing),
            repeatMode = RepeatMode.Restart,
        ),
        label = "caustic2",
    )
    return this.drawBehind {
        drawCaustic(
            tightPaint = tightPaint,
            broadPaint = broadPaint,
            tightAlpha = tightAlpha,
            broadAlpha = broadAlpha,
            intensity = intensity,
            cornerPx = cornerPx,
            glow = glow,
            lobeRadiusPx = lobeRadius,
            phase1 = phase1,
            phase2 = phase2,
        )
    }
}

private fun DrawScope.drawCaustic(
    tightPaint: Paint,
    broadPaint: Paint,
    tightAlpha: Float,
    broadAlpha: Float,
    intensity: Float,
    cornerPx: Float,
    glow: Color,
    lobeRadiusPx: Float,
    phase1: State<Float>,
    phase2: State<Float>,
) {
    val w = size.width
    val h = size.height
    val r = cornerPx.coerceAtMost(min(w, h) / 2f)
    drawIntoCanvas { canvas ->
        broadPaint.alpha = broadAlpha * intensity
        canvas.drawRoundRect(0f, 0f, w, h, r, r, broadPaint)
        tightPaint.alpha = tightAlpha * intensity
        canvas.drawRoundRect(0f, 0f, w, h, r, r, tightPaint)
    }
    // Travelling lobes. Radius breathes slowly so each smear stretches and
    // softens as it flows; peak alpha stays constant so luminosity is stable.
    drawLobe(phase1.value, 0f, w, h, r, lobeRadiusPx, glow, intensity)
    drawLobe(phase2.value, 2.1f, w, h, r, lobeRadiusPx, glow, intensity)
}

private fun DrawScope.drawLobe(
    phase: Float,
    wobblePhase: Float,
    w: Float,
    h: Float,
    r: Float,
    baseRadius: Float,
    glow: Color,
    intensity: Float,
) {
    val morph = 1f + 0.22f * sin(2f * PI.toFloat() * (phase * 2f) + wobblePhase)
    val radius = baseRadius * morph
    val head = perimeterPoint(phase, w, h, r)
    drawCausticLobe(head, radius, glow, 0.34f * intensity)
    val trailer = perimeterPoint(phase - 0.045f, w, h, r)
    drawCausticLobe(trailer, radius * 0.8f, glow, 0.20f * intensity)
}
