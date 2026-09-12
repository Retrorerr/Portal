package app.polarbear.setup.components

import android.graphics.RenderEffect
import android.graphics.RuntimeShader
import android.graphics.Shader
import android.os.Build
import androidx.annotation.RequiresApi
import androidx.compose.animation.core.*
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathMeasure
import androidx.compose.ui.graphics.asComposeRenderEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.layer.drawLayer
import androidx.compose.ui.graphics.rememberGraphicsLayer
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.IntSize
import app.polarbear.setup.APERTURE_PATH
import app.polarbear.setup.THRESHOLD_PATH
import app.polarbear.setup.PortalColors
import app.polarbear.setup.PortalDimens
import kotlin.math.PI
import kotlin.math.ceil
import kotlin.math.hypot
import kotlin.math.sin

private const val SCENE_SCALE = 0.25f
private const val TAU = (PI * 2).toFloat()

private data class Drift(
    val start: Float, val end: Float, val orange: Boolean,
    val extent: Float, val opacity: Float, val angle: Float,
    val seed: Float, val paceSec: Int,
)

// Five ivory aperture sections and two orange thresholds. Fragment shapes,
// opacities and base orientations are unchanged; only how they travel.
// Each piece traverses its own deterministic closed wandering loop (see
// LoopPath): 8 broadly spread waypoints, no card-centred anchors, so
// crossings behind the installer happen naturally. paceSec preserves the
// previous relative speed ratios exactly (faster pieces stay faster by the
// same factor); the global mean is calibrated ~15% below the previous drift.
private val DRIFTS = listOf(
    Drift(0.00f, 0.46f, false, 1.32f, 0.042f, -28f, 0.7f, 23),
    Drift(0.49f, 0.96f, false, 1.18f, 0.032f,  38f, 2.3f, 19),
    Drift(0.12f, 0.58f, false, 0.93f, 0.054f, -64f, 4.1f, 29),
    Drift(0.43f, 0.83f, false, 0.78f, 0.064f,  74f, 1.9f, 25),
    Drift(0.68f, 1.00f, false, 1.04f, 0.037f, 148f, 3.3f, 31),
    Drift(0.00f, 1.00f, true,  1.75f, 0.055f, -42f, 5.6f, 21),
    Drift(0.00f, 0.76f, true,  1.40f, 0.037f, 112f, 0.3f, 27),
)

// Mean translation speed in min-dimension fractions per second.
private const val BASE_SPEED = 0.041f
private const val REF_PACE_SEC = 25f
private const val WAYPOINTS = 8
private const val SAMPLES_PER_SEGMENT = 20

// Deterministic closed wandering loop: a uniform Catmull-Rom spline through
// seeded, broadly spread waypoints (golden-angle steps, so no clustering),
// traversed at uniform arc-length speed. Closed C1 tangents mean direction
// only ever changes through smooth curvature: the piece always drifts
// forward at constant speed — never braking to zero, never reversing back
// down its path, no ping-pong, no heading jumps, no orbit or figure-eight.
// Monotonic progress also means a path is never retraced backwards, and the
// long irregular loops (roughly a minute or more each, all different) do
// not perceptibly repeat during ordinary setup use. Bounded sums of curve
// samples only — no random walk, no accumulated state.
private class LoopPath private constructor(
    val xs: FloatArray,
    val ys: FloatArray,
    val cum: FloatArray,
    val total: Float,
) {
    val count: Int get() = xs.size

    companion object {
        fun forSeed(seed: Float, widthPx: Float, heightPx: Float): LoopPath {
            val wx = FloatArray(WAYPOINTS) { j -> 0.5f + 0.40f * sin(seed + j * 2.3999632f) }
            val wy = FloatArray(WAYPOINTS) { j ->
                0.5f + 0.38f * sin(seed * 1.31f + 1.0f + j * 1.4451326f)
            }
            val n = WAYPOINTS * SAMPLES_PER_SEGMENT
            val xs = FloatArray(n + 1)
            val ys = FloatArray(n + 1)
            val cum = FloatArray(n + 1)
            var k = 0
            for (i in 0 until WAYPOINTS) {
                val p0x = wx[(i + WAYPOINTS - 1) % WAYPOINTS] * widthPx
                val p1x = wx[i] * widthPx
                val p2x = wx[(i + 1) % WAYPOINTS] * widthPx
                val p3x = wx[(i + 2) % WAYPOINTS] * widthPx
                val p0y = wy[(i + WAYPOINTS - 1) % WAYPOINTS] * heightPx
                val p1y = wy[i] * heightPx
                val p2y = wy[(i + 1) % WAYPOINTS] * heightPx
                val p3y = wy[(i + 2) % WAYPOINTS] * heightPx
                for (s in 0 until SAMPLES_PER_SEGMENT) {
                    val t = s.toFloat() / SAMPLES_PER_SEGMENT
                    val t2 = t * t
                    val t3 = t2 * t
                    val x = 0.5f * ((2f * p1x) + (-p0x + p2x) * t +
                        (2f * p0x - 5f * p1x + 4f * p2x - p3x) * t2 +
                        (-p0x + 3f * p1x - 3f * p2x + p3x) * t3)
                    val y = 0.5f * ((2f * p1y) + (-p0y + p2y) * t +
                        (2f * p0y - 5f * p1y + 4f * p2y - p3y) * t2 +
                        (-p0y + 3f * p1y - 3f * p2y + p3y) * t3)
                    xs[k] = x
                    ys[k] = y
                    if (k > 0) cum[k] = cum[k - 1] + hypot(x - xs[k - 1], y - ys[k - 1])
                    k++
                }
            }
            xs[n] = xs[0]
            ys[n] = ys[0]
            cum[n] = cum[n - 1] + hypot(xs[0] - xs[n - 1], ys[0] - ys[n - 1])
            return LoopPath(xs, ys, cum, cum[n])
        }
    }

    /** Position at arc distance s (px, wraps). Dense samples make the linear blend invisible. */
    fun position(s: Float): Offset {
        var dist = s % total
        if (dist < 0f) dist += total
        var j = 0
        while (j < count - 2 && cum[j + 1] < dist) j++
        val span = cum[j + 1] - cum[j]
        val f = if (span > 0f) ((dist - cum[j]) / span).coerceIn(0f, 1f) else 0f
        return Offset(xs[j] + (xs[j + 1] - xs[j]) * f, ys[j] + (ys[j + 1] - ys[j]) * f)
    }
}

/** Two GPU blur passes over one opaque full scene, crossfaded by the
 * measured card's spatial field. The source holds the exact background
 * colour plus the seven fragments, so uniform charcoal stays exactly
 * charcoal under any influence. All motion state is read in draw;
 * CONFIGURE does not recompose with the clock.
 */
@Composable
fun PortalAmbientFragments(background: Color, cardBounds: () -> Rect, scenePx: IntSize) {
    // Modern Pad path. Older devices keep the ordinary backdrop/light without
    // trying to instantiate RuntimeShader (no new dependency or CPU blur fallback).
    if (Build.VERSION.SDK_INT >= 33) AmbientScene(background, cardBounds, scenePx)
}

@RequiresApi(33)
@Composable
private fun AmbientScene(background: Color, cardBounds: () -> Rect, scenePx: IntSize) {
    val paths = remember {
        val aperture = PathParser().parsePathString(APERTURE_PATH).toPath()
        val threshold = PathParser().parsePathString(THRESHOLD_PATH).toPath()
        val measure = PathMeasure()
        DRIFTS.map { drift ->
            measure.setPath(if (drift.orange) threshold else aperture, false)
            Path().also { measure.getSegment(measure.length * drift.start, measure.length * drift.end, it) }
        }
    }
    val centers = remember(paths) { paths.map { it.getBounds().center } }
    val stroke = remember { Stroke(54f) }
    val colors = remember { DRIFTS.map { (if (it.orange) PortalColors.Orange else PortalColors.Ivory).copy(alpha = it.opacity) } }
    val motion = rememberInfiniteTransition(label = "Portal environment")
    // Loops are built in real scene pixels (keyed on size, so rotation
    // rebuilds them) for exact arc-length uniformity. One clock per fragment;
    // the loop duration sets its mean speed, preserving the previous
    // relative ratios through paceSec.
    val sceneW = scenePx.width * SCENE_SCALE
    val sceneH = scenePx.height * SCENE_SCALE
    val minDim = minOf(sceneW, sceneH).coerceAtLeast(1f)
    val loops = remember(scenePx) {
        DRIFTS.map { LoopPath.forSeed(it.seed, sceneW, sceneH) }
    }
    val phases = loops.mapIndexed { index, loop ->
        val meanSpeedPx = BASE_SPEED * REF_PACE_SEC / DRIFTS[index].paceSec * minDim
        val loopMs = ((loop.total / meanSpeedPx) * 1000f).toInt().coerceAtLeast(1000)
        motion.animateFloat(0f, TAU,
            infiniteRepeatable(tween(loopMs, easing = LinearEasing), RepeatMode.Restart),
            label = "Portal drift $index")
    }
    val source = rememberGraphicsLayer()
    val surrounding = rememberGraphicsLayer()
    val frosted = rememberGraphicsLayer()
    val unit = LocalDensity.current.density * SCENE_SCALE
    val radius = with(LocalDensity.current) { PortalDimens.SurfaceCorner.toPx() } * SCENE_SCALE
    // One AGSL crossfade mask shared by the frosted pass. It carries no
    // colour of its own: it only outputs the strong scene scaled by the
    // rounded-card influence, leaving the soft scene underneath to show
    // through everywhere else.
    val shader = remember { RuntimeShader(FROST_FIELD) }
    val softBlur = remember(unit) { RenderEffect.createBlurEffect(16f * unit, 16f * unit, Shader.TileMode.CLAMP).asComposeRenderEffect() }
    val strongBlur = remember(unit) { RenderEffect.createBlurEffect(100f * unit, 100f * unit, Shader.TileMode.CLAMP) }
    // Only layout changes rebuild the mask effect. Moving fragments
    // invalidate the display list, not the compiled shader or its uniform snapshot.
    var lastBounds = remember { Rect.Zero }
    var lastUnit = remember { 0f }
    Canvas(Modifier.fillMaxSize()) {
        val sceneWidth = size.width * SCENE_SCALE
        val sceneHeight = size.height * SCENE_SCALE
        source.record(size = androidx.compose.ui.unit.IntSize(ceil(sceneWidth).toInt(), ceil(sceneHeight).toInt())) {
            // Complete opaque source scene: exact background colour first,
            // then the seven fragments. Both blur passes operate on this
            // same full scene, so a fragment-free region is identical
            // charcoal no matter which pass dominates it.
            drawRect(background)
            val base = minOf(sceneWidth, sceneHeight)
            DRIFTS.forEachIndexed { index, drift ->
                // Uniform forward glide along the loop: constant speed, so
                // the piece can neither stop nor reverse — heading only ever
                // changes through the spline's own smooth curvature.
                val at = loops[index].position(phases[index].value / TAU * loops[index].total)
                val x = at.x
                val y = at.y
                // Extremely subtle breathing, kept from the previous tuning:
                // one slow sway per loop or slower, far too small to read as
                // translational reversal.
                val phase = phases[index].value
                val magnification = base * drift.extent / 514f * (1f + .010f * sin(2f * phase + drift.seed * 2.71828f))
                translate(x, y) {
                    rotate(drift.angle + 1.5f * sin(phase + drift.seed * 1.41421f), Offset.Zero) {
                        scale(magnification, magnification, Offset.Zero) {
                            translate(-centers[index].x, -centers[index].y) {
                                drawPath(paths[index], colors[index], style = stroke)
                            }
                        }
                    }
                }
            }
        }
        // Until the card is measured there is no field to mask with: plain
        // soft surroundings. Once measured, the masked chain below replaces
        // this (and is only rebuilt when the field itself changes).
        surrounding.renderEffect = softBlur
        surrounding.record(size = source.size) { drawLayer(source) }
        scale(1f / SCENE_SCALE, 1f / SCENE_SCALE, Offset.Zero) { drawLayer(surrounding) }

        val bounds = cardBounds()
        if (!bounds.isEmpty) {
            if (bounds != lastBounds || unit != lastUnit) {
                shader.setFloatUniform("card", bounds.left * SCENE_SCALE, bounds.top * SCENE_SCALE,
                    bounds.right * SCENE_SCALE, bounds.bottom * SCENE_SCALE)
                shader.setFloatUniform("corner", radius)
                shader.setFloatUniform("feather", 190f * unit)
                frosted.renderEffect = RenderEffect.createChainEffect(
                    RenderEffect.createRuntimeShaderEffect(shader, "scene"), strongBlur,
                ).asComposeRenderEffect()
                lastBounds = bounds
                lastUnit = unit
            }
            frosted.record(size = source.size) { drawLayer(source) }
            scale(1f / SCENE_SCALE, 1f / SCENE_SCALE, Offset.Zero) { drawLayer(frosted) }
        }
    }
}

// Frost crossfade mask over the strong-blurred full scene. The blurred
// scene is sampled BEFORE masking, so no rectangular clipping seam can
// reveal the two passes. This shader carries no colour of its own: it
// outputs the strong scene premultiplied by the rounded-card influence, so
// compositing over the opaque soft scene yields exactly
// soft * (1 - influence) + strong * influence. Both layers contain the same
// opaque base colour, so uniform charcoal stays exactly charcoal regardless
// of influence — the field cannot tint, darken, brighten, desaturate, or
// otherwise modify the scene. Premultiplied alpha is preserved.
private const val FROST_FIELD = """
uniform shader scene;
uniform float4 card;
uniform float corner;
uniform float feather;
half4 main(float2 p) {
    float2 halfSize = (card.zw - card.xy) * 0.5;
    float r = min(corner, min(halfSize.x, halfSize.y));
    float2 q = abs(p - (card.xy + card.zw) * 0.5) - halfSize + r;
    float distance = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
    float influence = 1.0 - smoothstep(0.0, feather, distance);
    half4 color = scene.eval(p);
    return half4(color.rgb * influence, influence);
}
"""
