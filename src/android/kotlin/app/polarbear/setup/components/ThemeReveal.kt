package app.polarbear.setup.components

// Appearance change as a bloom from the fingertip. The screen as it was is
// frozen into one bitmap; the new theme is painted underneath immediately and
// the frozen frame is carved away by a circle growing from the touch point.
// The carved edge is not a hard line: just ahead of it the old frame thaws
// into frost (a progressive blur), and the whole old frame drifts back a
// fraction as though the new surface were rising through it.
//
// The capture comes from the same display list that is drawn, overlay
// included, so a second tap mid-bloom freezes exactly what is on screen.

import android.animation.ValueAnimator
import android.os.Build
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.isSpecified
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.BlurEffect
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.TileMode
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.layer.CompositingStrategy
import androidx.compose.ui.graphics.layer.GraphicsLayer
import androidx.compose.ui.graphics.layer.drawLayer
import androidx.compose.ui.graphics.rememberGraphicsLayer
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlin.math.ceil
import kotlin.math.hypot
import kotlin.math.max

private const val REVEAL_MS = 940
private const val FROST_SCALE = 0.25f
// Blur in quarter-resolution pixels: 20px on screen.
private const val FROST_BLUR = 5f

@Stable
class ThemeRevealState internal constructor(
    internal val capture: GraphicsLayer,
    internal val sharp: GraphicsLayer,
    internal val frost: GraphicsLayer,
    internal val frostMasked: GraphicsLayer,
) {
    internal var snapshot by mutableStateOf<ImageBitmap?>(null)
    internal var origin = Offset.Unspecified
    internal val progress = Animatable(0f)
    internal val frostEffect = BlurEffect(FROST_BLUR, FROST_BLUR, TileMode.Clamp)
    private var job: Job? = null

    /**
     * Freezes the current frame, runs [apply] (which repaints the tree in the
     * new theme) and blooms the new theme out from [from], in root pixels.
     */
    fun play(scope: CoroutineScope, from: Offset, apply: () -> Unit) {
        job?.cancel()
        job = scope.launch {
            val shot = if (ValueAnimator.areAnimatorsEnabled()) {
                runCatching { capture.toImageBitmap() }.getOrNull()
            } else {
                null
            }
            if (shot == null) {
                snapshot = null
                apply()
                return@launch
            }
            origin = from
            progress.snapTo(0f)
            snapshot = shot
            apply()
            progress.animateTo(1f, tween(REVEAL_MS, easing = PortalEmphasized))
            snapshot = null
        }
    }
}

@Composable
fun rememberThemeReveal(): ThemeRevealState {
    val capture = rememberGraphicsLayer()
    val sharp = rememberGraphicsLayer()
    val frost = rememberGraphicsLayer()
    val frostMasked = rememberGraphicsLayer()
    return remember { ThemeRevealState(capture, sharp, frost, frostMasked) }
}

/** Apply to the root of the themed tree. */
fun Modifier.themeReveal(state: ThemeRevealState): Modifier = drawWithContent {
    // Overlay layers are recorded first, from this scope: recording a layer
    // from inside another layer's record block corrupts the draw scope.
    val overlay = prepareOldTheme(state)
    state.capture.record {
        this@drawWithContent.drawContent()
        if (overlay != null) {
            scale(overlay.recede, overlay.recede, pivot = overlay.origin) {
                if (overlay.frosted) {
                    scale(1f / FROST_SCALE, 1f / FROST_SCALE, pivot = Offset.Zero) {
                        drawLayer(state.frostMasked)
                    }
                }
                drawLayer(state.sharp)
            }
        }
    }
    drawLayer(state.capture)
}

private class OldThemeOverlay(val origin: Offset, val recede: Float, val frosted: Boolean)

private fun DrawScope.prepareOldTheme(state: ThemeRevealState): OldThemeOverlay? {
    val shot = state.snapshot ?: return null
    val t = state.progress.value
    val o = if (state.origin.isSpecified) state.origin else center
    val far = maxOf(
        hypot(o.x, o.y),
        hypot(size.width - o.x, o.y),
        hypot(o.x, size.height - o.y),
        hypot(size.width - o.x, size.height - o.y),
    )
    val band = 110.dp.toPx()
    // Starts just inside zero so the first frame is still entirely the old
    // theme; ends a full band past the farthest corner.
    val r = -band * 0.3f + t * (far + band * 1.8f)
    val frosted = Build.VERSION.SDK_INT >= 31
    if (frosted) {
        val q = FROST_SCALE
        val small = IntSize(ceil(size.width * q).toInt(), ceil(size.height * q).toInt())
        state.frost.renderEffect = state.frostEffect
        state.frost.record(size = small) { drawImage(shot, dstSize = small) }
        state.frostMasked.compositingStrategy = CompositingStrategy.Offscreen
        state.frostMasked.record(size = small) {
            drawLayer(state.frost)
            drawRect(
                brush = ringMask(o * q, (r - band * 0.35f) * q, (r + band * 0.2f) * q),
                blendMode = BlendMode.DstIn,
            )
        }
    }
    state.sharp.compositingStrategy = CompositingStrategy.Offscreen
    state.sharp.record {
        drawImage(shot)
        drawRect(
            brush = ringMask(o, r + band * 0.1f, r + band * 1.25f),
            blendMode = BlendMode.DstIn,
        )
    }
    // The old surface recedes a touch as the new one comes through it.
    return OldThemeOverlay(o, 1f + 0.018f * t, frosted)
}

/** Clear inside [inner], opaque beyond [outer], eased in between. */
private fun ringMask(center: Offset, inner: Float, outer: Float): Brush {
    if (outer <= 1f) return Brush.linearGradient(listOf(Color.Black, Color.Black))
    val start = (max(inner, 0f) / outer).coerceIn(0f, 0.999f)
    val span = 1f - start
    return Brush.radialGradient(
        0f to Color.Transparent,
        start to Color.Transparent,
        start + span * 0.35f to Color.Black.copy(alpha = 0.22f),
        start + span * 0.7f to Color.Black.copy(alpha = 0.7f),
        1f to Color.Black,
        center = center,
        radius = outer,
        tileMode = TileMode.Clamp,
    )
}
