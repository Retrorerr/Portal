package app.polarbear.gaming

import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min

/**
 * One control of the on-screen controller, in px: its home centre, drawn
 * size and how far past its edge a landing finger still counts. [a] and [b]
 * hold the rest of its shape: a stick's knob radius and travel, the face
 * buttons' radius and spread, the d-pad's arm width. A [hidden] control is
 * one the player took out of play; only the layout editor shows it.
 */
internal class PadSpot(
    val control: Control,
    val x: Float,
    val y: Float,
    val w: Float,
    val h: Float,
    val reach: Float,
    val a: Float = 0f,
    val b: Float = 0f,
    val hidden: Boolean = false,
) {
    val r: Float get() = min(w, h) / 2f
}

/**
 * Where the on-screen controller's controls sit on a screen of any size.
 *
 * Thumbs are the same size on every screen, so the controls are measured in
 * millimetres from the screen's physical density and shrink together ([k])
 * only as far as a small screen needs. They hang off the left and right
 * edges in two clusters, where the hands hold a phone turned sideways: a
 * wider screen adds room in the middle and a taller one room above, instead
 * of pulling the controls apart. The arrangement is an Xbox pad's: left
 * stick high and d-pad low on the left, face buttons high and right stick
 * low on the right, triggers and bumpers along the top corners, and View,
 * Guide and Menu between them. The d-pad and right stick sit low and close
 * to the outer pair rather than out toward the middle, where a thumb
 * holding a phone would have to stretch.
 *
 * Over the desktop ([wholeScreen] false) the same arrangement floats over
 * the game: a finger that misses every control belongs to the desktop, so
 * each control only reaches a little past its edge and a stick keeps its
 * base near home, and the player's moves, sizes and hidden controls
 * ([ElementState]) apply on top of these defaults.
 *
 * Plain Kotlin, so the layout can be checked off the device.
 */
internal class PadLayout(
    val width: Float,
    val height: Float,
    /** Edges the screen keeps for itself (a camera cutout), px. */
    insetLeft: Float,
    insetTop: Float,
    insetRight: Float,
    insetBottom: Float,
    /** Physical px per millimetre. */
    val mm: Float,
    /** The player's Size setting. */
    userScale: Float,
    /** The controller is the whole screen (the touchpad page's), not controls floating over the desktop. */
    val wholeScreen: Boolean,
    /** Lowest edge of the Touchpad chip at the top centre, px; the middle row stays below it. */
    chipBottom: Float = 0f,
    /** Floating: where the player moved, resized or hid each control. */
    elements: Map<Control, ElementState> = emptyMap(),
) {
    /** Scale from the reference phone that fits this screen. */
    val k: Float

    /**
     * Scale of the whole pad: [k] with the Size setting. The pad grows and
     * shrinks from the screen's edges, where the thumbs are, and no further
     * than the screen holds it, so the controls never run into each other.
     */
    val s: Float

    /** The line between the left thumb's half and the right thumb's. */
    val middle: Float

    val spots: List<PadSpot>

    init {
        val left = insetLeft + EDGE_MM * mm
        val top = insetTop + EDGE_MM * mm
        val right = width - insetRight - EDGE_MM * mm
        val bottom = height - insetBottom - EDGE_MM * mm
        val usableWidth = (right - left).coerceAtLeast(1f)
        val usableHeight = (bottom - top).coerceAtLeast(1f)
        val fit = min(usableWidth / (REF_WIDTH_MM * mm), usableHeight / (REF_HEIGHT_MM * mm))
        // Turned upright (Android refused landscape) it only has to fit.
        k = fit.coerceIn(if (usableWidth >= usableHeight) MIN_SCALE else MIN_UPRIGHT_SCALE, MAX_SCALE)
        s = min(k * userScale, max(k, fit))
        middle = (left + right) / 2f
        // A taller screen keeps the clusters low, where thumbs rest.
        val region = REF_HEIGHT_MM * s * mm
        val squeeze = min(1f, usableHeight / region)
        val regionTop = top + max(0f, usableHeight - region) * LOW_BIAS

        fun at(value: Float) = value * s * mm
        // Floating, a landing near a control but on none of them is the desktop's.
        fun reach(value: Float) = at(if (wholeScreen) value else min(value, FLOAT_REACH_MM))
        val stickReach = at(if (wholeScreen) STICK_REACH_MM else FLOAT_REACH_MM)
        fun y(value: Float) = regionTop + at(value) * squeeze
        fun fromLeft(value: Float) = left + at(value)
        fun fromRight(value: Float) = right - at(value)

        val row = max(y(ROW_Y), chipBottom + at(SMALL_MM / 2f) + 2.5f * mm)
        val placed = listOf(
            PadSpot(Control.LT, fromLeft(14f), y(8f), at(24f), at(12f), reach(6f)),
            PadSpot(Control.LB, fromLeft(40f), y(7.5f), at(22f), at(10f), reach(6f)),
            PadSpot(Control.RB, fromRight(40f), y(7.5f), at(22f), at(10f), reach(6f)),
            PadSpot(Control.RT, fromRight(14f), y(8f), at(24f), at(12f), reach(6f)),
            PadSpot(Control.LEFT_STICK, fromLeft(24f), y(30f), at(STICK_MM), at(STICK_MM), stickReach,
                a = at(KNOB_MM), b = at(TRAVEL_MM)),
            PadSpot(Control.DPAD, fromLeft(44f), y(50f), at(27f), at(27f), reach(8f), a = at(9.4f)),
            PadSpot(Control.RIGHT_STICK, fromRight(51f), y(50f), at(STICK_MM), at(STICK_MM), stickReach,
                a = at(KNOB_MM), b = at(TRAVEL_MM)),
            PadSpot(Control.FACE, fromRight(26f), y(32.5f), at(2f * (FACE_SPREAD_MM + FACE_BUTTON_MM)),
                at(2f * (FACE_SPREAD_MM + FACE_BUTTON_MM)), reach(8f), a = at(FACE_BUTTON_MM), b = at(FACE_SPREAD_MM)),
            PadSpot(Control.BACK, middle - at(15f), row, at(SMALL_MM), at(SMALL_MM), reach(4f)),
            PadSpot(Control.GUIDE, middle, row, at(SMALL_MM), at(SMALL_MM), reach(4f)),
            PadSpot(Control.START, middle + at(15f), row, at(SMALL_MM), at(SMALL_MM), reach(4f)),
            PadSpot(Control.L3, fromLeft(9.5f), y(57.5f), at(8.4f), at(8.4f), reach(2.5f)),
            PadSpot(Control.R3, fromRight(9.5f), y(57.5f), at(8.4f), at(8.4f), reach(2.5f)),
        )
        spots = placed.map { home ->
            val element = elements[home.control]
            val grow = element?.scale ?: 1f
            val w = home.w * grow
            val h = home.h * grow
            // Whatever the sizes, every control stays whole on screen.
            val halfW = w / 2f
            val halfH = h / 2f
            val x = (if (element?.placed == true) element.x * width else home.x)
                .coerceIn(insetLeft + halfW, max(insetLeft + halfW, width - insetRight - halfW))
            val y = (if (element?.placed == true) element.y * height else home.y)
                .coerceIn(insetTop + halfH, max(insetTop + halfH, height - insetBottom - halfH))
            if (element == null && x == home.x && y == home.y) home
            else PadSpot(home.control, x, y, w, h, home.reach * grow, home.a * grow, home.b * grow, hidden = element?.visible == false)
        }
    }

    fun spot(control: Control): PadSpot? = spots.firstOrNull { it.control == control }

    /** Centre of face button [index] (A, B, X, Y) of [face]. */
    fun faceButtonX(face: PadSpot, index: Int): Float = face.x + FACE_DX[index] * face.b
    fun faceButtonY(face: PadSpot, index: Int): Float = face.y + FACE_DY[index] * face.b

    /** How far ([x], [y]) is outside [spot]'s shape, px; 0 inside it. */
    fun distance(spot: PadSpot, x: Float, y: Float): Float = when (spot.control.kind) {
        // The diamond's middle is the cluster's too: a thumb there slides onto a button.
        ControlKind.FACE -> if (hypot(x - spot.x, y - spot.y) <= spot.b) 0f else {
            var nearest = Float.MAX_VALUE
            for (index in 0 until 4) {
                nearest = min(nearest, hypot(x - faceButtonX(spot, index), y - faceButtonY(spot, index)))
            }
            max(0f, nearest - spot.a)
        }
        ControlKind.BUMPER, ControlKind.TRIGGER -> {
            val dx = max(0f, abs(x - spot.x) - spot.w / 2f)
            val dy = max(0f, abs(y - spot.y) - spot.h / 2f)
            hypot(dx, dy)
        }
        ControlKind.DPAD -> max(0f, hypot(x - spot.x, y - spot.y) - spot.r * 0.95f)
        else -> max(0f, hypot(x - spot.x, y - spot.y) - spot.r)
    }

    /**
     * The control a finger landing at ([x], [y]) is for: the nearest within
     * its reach, or none. On the whole screen a stick takes any landing on
     * its thumb's half that nothing else claims, and starts from wherever
     * that is. The layout editor also picks [hidden] controls.
     */
    fun pick(x: Float, y: Float, hidden: Boolean = false): PadSpot? {
        var best: PadSpot? = null
        var bestDistance = Float.MAX_VALUE
        for (spot in spots) {
            if (spot.hidden && !hidden) continue
            if (wholeScreen && spot.control == Control.LEFT_STICK && x >= middle) continue
            if (wholeScreen && spot.control == Control.RIGHT_STICK && x < middle) continue
            val distance = distance(spot, x, y)
            if (distance <= spot.reach && distance < bestDistance) {
                best = spot
                bestDistance = distance
            }
        }
        return best
    }

    /**
     * How far a stick's base may drift from home after the thumb: anywhere
     * on the whole screen; floating, a little, so it stays by its place.
     */
    fun drift(spot: PadSpot): Float = if (wholeScreen) Float.POSITIVE_INFINITY else spot.r * FLOAT_DRIFT

    companion object {
        /** The reference phone's room for the controls, turned sideways. */
        const val REF_WIDTH_MM = 148f
        const val REF_HEIGHT_MM = 66f
        const val MIN_SCALE = 0.7f
        const val MIN_UPRIGHT_SCALE = 0.4f
        const val MAX_SCALE = 1.18f
        const val MIN_USER_SCALE = 0.8f
        const val MAX_USER_SCALE = 1.2f
        private const val EDGE_MM = 1.5f
        private const val LOW_BIAS = 0.7f
        private const val ROW_Y = 22f
        private const val SMALL_MM = 9.2f
        private const val STICK_MM = 27f
        private const val KNOB_MM = 6.3f
        private const val TRAVEL_MM = 11f
        private const val STICK_REACH_MM = 34f
        private const val FACE_BUTTON_MM = 5.5f
        private const val FACE_SPREAD_MM = 11.2f
        /** Floating: how far past its edge a control takes a landing finger. */
        private const val FLOAT_REACH_MM = 4f
        /** Floating: how far a stick's base may follow the thumb, in stick radii. */
        private const val FLOAT_DRIFT = 0.6f

        /** A, B, X, Y around the face cluster's centre (screen y grows down). */
        val FACE_DX = floatArrayOf(0f, 1f, -1f, 0f)
        val FACE_DY = floatArrayOf(1f, 0f, 0f, -1f)
        val FACE_BITS = intArrayOf(PadBits.A, PadBits.B, PadBits.X, PadBits.Y)
        val FACE_LABELS = arrayOf("A", "B", "X", "Y")

        /** Face buttons side by side on the diamond (A-B, B-Y, Y-X, X-A). */
        fun adjacent(first: Int, second: Int): Boolean =
            first != second && (FACE_DX[first] != FACE_DX[second] && FACE_DY[first] != FACE_DY[second])
    }
}
