package app.polarbear

import android.os.SystemClock
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.VectorConverter
import androidx.compose.animation.core.spring
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.IntSize
import kotlinx.coroutines.*

/** Free floating, bounded movement; separate from the pill's edge docking. */
@Stable
internal class UtilityKeyboardMotion(private val scope: CoroutineScope) {
    private val position = Animatable(Offset.Zero, Offset.VectorConverter)
    private var initialized = false
    private var settling: Job? = null
    private var dragPosition by mutableStateOf(Offset.Zero)
    var dragging by mutableStateOf(false); private set
    val isMoving get() = dragging || position.isRunning
    private var size = IntSize.Zero
    private var area = UtilityMenuGeometry.Area(0f,0f,0f,0f)
    private var density = 1f
    private fun bound(value: Offset): Offset {
        val x = minOf(size.width / 2f + 12 * density, area.width / 2)
        val y = minOf(size.height / 2f + 12 * density, area.height / 2)
        return Offset(value.x.coerceIn(area.left+x, area.right-x), value.y.coerceIn(area.top+y, area.bottom-y))
    }
    private fun initialCenter() = bound(Offset((area.left+area.right)/2, area.bottom-size.height/2f-20*density))
    fun center(): Offset = if (dragging) dragPosition else if (initialized) position.value else initialCenter()
    fun updateArea(next: UtilityMenuGeometry.Area, scale: Float) {
        if (area == next && density == scale) return
        val current = center()
        area = next; density = scale
        if (initialized) relocate(current, bound(current))
    }
    fun resize(next: IntSize) {
        if (size == next) return
        val current = center()
        size = next
        val target = if (initialized) bound(current) else initialCenter()
        relocate(if (initialized) current else target, target)
    }
    private fun relocate(current: Offset, target: Offset) {
        settling?.cancel()
        settling = scope.launch(start = CoroutineStart.UNDISPATCHED) {
            position.snapTo(current); initialized = true; dragging = false
            position.animateTo(target, spring(dampingRatio = 1f, stiffness = 230f))
        }
    }
    fun stop() {
        val current = center()
        settling?.cancel()
        if (dragging) settling = scope.launch(start = CoroutineStart.UNDISPATCHED) {
            position.snapTo(current); initialized = true; dragging = false
        }
    }
    fun grip(enabled: Boolean): Modifier = Modifier.onGloballyPositioned { coordinates = it }.semantics {
        if (enabled) customActions = listOf(
            "Move left" to Offset(-64f,0f), "Move right" to Offset(64f,0f),
            "Move up" to Offset(0f,-64f), "Move down" to Offset(0f,64f),
        ).map { (label,delta) -> CustomAccessibilityAction(label) { moveBy(delta*density); true } }
    }.pointerInput(enabled) {
        if (!enabled) return@pointerInput
        val velocity = VelocityTracker()
        var origin = Offset.Zero
        var finger = Offset.Zero
        fun root(local: Offset) = coordinates?.takeIf { it.isAttached }?.localToRoot(local) ?: local
        detectDragGestures(
            onDragStart = { local ->
                val current = center()
                settling?.cancel()
                origin = current; finger = root(local)
                dragPosition = current; dragging = true
                velocity.resetTracking(); velocity.addPosition(SystemClock.uptimeMillis(), finger)
            },
            onDrag = { change, _ ->
                val current = root(change.position)
                velocity.addPosition(change.uptimeMillis, current)
                dragPosition = bound(origin + current-finger)
                change.consume()
            },
            onDragCancel = { settle(Offset.Zero) },
            onDragEnd = {
                val v = velocity.calculateVelocity()
                settle(Offset(v.x,v.y))
            },
        )
    }
    private var coordinates: LayoutCoordinates? = null
    private fun moveBy(delta: Offset) {
        val target = bound(center()+delta)
        settling?.cancel()
        settling = scope.launch {
            position.snapTo(center()); initialized = true; dragging = false
            position.animateTo(target, spring(dampingRatio = 1f, stiffness = 160f))
        }
    }
    private fun settle(velocity: Offset) {
        val current = center()
        val v = Offset(velocity.x.coerceIn(-2200*density,2200*density), velocity.y.coerceIn(-1800*density,1800*density))
        val target = bound(current + v * 0.16f)
        // Near an edge, projected momentum must not throw the surface past
        // its bounds. A critically damped spring can absorb this much speed.
        fun arrivalSpeed(speed: Float, distance: Float) = if (speed * distance <= 0f) 0f
            else speed.coerceIn(-kotlin.math.abs(distance) * 12f, kotlin.math.abs(distance) * 12f)
        val arrival = Offset(arrivalSpeed(v.x, target.x-current.x), arrivalSpeed(v.y, target.y-current.y))
        settling?.cancel()
        settling = scope.launch {
            position.snapTo(current)
            initialized = true; dragging = false
            position.animateTo(target, spring(dampingRatio = 1f, stiffness = 160f), initialVelocity = arrival)
        }
    }
}
