package app.polarbear.setup

import android.animation.ValueAnimator
import android.graphics.RenderEffect
import android.graphics.RuntimeShader
import android.os.Build
import android.util.Log
import androidx.annotation.RequiresApi
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.asComposeRenderEffect
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.LifecycleOwner
import kotlin.math.abs

private const val TAG = "PortalVeilReveal"
private const val COMMIT_TRAVEL_FRACTION = 0.28f
private const val COMMIT_VELOCITY_DP_PER_SECOND = 1_450f
private const val EFFECT_START_DP_PER_SECOND = 280f
private const val EFFECT_FULL_DP_PER_SECOND = 2_600f
private val MAX_SMEAR = 15.dp

/**
 * Stationary input shell plus one translated Compose visual layer. Keeping the
 * gesture shell still avoids coordinate feedback while the entire visible
 * veil follows the finger. The Android sibling host becomes transparent at
 * the Ready prelude, so the exposed pixels are the live SurfaceView.
 */
@Composable
internal fun PortalRevealVeil(
    eligible: Boolean,
    modifier: Modifier = Modifier,
    onCommitted: () -> Unit,
    onFinished: () -> Unit,
    content: @Composable BoxScope.() -> Unit,
) {
    val density = LocalDensity.current
    val densityScale = density.density
    val lifecycle = (LocalContext.current as LifecycleOwner).lifecycle
    var displacement by remember { mutableFloatStateOf(0f) }
    var effectStrength by remember { mutableFloatStateOf(0f) }
    val animationScope = rememberCoroutineScope()
    val settleJob = remember { arrayOfNulls<Job>(1) }
    val commitInFlight = remember { booleanArrayOf(false) }
    val currentCommitted by rememberUpdatedState(onCommitted)
    val currentFinished by rememberUpdatedState(onFinished)

    LaunchedEffect(eligible) {
        if (eligible) {
            Log.i(TAG, "final swipe affordance enabled; fullscreen acquisition active")
        } else {
            settleJob[0]?.cancel()
            settleJob[0] = null
            commitInFlight[0] = false
            effectStrength = 0f
            displacement = 0f
        }
    }

    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_PAUSE || event == Lifecycle.Event.ON_STOP) {
                settleJob[0]?.cancel()
                settleJob[0] = null
                effectStrength = 0f
                if (commitInFlight[0]) {
                    // The app is no longer visible, so finish the already
                    // committed removal atomically instead of resuming on a
                    // partially displaced veil.
                    commitInFlight[0] = false
                    currentFinished()
                } else {
                    displacement = 0f
                }
            }
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }

    val motionLayer = rememberPortalVeilMotionLayer(
        displacement = { displacement },
        strength = { effectStrength },
    )
    val gesture = Modifier.pointerInput(eligible, densityScale) {
        if (!eligible) return@pointerInput
        val velocityThresholdPx = COMMIT_VELOCITY_DP_PER_SECOND * densityScale
        val fullHeight = size.height.toFloat().coerceAtLeast(1f)
        awaitEachGesture {
            val down = awaitFirstDown(
                requireUnconsumed = false,
                pass = PointerEventPass.Initial,
            )
            if (commitInFlight[0]) {
                return@awaitEachGesture
            }

            val velocityTracker = VelocityTracker().apply {
                addPosition(down.uptimeMillis, down.position)
            }
            settleJob[0]?.cancel()
            settleJob[0] = null
            val originDisplacement = displacement
            var total = Offset.Zero
            var dragging = false
            var cancelled = false

            while (true) {
                val event = awaitPointerEvent(PointerEventPass.Initial)
                if (event.changes.count { it.pressed } > 1) {
                    cancelled = true
                    event.changes.forEach { it.consume() }
                    break
                }
                val change = event.changes.firstOrNull { it.id == down.id }
                if (change == null) {
                    cancelled = true
                    break
                }
                velocityTracker.addPosition(change.uptimeMillis, change.position)
                val delta = change.positionChange()
                total += delta

                val upwardTravel = -total.y
                if (
                    !dragging &&
                    upwardTravel > viewConfiguration.touchSlop &&
                    upwardTravel > abs(total.x) * 1.15f
                ) {
                    dragging = true
                    Log.i(TAG, "fullscreen reveal gesture acquired")
                }
                if (dragging) {
                    change.consume()
                    if (change.pressed) {
                        displacement = (
                            originDisplacement - total.y - viewConfiguration.touchSlop
                        ).coerceIn(0f, fullHeight)
                        val upwardVelocity = (-velocityTracker.calculateVelocity().y).coerceAtLeast(0f)
                        effectStrength = effectStrengthFor(upwardVelocity, densityScale)
                    }
                }
                if (!change.pressed) break
            }

            if (!dragging) return@awaitEachGesture
            val upwardVelocity = if (cancelled) 0f else {
                (-velocityTracker.calculateVelocity().y).coerceAtLeast(0f)
            }
            val commit = !cancelled && shouldCommitReveal(
                displacement = displacement,
                viewportHeight = fullHeight,
                upwardVelocity = upwardVelocity,
                velocityThreshold = velocityThresholdPx,
            )

            if (!ValueAnimator.areAnimatorsEnabled()) {
                effectStrength = 0f
                if (commit) {
                    currentCommitted()
                    displacement = fullHeight
                    currentFinished()
                } else {
                    displacement = 0f
                }
                return@awaitEachGesture
            }

            if (commit) {
                Log.i(
                    TAG,
                    "reveal committed travel=${displacement / fullHeight} velocityPx=$upwardVelocity",
                )
                effectStrength = maxOf(
                    effectStrength,
                    effectStrengthFor(upwardVelocity, densityScale),
                )
                val peakEffectStrength = effectStrength
                val releaseDisplacement = displacement
                val remainingTravel = (fullHeight - releaseDisplacement).coerceAtLeast(1f)
                commitInFlight[0] = true
                currentCommitted()
                val launchVelocity = upwardVelocity.coerceAtMost(fullHeight * 4f)
                settleJob[0] = animationScope.launch {
                    Animatable(displacement).animateTo(
                        targetValue = fullHeight,
                        animationSpec = spring(
                            dampingRatio = Spring.DampingRatioNoBouncy,
                            stiffness = 420f,
                        ),
                        initialVelocity = launchVelocity,
                    ) {
                        displacement = value.coerceIn(0f, fullHeight)
                        // Hold the release energy through most of the exit, then
                        // collapse it over the final 22% of remaining travel.
                        // This ties the last motion frame to veil geometry
                        // instead of guessing at a fixed delay.
                        val remainingFraction = (
                            (fullHeight - displacement) / remainingTravel
                        ).coerceIn(0f, 1f)
                        effectStrength = peakEffectStrength *
                            (remainingFraction / 0.22f).coerceIn(0f, 1f)
                    }
                    effectStrength = 0f
                    displacement = fullHeight
                    commitInFlight[0] = false
                    currentFinished()
                }
            } else {
                Log.i(TAG, "reveal cancelled; veil returning to rest")
                settleJob[0] = animationScope.launch {
                    coroutineScope {
                        launch {
                            Animatable(effectStrength).animateTo(0f, tween(160)) {
                                effectStrength = value
                            }
                        }
                        Animatable(displacement).animateTo(
                            targetValue = 0f,
                            animationSpec = spring(
                                dampingRatio = Spring.DampingRatioNoBouncy,
                                stiffness = 420f,
                            ),
                        ) {
                            displacement = value.coerceAtLeast(0f)
                        }
                    }
                    displacement = 0f
                    effectStrength = 0f
                }
            }
        }
    }

    // This outer shell remains stationary and input-owning. Only the inner
    // visual layer moves, so the same finger coordinates stay stable.
    val affordanceRisePx = with(density) { 10.dp.roundToPx() }
    Box(Modifier.fillMaxSize().then(gesture)) {
        Box(
            modifier = Modifier
                .fillMaxSize()
                .then(motionLayer)
                .then(modifier),
        ) {
            content()
            AnimatedVisibility(
                visible = eligible,
                enter = fadeIn(tween(360, delayMillis = 40)) +
                    slideInVertically(
                        animationSpec = tween(420, delayMillis = 20),
                        initialOffsetY = { affordanceRisePx },
                    ) + scaleIn(
                        animationSpec = tween(420, delayMillis = 20),
                        initialScale = 0.97f,
                    ),
                exit = fadeOut(tween(140)) +
                    slideOutVertically(tween(160)) { affordanceRisePx / 2 } +
                    scaleOut(tween(160), targetScale = 0.985f),
                modifier = Modifier.align(Alignment.BottomCenter),
            ) {
                PortalHomeGestureAffordance()
            }
        }
    }
}

internal fun shouldCommitReveal(
    displacement: Float,
    viewportHeight: Float,
    upwardVelocity: Float,
    velocityThreshold: Float,
): Boolean {
    if (viewportHeight <= 0f) return false
    return displacement.coerceAtLeast(0f) >= viewportHeight * COMMIT_TRAVEL_FRACTION ||
        upwardVelocity >= velocityThreshold
}

internal fun effectStrengthFor(upwardVelocityPx: Float, density: Float): Float {
    val velocityDp = upwardVelocityPx / density.coerceAtLeast(0.1f)
    return ((velocityDp - EFFECT_START_DP_PER_SECOND) /
        (EFFECT_FULL_DP_PER_SECOND - EFFECT_START_DP_PER_SECOND)).coerceIn(0f, 1f)
}

@Composable
private fun PortalHomeGestureAffordance() {
    val sweep = remember { Animatable(0f) }
    val sweepLayer = rememberPortalAffordanceSweepLayer { sweep.value }

    LaunchedEffect(Unit) {
        if (ValueAnimator.areAnimatorsEnabled()) {
            sweep.snapTo(0f)
            sweep.animateTo(1f, tween(560, easing = FastOutSlowInEasing))
        } else {
            sweep.snapTo(1f)
        }
        Log.i(TAG, "final affordance entrance sweep settled; shader dormant")
    }

    Column(
        modifier = Modifier
            .navigationBarsPadding()
            .padding(bottom = 16.dp)
            .then(sweepLayer),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(
            text = "Swipe up to enter Portal",
            color = PortalColors.Ivory.copy(alpha = 0.48f),
            fontSize = 11.sp,
            fontWeight = FontWeight.Medium,
        )
        Spacer(Modifier.height(8.dp))
        Box(
            Modifier
                .size(width = 104.dp, height = 5.dp)
                .background(
                    PortalColors.Ivory.copy(alpha = 0.68f),
                    RoundedCornerShape(50),
                ),
        )
    }
}

/**
 * Six-sample vertical smear adapted from the MIT MotionBlur.kt sampling idea
 * in mejdi14/Android-AGSL-Shader-Playground. Portal uses a bounded global
 * velocity scalar and a lower-edge falloff rather than a pointer-local field.
 * See third_party/android-agsl-shader-playground.
 */
@Composable
private fun rememberPortalVeilMotionLayer(
    displacement: () -> Float,
    strength: () -> Float,
): Modifier {
    val density = LocalDensity.current
    val maxSmearPx = with(density) { MAX_SMEAR.toPx() }
    val shader = if (Build.VERSION.SDK_INT >= 33) remember { RuntimeShader(VEIL_MOTION_SHADER) } else null
    val effect = if (Build.VERSION.SDK_INT >= 33 && shader != null) {
        remember(shader) { createVeilMotionEffect(shader) }
    } else null

    return Modifier.graphicsLayer {
        translationY = -displacement().coerceAtLeast(0f)
        val amount = strength().coerceIn(0f, 1f)
        if (shader != null && effect != null && amount > 0.001f) {
            shader.setFloatUniform("size", size.width, size.height)
            shader.setFloatUniform("strength", amount)
            shader.setFloatUniform("maxSmear", maxSmearPx)
            renderEffect = effect
        } else {
            renderEffect = null
        }
    }
}

@RequiresApi(33)
private fun createVeilMotionEffect(shader: RuntimeShader) =
    RenderEffect.createRuntimeShaderEffect(shader, "inputShader").asComposeRenderEffect()

@Composable
private fun rememberPortalAffordanceSweepLayer(progress: () -> Float): Modifier {
    val density = LocalDensity.current
    val maxShiftPx = with(density) { 1.6.dp.toPx() }
    val shader = if (Build.VERSION.SDK_INT >= 33) {
        remember { RuntimeShader(AFFORDANCE_SWEEP_SHADER) }
    } else null
    val effect = if (Build.VERSION.SDK_INT >= 33 && shader != null) {
        remember(shader) {
            RenderEffect.createRuntimeShaderEffect(shader, "inputShader").asComposeRenderEffect()
        }
    } else null

    return Modifier.graphicsLayer {
        val phase = progress().coerceIn(0f, 1f)
        if (shader != null && effect != null && phase > 0.001f && phase < 0.999f) {
            shader.setFloatUniform("size", size.width, size.height)
            shader.setFloatUniform("progress", phase)
            shader.setFloatUniform("maxShift", maxShiftPx)
            renderEffect = effect
        } else {
            renderEffect = null
        }
    }
}

private const val VEIL_MOTION_SHADER = """
uniform shader inputShader;
uniform float2 size;
uniform float strength;
uniform float maxSmear;

half4 main(float2 p) {
    half4 base = inputShader.eval(p);
    float trailing = mix(0.22, 1.0,
                         smoothstep(size.y * 0.08, size.y * 0.82, p.y));
    float amount = clamp(strength * trailing, 0.0, 1.0);
    if (amount <= 0.001) return base;

    float smear = maxSmear * amount;
    half4 s1 = inputShader.eval(p - float2(0.0, smear * 0.24));
    half4 s2 = inputShader.eval(p - float2(0.0, smear * 0.56));
    half4 s3 = inputShader.eval(p - float2(0.0, smear));
    half4 blurred = (base + s1 + s2 + s3) * 0.25;

    // Preserve the visible vertical smear while dropping the ineffective
    // chromatic-aberration samples.
    half3 treated = mix(blurred.rgb, base.rgb, half(0.66));
    return half4(mix(base.rgb, treated, half(amount * 0.86)),
                 max(base.a, blurred.a));
}
"""

private const val AFFORDANCE_SWEEP_SHADER = """
uniform shader inputShader;
uniform float2 size;
uniform float progress;
uniform float maxShift;

half4 main(float2 p) {
    half4 base = inputShader.eval(p);
    float centre = mix(-size.x * 0.20, size.x * 1.20, progress);
    float width = max(size.x * 0.18, 1.0);
    float normalized = abs(p.x - centre) / width;
    float band = 1.0 - smoothstep(0.12, 1.0, normalized);
    if (band <= 0.001) return base;

    half4 refracted = inputShader.eval(p + float2(maxShift * band, 0.0));
    half3 ivory = half3(0.945, 0.922, 0.867);
    half3 orange = half3(0.941, 0.475, 0.286);
    half3 lit = mix(base.rgb, refracted.rgb, half(band * 0.28));
    lit += ivory * half(band * 0.12) * base.a;
    lit += orange * half(band * 0.055) * base.a;
    return half4(lit, max(base.a, refracted.a));
}
"""
