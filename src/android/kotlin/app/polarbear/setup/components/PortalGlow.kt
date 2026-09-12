package app.polarbear.setup.components

// SPIKE-ONLY: Portal bloom — persistent emitted light shaped like the
// target rounded rect, rendered with Android framework Paint +
// BlurMaskFilter, the same technique family as the Apache-2.0
// StarkDroid/compose-ShadowGlow project, reimplemented here in a small
// Portal-specific form with no extra dependency.
//
// The light has two parts, both drawn UNDER the button interior so the
// center can never become a filled orange shape:
//   - a low, constant diffuse bloom over the full silhouette (never animated);
//   - ONE flowing highlight region travelling the exact rounded perimeter:
//     the real outline goes through framework PathMeasure, and three nested
//     trail layers (faint/long + diffuse/medium + main/shorter, all sharing
//     the same progress centre) are sampled off it and stroked with heavy
//     blur and round caps. Lengths morph very slowly so the distribution
//     feels alive; peak alphas never move, so overall luminosity is stable.
//     No dots, no orbs, no hard travelling line, no spinner.
// Only the drawn alpha responds to `intensity` (press dip). Blur radii,
// geometry and circulation are never driven by brightness.

import android.graphics.BlurMaskFilter
import android.graphics.PathMeasure
import android.graphics.RectF
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Paint
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlin.math.PI
import kotlin.math.min
import kotlin.math.sin

private const val TRAIL_PERIOD_MS = 8000
private const val MORPH_PERIOD_MS = 12000
private const val SAMPLE_POINTS = 64

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
    val mainStrokePx = with(density) { 9.dp.toPx() }
    val diffuseStrokePx = with(density) { 12.dp.toPx() }
    val faintStrokePx = with(density) { 15.dp.toPx() }
    val mainBlurPx = with(density) { 16.dp.toPx() }
    val diffuseBlurPx = with(density) { 28.dp.toPx() }
    val faintBlurPx = with(density) { 38.dp.toPx() }
    // Static full-silhouette bloom. Remembered: only per-frame alpha moves.
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
    // Trail layer paints: round-cap strokes; each layer's blur stays fixed.
    fun trailPaint(strokePx: Float, blurPx: Float): Paint {
        return Paint().apply {
            color = glow
            asFrameworkPaint().apply {
                isAntiAlias = true
                style = android.graphics.Paint.Style.STROKE
                strokeWidth = strokePx
                strokeCap = android.graphics.Paint.Cap.ROUND
                strokeJoin = android.graphics.Paint.Join.ROUND
                maskFilter = BlurMaskFilter(blurPx, BlurMaskFilter.Blur.NORMAL)
            }
        }
    }
    val mainPaint = remember(glow, mainStrokePx, mainBlurPx) {
        trailPaint(mainStrokePx, mainBlurPx)
    }
    val diffusePaint = remember(glow, diffuseStrokePx, diffuseBlurPx) {
        trailPaint(diffuseStrokePx, diffuseBlurPx)
    }
    val faintPaint = remember(glow, faintStrokePx, faintBlurPx) {
        trailPaint(faintStrokePx, faintBlurPx)
    }
    // Reused path plumbing: framework outline + measure, Compose segment
    // paths, sampling scratch. Rebuilt only when the layout size changes.
    val outline = remember { android.graphics.Path() }
    val measure = remember { PathMeasure() }
    val outlineBounds = remember { RectF(-1f, -1f, -1f, -1f) }
    val segMain = remember { Path() }
    val segDiffuse = remember { Path() }
    val segFaint = remember { Path() }
    val pos = remember { FloatArray(2) }
    val tan = remember { FloatArray(2) }
    // Circulation progress plus a much slower shared morph phase. Values are
    // read inside drawBehind so only the glow redraws, never the button.
    val travel = rememberInfiniteTransition(label = "trail")
    val progress = travel.animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = TRAIL_PERIOD_MS, easing = LinearEasing),
            repeatMode = RepeatMode.Restart,
        ),
        label = "trailProgress",
    )
    val morph = travel.animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = MORPH_PERIOD_MS, easing = LinearEasing),
            repeatMode = RepeatMode.Restart,
        ),
        label = "trailMorph",
    )
    return this.drawBehind {
        drawGlow(
            tightPaint = tightPaint,
            broadPaint = broadPaint,
            tightAlpha = tightAlpha,
            broadAlpha = broadAlpha,
            intensity = intensity,
            cornerPx = cornerPx,
            outline = outline,
            measure = measure,
            outlineBounds = outlineBounds,
            segMain = segMain,
            segDiffuse = segDiffuse,
            segFaint = segFaint,
            mainPaint = mainPaint,
            diffusePaint = diffusePaint,
            faintPaint = faintPaint,
            pos = pos,
            tan = tan,
            progress = progress,
            morph = morph,
        )
    }
}

private fun DrawScope.drawGlow(
    tightPaint: Paint,
    broadPaint: Paint,
    tightAlpha: Float,
    broadAlpha: Float,
    intensity: Float,
    cornerPx: Float,
    outline: android.graphics.Path,
    measure: PathMeasure,
    outlineBounds: RectF,
    segMain: Path,
    segDiffuse: Path,
    segFaint: Path,
    mainPaint: Paint,
    diffusePaint: Paint,
    faintPaint: Paint,
    pos: FloatArray,
    tan: FloatArray,
    progress: State<Float>,
    morph: State<Float>,
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
    if (w <= 0f || h <= 0f) {
        return
    }
    if (outlineBounds.left != 0f || outlineBounds.top != 0f ||
        outlineBounds.right != w || outlineBounds.bottom != h
    ) {
        outline.rewind()
        outline.addRoundRect(RectF(0f, 0f, w, h), r, r, android.graphics.Path.Direction.CW)
        measure.setPath(outline, false)
        outlineBounds.set(0f, 0f, w, h)
    }
    val total = measure.length
    if (total <= 0f) {
        return
    }
    // One shared centre; only the lengths morph (same slow phase), so the
    // layers can never separate into independent moving objects.
    val swell = sin(2f * PI.toFloat() * morph.value)
    val mainFrac = 0.35f + 0.05f * swell
    val diffuseFrac = 0.54f + 0.06f * swell
    val center = progress.value * total
    drawTrailLayer(segFaint, faintPaint, 0.05f * intensity, center, 0.70f, total, measure, pos, tan)
    drawTrailLayer(segDiffuse, diffusePaint, 0.16f * intensity, center, diffuseFrac, total, measure, pos, tan)
    drawTrailLayer(segMain, mainPaint, 0.45f * intensity, center, mainFrac, total, measure, pos, tan)
}

/**
 * Samples [fraction] of the measured outline centred on [center] into
 * [segment] and strokes it. Wrap-around is continuous, so there is never a
 * seam; round caps plus blur leave no hard ends.
 */
private fun DrawScope.drawTrailLayer(
    segment: Path,
    paint: Paint,
    alpha: Float,
    center: Float,
    fraction: Float,
    total: Float,
    measure: PathMeasure,
    pos: FloatArray,
    tan: FloatArray,
) {
    if (fraction <= 0f || alpha <= 0f) {
        return
    }
    val length = fraction * total
    var start = (center - length / 2f) % total
    if (start < 0f) {
        start += total
    }
    segment.rewind()
    for (i in 0..SAMPLE_POINTS) {
        var d = start + length * i / SAMPLE_POINTS
        if (d >= total) {
            d -= total
        }
        measure.getPosTan(d, pos, tan)
        if (i == 0) {
            segment.moveTo(pos[0], pos[1])
        } else {
            segment.lineTo(pos[0], pos[1])
        }
    }
    paint.alpha = alpha
    drawIntoCanvas { canvas ->
        canvas.drawPath(segment, paint)
    }
}
