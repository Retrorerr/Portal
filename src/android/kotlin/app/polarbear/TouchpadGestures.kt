package app.polarbear

import kotlin.math.abs
import kotlin.math.hypot

/**
 * Touchpad gestures for contacts Android hands over raw (pointer capture
 * bypasses its own touchpad recognizer). Positions are millimetres and times
 * milliseconds, so it behaves the same on every touchpad. Free of Android
 * types so the host tests drive it directly.
 *
 * Taps follow libinput, the recognizer Plasma uses on Linux touchpads. A tap
 * presses the primary button and holds it for [TAP_GAP_MS]. A touch in that gap
 * that moves, or rests past [DRAG_HOLD_MS], drags with the same press; lifted
 * quickly it completes a double click. A tap is never a full click followed
 * by a fresh press, which Plasma would read as a double click.
 */
internal class TouchpadGestures(
    private val move: (Float, Float) -> Unit,
    private val button: (Int, Boolean) -> Unit,
    private val scroll: (Float, Float, Boolean) -> Unit,
    private val pinch: (PinchPhase, Float) -> Unit,
) {
    enum class PinchPhase { Begin, Update, End, Cancel }

    class Point(val x: Float, val y: Float) {
        operator fun minus(other: Point) = Point(x - other.x, y - other.y)
        fun midpoint(other: Point) = Point((x + other.x) / 2f, (y + other.y) / 2f)
        fun distance(other: Point = ORIGIN) = hypot(x - other.x, y - other.y)
    }

    private enum class Gesture { Idle, OneFinger, Move, TwoFingers, Scroll, Pinch, WaitForLift }

    /** The primary button a tap holds down. */
    private enum class Tap { None, Held, SecondTouch, Dragging }

    private var gesture = Gesture.Idle
    private var tap = Tap.None
    private var deadline = 0L
    private var tapButton: Int? = null
    private var startTime = 0L
    private var origin = ORIGIN
    private var previous = ORIGIN
    private var previousScrollTime = 0L
    private var startSpan = 1f
    private var pinchScale = 1f
    private var earliestTap = 0L
    private val physicalButtons = mutableSetOf<Int>()

    /** Uptime when [timeout] must run next, or 0. */
    val nextDeadline: Long get() = if (tap == Tap.Held || tap == Tap.SecondTouch) deadline else 0L

    fun startCapture(now: Long) {
        reset()
        // A click used to take control can leave its finger on the pad as
        // Android switches from processed to raw contacts.
        earliestTap = now + ACTIVATION_SETTLE_MS
    }

    fun reset() {
        cancel()
        physicalButtons.clear()
    }

    fun cancel() {
        endGesture(cancelled = true)
        releaseTap()
    }

    fun timeout(now: Long) {
        if (nextDeadline == 0L || now < deadline) return
        if (tap == Tap.Held) releaseTap() else tap = Tap.Dragging
        deadline = 0L
    }

    fun physicalButton(code: Int, pressed: Boolean) {
        if (pressed) {
            // A physical click ends any tap; the click itself is reported by the caller.
            physicalButtons.add(code)
            releaseTap()
            tapButton = null
        } else {
            physicalButtons.remove(code)
        }
    }

    fun down(time: Long, at: Point, buttonsDown: Boolean) {
        endGesture(cancelled = true)
        startTime = time
        origin = at
        previous = at
        gesture = Gesture.OneFinger
        val allowTap = time >= earliestTap && !buttonsDown && physicalButtons.isEmpty()
        tapButton = if (allowTap) PRIMARY else null
        if (tap == Tap.Held) {
            if (allowTap && time <= deadline) {
                tap = Tap.SecondTouch
                deadline = time + DRAG_HOLD_MS
            } else {
                releaseTap()
            }
        }
    }

    fun secondFinger(time: Long, first: Point, second: Point) {
        val wasTap = tapButton == PRIMARY
        val canTrackTwo = gesture == Gesture.OneFinger || gesture == Gesture.Move
        endGesture(cancelled = true)
        releaseTap()
        if (canTrackTwo && physicalButtons.isEmpty()) {
            origin = first.midpoint(second)
            previous = origin
            previousScrollTime = time
            startSpan = first.distance(second).coerceAtLeast(1f)
            tapButton = if (wasTap) SECONDARY else null
            gesture = Gesture.TwoFingers
        } else {
            gesture = Gesture.WaitForLift
        }
    }

    /** More than two fingers: none of the gestures above. */
    fun extraFinger() {
        endGesture(cancelled = true)
        releaseTap()
        gesture = Gesture.WaitForLift
    }

    fun moveOne(at: Point) {
        when (gesture) {
            Gesture.OneFinger, Gesture.Move -> {
                if (gesture == Gesture.OneFinger) {
                    val slop = if (physicalButtons.isEmpty()) TAP_SLOP_MM else 0f
                    if ((at - origin).distance() <= slop) return
                    tapButton = null
                    gesture = Gesture.Move
                    if (tap == Tap.SecondTouch) {
                        tap = Tap.Dragging
                        deadline = 0L
                    }
                }
                val dx = (at.x - previous.x) * POINTER_PIXELS_PER_MM
                val dy = (at.y - previous.y) * POINTER_PIXELS_PER_MM
                previous = at
                if (dx != 0f || dy != 0f) move(dx, dy)
            }
            Gesture.WaitForLift -> {
                if (tapButton != null && (at - origin).distance() > TAP_SLOP_MM) tapButton = null
            }
            else -> Unit
        }
    }

    fun moveTwo(time: Long, first: Point, second: Point) {
        if (gesture != Gesture.TwoFingers && gesture != Gesture.Scroll && gesture != Gesture.Pinch) return
        val center = first.midpoint(second)
        val span = first.distance(second)
        if (gesture == Gesture.TwoFingers) {
            val travel = (center - origin).distance()
            val spread = abs(span - startSpan)
            if (spread > TAP_SLOP_MM && spread > travel * 1.6f) {
                gesture = Gesture.Pinch
                pinch(PinchPhase.Begin, 1f)
            } else if (travel > TAP_SLOP_MM) {
                gesture = Gesture.Scroll
            } else {
                return
            }
            tapButton = null
        }
        if (gesture == Gesture.Pinch) {
            pinchScale = (span / startSpan).coerceIn(0.05f, 20f)
            pinch(PinchPhase.Update, pinchScale)
        } else {
            val elapsed = (time - previousScrollTime).coerceAtLeast(1L)
            val delta = center - previous
            if (delta.x != 0f || delta.y != 0f) {
                // Slow movements stay precise; fast flicks travel further.
                val speed = delta.distance() * 1000f / elapsed
                val gain = (SLOW_SCROLL_PIXELS_PER_MM + (speed - SCROLL_ACCELERATION_START).coerceAtLeast(0f) *
                    SCROLL_ACCELERATION).coerceAtMost(FAST_SCROLL_PIXELS_PER_MM)
                // Signed like Android's own two-finger scroll distance, which
                // runs against the fingers, so both ways in scroll alike.
                scroll(-delta.x * gain, -delta.y * gain, false)
            }
            previousScrollTime = time
        }
        previous = center
    }

    /** A finger lifted and [remaining] (one contact) stays down. */
    fun fingerLifted(remaining: Point) {
        // A two-finger tap waits for the last finger; a scroll or pinch ends
        // here, and the remaining finger must lift before it points again.
        val click = tapButton.takeIf { gesture == Gesture.TwoFingers }
        endGesture()
        tapButton = click
        origin = remaining
        gesture = Gesture.WaitForLift
    }

    fun up(time: Long) {
        val quick = time - startTime in 0..TAP_TIME_MS
        val click = tapButton.takeIf { quick }
        when (tap) {
            Tap.SecondTouch -> {
                // Lifted without moving: the first tap's click ends and, when
                // this was a tap too, the second click begins.
                button(PRIMARY, false)
                tap = Tap.None
                if (click == PRIMARY) holdTap(time)
            }
            Tap.Dragging -> releaseTap()
            Tap.Held -> releaseTap()
            Tap.None -> when (click) {
                PRIMARY -> holdTap(time)
                SECONDARY -> {
                    button(SECONDARY, true)
                    button(SECONDARY, false)
                }
                else -> Unit
            }
        }
        endGesture()
    }

    private fun holdTap(time: Long) {
        button(PRIMARY, true)
        tap = Tap.Held
        deadline = time + TAP_GAP_MS
    }

    private fun releaseTap() {
        if (tap != Tap.None) button(PRIMARY, false)
        tap = Tap.None
        deadline = 0L
    }

    private fun endGesture(cancelled: Boolean = false) {
        when (gesture) {
            Gesture.Scroll -> scroll(0f, 0f, true)
            Gesture.Pinch -> pinch(if (cancelled) PinchPhase.Cancel else PinchPhase.End, pinchScale)
            else -> Unit
        }
        gesture = Gesture.Idle
        tapButton = null
        pinchScale = 1f
    }

    internal companion object {
        val ORIGIN = Point(0f, 0f)
        const val PRIMARY = 0x110
        const val SECONDARY = 0x111
        /** Longest touch that still counts as a tap. */
        const val TAP_TIME_MS = 220L
        /** How long a tap's press waits for a second touch. */
        const val TAP_GAP_MS = 200L
        /** A second touch resting this long drags without moving. */
        const val DRAG_HOLD_MS = 200L
        const val ACTIVATION_SETTLE_MS = 110L
        const val TAP_SLOP_MM = 1.5f
        const val POINTER_PIXELS_PER_MM = 19f
        const val SLOW_SCROLL_PIXELS_PER_MM = 11f
        const val FAST_SCROLL_PIXELS_PER_MM = 36f
        const val SCROLL_ACCELERATION_START = 65f // mm/s
        const val SCROLL_ACCELERATION = 0.27f
    }
}
