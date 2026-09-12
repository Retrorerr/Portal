package app.polarbear.setup.components

// SPIKE-ONLY: thin Portal wrapper driving the vendored upstream
// PulsingBorder shader (../vendor/papershaders). Uniform application mirrors
// upstream PulsingBorderShader.apply() and the shared setGlobalUniforms
// behaviour exactly (same uniform names, same color-array packing, same
// frame-milliseconds time base); only Portal parameter values are applied:
// monochrome Portal orange entries, pill geometry, uniform glow margins and
// a Northern-lights-derived motion character. No Paper Shaders dependency.

import android.graphics.BitmapFactory
import android.graphics.BitmapShader
import android.graphics.RuntimeShader
import android.graphics.Shader
import android.os.Build
import android.util.Log
import androidx.compose.foundation.Canvas
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.ShaderBrush
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import app.polarbear.setup.vendor.papershaders.PulsingBorderAgsl
import app.polarbear.setup.vendor.papershaders.ShaderColor

private const val TAG = "PortalAgslGlow"

// Upstream frame clock: milliseconds advanced by speed, u_time = frame/1000.
private const val SPEED = 0.4f

// Monochrome Portal orange entries (same hue at several alpha levels) keep
// the upstream multi-entry accumulation structure intact.
private val PORTAL_COLORS = listOf("#F07949", "#F07949B3", "#F0794973", "#F0794940")

private fun RuntimeShader.setPortalColorUniform(name: String, hex: String) {
    val c = ShaderColor.parse(hex).components
    setFloatUniform(name, c[0], c[1], c[2], c[3])
}

private fun RuntimeShader.setPortalColorArrayUniform(name: String, hexes: List<String>) {
    val values = FloatArray(5 * 4)
    hexes.take(5).forEachIndexed { index, hex ->
        val c = ShaderColor.parse(hex).components
        values[index * 4] = c[0]
        values[index * 4 + 1] = c[1]
        values[index * 4 + 2] = c[2]
        values[index * 4 + 3] = c[3]
    }
    setFloatUniform(name, values)
}

/**
 * Atmospheric Portal-orange glow around the Begin Install silhouette,
 * rendered by the vendored upstream shader into exactly this layer's bounds.
 * Emits nothing on API < 33 (the static BlurMaskFilter bloom underneath
 * remains) and disables itself silently if the shader fails to compile.
 *
 * Per-frame work is two uniform writes (resolution, time); everything else
 * is set once per the upstream apply() path.
 */
@Composable
fun PortalAgslGlow(
    modifier: Modifier,
    buttonWidth: Dp,
    buttonHeight: Dp,
    margin: Dp,
    glowAlpha: Float,
) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
        return
    }
    val density = LocalDensity.current
    val context = LocalContext.current
    val shader = remember {
        runCatching { RuntimeShader(PulsingBorderAgsl) }
            .onFailure { Log.e(TAG, "Portal glow shader failed to compile", it) }
            .getOrNull()
    } ?: return
    val brush = remember(shader) { ShaderBrush(shader) }
    // Exact upstream ps_noise asset (no drawable-class reference, so both
    // application IDs keep working): BitmapFactory + CLAMP + LINEAR filter.
    val noise = remember(context) {
        val resId = context.resources.getIdentifier("ps_noise", "drawable", context.packageName)
        if (resId == 0) {
            Log.e(TAG, "Portal glow noise asset missing")
            null
        } else {
            runCatching {
                val bitmap = BitmapFactory.decodeResource(
                    context.resources,
                    resId,
                    BitmapFactory.Options().apply {
                        inPreferredConfig = android.graphics.Bitmap.Config.ARGB_8888
                        inScaled = false
                    },
                ) ?: return@runCatching null
                val bitmapShader =
                    BitmapShader(bitmap, Shader.TileMode.CLAMP, Shader.TileMode.CLAMP)
                bitmapShader.setFilterMode(BitmapShader.FILTER_MODE_LINEAR)
                Pair(bitmapShader, bitmap.width.toFloat() to bitmap.height.toFloat())
            }.onFailure {
                Log.e(TAG, "Portal glow noise bind failed", it)
            }.getOrNull()
        }
    } ?: return
    // Static uniforms, mirroring upstream apply() + setGlobalUniforms.
    // u_scale is 1.0 rather than the Northern-lights 1.1 so the shader box
    // aligns exactly with the button rect carved by the margins; roundness
    // 1.0 matches the pill silhouette (Circle-preset precedent).
    remember(shader, density, buttonWidth, buttonHeight, margin) {
        val canvasWPx = with(density) { (buttonWidth + margin * 2).toPx() }
        val canvasHPx = with(density) { (buttonHeight + margin * 2).toPx() }
        val marginXPx = with(density) { margin.toPx() }
        shader.setFloatUniform("u_resolution", canvasWPx, canvasHPx)
        shader.setFloatUniform("u_pixelRatio", density.density)
        shader.setFloatUniform("u_fit", 1f)
        shader.setFloatUniform("u_scale", 1f)
        shader.setFloatUniform("u_rotation", 0f)
        shader.setFloatUniform("u_originX", 0.5f)
        shader.setFloatUniform("u_originY", 0.5f)
        shader.setFloatUniform("u_offsetX", 0f)
        shader.setFloatUniform("u_offsetY", 0f)
        shader.setFloatUniform("u_worldWidth", 0f)
        shader.setFloatUniform("u_worldHeight", 0f)
        shader.setFloatUniform("u_imageAspectRatio", 1f)
        shader.setPortalColorUniform("u_colorBack", "#00000000")
        shader.setPortalColorArrayUniform("u_colors", PORTAL_COLORS)
        shader.setFloatUniform("u_colorsCount", PORTAL_COLORS.size.toFloat())
        shader.setFloatUniform("u_roundness", 1f)
        shader.setFloatUniform("u_thickness", 0.1f)
        val marginX = marginXPx / canvasWPx
        val marginY = marginXPx / canvasHPx
        shader.setFloatUniform("u_marginLeft", marginX)
        shader.setFloatUniform("u_marginRight", marginX)
        shader.setFloatUniform("u_marginTop", marginY)
        shader.setFloatUniform("u_marginBottom", marginY)
        shader.setFloatUniform("u_aspectRatio", 0f)
        shader.setFloatUniform("u_softness", 1f)
        shader.setFloatUniform("u_intensity", 0.1f)
        shader.setFloatUniform("u_bloom", 0.2f)
        shader.setFloatUniform("u_spotSize", 0.7f)
        shader.setFloatUniform("u_spots", 4f)
        shader.setFloatUniform("u_pulse", 0f)
        shader.setFloatUniform("u_smoke", 0.32f)
        shader.setFloatUniform("u_smokeSize", 0.7f)
        shader.setInputBuffer("u_noiseTexture", noise.first)
        shader.setFloatUniform("u_noiseTextureSize", noise.second.first, noise.second.second)
        true
    }
    var frameMs by remember { mutableFloatStateOf(0f) }
    LaunchedEffect(shader) {
        var last = withFrameMillis { it }
        while (true) {
            val now = withFrameMillis { it }
            frameMs += (now - last) * SPEED
            last = now
        }
    }
    Canvas(modifier = modifier) {
        runCatching {
            shader.setFloatUniform("u_resolution", size.width, size.height)
            shader.setFloatUniform("u_time", frameMs * 0.001f)
            drawRect(brush = brush, alpha = glowAlpha)
        }.onFailure {
            Log.e(TAG, "Portal glow draw failed", it)
        }
    }
}
