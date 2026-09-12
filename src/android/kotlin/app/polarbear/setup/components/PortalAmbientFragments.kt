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
import app.polarbear.setup.APERTURE_PATH
import app.polarbear.setup.THRESHOLD_PATH
import app.polarbear.setup.PortalColors
import app.polarbear.setup.PortalDimens
import kotlin.math.PI
import kotlin.math.ceil
import kotlin.math.sin

private const val SCENE_SCALE = 0.25f
private const val TAU = (PI * 2).toFloat()

private data class Drift(
    val start: Float, val end: Float, val orange: Boolean,
    val extent: Float, val opacity: Float, val angle: Float,
    val cx: Float, val cy: Float, val ax: Float, val ay: Float,
    val periodASec: Int, val periodBSec: Int, val seed: Float,
)

// Five ivory aperture sections and two orange thresholds. Every piece roams
// a broad overlapping region (no card-centred anchors), so crossings behind
// the installer happen naturally. Motion is quasiperiodic, never a loop:
// each axis sums two very-low-frequency oscillations whose periods are
// pairwise incommensurate (prime seconds), with independent phases derived
// from the per-piece seed. Bounded sums of sines only — no random walk.
private val DRIFTS = listOf(
    Drift(0.00f, 0.46f, false, 1.32f, 0.042f, -28f, .18f, .28f, .24f, .22f, 23, 37, 0.7f),
    Drift(0.49f, 0.96f, false, 1.18f, 0.032f,  38f, .82f, .30f, .22f, .24f, 19, 43, 2.3f),
    Drift(0.12f, 0.58f, false, 0.93f, 0.054f, -64f, .35f, .55f, .26f, .25f, 29, 31, 4.1f),
    Drift(0.43f, 0.83f, false, 0.78f, 0.064f,  74f, .62f, .48f, .25f, .27f, 25, 41, 1.9f),
    Drift(0.68f, 1.00f, false, 1.04f, 0.037f, 148f, .48f, .72f, .27f, .23f, 31, 35, 3.3f),
    Drift(0.00f, 1.00f, true,  1.75f, 0.055f, -42f, .68f, .62f, .24f, .22f, 21, 29, 5.6f),
    Drift(0.00f, 0.76f, true,  1.40f, 0.037f, 112f, .28f, .68f, .23f, .25f, 27, 39, 0.3f),
)

/** Two whole-scene GPU blur passes, blended by the measured card's spatial field.
 * All motion state is read in draw; CONFIGURE does not recompose with the clock.
 */
@Composable
fun PortalAmbientFragments(background: Color, cardBounds: () -> Rect) {
    // Modern Pad path. Older devices keep the ordinary backdrop/light without
    // trying to instantiate RuntimeShader (no new dependency or CPU blur fallback).
    if (Build.VERSION.SDK_INT >= 33) AmbientScene(background, cardBounds)
}

@RequiresApi(33)
@Composable
private fun AmbientScene(background: Color, cardBounds: () -> Rect) {
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
    // Two independent slow clocks per fragment; every period pair is
    // incommensurate, so the combined wander never visibly repeats.
    val phasesA = DRIFTS.mapIndexed { index, drift ->
        motion.animateFloat(0f, TAU,
            infiniteRepeatable(tween(drift.periodASec * 1000, easing = LinearEasing), RepeatMode.Restart),
            label = "Portal drift A $index")
    }
    val phasesB = DRIFTS.mapIndexed { index, drift ->
        motion.animateFloat(0f, TAU,
            infiniteRepeatable(tween(drift.periodBSec * 1000, easing = LinearEasing), RepeatMode.Restart),
            label = "Portal drift B $index")
    }
    val source = rememberGraphicsLayer()
    val surrounding = rememberGraphicsLayer()
    val frosted = rememberGraphicsLayer()
    val unit = LocalDensity.current.density * SCENE_SCALE
    val radius = with(LocalDensity.current) { PortalDimens.SurfaceCorner.toPx() } * SCENE_SCALE
    val shader = remember { RuntimeShader(FROST_FIELD) }
    val softBlur = remember(unit) { RenderEffect.createBlurEffect(16f * unit, 16f * unit, Shader.TileMode.CLAMP).asComposeRenderEffect() }
    val strongBlur = remember(unit) { RenderEffect.createBlurEffect(100f * unit, 100f * unit, Shader.TileMode.CLAMP) }
    // Only layout/palette changes rebuild the mask effect. Moving fragments
    // invalidate the display list, not the compiled shader or its uniform snapshot.
    var lastBounds = remember { Rect.Zero }
    var lastBackground = remember { Color.Unspecified }
    var lastUnit = remember { 0f }
    Canvas(Modifier.fillMaxSize()) {
        val sceneWidth = size.width * SCENE_SCALE
        val sceneHeight = size.height * SCENE_SCALE
        source.record(size = androidx.compose.ui.unit.IntSize(ceil(sceneWidth).toInt(), ceil(sceneHeight).toInt())) {
            // Opaque background makes SRC_OVER of pass B a true interpolation,
            // replacing the recognizable pass rather than stacking two silhouettes.
            drawRect(background)
            val base = minOf(sceneWidth, sceneHeight)
            DRIFTS.forEachIndexed { index, drift ->
                val a = phasesA[index].value
                val b = phasesB[index].value
                val s = drift.seed
                // Aimless bounded wander: per axis, a dominant slow component
                // plus a weaker incommensurate one, cross-coupled between the
                // two clocks with irrationally related seed phases. No single
                // trajectory, no ellipse/figure-eight, no endpoint or
                // reversal, no synchronized direction changes. Weights sum to
                // <= 1 so travel stays inside cx +/- ax, cy +/- ay.
                val x = sceneWidth * (drift.cx + drift.ax *
                    (0.72f * sin(a + s) + 0.28f * sin(b + s * 2.39996f)))
                val y = sceneHeight * (drift.cy + drift.ay *
                    (0.72f * sin(b + s * 1.61803f) + 0.28f * sin(a + s * 3.14159f)))
                // Extremely subtle, independently slow breathing: rotation on
                // clock B, scale on clock A, neither synced with position.
                val magnification = base * drift.extent / 514f * (1f + .010f * sin(a + s * 2.71828f))
                translate(x, y) {
                    rotate(drift.angle + 1.5f * sin(b + s * 1.41421f), Offset.Zero) {
                        scale(magnification, magnification, Offset.Zero) {
                            translate(-centers[index].x, -centers[index].y) {
                                drawPath(paths[index], colors[index], style = stroke)
                            }
                        }
                    }
                }
            }
        }
        surrounding.renderEffect = softBlur
        surrounding.record(size = source.size) { drawLayer(source) }
        scale(1f / SCENE_SCALE, 1f / SCENE_SCALE, Offset.Zero) { drawLayer(surrounding) }

        val bounds = cardBounds()
        if (!bounds.isEmpty) {
            if (bounds != lastBounds || background != lastBackground || unit != lastUnit) {
                shader.setFloatUniform("card", bounds.left * SCENE_SCALE, bounds.top * SCENE_SCALE,
                    bounds.right * SCENE_SCALE, bounds.bottom * SCENE_SCALE)
                shader.setFloatUniform("corner", radius)
                shader.setFloatUniform("feather", 190f * unit)
                shader.setFloatUniform("backdrop", background.red, background.green, background.blue)
                frosted.renderEffect = RenderEffect.createChainEffect(
                    RenderEffect.createRuntimeShaderEffect(shader, "scene"), strongBlur,
                ).asComposeRenderEffect()
                lastBounds = bounds
                lastBackground = background
                lastUnit = unit
            }
            frosted.record(size = source.size) { drawLayer(source) }
            scale(1f / SCENE_SCALE, 1f / SCENE_SCALE, Offset.Zero) { drawLayer(frosted) }
        }
    }
}

// Original Portal AGSL: rounded-card signed distance, broad exterior feather.
// The complete blurred scene is sampled BEFORE masking, so no rectangular
// clipping seam can reveal the two passes. Premultiplied alpha is preserved.
private const val FROST_FIELD = """
uniform shader scene;
uniform float4 card;
uniform float corner;
uniform float feather;
uniform float3 backdrop;
half4 main(float2 p) {
    float2 halfSize = (card.zw - card.xy) * 0.5;
    float r = min(corner, min(halfSize.x, halfSize.y));
    float2 q = abs(p - (card.xy + card.zw) * 0.5) - halfSize + r;
    float distance = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
    float influence = 1.0 - smoothstep(0.0, feather, distance);
    half3 color = scene.eval(p).rgb;
    // Dissolve beneath glass: fragments become broad indistinct light and
    // shadow, without increasing card opacity. The wide feather above keeps
    // the transition boundary-free.
    color = mix(half3(backdrop), color, half(1.0 - 0.50 * influence));
    return half4(color * half(influence), half(influence));
}
"""
