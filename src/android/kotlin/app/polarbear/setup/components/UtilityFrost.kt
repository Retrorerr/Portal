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
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
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

/**
 * Capture actual rendered windows, with a small crop while the edge tab is stationary. A parked
 * tab resamples its content every 2 s: over a changing desktop (video, chat) the 300 ms sample
 * plus 100 ms fade kept Portal's own window redrawing nearly every frame. Layout changes wake
 * the sampler at once, so a rotation or resize never leaves stale glass behind.
 */
@Composable
internal fun rememberUtilityFrost(active: Boolean, crop: () -> Rect?, moving: () -> Boolean, parked: () -> Boolean): UtilityFrost {
    val view = LocalView.current
    val lifecycle = (LocalContext.current as? LifecycleOwner)?.lifecycle
    val requestedCrop by rememberUpdatedState(crop)
    val movingNow by rememberUpdatedState(moving)
    val parkedNow by rememberUpdatedState(parked)
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
        var lastGeometry: List<Any?>? = null
        var sampledAt = 0L
        var sampledFrames = 0L
        // Ticks and state changes arrive on one conflated channel: a
        // withTimeoutOrNull + snapshotFlow per tick threw and unwound a
        // timeout exception and rebuilt a snapshot observer three times a
        // second on an idle desktop.
        val wake = Channel<Unit>(Channel.CONFLATED)
        // Rotation, window and surface resizes all relayout these views, so a
        // parked tab learns of new geometry without polling for it.
        val relayout = android.view.View.OnLayoutChangeListener { _, l, t, r, b, ol, ot, or, ob ->
            if (l != ol || t != ot || r != or || b != ob) wake.trySend(Unit)
        }
        val root = view.rootView
        root.addOnLayoutChangeListener(relayout)
        surface.addOnLayoutChangeListener(relayout)
        launch {
            try { awaitCancellation() } finally {
                root.removeOnLayoutChangeListener(relayout)
                surface.removeOnLayoutChangeListener(relayout)
            }
        }
        launch {
            var previous: Pair<Boolean,Boolean>? = null
            snapshotFlow { movingNow() to parkedNow() }.collectLatest { state ->
                val last = previous
                previous = state
                // Wake at once when motion ends or the tab wakes; parking
                // itself needs no fresh sample. Starting motion waits for its
                // first tick: the cropped sample's padding covers those frames,
                // and the full copy stays off the frame that starts a drag.
                if (last != null && !state.first && (last.first || (last.second && !state.second))) wake.trySend(Unit)
                // Moving content refreshes at 30 Hz, an open surface every
                // 300 ms and a parked tab every 2 s. Each Compose resume also
                // schedules a frame, so the parked tab must not tick faster.
                val interval = if (state.first) 32L else if (state.second) 2_000L else 300L
                while (true) { delay(interval); wake.trySend(Unit) }
            }
        }
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
            val geometry = listOf(wanted,width,height,dx,dy)
            val now = android.os.SystemClock.uptimeMillis()
            // Copy only once the desktop presented a new frame: PixelCopy runs
            // on the UI's RenderThread, and 30 full-desktop copies a second
            // over a still desktop delayed the dragged control's own frames.
            // An unknown count (0) samples on every tick as before.
            val frames = app.polarbear.SessionControls.desktopFrames()
            val changed = frames == 0L || frames != sampledFrames
            val due = geometry != lastGeometry || changed && (!parkedNow() || now-sampledAt >= 1_900L)
            val source = if (wanted == null) AndroidRect(0,0,width,height) else AndroidRect(
                floor(wanted.left-dx-CAPTURE_PADDING).toInt().coerceIn(0,width.coerceAtLeast(0)),
                floor(wanted.top-dy-CAPTURE_PADDING).toInt().coerceIn(0,height.coerceAtLeast(0)),
                ceil(wanted.right-dx+CAPTURE_PADDING).toInt().coerceIn(0,width.coerceAtLeast(0)),
                ceil(wanted.bottom-dy+CAPTURE_PADDING).toInt().coerceIn(0,height.coerceAtLeast(0)))
            if (due && source.width() >= FROST_SCALE*2 && source.height() >= FROST_SCALE*2 && surface.holder.surface?.isValid == true) {
                sampledAt = now
                val w = (source.width()/FROST_SCALE).coerceAtLeast(2)
                val h = (source.height()/FROST_SCALE).coerceAtLeast(2)
                val bitmap = buffers[index]?.takeIf { it.width == w && it.height == h }
                    ?: Bitmap.createBitmap(w,h,Bitmap.Config.ARGB_8888).also { buffers[index] = it }
                val result = suspendCancellableCoroutine { continuation ->
                    try { PixelCopy.request(surface,source,bitmap,{ continuation.resume(it) },handler) }
                    catch (_: IllegalArgumentException) { continuation.resume(PixelCopy.ERROR_SOURCE_INVALID) }
                }
                if (result == PixelCopy.SUCCESS) {
                    // Read before the copy: a frame queued meanwhile copies again.
                    lastGeometry = geometry; sampledFrames = frames
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
            wake.receive()
        }
    }
    LaunchedEffect(frost) {
        snapshotFlow { frame.value }.collectLatest { next ->
            val outgoing = frost.current
            val fade = next != null && outgoing != null && !movingNow()
            frost.previous = if (fade) outgoing else null
            frost.current = next
            frost.blend.snapTo(if (fade) 0f else 1f)
            if (fade) { frost.blend.animateTo(1f,tween(100)); frost.previous = null }
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
