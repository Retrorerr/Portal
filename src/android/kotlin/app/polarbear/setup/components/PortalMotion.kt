package app.polarbear.setup.components

// Shared Portal motion vocabulary: one easing family, one blur primitive and
// the veil's live motion state. Every blur is a GPU RenderEffect on API 31+
// and a silent no-op below it; nothing here allocates per frame.

import android.os.Build
import androidx.compose.animation.AnimatedVisibilityScope
import androidx.compose.animation.EnterExitState
import androidx.compose.animation.ExperimentalAnimationApi
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.tween
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.BlurEffect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.GraphicsLayerScope
import androidx.compose.ui.graphics.TileMode
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/** Material "emphasized" curves: a quick, confident start and a long, soft landing. */
val PortalEmphasized = CubicBezierEasing(0.2f, 0f, 0f, 1f)
val PortalEmphasizedDecelerate = CubicBezierEasing(0.05f, 0.7f, 0.1f, 1f)
val PortalEmphasizedAccelerate = CubicBezierEasing(0.3f, 0f, 0.8f, 0.15f)

/** Apply a Gaussian blur of [radiusPx] to this layer; clears the effect at ~0. */
fun GraphicsLayerScope.portalBlur(radiusPx: Float) {
    renderEffect = if (Build.VERSION.SDK_INT >= 31 && radiusPx > 0.4f) {
        BlurEffect(radiusPx, radiusPx, TileMode.Decal)
    } else {
        null
    }
}

/**
 * Blur-dissolve bound to an enter/exit transition: content condenses out of
 * a soft blur as it appears and melts back into one as it leaves. Pair it
 * with the usual fade so the two read as one gesture.
 */
@OptIn(ExperimentalAnimationApi::class)
@Composable
fun Modifier.dissolveBlur(
    scope: AnimatedVisibilityScope,
    radius: Dp = 10.dp,
    enterMillis: Int = 420,
    exitMillis: Int = 220,
): Modifier {
    val radiusPx = with(LocalDensity.current) { radius.toPx() }
    val amount by scope.transition.animateFloat(
        transitionSpec = {
            if (targetState == EnterExitState.Visible) {
                tween(enterMillis, easing = PortalEmphasizedDecelerate)
            } else {
                tween(exitMillis, easing = PortalEmphasizedAccelerate)
            }
        },
        label = "dissolve blur",
    ) { state -> if (state == EnterExitState.Visible) 0f else 1f }
    return this.graphicsLayer { portalBlur(amount * radiusPx) }
}

/**
 * Live state of the final reveal veil, shared with everything drawn inside
 * it. Values are snapshot state read only in draw/layout lambdas, so the
 * gesture never recomposes the screens beneath it.
 */
@Stable
class PortalVeilMotion {
    /** Finger-driven lift in px. */
    var gesture by mutableFloatStateOf(0f)

    /** Idle "breathing" lift in px that hints at the gesture. */
    var hint by mutableFloatStateOf(0f)

    /** Veil height in px (the viewport). */
    var height by mutableFloatStateOf(0f)

    /** True once the swipe may reveal the live desktop. */
    var eligible by mutableStateOf(false)

    /** Ink for the veil's own affordance, set by the screen beneath it. */
    var ink by mutableStateOf(Color(0xFFF1EBDD))

    /** Total visual lift in px. */
    val lift: Float get() = (gesture + hint).coerceAtLeast(0f)

    /** Lift as a fraction of the veil height. */
    val progress: Float get() = if (height > 0f) (lift / height).coerceIn(0f, 1f) else 0f

    /**
     * Height of the leading edge's progressive feather: zero at rest, so the
     * resting veil is untouched, growing with the lift to a quarter screen.
     */
    val featherBand: Float get() = minOf(lift * 0.9f, height * 0.22f)
}

val LocalPortalVeil = staticCompositionLocalOf { PortalVeilMotion() }
