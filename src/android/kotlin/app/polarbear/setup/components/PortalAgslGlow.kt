package app.polarbear.setup.components

// SPIKE-ONLY: Portal atmospheric button glow.
//
// Adapted from AndreFrelicot/paper-shaders-android (Apache License 2.0,
// https://github.com/AndreFrelicot/paper-shaders-android): the rounded-box
// SDF, soft border mask, multi-spot accumulation, bloom mixing and
// texture-based value-noise smoke field of its PulsingBorder shader, plus
// the RuntimeShader + BitmapShader noise-binding runtime pattern. Only the
// minimum needed is ported here; there is no Paper Shaders dependency.
//
// Portal tuning (hardcoded, monochrome #F07949):
//   - 3 broad heavily-overlapping spots with randomized speeds/directions,
//     deformed by the smoke field so no orbit can be tracked;
//   - pulse behaviour is zero: no global brightness breathing;
//   - smoke does the slow morphing work; overall luminosity stays stable.
// The static BlurMaskFilter bloom in PortalGlow.kt stays underneath this
// layer. The opaque button draws above it, so text is never distorted.

import android.graphics.Bitmap
import android.graphics.BitmapShader
import android.graphics.RuntimeShader
import android.graphics.Shader
import android.os.Build
import android.util.Log
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.ShaderBrush
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import java.util.Random

private fun hexComponents(hex: String): FloatArray {
    val c = android.graphics.Color.parseColor(hex)
    return floatArrayOf(
        android.graphics.Color.red(c) / 255f,
        android.graphics.Color.green(c) / 255f,
        android.graphics.Color.blue(c) / 255f,
        android.graphics.Color.alpha(c) / 255f,
    )
}

/**
 * Deterministic white-noise bitmap generated once and reused as the smoke
 * source. Upstream ships a bundled noise PNG; generating 128x128 gray noise
 * with a fixed seed avoids a drawable resource while staying stable across
 * frames and launches.
 */
private fun generateNoiseBitmap(): android.graphics.Bitmap {
    val random = Random(NOISE_SEED)
    val pixels = IntArray(NOISE_SIZE * NOISE_SIZE) {
        val v = random.nextInt(256)
        (255 shl 24) or (v shl 16) or (v shl 8) or v
    }
    return Bitmap.createBitmap(pixels, NOISE_SIZE, NOISE_SIZE, Bitmap.Config.ARGB_8888)
}

/**
 * Atmospheric Portal-orange glow around the Begin Install silhouette. Renders
 * NOTHING on API < 33 (the static BlurMaskFilter bloom underneath remains),
 * and silently disables itself if the shader fails to compile.
 *
 * Per-frame work is two uniform writes (resolution, time); everything else
 * is set once. The layer is exactly the glow region, never fullscreen.
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
    val shader = remember {
        runCatching { RuntimeShader(PORTAL_BORDER_AGSL) }
            .onFailure { Log.e(TAG, "Portal glow shader failed to compile", it) }
            .getOrNull()
    } ?: return
    val brush = remember(shader) { ShaderBrush(shader) }
    val noise = remember {
        val bitmap = generateNoiseBitmap()
        val bitmapShader = BitmapShader(bitmap, Shader.TileMode.CLAMP, Shader.TileMode.CLAMP)
        bitmapShader.setFilterMode(BitmapShader.FILTER_MODE_LINEAR)
        bitmapShader
    }
    // Static uniforms, set once. Resolution follows the draw size per frame.
    remember(shader, density) {
        val orange = hexComponents(PORTAL_ORANGE)
        val colors = FloatArray(5 * 4)
        colors[0] = orange[0]
        colors[1] = orange[1]
        colors[2] = orange[2]
        colors[3] = orange[3]
        val canvasW = with(density) { (buttonWidth + margin * 2).toPx() }
        val canvasH = with(density) { (buttonHeight + margin * 2).toPx() }
        val marginX = with(density) { margin.toPx() } / canvasW
        val marginY = with(density) { margin.toPx() } / canvasH
        shader.setFloatUniform("u_colorBack", 0f, 0f, 0f, 0f)
        shader.setFloatUniform("u_colors", colors)
        shader.setFloatUniform("u_colorsCount", 1f)
        shader.setFloatUniform("u_roundness", 1f)
        shader.setFloatUniform("u_thickness", 0.12f)
        shader.setFloatUniform("u_marginLeft", marginX)
        shader.setFloatUniform("u_marginRight", marginX)
        shader.setFloatUniform("u_marginTop", marginY)
        shader.setFloatUniform("u_marginBottom", marginY)
        shader.setFloatUniform("u_aspectRatio", 0f)
        shader.setFloatUniform("u_softness", 0.95f)
        shader.setFloatUniform("u_intensity", 0.06f)
        shader.setFloatUniform("u_bloom", 0.25f)
        shader.setFloatUniform("u_spotSize", 0.7f)
        shader.setFloatUniform("u_spots", 3f)
        shader.setFloatUniform("u_pulse", 0f)
        shader.setFloatUniform("u_smoke", 0.4f)
        shader.setFloatUniform("u_smokeSize", 0.7f)
        shader.setFloatUniform("u_fit", 1f)
        shader.setFloatUniform("u_scale", 0.6f)
        shader.setFloatUniform("u_rotation", 0f)
        shader.setFloatUniform("u_originX", 0.5f)
        shader.setFloatUniform("u_originY", 0.5f)
        shader.setFloatUniform("u_offsetX", 0f)
        shader.setFloatUniform("u_offsetY", 0f)
        shader.setFloatUniform("u_worldWidth", 0f)
        shader.setFloatUniform("u_worldHeight", 0f)
        shader.setFloatUniform("u_imageAspectRatio", 1f)
        shader.setFloatUniform("u_pixelRatio", density.density)
        shader.setInputBuffer("u_noiseTexture", noise)
        shader.setFloatUniform("u_noiseTextureSize", NOISE_SIZE.toFloat(), NOISE_SIZE.toFloat())
        true
    }
    var timeSec by remember { mutableFloatStateOf(0f) }
    LaunchedEffect(shader) {
        var last = withFrameMillis { it }
        while (true) {
            val now = withFrameMillis { it }
            timeSec += (now - last) * SPEED / 1000f
            last = now
        }
    }
    Canvas(modifier = modifier) {
        runCatching {
            shader.setFloatUniform("u_resolution", size.width, size.height)
            shader.setFloatUniform("u_time", timeSec)
            drawRect(brush = brush, alpha = glowAlpha)
        }.onFailure {
            Log.e(TAG, "Portal glow draw failed", it)
        }
    }
}


private const val TAG = "PortalAgslGlow"
private const val NOISE_SIZE = 128
private const val NOISE_SEED = 0x907A1L

private const val PORTAL_ORANGE = "#F07949"
private const val SPEED = 0.5f

// AGSL preamble: trimmed port of the upstream common helpers actually used
// by the border shader (resolution/time/sizing uniforms, SDF box helpers,
// pattern UVs). Unused object/image/noise-math helpers are omitted.
private const val PORTAL_COMMON_AGSL = """
uniform vec2 u_resolution;
uniform float u_pixelRatio;
uniform float u_time;
uniform float u_fit;
uniform float u_scale;
uniform float u_rotation;
uniform float u_originX;
uniform float u_originY;
uniform float u_offsetX;
uniform float u_offsetY;
uniform float u_worldWidth;
uniform float u_worldHeight;
uniform float u_imageAspectRatio;

const float PI = 3.14159265358979323846;
const float TWO_PI = 6.28318530718;

vec3 ps_getBoxSize(float boxRatio, vec2 givenBoxSize, float fit, vec2 resolution) {
  vec2 box = vec2(0.0);
  box.x = boxRatio * min(givenBoxSize.x / boxRatio, givenBoxSize.y);
  float noFitBoxWidth = box.x;
  if (fit == 1.0) {
    box.x = boxRatio * min(resolution.x / boxRatio, resolution.y);
  } else if (fit == 2.0) {
    box.x = boxRatio * max(resolution.x / boxRatio, resolution.y);
  }
  box.y = box.x / boxRatio;
  return vec3(box, noFitBoxWidth);
}

vec2 ps_baseUVAt(vec2 pos) {
  return vec2(pos.x / u_resolution.x - 0.5, 0.5 - pos.y / u_resolution.y);
}

float ps_pixelDerivative(float multiplier) {
  return max(multiplier / max(min(u_resolution.x, u_resolution.y), 1.0), 1e-4);
}

vec2 ps_patternUVAt(vec2 pos) {
  vec2 uv = ps_baseUVAt(pos);
  vec2 boxOrigin = vec2(0.5 - u_originX, u_originY - 0.5);
  float r = u_rotation * PI / 180.0;
  mat2 graphicRotation = mat2(cos(r), sin(r), -sin(r), cos(r));
  vec2 graphicOffset = vec2(-u_offsetX, u_offsetY);
  vec2 givenBoxSize = max(vec2(u_worldWidth, u_worldHeight), vec2(1.0)) * u_pixelRatio;
  vec2 patternBoxGivenSize = vec2(
    (u_worldWidth == 0.0) ? u_resolution.x : givenBoxSize.x,
    (u_worldHeight == 0.0) ? u_resolution.y : givenBoxSize.y
  );
  float patternBoxRatio = patternBoxGivenSize.x / patternBoxGivenSize.y;
  vec3 boxSizeData = ps_getBoxSize(patternBoxRatio, patternBoxGivenSize, u_fit, u_resolution);
  vec2 patternBoxScale = u_resolution / boxSizeData.xy;
  vec2 patternUV = uv;
  patternUV += graphicOffset / patternBoxScale;
  patternUV += boxOrigin;
  patternUV -= boxOrigin / patternBoxScale;
  patternUV *= u_resolution;
  patternUV /= u_pixelRatio;
  if (u_fit > 0.0) {
    patternUV *= (boxSizeData.z / boxSizeData.x);
  }
  patternUV /= u_scale;
  patternUV = graphicRotation * patternUV;
  patternUV += boxOrigin / patternBoxScale;
  patternUV -= boxOrigin;
  patternUV *= 0.01;
  return patternUV;
}

vec2 ps_patternUV(vec2 fragCoord) {
  return ps_patternUVAt(fragCoord);
}

vec2 ps_responsiveBoxGivenSize() {
  vec2 givenBoxSize = max(vec2(u_worldWidth, u_worldHeight), vec2(1.0)) * u_pixelRatio;
  return vec2(
    (u_worldWidth == 0.0) ? u_resolution.x : givenBoxSize.x,
    (u_worldHeight == 0.0) ? u_resolution.y : givenBoxSize.y
  );
}

vec2 ps_responsiveUVAt(vec2 pos) {
  vec2 uv = ps_baseUVAt(pos);
  vec2 boxOrigin = vec2(0.5 - u_originX, u_originY - 0.5);
  float r = u_rotation * PI / 180.0;
  mat2 graphicRotation = mat2(cos(r), sin(r), -sin(r), cos(r));
  vec2 graphicOffset = vec2(-u_offsetX, u_offsetY);
  vec2 responsiveBoxGivenSize = ps_responsiveBoxGivenSize();
  float responsiveRatio = responsiveBoxGivenSize.x / responsiveBoxGivenSize.y;
  vec2 responsiveBoxSize = ps_getBoxSize(responsiveRatio, responsiveBoxGivenSize, u_fit, u_resolution).xy;
  vec2 responsiveBoxScale = u_resolution / responsiveBoxSize;
  vec2 responsiveUV = uv;
  responsiveUV *= responsiveBoxScale;
  responsiveUV += boxOrigin * (responsiveBoxScale - 1.0);
  responsiveUV += graphicOffset;
  responsiveUV /= u_scale;
  responsiveUV.x *= responsiveRatio;
  responsiveUV = graphicRotation * responsiveUV;
  responsiveUV.x /= responsiveRatio;
  return responsiveUV;
}

vec2 ps_responsiveUV(vec2 fragCoord) {
  return ps_responsiveUVAt(fragCoord);
}
"""

// Portal border atmosphere: the upstream PulsingBorder fragment core with a
// single orange entry (u_colorsCount = 1). Spot speeds/directions stay
// randomized per spot; smoke does the deformation work.
private const val PORTAL_BORDER_AGSL: String = PORTAL_COMMON_AGSL + """

uniform shader u_noiseTexture;
uniform vec2 u_noiseTextureSize;
uniform vec4 u_colorBack;
uniform vec4 u_colors[5];
uniform float u_colorsCount;
uniform float u_roundness;
uniform float u_thickness;
uniform float u_marginLeft;
uniform float u_marginRight;
uniform float u_marginTop;
uniform float u_marginBottom;
uniform float u_aspectRatio;
uniform float u_softness;
uniform float u_intensity;
uniform float u_bloom;
uniform float u_spotSize;
uniform float u_spots;
uniform float u_pulse;
uniform float u_smoke;
uniform float u_smokeSize;

float pb_sst(float edge0, float edge1, float x) {
  return smoothstep(edge0, edge1, x);
}

float pb_roundedBox(vec2 uv, vec2 halfSize, float distance, float cornerDistance, float thickness, float softness) {
  float aa = ps_pixelDerivative(2.0);
  float borderDistance = abs(distance);
  float border = 1.0 - pb_sst(min(mix(thickness, -thickness, softness), thickness + aa), max(mix(thickness, -thickness, softness), thickness + aa), borderDistance);
  float cornerFadeCircles = 0.0;
  cornerFadeCircles = mix(1.0, cornerFadeCircles, pb_sst(0.0, 1.0, length((uv + halfSize) / thickness)));
  cornerFadeCircles = mix(1.0, cornerFadeCircles, pb_sst(0.0, 1.0, length((uv - vec2(-halfSize.x, halfSize.y)) / thickness)));
  cornerFadeCircles = mix(1.0, cornerFadeCircles, pb_sst(0.0, 1.0, length((uv - vec2(halfSize.x, -halfSize.y)) / thickness)));
  cornerFadeCircles = mix(1.0, cornerFadeCircles, pb_sst(0.0, 1.0, length((uv - halfSize) / thickness)));
  aa = ps_pixelDerivative(1.0);
  float cornerFade = pb_sst(0.0, mix(aa, thickness, softness), cornerDistance);
  cornerFade *= cornerFadeCircles;
  return border + cornerFade;
}

float pb_randomG(vec2 p) {
  vec2 uv = floor(p) / 100.0 + 0.5;
  return u_noiseTexture.eval(fract(uv) * u_noiseTextureSize).g;
}

float pb_valueNoise(vec2 st) {
  vec2 i = floor(st);
  vec2 f = fract(st);
  float a = pb_randomG(i);
  float b = pb_randomG(i + vec2(1.0, 0.0));
  float c = pb_randomG(i + vec2(0.0, 1.0));
  float d = pb_randomG(i + vec2(1.0, 1.0));
  vec2 u = f * f * (3.0 - 2.0 * f);
  return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

vec4 main(vec2 fragCoord) {
  float t = 1.2 * (u_time + 109.0);
  vec2 borderUV = ps_responsiveUV(fragCoord);
  float canvasRatio = ps_responsiveBoxGivenSize().x / ps_responsiveBoxGivenSize().y;
  vec2 halfSize = vec2(0.5);
  borderUV.x *= max(canvasRatio, 1.0);
  borderUV.y /= min(canvasRatio, 1.0);
  halfSize.x *= max(canvasRatio, 1.0);
  halfSize.y /= min(canvasRatio, 1.0);

  float mL = u_marginLeft;
  float mR = u_marginRight;
  float mT = u_marginTop;
  float mB = u_marginBottom;
  float mX = mL + mR;
  float mY = mT + mB;

  float thickness = 0.5 * u_thickness * min(halfSize.x, halfSize.y);
  halfSize.x *= 1.0 - mX;
  halfSize.y *= 1.0 - mY;
  vec2 centerShift = vec2((mL - mR) * max(canvasRatio, 1.0) * 0.5, (mB - mT) / min(canvasRatio, 1.0) * 0.5);
  borderUV -= centerShift;
  halfSize -= mix(thickness, 0.0, u_softness);

  float radius = mix(0.0, min(halfSize.x, halfSize.y), u_roundness);
  vec2 d = abs(borderUV) - halfSize + radius;
  float outsideDistance = length(max(d, 0.0001)) - radius;
  float insideDistance = min(max(d.x, d.y), 0.0001);
  float cornerDistance = abs(min(max(d.x, d.y) - 0.45 * radius, 0.0));
  float distance = outsideDistance + insideDistance;

  float borderThickness = mix(thickness, 3.0 * thickness, u_softness);
  float border = pb_roundedBox(borderUV, halfSize, distance, cornerDistance, borderThickness, u_softness);
  border = pow(border, 1.0 + u_softness);

  vec2 smokeUV = 0.3 * u_smokeSize * ps_patternUV(fragCoord);
  float smoke = clamp(3.0 * pb_valueNoise(2.7 * smokeUV + 0.5 * t), 0.0, 1.0);
  smoke -= pb_valueNoise(3.4 * smokeUV - 0.5 * t);
  float smokeThickness = min(0.4, max(thickness + 0.2, 0.1));
  smoke *= pb_roundedBox(borderUV, halfSize, distance, cornerDistance, smokeThickness, 1.0);
  smoke = 30.0 * smoke * smoke;
  smoke *= mix(0.0, 0.5, pow(u_smoke, 2.0));
  border += clamp(smoke, 0.0, 1.0);
  border = clamp(border, 0.0, 1.0);

  vec3 blendColor = vec3(0.0);
  float blendAlpha = 0.0;
  vec3 addColor = vec3(0.0);
  float addAlpha = 0.0;
  float bloom = 4.0 * u_bloom;
  float intensity = 1.0 + (1.0 + 4.0 * u_softness) * u_intensity;
  float angle = atan(borderUV.y, borderUV.x) / TWO_PI;

  vec3 c = u_colors[0].rgb * u_colors[0].a;
  float a = u_colors[0].a;
  for (int spotIdx = 0; spotIdx < 4; spotIdx++) {
    if (spotIdx >= int(u_spots)) break;
    float spotIdxF = float(spotIdx);
    float rnd = fract(sin(dot(vec2(spotIdxF * 12.9898, 78.233), vec2(12.9898, 78.233))) * 43758.5453);
    float rndDir = fract(sin(dot(vec2(spotIdxF * 39.346, 11.135), vec2(12.9898, 78.233))) * 24634.6345);
    float time = (0.1 + 0.15 * abs(sin(spotIdxF * 2.0) * cos(spotIdxF * 2.0))) * t + rnd * 3.0;
    time *= mix(1.0, -1.0, step(0.5, rndDir));
    float mask = 0.5 + 0.5 * sin(t * 0.9 + spotIdxF * 2.4);
    float atg1 = fract(angle + time);
    float spotSize = 0.05 + 0.6 * pow(u_spotSize, 2.0) + 0.05 * rnd;
    float sector = pb_sst(0.5 - spotSize, 0.5, atg1) * (1.0 - pb_sst(0.5, 0.5 + spotSize, atg1));
    sector *= mask * border * intensity;
    sector = clamp(sector, 0.0, 1.0);
    vec3 srcColor = c * sector;
    float srcAlpha = a * sector;
    blendColor += (1.0 - blendAlpha) * srcColor;
    blendAlpha += (1.0 - blendAlpha) * srcAlpha;
    addColor += srcColor;
    addAlpha += srcAlpha;
  }

  vec3 accumColor = mix(blendColor, addColor, bloom);
  float accumAlpha = clamp(mix(blendAlpha, addAlpha, bloom), 0.0, 1.0);
  vec3 bgColor = u_colorBack.rgb * u_colorBack.a;
  vec3 color = accumColor + (1.0 - accumAlpha) * bgColor;
  float opacity = accumAlpha + (1.0 - accumAlpha) * u_colorBack.a;
  return vec4(color, opacity);
}
"""
