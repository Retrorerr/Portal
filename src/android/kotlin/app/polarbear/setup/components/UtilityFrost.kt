package app.polarbear.setup.components

import android.graphics.Bitmap
import android.graphics.Rect as AndroidRect
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.view.PixelCopy
import androidx.compose.runtime.*
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.BlurEffect
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.TileMode
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.layer.GraphicsLayer
import androidx.compose.ui.graphics.layer.drawLayer
import androidx.compose.ui.graphics.rememberGraphicsLayer
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.IntSize
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeoutOrNull
import kotlin.coroutines.resume
import kotlin.math.ceil
import kotlin.math.floor

private const val FROST_SCALE = PortalFrostStyle.SnapshotDivisor
private const val FROST_RADIUS = PortalFrostStyle.StrongBlur
private val CAPTURE_PADDING = (FROST_RADIUS*FROST_SCALE*3).toInt()

internal data class UtilityFrostFrame(val image: ImageBitmap, val region: Rect)
private data class FrostTransform(val origin: Offset, val sx: Float, val sy: Float)

/** One desktop sample and two reusable blur layers shared by all utility surfaces. */
@Stable
internal class UtilityFrost(
    private val previousLayer: GraphicsLayer,
    private val currentLayer: GraphicsLayer,
) {
    var previous by mutableStateOf<UtilityFrostFrame?>(null)
    var current by mutableStateOf<UtilityFrostFrame?>(null)
    val blend = Animatable(1f)
    private var recordedPrevious: UtilityFrostFrame? = null
    private var recordedCurrent: UtilityFrostFrame? = null
    private val blur = if (Build.VERSION.SDK_INT >= 31) BlurEffect(FROST_RADIUS,FROST_RADIUS,TileMode.Clamp) else null
    private val vibrancy = PortalFrostStyle.vibrancy

    fun record(scope: DrawScope) = with(scope) {
        previous?.let { frame -> if (recordedPrevious !== frame) {
            previousLayer.renderEffect = blur
            previousLayer.record(size = IntSize(frame.image.width,frame.image.height)) { drawImage(frame.image,colorFilter = vibrancy) }
            recordedPrevious = frame
        } }
        current?.let { frame -> if (recordedCurrent !== frame) {
            currentLayer.renderEffect = blur
            currentLayer.record(size = IntSize(frame.image.width,frame.image.height)) { drawImage(frame.image,colorFilter = vibrancy) }
            recordedCurrent = frame
        } }
        previousLayer.alpha = 1f
        currentLayer.alpha = if (previous == null) 1f else blend.value
    }

    fun draw(scope: DrawScope, origin: Offset, sx: Float, sy: Float) = with(scope) {
        if (sx < 0.01f || sy < 0.01f) return@with
        fun drawFrame(frame: UtilityFrostFrame?,layer: GraphicsLayer) {
            if (frame == null) return
            translate((frame.region.left-origin.x)/sx,(frame.region.top-origin.y)/sy) {
                scale(frame.region.width/frame.image.width/sx,frame.region.height/frame.image.height/sy,Offset.Zero) {
                    // Never extend a stale cropped sample across newly exposed pixels.
                    clipRect(0f,0f,frame.image.width.toFloat(),frame.image.height.toFloat()) { drawLayer(layer) }
                }
            }
        }
        clipRect {
            drawFrame(previous,previousLayer)
            drawFrame(current,currentLayer)
        }
    }
}

/** Capture actual rendered windows, with a small crop while the edge tab is stationary. */
@Composable
internal fun rememberUtilityFrost(active: Boolean, crop: () -> Rect?, moving: () -> Boolean): UtilityFrost {
    val view = LocalView.current
    val lifecycle = (LocalContext.current as? LifecycleOwner)?.lifecycle
    val requestedCrop by rememberUpdatedState(crop)
    val movingNow by rememberUpdatedState(moving)
    val frame = remember { mutableStateOf<UtilityFrostFrame?>(null) }
    val oldLayer = rememberGraphicsLayer()
    val newLayer = rememberGraphicsLayer()
    val frost = remember { UtilityFrost(oldLayer,newLayer) }
    LaunchedEffect(active) {
        if (!active || Build.VERSION.SDK_INT < 31) { frame.value = null; return@LaunchedEffect }
        val surface = findDesktopSurface(view.rootView) ?: return@LaunchedEffect
        val handler = Handler(Looper.getMainLooper())
        val buffers = arrayOfNulls<Bitmap>(2)
        var index = 0
        var last: Bitmap? = null
        var lastRegion: Rect? = null
        while (isActive) {
            if (lifecycle != null && !lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) {
                delay(500); continue
            }
            val width = surface.width
            val height = surface.height
            val sourceOrigin = IntArray(2).also { surface.getLocationInWindow(it) }
            val rootOrigin = IntArray(2).also { view.getLocationInWindow(it) }
            val dx = sourceOrigin[0]-rootOrigin[0]
            val dy = sourceOrigin[1]-rootOrigin[1]
            val wanted = requestedCrop()
            val source = if (wanted == null) AndroidRect(0,0,width,height) else AndroidRect(
                floor(wanted.left-dx-CAPTURE_PADDING).toInt().coerceIn(0,width.coerceAtLeast(0)),
                floor(wanted.top-dy-CAPTURE_PADDING).toInt().coerceIn(0,height.coerceAtLeast(0)),
                ceil(wanted.right-dx+CAPTURE_PADDING).toInt().coerceIn(0,width.coerceAtLeast(0)),
                ceil(wanted.bottom-dy+CAPTURE_PADDING).toInt().coerceIn(0,height.coerceAtLeast(0)))
            if (source.width() >= FROST_SCALE*2 && source.height() >= FROST_SCALE*2 && surface.holder.surface?.isValid == true) {
                val w = (source.width()/FROST_SCALE).coerceAtLeast(2)
                val h = (source.height()/FROST_SCALE).coerceAtLeast(2)
                val bitmap = buffers[index]?.takeIf { it.width == w && it.height == h }
                    ?: Bitmap.createBitmap(w,h,Bitmap.Config.ARGB_8888).also { buffers[index] = it }
                val result = suspendCancellableCoroutine { continuation ->
                    try { PixelCopy.request(surface,source,bitmap,{ continuation.resume(it) },handler) }
                    catch (_: IllegalArgumentException) { continuation.resume(PixelCopy.ERROR_SOURCE_INVALID) }
                }
                if (result == PixelCopy.SUCCESS) {
                    val region = Rect((source.left+dx).toFloat(),(source.top+dy).toFloat(),(source.right+dx).toFloat(),(source.bottom+dy).toFloat())
                    // Static desktops incur no texture upload or animation between samples.
                    if (lastRegion != region || last?.sameAs(bitmap) != true) {
                        bitmap.prepareToDraw()
                        frame.value = UtilityFrostFrame(bitmap.asImageBitmap(),region)
                        last = bitmap; lastRegion = region; index = 1-index
                    }
                }
                // A transient PixelCopy failure must not flash away the glass.
                // Keep the last successful sample and retry on the next tick.
            }
            val moving = movingNow()
            // Wake immediately when a drag starts, then refresh moving
            // content at 30 Hz. Spatial sampling redraws at display rate.
            withTimeoutOrNull(if (moving) 32L else 300L) {
                snapshotFlow { movingNow() }.first { it != moving }
            }
        }
    }
    LaunchedEffect(frost) {
        snapshotFlow { frame.value }.collectLatest { next ->
            frost.previous = if (movingNow()) null else frost.current
            frost.current = next
            frost.blend.snapTo(if (next != null && frost.previous != null) 0f else 1f)
            if (next != null && frost.previous != null) frost.blend.animateTo(1f,tween(100))
            frost.previous = null
        }
    }
    return frost
}

internal fun Modifier.recordUtilityFrost(frost: UtilityFrost) = drawWithContent {
    frost.record(this)
    drawContent()
}

@Composable
internal fun Modifier.utilityFrost(frost: UtilityFrost): Modifier {
    var transform by remember { mutableStateOf<FrostTransform?>(null) }
    return onGloballyPositioned { coordinates ->
        val origin = coordinates.localToRoot(Offset.Zero)
        transform = FrostTransform(origin, coordinates.localToRoot(Offset(1f,0f)).x-origin.x,
            coordinates.localToRoot(Offset(0f,1f)).y-origin.y)
    }.drawWithContent {
        // Placement alone reuses a layer's display list. Observing the
        // transform invalidates just this draw when a floating surface moves.
        transform?.let { frost.draw(this,it.origin,it.sx,it.sy) }
        drawContent()
    }
}
