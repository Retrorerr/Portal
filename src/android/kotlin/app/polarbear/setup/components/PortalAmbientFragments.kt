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
import kotlin.math.cos
import kotlin.math.sin

private const val SCENE_SCALE = 0.25f
private const val TAU = (PI * 2).toFloat()

private data class Drift(
    val start: Float, val end: Float, val orange: Boolean,
    val extent: Float, val opacity: Float, val angle: Float,
    val x: Float, val y: Float, val ax: Float, val ay: Float,
    val period: Int, val phase: Float,
)

// Five ivory aperture sections and two orange thresholds. The middle three
// trajectories cross the installer; the outer two keep the scene open at its edges.
private val DRIFTS = listOf(
    Drift(0.00f, 0.46f, false, 1.32f, 0.042f, -28f, .08f, .16f, .23f, .25f, 15000, .4f),
    Drift(0.49f, 0.96f, false, 1.18f, 0.032f,  38f, .91f, .75f, .22f, .24f, 14200, 2.1f),
    Drift(0.12f, 0.58f, false, 0.93f, 0.054f, -64f, .48f, .48f, .32f, .28f, 12100, 1.3f),
    Drift(0.43f, 0.83f, false, 0.78f, 0.064f,  74f, .52f, .51f, .29f, .32f, 10900, 3.8f),
    Drift(0.68f, 1.00f, false, 1.04f, 0.037f, 148f, .43f, .57f, .34f, .24f, 13300, 5.2f),
    Drift(0.00f, 1.00f, true,  1.75f, 0.055f, -42f, .62f, .43f, .26f, .30f,  9500, 2.8f),
    Drift(0.00f, 0.76f, true,  1.40f, 0.037f, 112f, .23f, .71f, .27f, .21f, 10400, 5.7f),
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
    val phases = DRIFTS.mapIndexed { index, drift ->
        motion.animateFloat(0f, TAU,
            infiniteRepeatable(tween(drift.period, easing = LinearEasing), RepeatMode.Restart),
            label = "Portal drift $index")
    }
    val source = rememberGraphicsLayer()
    val surrounding = rememberGraphicsLayer()
    val frosted = rememberGraphicsLayer()
    val unit = LocalDensity.current.density * SCENE_SCALE
    val radius = with(LocalDensity.current) { PortalDimens.SurfaceCorner.toPx() } * SCENE_SCALE
    val shader = remember { RuntimeShader(FROST_FIELD) }
    val softBlur = remember(unit) { RenderEffect.createBlurEffect(16f * unit, 16f * unit, Shader.TileMode.CLAMP).asComposeRenderEffect() }
    val strongBlur = remember(unit) { RenderEffect.createBlurEffect(68f * unit, 68f * unit, Shader.TileMode.CLAMP) }
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
                val phase = phases[index].value + drift.phase
                // Integer harmonics close position AND velocity at 2π. Different
                // anchors and asymmetric crossing paths avoid a common orbit or
                // ping-pong reversal. Travel spans substantial screen fractions.
                val x = sceneWidth * (drift.x + drift.ax * (sin(phase) + .20f * sin(2f * phase + .7f)))
                val y = sceneHeight * (drift.y + drift.ay * (sin(2f * phase + 1.1f) + .16f * cos(3f * phase + .4f)))
                val magnification = base * drift.extent / 514f * (1f + .022f * sin(2f * phase + 1.4f))
                translate(x, y) {
                    rotate(drift.angle + 3.5f * sin(phase + 2f), Offset.Zero) {
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
                shader.setFloatUniform("feather", 130f * unit)
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
    // Slightly quieter beneath glass, without increasing card opacity.
    color = mix(half3(backdrop), color, half(1.0 - 0.18 * influence));
    return half4(color * half(influence), half(influence));
}
"""
