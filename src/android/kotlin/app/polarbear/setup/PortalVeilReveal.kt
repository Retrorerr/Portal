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
import androidx.compose.animation.core.Animatable
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

private const val TAG = "PortalVeilReveal"
private const val COMMIT_TRAVEL_FRACTION = 0.28f
private const val COMMIT_VELOCITY_DP_PER_SECOND = 1_450f
private const val EFFECT_START_DP_PER_SECOND = 280f
private const val EFFECT_FULL_DP_PER_SECOND = 2_600f
private val ACTIVATION_HEIGHT = 112.dp
private val MAX_SMEAR = 9.dp
private val MAX_CHROMA = 1.8.dp

/**
 * Stationary input shell plus one translated opaque Compose layer. Keeping the
 * gesture shell still avoids coordinate feedback while the entire visible
 * veil follows the finger. The Android sibling host becomes transparent only
 * when [eligible], so the exposed pixels are the live SurfaceView.
 */
@Composable
internal fun PortalRevealVeil(
    eligible: Boolean,
    modifier: Modifier = Modifier,
    onEligibilityChanged: (Boolean) -> Unit,
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
    val currentEligibilityChanged by rememberUpdatedState(onEligibilityChanged)
    val currentCommitted by rememberUpdatedState(onCommitted)
    val currentFinished by rememberUpdatedState(onFinished)

    LaunchedEffect(eligible) {
        currentEligibilityChanged(eligible)
        if (!eligible) {
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
        val activationPx = with(density) { ACTIVATION_HEIGHT.toPx() }
        val velocityThresholdPx = COMMIT_VELOCITY_DP_PER_SECOND * densityScale
        val fullHeight = size.height.toFloat().coerceAtLeast(1f)
        awaitEachGesture {
            val down = awaitFirstDown(
                requireUnconsumed = false,
                pass = PointerEventPass.Initial,
            )
            if (commitInFlight[0] || down.position.y < fullHeight - activationPx) {
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

                if (!dragging && -total.y > viewConfiguration.touchSlop) {
                    dragging = true
                    Log.i(TAG, "bottom reveal gesture acquired")
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
                commitInFlight[0] = true
                currentCommitted()
                val launchVelocity = upwardVelocity.coerceAtMost(fullHeight * 4f)
                settleJob[0] = animationScope.launch {
                    coroutineScope {
                        launch {
                            // The critical spring normally resolves in about 300ms;
                            // collapse refraction over its final visible portion.
                            kotlinx.coroutines.delay(190)
                            Animatable(effectStrength).animateTo(0f, tween(110)) {
                                effectStrength = value
                            }
                        }
                        Animatable(displacement).animateTo(
                            targetValue = fullHeight,
                            animationSpec = spring(
                                dampingRatio = Spring.DampingRatioNoBouncy,
                                stiffness = 420f,
                            ),
                            initialVelocity = launchVelocity,
                        ) {
                            displacement = value.coerceIn(0f, fullHeight)
                        }
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
    // opaque layer moves, so the same finger coordinates stay stable.
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
                enter = fadeIn(tween(260)),
                exit = fadeOut(tween(120)),
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
    Column(
        modifier = Modifier
            .navigationBarsPadding()
            .padding(bottom = 16.dp),
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
    val maxChromaPx = with(density) { MAX_CHROMA.toPx() }
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
            shader.setFloatUniform("maxChroma", maxChromaPx)
            renderEffect = effect
        } else {
            renderEffect = null
        }
    }
}

@RequiresApi(33)
private fun createVeilMotionEffect(shader: RuntimeShader) =
    RenderEffect.createRuntimeShaderEffect(shader, "inputShader").asComposeRenderEffect()

private const val VEIL_MOTION_SHADER = """
uniform shader inputShader;
uniform float2 size;
uniform float strength;
uniform float maxSmear;
uniform float maxChroma;

half4 main(float2 p) {
    half4 base = inputShader.eval(p);
    float trailing = smoothstep(size.y * 0.30, size.y, p.y);
    float amount = clamp(strength * trailing, 0.0, 1.0);
    if (amount <= 0.001) return base;

    float smear = maxSmear * amount;
    half4 s1 = inputShader.eval(p - float2(0.0, smear * 0.28));
    half4 s2 = inputShader.eval(p - float2(0.0, smear * 0.58));
    half4 s3 = inputShader.eval(p - float2(0.0, smear));
    half4 blurred = (base + s1 + s2 + s3) * 0.25;

    float chroma = maxChroma * amount;
    half red = inputShader.eval(p - float2(0.0, chroma)).r;
    half blue = inputShader.eval(p + float2(0.0, chroma * 0.45)).b;
    half3 refracted = half3(red, base.g, blue);
    half3 treated = mix(blurred.rgb, refracted, half(0.22));
    return half4(mix(base.rgb, treated, half(amount * 0.56)),
                 max(base.a, blurred.a));
}
"""
