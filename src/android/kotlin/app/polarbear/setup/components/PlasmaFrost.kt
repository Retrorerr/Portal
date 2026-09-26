package app.polarbear.setup.components

// Frosted glass over the live desktop.
//
// The veil is a View above the native SurfaceView in the same window, so no
// View-side blur can reach the desktop's pixels: RenderEffect only filters a
// view's own content, and window blur-behind would need a second window.
// Instead Portal reads the SurfaceView's latest buffer with PixelCopy at 1/6
// resolution, a few times a second, and draws that copy inside the veil,
// blurred and counter-translated so it stays registered to the real desktop
// while the veil slides. The copy never includes the overlay itself.
//
// Blur is progressive: a strong frost across the body of the veil thins to
// a light frost over a band above the leading edge, where the veil's own
// alpha feather then hands off to the sharp live desktop. As the veil lifts,
// the whole glass thaws toward the light frost.

import android.graphics.Bitmap
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.PixelCopy
import android.view.SurfaceView
import android.view.View
import android.view.ViewGroup
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.BlurEffect
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.ColorMatrix
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.TileMode
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.layer.CompositingStrategy
import androidx.compose.ui.graphics.layer.drawLayer
import androidx.compose.ui.graphics.rememberGraphicsLayer
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume

private const val TAG = "PortalFrost"
private const val SNAPSHOT_DIVISOR = 6
private const val REFRESH_MS = 140L

// Blur radii in snapshot pixels (x6 on screen).
private const val STRONG_BLUR = 11f
private const val LIGHT_BLUR = 2.5f

/**
 * A low-resolution live copy of the native desktop, refreshed while [active].
 * Null until the first successful copy (e.g. no desktop frame yet).
 */
@Composable
fun rememberPlasmaSnapshot(active: Boolean): State<ImageBitmap?> {
    val view = LocalView.current
    val lifecycle = (LocalContext.current as? LifecycleOwner)?.lifecycle
    val snapshot = remember { mutableStateOf<ImageBitmap?>(null) }
    LaunchedEffect(active) {
        if (!active) {
            snapshot.value = null
            return@LaunchedEffect
        }
        if (Build.VERSION.SDK_INT < 24) return@LaunchedEffect
        val surface = findDesktopSurface(view.rootView)
        if (surface == null) {
            Log.w(TAG, "desktop SurfaceView not found; veil stays untinted")
            return@LaunchedEffect
        }
        val handler = Handler(Looper.getMainLooper())
        // Double-buffered: PixelCopy writes one bitmap while the other is drawn.
        val buffers = arrayOfNulls<Bitmap>(2)
        var index = 0
        var failureLogged = false
        var firstLogged = false
        while (isActive) {
            // Only copy while Portal is on screen; the last copy stays drawn.
            if (lifecycle != null && !lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) {
                delay(REFRESH_MS * 4)
                continue
            }
            val width = surface.width / SNAPSHOT_DIVISOR
            val height = surface.height / SNAPSHOT_DIVISOR
            if (width >= 8 && height >= 8 && surface.holder.surface?.isValid == true) {
                val existing = buffers[index]
                val bitmap = if (existing != null && existing.width == width && existing.height == height) {
                    existing
                } else {
                    Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888).also { buffers[index] = it }
                }
                val result = suspendCancellableCoroutine { continuation ->
                    try {
                        PixelCopy.request(surface, bitmap, { code -> continuation.resume(code) }, handler)
                    } catch (e: IllegalArgumentException) {
                        continuation.resume(PixelCopy.ERROR_SOURCE_INVALID)
                    }
                }
                if (result == PixelCopy.SUCCESS) {
                    // A fresh wrapper per copy: HWUI re-uploads the texture.
                    bitmap.prepareToDraw()
                    snapshot.value = bitmap.asImageBitmap()
                    index = 1 - index
                    if (!firstLogged) {
                        firstLogged = true
                        Log.i(TAG, "live desktop frost active (${width}x$height)")
                    }
                } else if (!failureLogged) {
                    failureLogged = true
                    Log.i(TAG, "desktop copy unavailable (code=$result); retrying quietly")
                }
            }
            delay(REFRESH_MS)
        }
    }
    return snapshot
}

private fun findDesktopSurface(root: View): SurfaceView? {
    if (root is SurfaceView) return root
    if (root is ViewGroup) {
        for (i in 0 until root.childCount) {
            findDesktopSurface(root.getChildAt(i))?.let { return it }
        }
    }
    return null
}

/**
 * Draws the frosted desktop copy. Place it inside the veil, beneath the
 * veil's tint. [opacity] fades the whole glass in and out.
 */
@Composable
fun PlasmaFrostBackdrop(
    snapshot: () -> ImageBitmap?,
    opacity: () -> Float,
) {
    if (Build.VERSION.SDK_INT < 31) return
    val veil = LocalPortalVeil.current
    val strong = rememberGraphicsLayer()
    val light = rememberGraphicsLayer()
    val lightMasked = rememberGraphicsLayer()
    val strongEffect = remember { BlurEffect(STRONG_BLUR, STRONG_BLUR, TileMode.Clamp) }
    val lightEffect = remember { BlurEffect(LIGHT_BLUR, LIGHT_BLUR, TileMode.Clamp) }
    // Gentle vibrancy: blurred colour reads as light through glass, not mud.
    val vibrancy = remember {
        ColorFilter.colorMatrix(ColorMatrix().apply { setToSaturation(1.35f) })
    }
    val softBandPx = with(LocalDensity.current) { 56.dp.toPx() }
    Canvas(Modifier.fillMaxSize()) {
        val image = snapshot() ?: return@Canvas
        val alpha = opacity().coerceIn(0f, 1f)
        if (alpha <= 0.001f) return@Canvas
        val imageSize = IntSize(image.width, image.height)
        val sx = size.width / image.width
        val sy = size.height / image.height
        val lift = veil.lift

        strong.renderEffect = strongEffect
        strong.record(size = imageSize) { drawImage(image, colorFilter = vibrancy) }
        light.renderEffect = lightEffect
        light.record(size = imageSize) { drawImage(image, colorFilter = vibrancy) }

        // Light frost fills the band above the leading edge and, as the veil
        // lifts, thaws the whole pane. Mask in snapshot space (cheap), after
        // the blur so the mask edge itself stays crisp in its falloff.
        val thaw = (veil.progress * 1.6f).coerceIn(0f, 0.85f)
        val band = veil.featherBand * 1.6f + softBandPx
        // The veil's leading edge (local y = height) in snapshot coordinates.
        val edge = (size.height - lift) / sy
        val bandTop = edge - band / sy
        lightMasked.compositingStrategy = CompositingStrategy.Offscreen
        lightMasked.record(size = imageSize) {
            drawLayer(light)
            drawRect(
                brush = Brush.verticalGradient(
                    0f to Color.Black.copy(alpha = thaw),
                    1f to Color.Black,
                    startY = bandTop,
                    endY = edge,
                ),
                blendMode = BlendMode.DstIn,
            )
        }
        strong.alpha = alpha
        lightMasked.alpha = alpha
        // Counter-translate so the copy stays registered to the real desktop.
        translate(top = lift) {
            scale(sx, sy, Offset.Zero) {
                drawLayer(strong)
                drawLayer(lightMasked)
            }
        }
    }
}
