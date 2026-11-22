package app.polarbear.gaming

import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min

/**
 * One control of the presented controller, in px: its home centre, drawn
 * size and how far past its edge a landing finger still counts. [a] and [b]
 * hold the rest of its shape: a stick's knob radius and travel, the face
 * buttons' radius and spread, the d-pad's arm width.
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
) {
    val r: Float get() = min(w, h) / 2f
}

/**
 * Where the presented controller's controls sit on a screen of any size.
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
    userScale: Float = 1f,
    /** Lowest edge of the Touchpad chip at the top centre, px; the middle row stays below it. */
    chipBottom: Float = 0f,
) {
    /** Scale from the reference phone, for positions. */
    val k: Float

    /** Scale for sizes: [k] with the Size setting. */
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
        s = k * userScale.coerceIn(MIN_USER_SCALE, MAX_USER_SCALE)
        middle = (left + right) / 2f
        // A taller screen keeps the clusters low, where thumbs rest.
        val region = REF_HEIGHT_MM * k * mm
        val squeeze = min(1f, usableHeight / region)
        val regionTop = top + max(0f, usableHeight - region) * LOW_BIAS

        fun at(value: Float) = value * k * mm
        fun size(value: Float) = value * s * mm
        fun y(value: Float) = regionTop + value * k * mm * squeeze
        fun fromLeft(value: Float) = left + at(value)
        fun fromRight(value: Float) = right - at(value)

        val row = max(y(ROW_Y), chipBottom + size(SMALL_MM / 2f) + 2.5f * mm)
        val placed = listOf(
            PadSpot(Control.LT, fromLeft(14f), y(8f), size(24f), size(12f), size(6f)),
            PadSpot(Control.LB, fromLeft(40f), y(7.5f), size(22f), size(10f), size(6f)),
            PadSpot(Control.RB, fromRight(40f), y(7.5f), size(22f), size(10f), size(6f)),
            PadSpot(Control.RT, fromRight(14f), y(8f), size(24f), size(12f), size(6f)),
            PadSpot(Control.LEFT_STICK, fromLeft(24f), y(30f), size(STICK_MM), size(STICK_MM), at(STICK_REACH_MM),
                a = size(KNOB_MM), b = size(TRAVEL_MM)),
            PadSpot(Control.DPAD, fromLeft(44f), y(50f), size(27f), size(27f), size(8f), a = size(9.4f)),
            PadSpot(Control.RIGHT_STICK, fromRight(51f), y(50f), size(STICK_MM), size(STICK_MM), at(STICK_REACH_MM),
                a = size(KNOB_MM), b = size(TRAVEL_MM)),
            PadSpot(Control.FACE, fromRight(26f), y(32.5f), size(2f * (FACE_SPREAD_MM + FACE_BUTTON_MM)),
                size(2f * (FACE_SPREAD_MM + FACE_BUTTON_MM)), size(8f), a = size(FACE_BUTTON_MM), b = size(FACE_SPREAD_MM)),
            PadSpot(Control.BACK, middle - at(15f), row, size(SMALL_MM), size(SMALL_MM), size(4f)),
            PadSpot(Control.GUIDE, middle, row, size(SMALL_MM), size(SMALL_MM), size(4f)),
            PadSpot(Control.START, middle + at(15f), row, size(SMALL_MM), size(SMALL_MM), size(4f)),
            PadSpot(Control.L3, fromLeft(9.5f), y(57.5f), size(8.4f), size(8.4f), size(2.5f)),
            PadSpot(Control.R3, fromRight(9.5f), y(57.5f), size(8.4f), size(8.4f), size(2.5f)),
        )
        // Whatever the Size setting, every control stays whole on screen.
        spots = placed.map { spot ->
            val halfW = spot.w / 2f
            val halfH = spot.h / 2f
            val x = spot.x.coerceIn(insetLeft + halfW, max(insetLeft + halfW, width - insetRight - halfW))
            val y = spot.y.coerceIn(insetTop + halfH, max(insetTop + halfH, height - insetBottom - halfH))
            if (x == spot.x && y == spot.y) spot
            else PadSpot(spot.control, x, y, spot.w, spot.h, spot.reach, spot.a, spot.b)
        }
    }

    fun spot(control: Control): PadSpot? = spots.firstOrNull { it.control == control }

    /** Centre of face button [index] (A, B, X, Y) of [face]. */
    fun faceButtonX(face: PadSpot, index: Int): Float = face.x + FACE_DX[index] * face.b
    fun faceButtonY(face: PadSpot, index: Int): Float = face.y + FACE_DY[index] * face.b

    /** How far ([x], [y]) is outside [spot]'s shape, px; 0 inside it. */
    fun distance(spot: PadSpot, x: Float, y: Float): Float = when (spot.control.kind) {
        ControlKind.FACE -> {
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
     * its reach. A stick takes any landing on its thumb's half of the screen
     * that nothing else claims, and starts from wherever that is.
     */
    fun pick(x: Float, y: Float): PadSpot? {
        var best: PadSpot? = null
        var bestDistance = Float.MAX_VALUE
        for (spot in spots) {
            if (spot.control == Control.LEFT_STICK && x >= middle) continue
            if (spot.control == Control.RIGHT_STICK && x < middle) continue
            val distance = distance(spot, x, y)
            if (distance <= spot.reach && distance < bestDistance) {
                best = spot
                bestDistance = distance
            }
        }
        return best
    }

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
