package app.polarbear.gaming

import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min

/** What a touch did that a thumb should feel ([PadHaptics]). */
internal enum class PadFeel { Press, Step, Grab, Rim, Click }

/**
 * The on-screen controller's touch logic, from fingers to the pad the game
 * sees ([buttons], [axes] laid out as GamepadBridge's).
 *
 * Feel: a stick floats to the thumb and follows it past its rim, with a small
 * dead zone in the middle; a quick tap and then a hold on a stick clicks it
 * (L3, R3). The d-pad favours its four main directions and keeps a direction
 * until the thumb is clearly in the next. A thumb between two face buttons
 * presses both, and sliding across them moves the press.
 *
 * Fingers come from one or more sources, the views that caught them: on the
 * whole screen one view takes every finger, floating over the desktop each
 * control's own view does. A source's stream ends without touching the
 * others' fingers.
 *
 * Plain Kotlin, so the feel can be checked off the device.
 */
internal class PadInput(
    /** Physical px per millimetre. */
    private val mm: Float,
) {
    var layout: PadLayout? = null
    var analogTriggers = false

    /** Buttons in xpad order ([PadBits]). */
    var buttons = 0
        private set
    val axes = FloatArray(10)
    /** Face buttons held, bits 0..3 for A, B, X, Y. */
    var faceMask = 0
        private set
    var hatX = 0
        private set
    var hatY = 0
        private set

    val leftStick = Stick(Control.LEFT_STICK)
    val rightStick = Stick(Control.RIGHT_STICK)

    class Finger {
        var source: Any? = null
        var id = -1
        var spot: PadSpot? = null
        var x = 0f
        var y = 0f
        internal var downX = 0f
        internal var downY = 0f
        internal var downTime = 0L
        /** How far it has strayed from where it landed, px. */
        internal var travel = 0f
        /** Face buttons it holds, bits 0..3 for A, B, X, Y. */
        internal var faces = 0
        /** D-pad direction it holds, a sector clockwise from east, or -1. */
        internal var sector = -1
    }

    private val fingers = Array(MAX_FINGERS) { Finger() }
    var fingerCount = 0
        private set

    /** Finger [index] of [fingerCount], for drawing. */
    fun finger(index: Int): Finger = fingers[index]

    // What the last change did that a thumb should feel.
    private var grabbed = false
    private var reachedRim = false
    private var stickClicked = false

    /**
     * A finger [id] from [source] lands at ([x], [y]); false when it lands on
     * no control (floating, it then belongs to the desktop).
     */
    fun land(source: Any, id: Int, x: Float, y: Float, time: Long): Boolean {
        // Android reuses a lifted finger's id: one still tracked here lost its UP.
        lift(id, time, cancelled = true)
        val pad = layout ?: return false
        if (fingerCount == MAX_FINGERS) return false
        var spot = pad.pick(x, y)
        val stick = spot?.let { stickFor(it.control) }
        // A second thumb on a held stick is meant for nothing.
        if (stick != null && stick.held) spot = null
        spot ?: return false
        val finger = fingers[fingerCount++]
        finger.source = source
        finger.id = id
        finger.spot = spot
        finger.x = x; finger.y = y
        finger.downX = x; finger.downY = y
        finger.downTime = time
        finger.travel = 0f
        finger.faces = 0
        finger.sector = -1
        stick?.grab(id, x, y, time, spot, pad.drift(spot))
        return true
    }

    /** Finger [id] is at ([x], [y]) now; ignored unless it landed on a control. */
    fun move(id: Int, x: Float, y: Float) {
        val pad = layout ?: return
        val finger = find(id) ?: return
        val spot = finger.spot ?: return
        finger.x = x
        finger.y = y
        finger.travel = max(finger.travel, hypot(x - finger.downX, y - finger.downY))
        when (spot.control.kind) {
            ControlKind.STICK -> stickFor(spot.control)?.drag(x, y, spot, pad.drift(spot))
            ControlKind.DPAD -> finger.sector = dpadSector(spot, x, y, finger.sector)
            ControlKind.FACE -> finger.faces = faceButtons(pad, spot, x, y, finger.faces)
            else -> Unit
        }
    }

    /** Finger [id] lifts; [cancelled] when Android took it back, which never counts as a tap. */
    fun lift(id: Int, time: Long, cancelled: Boolean) {
        val index = indexOf(id)
        if (index < 0) return
        val finger = fingers[index]
        val spot = finger.spot
        if (spot != null && spot.control.kind == ControlKind.STICK) {
            val tap = !cancelled && time - finger.downTime <= TAP_MS && finger.travel <= TAP_SLOP_MM * mm
            stickFor(spot.control)?.letGo(id, time, tap, finger.x, finger.y)
        }
        // Keep the array dense: the last finger takes this one's place.
        val last = fingers[fingerCount - 1]
        fingers[fingerCount - 1] = finger
        fingers[index] = last
        finger.source = null
        finger.id = -1
        finger.spot = null
        fingerCount--
    }

    /** Every finger from [source] lets go, or every finger at all for null; none of them counts as a tap. */
    fun liftAll(source: Any?, time: Long) {
        for (index in fingerCount - 1 downTo 0) {
            val finger = fingers[index]
            if (source == null || finger.source === source) lift(finger.id, time, cancelled = true)
        }
        if (source == null) {
            leftStick.reset()
            rightStick.reset()
        }
    }

    /** Lets go of every finger from [source] that is not [present]: Android dropped its UP. */
    fun dropMissing(source: Any, time: Long, present: (Int) -> Boolean) {
        // From the end: a lift moves the last finger into the freed place.
        for (index in fingerCount - 1 downTo 0) {
            val finger = fingers[index]
            if (finger.source === source && !present(finger.id)) lift(finger.id, time, cancelled = true)
        }
    }

    private fun indexOf(id: Int): Int {
        for (index in 0 until fingerCount) if (fingers[index].id == id) return index
        return -1
    }

    private fun find(id: Int): Finger? = indexOf(id).takeIf { it >= 0 }?.let { fingers[it] }

    fun stickFor(control: Control): Stick? = when (control) {
        Control.LEFT_STICK -> leftStick
        Control.RIGHT_STICK -> rightStick
        else -> null
    }

    /**
     * The direction a thumb at ([x], [y]) holds on the d-pad [spot], as a
     * sector clockwise from east (screen y grows down), or -1 in its middle.
     * The four main directions are wider than the diagonals, and [held] stays
     * until the thumb is clearly in the next sector.
     */
    private fun dpadSector(spot: PadSpot, x: Float, y: Float, held: Int): Int {
        val dx = x - spot.x
        val dy = y - spot.y
        val distance = hypot(dx, dy)
        if (distance < spot.r * (if (held >= 0) DPAD_RELEASE else DPAD_DEAD)) return -1
        var angle = Math.toDegrees(atan2(dy, dx).toDouble()).toFloat()
        if (angle < 0f) angle += 360f
        if (held >= 0 && inSector(angle, held, sectorHalf(held) + DPAD_HYSTERESIS)) return held
        for (sector in 0 until 8) if (inSector(angle, sector, sectorHalf(sector))) return sector
        return held
    }

    private fun sectorHalf(sector: Int): Float = if (sector % 2 == 0) CARDINAL_HALF else DIAGONAL_HALF

    private fun inSector(angle: Float, sector: Int, half: Float): Boolean {
        var delta = abs(angle - sector * 45f) % 360f
        if (delta > 180f) delta = 360f - delta
        return delta <= half
    }

    /**
     * The face buttons a thumb at ([x], [y]) holds: the nearest, and its
     * neighbour as well when the thumb sits between the two. A held button
     * keeps the thumb a little longer, so a press does not flicker on the
     * line between two.
     */
    private fun faceButtons(pad: PadLayout, spot: PadSpot, x: Float, y: Float, held: Int): Int {
        val radius = spot.a
        // The diamond's middle is no button's.
        if (hypot(x - spot.x, y - spot.y) < spot.b * FACE_MIDDLE) return 0
        var nearest = -1
        var nearestDistance = Float.MAX_VALUE
        var second = -1
        var secondDistance = Float.MAX_VALUE
        for (index in 0 until 4) {
            var distance = hypot(x - pad.faceButtonX(spot, index), y - pad.faceButtonY(spot, index))
            if (held and (1 shl index) != 0) distance -= radius * FACE_KEEP
            if (distance < nearestDistance) {
                second = nearest; secondDistance = nearestDistance
                nearest = index; nearestDistance = distance
            } else if (distance < secondDistance) {
                second = index; secondDistance = distance
            }
        }
        // Slid off the cluster altogether.
        if (nearest < 0 || nearestDistance > radius + spot.reach + radius * FACE_SLIDE_OFF) return 0
        var mask = 1 shl nearest
        if (second >= 0 && PadLayout.adjacent(nearest, second) &&
            secondDistance - nearestDistance < radius * FACE_LENS && secondDistance < radius * FACE_BOTH
        ) {
            mask = mask or (1 shl second)
        }
        return mask
    }

    /** An analog trigger presses harder the further the finger is from the screen's edge. */
    fun triggerLevel(spot: PadSpot, y: Float): Float {
        if (!analogTriggers) return 1f
        val top = spot.y - spot.h / 2f
        val fraction = ((y - top) / (spot.h * 0.85f)).coerceIn(0f, 1f)
        return 0.2f + 0.8f * fraction
    }

    /** Works the pad out from the fingers; returns what the thumbs should feel of the change, if anything. */
    fun publish(): PadFeel? {
        val before = buttons
        val hatBefore = hatX * 3 + hatY
        val leftTriggerBefore = axes[4]
        val rightTriggerBefore = axes[5]
        var next = 0
        var mask = 0
        var leftTrigger = 0f
        var rightTrigger = 0f
        var sector = -1
        for (index in 0 until fingerCount) {
            val finger = fingers[index]
            val spot = finger.spot ?: continue
            when (spot.control.kind) {
                ControlKind.FACE -> mask = mask or finger.faces
                ControlKind.BUMPER, ControlKind.BUTTON -> next = next or spot.control.bit
                ControlKind.TRIGGER -> {
                    val level = triggerLevel(spot, finger.y)
                    if (spot.control == Control.LT) leftTrigger = max(leftTrigger, level) else rightTrigger = max(rightTrigger, level)
                }
                // The latest thumb on the d-pad has it.
                ControlKind.DPAD -> if (finger.sector >= 0 || sector < 0) sector = finger.sector
                ControlKind.STICK -> Unit
            }
        }
        for (index in 0 until 4) if (mask and (1 shl index) != 0) next = next or PadLayout.FACE_BITS[index]
        if (leftStick.clicked) next = next or PadBits.L3
        if (rightStick.clicked) next = next or PadBits.R3
        buttons = next
        faceMask = mask
        hatX = if (sector < 0) 0 else SECTOR_X[sector]
        hatY = if (sector < 0) 0 else SECTOR_Y[sector]
        axes[0] = leftStick.ax
        axes[1] = leftStick.ay
        axes[2] = rightStick.ax
        axes[3] = rightStick.ay
        axes[4] = leftTrigger
        axes[5] = rightTrigger
        axes[8] = hatX.toFloat()
        axes[9] = hatY.toFloat()

        val pressed = next and before.inv()
        val feel = when {
            stickClicked -> PadFeel.Click
            pressed != 0 || leftTrigger > 0f && leftTriggerBefore == 0f || rightTrigger > 0f && rightTriggerBefore == 0f ->
                PadFeel.Press
            hatX * 3 + hatY != hatBefore && (hatX != 0 || hatY != 0) -> PadFeel.Step
            grabbed -> PadFeel.Grab
            reachedRim -> PadFeel.Rim
            else -> null
        }
        stickClicked = false
        grabbed = false
        reachedRim = false
        return feel
    }

    /** A stick that floats to where the thumb lands. */
    inner class Stick(val control: Control) {
        var finger = -1
            private set
        val held: Boolean get() = finger >= 0
        /** Where its base is (it follows the thumb past the rim) and where the thumb is, px. */
        var originX = 0f
            private set
        var originY = 0f
            private set
        var thumbX = 0f
            private set
        var thumbY = 0f
            private set
        var ax = 0f
            private set
        var ay = 0f
            private set
        /** How far it leans, 0 to 1 after the dead zone. */
        var amount = 0f
            private set
        var clicked = false
            private set
        /** How many clicks it has had, so a drawing can ring each one. */
        var clicks = 0
            private set
        private var atRim = false
        private var lastTap = Long.MIN_VALUE / 2
        private var tapX = 0f
        private var tapY = 0f

        internal fun grab(id: Int, x: Float, y: Float, time: Long, home: PadSpot, drift: Float) {
            finger = id
            originX = x; originY = y
            keepNear(home, drift)
            thumbX = x; thumbY = y
            ax = 0f; ay = 0f; amount = 0f
            atRim = false
            // A quick tap and then a hold, in the same place, clicks the stick.
            clicked = time - lastTap <= DOUBLE_TAP_MS && hypot(x - tapX, y - tapY) <= DOUBLE_TAP_MM * mm
            lastTap = Long.MIN_VALUE / 2
            if (clicked) {
                clicks++
                stickClicked = true
            } else {
                grabbed = true
            }
        }

        internal fun drag(x: Float, y: Float, home: PadSpot, drift: Float) {
            val travel = home.b
            if (!held || travel <= 0f) return
            thumbX = x; thumbY = y
            var dx = x - originX
            var dy = y - originY
            val length = hypot(dx, dy)
            if (length > travel) {
                // Past the rim the stick follows the thumb, so turning back answers at once.
                val excess = (length - travel) / length
                originX += dx * excess
                originY += dy * excess
                keepNear(home, drift)
                dx = x - originX
                dy = y - originY
            }
            val reach = hypot(dx, dy)
            val lean = min(reach, travel) / travel
            amount = ((lean - DEAD_ZONE) / (FULL_AT - DEAD_ZONE)).coerceIn(0f, 1f)
            if (reach > 0f && amount > 0f) {
                ax = dx / reach * amount
                ay = dy / reach * amount
            } else {
                ax = 0f; ay = 0f
            }
            if (amount >= 1f && !atRim) {
                atRim = true
                reachedRim = true
            } else if (amount < RIM_REARM) {
                atRim = false
            }
        }

        /** Floating, the base stays within [drift] of its home. */
        private fun keepNear(home: PadSpot, drift: Float) {
            val dx = originX - home.x
            val dy = originY - home.y
            val distance = hypot(dx, dy)
            if (distance <= drift) return
            originX = home.x + dx * drift / distance
            originY = home.y + dy * drift / distance
        }

        internal fun letGo(id: Int, time: Long, tap: Boolean, x: Float, y: Float) {
            if (finger != id) return
            reset()
            if (tap) {
                lastTap = time
                tapX = x; tapY = y
            }
        }

        internal fun reset() {
            finger = -1
            ax = 0f; ay = 0f; amount = 0f
            clicked = false
            atRim = false
        }
    }

    companion object {
        private const val MAX_FINGERS = 10

        /** Stick: no output within this share of its travel, full tilt from this share. */
        private const val DEAD_ZONE = 0.08f
        private const val FULL_AT = 0.92f
        private const val RIM_REARM = 0.85f
        private const val DOUBLE_TAP_MS = 260L
        private const val DOUBLE_TAP_MM = 12f
        private const val TAP_MS = 220L
        private const val TAP_SLOP_MM = 3f

        /** D-pad: the middle is neutral; main directions 55 degrees wide, diagonals 35. */
        private const val DPAD_DEAD = 0.2f
        private const val DPAD_RELEASE = 0.14f
        private const val CARDINAL_HALF = 27.5f
        private const val DIAGONAL_HALF = 17.5f
        private const val DPAD_HYSTERESIS = 7f
        private val SECTOR_X = intArrayOf(1, 1, 0, -1, -1, -1, 0, 1)
        private val SECTOR_Y = intArrayOf(0, 1, 1, 1, 0, -1, -1, -1)

        /** Face buttons, in button radii (the middle in spreads). */
        private const val FACE_MIDDLE = 0.42f
        private const val FACE_KEEP = 0.15f
        private const val FACE_LENS = 0.32f
        private const val FACE_BOTH = 1.75f
        private const val FACE_SLIDE_OFF = 0.5f
    }
}
