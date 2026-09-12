package app.polarbear.setup

import android.animation.ValueAnimator
import android.graphics.RenderEffect
import android.graphics.RuntimeShader
import android.graphics.Shader
import android.os.Build
import android.util.Log
import android.view.ViewTreeObserver
import androidx.annotation.RequiresApi
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.asComposeRenderEffect
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.layer.drawLayer
import androidx.compose.ui.graphics.rememberGraphicsLayer
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.LifecycleOwner
import kotlin.math.ceil
import kotlin.math.hypot

private const val TAG = "PortalLaunch"
private const val DURATION_MS = 1200
private const val CAPTURE_SCALE = 0.25f
private val TravelEase = CubicBezierEasing(0.42f, 0f, 0.18f, 1f)
private val ApertureEase = CubicBezierEasing(0.58f, 0f, 0.30f, 1f)

/** One setup composition, with a temporary rendering treatment and one visible mark.
 * The host owns eligibility; this can later be gated to actual first-time setup.
 * No native surface, window, provisioning or Activity lifecycle policy is involved.
 */
@Composable
fun PortalLaunchTransition(
    playIntro: Boolean,
    onContentPreDraw: () -> Unit,
    onIntroResolved: () -> Unit,
    onBeginInstall: () -> Unit,
) {
    val view = LocalView.current
    val lifecycle = (LocalContext.current as LifecycleOwner).lifecycle
    var resolved by rememberSaveable { mutableStateOf(!playIntro || Build.VERSION.SDK_INT < 33) }
    var ready by remember { mutableStateOf(false) }
    var rootBounds by remember { mutableStateOf(Rect.Zero) }
    var headerBounds by remember { mutableStateOf(Rect.Zero) }
    val clock = remember { Animatable(0f) }
    val currentPreDraw by rememberUpdatedState(onContentPreDraw)
    val currentResolved by rememberUpdatedState(onIntroResolved)
    val laidOut = rootBounds.width > 0f && headerBounds.width > 0f

    // Unlike a FrameLayout pre-draw, this gate cannot release before both the
    // actual CONFIGURE and its logo destination have participated in layout.
    DisposableEffect(view, laidOut) {
        val listener = object : ViewTreeObserver.OnPreDrawListener {
            override fun onPreDraw(): Boolean {
                if (laidOut) {
                    view.viewTreeObserver.removeOnPreDrawListener(this)
                    currentPreDraw()
                    ready = true
                }
                return true
            }
        }
        if (laidOut) view.viewTreeObserver.addOnPreDrawListener(listener)
        onDispose {
            if (view.viewTreeObserver.isAlive) view.viewTreeObserver.removeOnPreDrawListener(listener)
        }
    }

    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_PAUSE || event == Lifecycle.Event.ON_STOP) {
                // Resolve while backgrounded, even if the frame clock stops.
                resolved = true
                currentResolved()
            }
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    LaunchedEffect(ready, resolved) {
        if (resolved) {
            currentResolved()
            Log.i(TAG, "CONFIGURE interactive; launch capture/effects released")
        } else if (ready) {
            if (!ValueAnimator.areAnimatorsEnabled() ||
                !lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) {
                resolved = true
            } else {
                Log.i(TAG, "Compose pre-draw ready; aperture intro started")
                // Compose's animation clock honors MotionDurationScale, including
                // a scale change to zero during playback. No wall-clock delay.
                clock.animateTo(DURATION_MS.toFloat(), tween(DURATION_MS, easing = LinearEasing))
                resolved = true
            }
        }
    }

    val active = !resolved
    val treatment = if (active && Build.VERSION.SDK_INT >= 33) {
        rememberApertureTreatment { clock.value }
    } else Modifier
    val guard = if (active) {
        Modifier.clearAndSetSemantics { }
            .onPreviewKeyEvent { true }
            .pointerInput(Unit) {
                awaitPointerEventScope {
                    while (true) {
                        awaitPointerEvent(PointerEventPass.Initial).changes.forEach { it.consume() }
                    }
                }
            }
    } else Modifier

    Box(Modifier.fillMaxSize().background(PortalColors.Charcoal).onGloballyPositioned {
        rootBounds = it.boundsInRoot()
    }.then(guard)) {
        // Keep this call at the same composition slot before/during/after intro:
        // no second screen, second fade, duplicated state or persistent capture.
        Box(Modifier.fillMaxSize().then(treatment)) {
            PortalSetupScreen(
                onBeginInstall = { if (!active) onBeginInstall() },
                launchMarkModifier = Modifier.onGloballyPositioned {
                    headerBounds = it.boundsInRoot()
                }.then(if (active) Modifier.drawWithContent { } else Modifier),
            )
        }
        if (active) {
            TravellingPortalMark(rootBounds, headerBounds) { clock.value }
        }
    }
}

/** Geometry comes from the splash drawable viewport and the laid-out Image slot.
 * Header Image uses ContentScale.Fit with the canonical painter's 56:84 aspect.
 * Drawing that same painter here avoids a crossfade or a second visible mark.
 */
@Composable
private fun TravellingPortalMark(root: Rect, header: Rect, elapsed: () -> Float) {
    val painter = portalMarkPainter(PortalColors.Ivory, PortalColors.Orange)
    val splashSize = with(LocalDensity.current) { 288.dp.toPx() }
    val glow = remember {
        Brush.radialGradient(
            listOf(PortalColors.Orange.copy(alpha = 0.075f), PortalColors.Orange.copy(alpha = 0f)),
            center = Offset.Zero,
            radius = 1f,
        )
    }
    Canvas(Modifier.fillMaxSize()) {
        val time = elapsed()
        val travel = TravelEase.transform(((time - 250f) / 670f).coerceIn(0f, 1f))
        val settle = ((time - 920f) / 280f).coerceIn(0f, 1f)
        val assetScale = splashSize / 1024f
        val start = Rect(
            size.width / 2f + (336f - 512f) * assetScale,
            size.height / 2f + (245f - 512f) * assetScale,
            size.width / 2f + (681f - 512f) * assetScale,
            size.height / 2f + (759f - 512f) * assetScale,
        )
        val fittedWidth = header.height * painter.intrinsicSize.width / painter.intrinsicSize.height
        val end = if (header.width > 0f) Rect(
            header.left - root.left + (header.width - fittedWidth) / 2f,
            header.top - root.top,
            header.left - root.left + (header.width + fittedWidth) / 2f,
            header.bottom - root.top,
        ) else start
        val rect = androidx.compose.ui.geometry.lerp(start, end, travel)
        // Restrained warm emission travels with the geometry, never orbits it.
        translate(rect.center.x, rect.center.y) {
            val radius = rect.height * 0.9f
            scale(radius, radius, Offset.Zero) {
                drawCircle(glow, radius = 1f, center = Offset.Zero, alpha = (1f - settle))
            }
        }
        translate(rect.left, rect.top) {
            with(painter) { draw(Size(rect.width, rect.height)) }
        }
    }
}

@RequiresApi(33)
@Composable
private fun rememberApertureTreatment(elapsed: () -> Float): Modifier {
    // RenderNode display-list capture, never a CPU bitmap or SurfaceView capture.
    // Only the frost target is rasterized, at 1/16 of the full pixel count.
    val source = rememberGraphicsLayer()
    val frost = rememberGraphicsLayer()
    val density = LocalDensity.current.density
    val shader = remember { RuntimeShader(PORTAL_APERTURE_SHADER) }
    val blur = remember(density) {
        RenderEffect.createBlurEffect(24f * density * CAPTURE_SCALE, 24f * density * CAPTURE_SCALE, Shader.TileMode.CLAMP)
    }
    return Modifier.drawWithContent {
        source.record { this@drawWithContent.drawContent() }
        drawLayer(source)
        val time = elapsed()
        val reveal = ApertureEase.transform(((time - 120f) / 970f).coerceIn(0f, 1f))
        val settle = ((time - 920f) / 280f).coerceIn(0f, 1f)
        val w = size.width * CAPTURE_SCALE
        val h = size.height * CAPTURE_SCALE
        val unit = density * CAPTURE_SCALE
        shader.setFloatUniform("origin", w / 2f, h / 2f)
        shader.setFloatUniform("radius", -24f * unit + reveal * (hypot(w, h) / 2f + 64f * unit))
        shader.setFloatUniform("unit", unit)
        shader.setFloatUniform("time", time / 1000f)
        shader.setFloatUniform("settle", settle)
        // Runtime RenderEffects snapshot uniforms; renew the lightweight wrapper,
        // never the compiled shader or blur kernel. Read animation in draw only.
        frost.renderEffect = RenderEffect.createChainEffect(
            RenderEffect.createRuntimeShaderEffect(shader, "inputShader"), blur,
        ).asComposeRenderEffect()
        frost.record(size = IntSize(ceil(w).toInt(), ceil(h).toInt())) {
            scale(CAPTURE_SCALE, CAPTURE_SCALE, Offset.Zero) { drawLayer(source) }
        }
        scale(1f / CAPTURE_SCALE, 1f / CAPTURE_SCALE, Offset.Zero) { drawLayer(frost) }
    }
}

// Adapted from Mejdi Hafiene's MIT RevealContentTransition / ripple displacement
// at cdb866cbc3192dba326871354b98cbf5036227e5. See third_party attribution.
// Portal: angular contour (no concentric rings), a single localized glass front,
// safe nonzero normal/edge width, premultiplied mask, charcoal frost/orange light.
private const val PORTAL_APERTURE_SHADER = """
uniform shader inputShader;
uniform float2 origin;
uniform float radius;
uniform float unit;
uniform float time;
uniform float settle;

half4 main(float2 p) {
    float2 delta = p - origin;
    float distance = length(delta);
    float angle = atan(delta.y, delta.x);
    float presence = smoothstep(0.0, 48.0 * unit, max(radius, 0.0));
    // Fixed angular lobes gently breathe; no rotating/noise object.
    float contour = sin(angle * 3.0 + 0.35) * 7.0
                  + sin(angle * 5.0 - 0.8) * 3.0
                  + sin(angle * 9.0 + sin(time * 2.4) * 0.24) * 1.4;
    float front = distance - radius - contour * unit * presence;
    float width = (9.0 + 21.0 * settle) * unit;
    float mask = smoothstep(-width, width, front);
    if (mask <= 0.0) return half4(0.0);
    float edge = exp(-abs(front) / (7.0 * unit)) * presence * (1.0 - settle);
    float2 normal = delta / max(distance, 0.001);
    float2 samplePos = p + normal * (1.6 * unit * edge);
    half3 captured = inputShader.eval(samplePos).rgb;
    half3 charcoal = half3(0.0980392, 0.1058824, 0.1098039);
    // Flat charcoal at handoff; real blurred detail becomes perceptible in the
    // initial stillness. Frost stays strong outside, sharp content is underneath.
    float detail = mix(0.0, 0.24, smoothstep(0.0, 0.12, time));
    half3 glass = mix(charcoal, captured, detail);
    float lit = 0.045 + 0.055 * pow(0.5 + 0.5 * sin(angle * 2.0 + 0.6), 2.0);
    glass += half3(0.9411765, 0.4745098, 0.2862745) * half(edge * lit);
    return half4(glass * half(mask), half(mask));
}
"""
