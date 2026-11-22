package app.polarbear.gaming

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Matrix
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RadialGradient
import android.graphics.Rect
import android.graphics.RectF
import android.graphics.Region
import android.graphics.Shader
import android.graphics.Typeface
import android.os.Build
import android.os.SystemClock
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.util.Log
import android.view.MotionEvent
import android.view.View
import android.view.WindowInsets
import android.view.animation.PathInterpolator
import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.exp
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/** Where the screen controller's state goes: the guest's touch pad. */
internal fun interface PadSink {
    /**
     * The whole pad: [buttons] in xpad order ([PadBits]), [axes] laid out as
     * GamepadBridge's. False when it could not go to the guest.
     */
    fun send(buttons: Int, axes: FloatArray): Boolean
}

/**
 * Portal's palette as the screen controller paints it (ARGB): the surface,
 * outline, ink and orange accent of the touchpad page's own buttons.
 */
internal data class PadColors(
    val surface: Int,
    val edge: Int,
    val ink: Int,
    val muted: Int,
    val accent: Int,
    /** Ink on the accent, for a held control. */
    val accentInk: Int,
    val dark: Boolean,
) {
    companion object {
        /** Portal's dark reading, until the page hands over its live palette. */
        val Dark = PadColors(
            surface = 0x16F1EBDD,
            edge = 0x14F1EBDD,
            ink = 0xFFF1EBDD.toInt(),
            muted = 0xC4BDB7AA.toInt(),
            accent = 0xFFF07949.toInt(),
            accentInk = 0xFF191B1C.toInt(),
            dark = true,
        )
    }
}

/**
 * Presses you can feel. Buttons click, the d-pad ticks on each new
 * direction, a stick ticks softly as the thumb takes it and again at its
 * rim, and a stick click lands harder. Built from the vibrator's primitives
 * where it has them (crisp on a good motor), else Android's predefined
 * effects. Silent while [quiet] says the game is rumbling: a tick would cut
 * the rumble short.
 */
internal class PadHaptics(context: Context, private val quiet: () -> Boolean) {
    enum class Feel { Press, Step, Grab, Rim, Click }

    var enabled = true

    private val vibrator: Vibrator? = try {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            context.getSystemService(VibratorManager::class.java)?.defaultVibrator
        } else {
            @Suppress("DEPRECATION")
            context.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
        }
    } catch (_: Exception) {
        null
    }?.takeIf { it.hasVibrator() }

    /** Old motors: a plain pulse per feel, ms; 0 stays silent. */
    private val pulses = longArrayOf(14L, 8L, 6L, 0L, 22L)

    // A VibrationEffect per feel from Android 8; null where that feel stays silent.
    private val effects = arrayOfNulls<Any>(Feel.entries.size)

    init {
        val vibrator = vibrator
        if (vibrator != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            for (feel in Feel.entries) effects[feel.ordinal] = try { effectFor(vibrator, feel) } catch (_: Exception) { null }
        }
    }

    private fun effectFor(vibrator: Vibrator, feel: Feel): Any? {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R &&
            vibrator.areAllPrimitivesSupported(VibrationEffect.Composition.PRIMITIVE_CLICK, VibrationEffect.Composition.PRIMITIVE_TICK)
        ) {
            val low = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S &&
                vibrator.areAllPrimitivesSupported(VibrationEffect.Composition.PRIMITIVE_LOW_TICK)
            val primitive: Int
            val scale: Float
            when (feel) {
                Feel.Press -> { primitive = VibrationEffect.Composition.PRIMITIVE_CLICK; scale = 0.7f }
                Feel.Step -> { primitive = VibrationEffect.Composition.PRIMITIVE_TICK; scale = 0.75f }
                Feel.Grab -> if (low) { primitive = VibrationEffect.Composition.PRIMITIVE_LOW_TICK; scale = 0.8f }
                    else { primitive = VibrationEffect.Composition.PRIMITIVE_TICK; scale = 0.4f }
                Feel.Rim -> if (low) { primitive = VibrationEffect.Composition.PRIMITIVE_LOW_TICK; scale = 0.45f }
                    else { primitive = VibrationEffect.Composition.PRIMITIVE_TICK; scale = 0.2f }
                Feel.Click -> { primitive = VibrationEffect.Composition.PRIMITIVE_CLICK; scale = 1f }
            }
            return VibrationEffect.startComposition().addPrimitive(primitive, scale).compose()
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            return when (feel) {
                Feel.Press -> VibrationEffect.createPredefined(VibrationEffect.EFFECT_CLICK)
                Feel.Step, Feel.Grab -> VibrationEffect.createPredefined(VibrationEffect.EFFECT_TICK)
                // A buzz at every rim would tire on a plain motor.
                Feel.Rim -> null
                Feel.Click -> VibrationEffect.createPredefined(VibrationEffect.EFFECT_HEAVY_CLICK)
            }
        }
        val length = pulses[feel.ordinal]
        if (length <= 0L) return null
        val amplitude = if (vibrator.hasAmplitudeControl()) (60 + length * 9).toInt().coerceAtMost(255) else VibrationEffect.DEFAULT_AMPLITUDE
        return VibrationEffect.createOneShot(length, amplitude)
    }

    fun play(feel: Feel) {
        if (!enabled) return
        val vibrator = vibrator ?: return
        if (quiet()) return
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                (effects[feel.ordinal] as? VibrationEffect)?.let(vibrator::vibrate)
            } else {
                val length = pulses[feel.ordinal]
                @Suppress("DEPRECATION")
                if (length > 0L) vibrator.vibrate(length)
            }
        } catch (_: Exception) {
        }
    }
}

/**
 * The whole screen as a game controller, while the desktop is on a monitor
 * and this screen is its touchpad page. One view takes every finger, so a
 * thumb may land anywhere: [PadLayout] gives it to the nearest control, and a
 * landing on either half that no other control claims takes that half's
 * stick, wherever it lands. Sizes come from the screen's physical density.
 *
 * Feel: a stick floats to the thumb and follows it past its rim, with a small
 * dead zone in the middle; a quick tap and then a hold on a stick clicks it
 * (L3, R3). The d-pad favours its four main directions and keeps a direction
 * until the thumb is clearly in the next. A thumb between two face buttons
 * presses both, and sliding across them moves the press. Touches arrive
 * unbuffered and the state goes out on the event that changed it.
 */
@SuppressLint("ViewConstructor")
internal class PresentedGamepad(
    context: Context,
    private val sink: PadSink,
    private val haptics: PadHaptics,
) : View(context) {
    private val dp = resources.displayMetrics.density

    /** Physical px per millimetre: thumbs are the same size on every screen. */
    private val mm: Float = resources.displayMetrics.let { metrics ->
        val physical = (metrics.xdpi + metrics.ydpi) / 2f
        val nominal = metrics.densityDpi.toFloat()
        // Some panels report a placeholder; Android's own density is the better guess then.
        (if (physical > nominal * 0.6f && physical < nominal * 1.6f) physical else nominal) / MM_PER_INCH
    }

    var colors: PadColors = PadColors.Dark
        set(value) {
            if (field == value) return
            field = value
            restyle()
            invalidate()
        }

    /** The player's Size setting. */
    var userScale = 1f
        set(value) {
            if (field == value) return
            field = value
            relayout()
        }

    var analogTriggers = false

    /** Lowest edge of the chip at the top centre, px in this view; the middle row stays below it. */
    var chipBottom = 0f
        set(value) {
            if (field == value) return
            field = value
            relayout()
        }

    /**
     * A Portal page is open over the controller: every touch ends and the
     * controls dim until it closes.
     */
    var suspended = false
        set(value) {
            if (field == value) return
            field = value
            if (value) release()
            animate().cancel()
            animate().alpha(if (value) SUSPENDED_ALPHA else 1f)
                .setDuration(if (value) 160L else 260L)
                .setInterpolator(if (value) ACCELERATE else DECELERATE)
                .start()
        }

    private var pad: PadLayout? = null
    private val insets = Rect()
    private var relayoutWaiting = false

    // ---- Touch state -----------------------------------------------------

    private class Finger {
        var id = -1
        var spot: PadSpot? = null
        var x = 0f
        var y = 0f
        var downX = 0f
        var downY = 0f
        var downTime = 0L
        /** How far it has strayed from where it landed, px. */
        var travel = 0f
        /** Face buttons it holds, bits 0..3 for A, B, X, Y. */
        var faces = 0
        /** D-pad direction it holds, a sector clockwise from east, or -1. */
        var sector = -1
    }

    private val fingers = Array(MAX_FINGERS) { Finger() }
    private var fingerCount = 0

    private val leftStick = Stick(Control.LEFT_STICK)
    private val rightStick = Stick(Control.RIGHT_STICK)

    private var buttons = 0
    private val axes = FloatArray(10)
    private var sentButtons = -1
    private val sentAxes = FloatArray(10)
    private var hatX = 0
    private var hatY = 0
    private var faceMask = 0

    // What the last event did that a thumb should feel.
    private var grabbed = false
    private var reachedRim = false
    private var stickClicked = false

    // ---- Drawing state ---------------------------------------------------

    /** How pressed each control looks, by [Control.ordinal]; a trigger's is its level. */
    private val press = Array(Control.entries.size) { Ease(PRESS_IN_MS, PRESS_OUT_MS) }
    private val facePress = Array(4) { Ease(PRESS_IN_MS, PRESS_OUT_MS) }
    /** D-pad arms: east, south, west, north. */
    private val armPress = Array(4) { Ease(PRESS_IN_MS, PRESS_OUT_MS) }

    private var showing = false
    private var waitingForTurn = false
    private var appearStart = 0L
    private var hideStart = 0L
    private var onHidden: (() -> Unit)? = null
    private val shown = FloatArray(Control.entries.size)
    private val hideFrom = FloatArray(Control.entries.size)
    private var lastFrame = 0L

    private val fill = Paint(Paint.ANTI_ALIAS_FLAG)
    private val outline = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.STROKE }
    private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
    }
    private val label = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        textAlign = Paint.Align.CENTER
        typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
    }
    private val halo = Paint(Paint.ANTI_ALIAS_FLAG)
    private val haloMatrix = Matrix()
    private val rect = RectF()
    private val path = Path()
    private val cross = Path()
    private var faceTints = IntArray(4)

    private val turnTimeout = Runnable { if (waitingForTurn) startEntrance() }

    init {
        contentDescription = "Controller"
        isHapticFeedbackEnabled = false
        isSoundEffectsEnabled = false
        restyle()
    }

    // ---- Coming and going ------------------------------------------------

    /**
     * Comes in: the controls rise into place, the shoulders first. With
     * [waitForTurn] while the screen is upright, Android is turning it
     * sideways, so they wait for that (or a moment) and arrive in place.
     */
    fun show(waitForTurn: Boolean) {
        onHidden = null
        showing = true
        keepScreenOn = true
        voteFrameRate(high = true)
        // Plug the pad in now, so the game sees it before the first touch.
        axes.fill(0f)
        buttons = 0
        sentButtons = -1
        send()
        if (waitForTurn && height > width) {
            waitingForTurn = true
            shown.fill(0f)
            removeCallbacks(turnTimeout)
            postDelayed(turnTimeout, TURN_WAIT_MS)
            invalidate()
        } else {
            startEntrance()
        }
    }

    private fun startEntrance() {
        waitingForTurn = false
        removeCallbacks(turnTimeout)
        appearStart = SystemClock.uptimeMillis()
        lastFrame = 0L
        invalidate()
    }

    /** Goes: every touch ends now; [done] runs once the controls have faded. */
    fun hide(done: () -> Unit) {
        release()
        removeCallbacks(turnTimeout)
        keepScreenOn = false
        if (!showing) {
            voteFrameRate(high = false)
            done()
            return
        }
        showing = false
        waitingForTurn = false
        onHidden = done
        System.arraycopy(shown, 0, hideFrom, 0, shown.size)
        hideStart = SystemClock.uptimeMillis()
        lastFrame = 0L
        invalidate()
    }

    /**
     * Android 15 counts a vote from each view that draws, and OxygenOS holds
     * an app to 60 Hz by default: while the controller shows, a moving thumb
     * draws at the panel's full rate. Idle, it draws nothing and asks nothing.
     */
    private fun voteFrameRate(high: Boolean) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.VANILLA_ICE_CREAM) return
        requestedFrameRate = if (high) View.REQUESTED_FRAME_RATE_CATEGORY_HIGH else View.REQUESTED_FRAME_RATE_CATEGORY_NO_PREFERENCE
    }

    /** Every finger lets go and the pad goes back to rest. */
    fun release() {
        liftAll(cancelled = true)
        publish()
    }

    // ---- Layout ----------------------------------------------------------

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        requestApplyInsets()
    }

    override fun onApplyWindowInsets(windowInsets: WindowInsets): WindowInsets {
        // The bars as they are now: hidden, the controls may use their room.
        val left: Int
        val top: Int
        val right: Int
        val bottom: Int
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val bars = windowInsets.getInsets(WindowInsets.Type.systemBars())
            left = bars.left; top = bars.top; right = bars.right; bottom = bars.bottom
        } else {
            @Suppress("DEPRECATION")
            left = windowInsets.systemWindowInsetLeft
            @Suppress("DEPRECATION")
            top = windowInsets.systemWindowInsetTop
            @Suppress("DEPRECATION")
            right = windowInsets.systemWindowInsetRight
            @Suppress("DEPRECATION")
            bottom = windowInsets.systemWindowInsetBottom
        }
        if (left != insets.left || top != insets.top || right != insets.right || bottom != insets.bottom) {
            insets.set(left, top, right, bottom)
            relayout()
        }
        return windowInsets
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        // A turn mid-gesture: nothing held means the same on the new screen.
        release()
        relayout()
        if (waitingForTurn && w >= h) startEntrance()
    }

    /** Lays the controls out for the screen as it is; while fingers are down, once they lift. */
    private fun relayout() {
        if (width <= 0 || height <= 0) return
        if (fingerCount > 0) {
            relayoutWaiting = true
            return
        }
        relayoutWaiting = false
        val next = PadLayout(
            width.toFloat(), height.toFloat(),
            insets.left.toFloat(), insets.top.toFloat(), insets.right.toFloat(), insets.bottom.toFloat(),
            mm, userScale, chipBottom,
        )
        pad = next
        // One line per layout, for bug reports and the device check: where each control sits, px.
        Log.i(TAG, "layout ${width}x$height at ${(mm * 100).roundToInt() / 100f} px/mm, size ${(userScale * 100).roundToInt()}%: " +
            next.spots.joinToString(" ") { "${it.control.name}@${it.x.roundToInt()},${it.y.roundToInt()},r${it.r.roundToInt()},b${it.b.roundToInt()}" })
        next.spot(Control.DPAD)?.let(::buildCross)
        leftStick.rest(next)
        rightStick.rest(next)
        excludeEdges(next)
        invalidate()
    }

    private fun buildCross(spot: PadSpot) {
        val size = spot.r * 2f
        val arm = spot.a
        val corner = arm * 0.3f
        val horizontal = Path().apply {
            addRoundRect(RectF(spot.x - size / 2f, spot.y - arm / 2f, spot.x + size / 2f, spot.y + arm / 2f), corner, corner, Path.Direction.CW)
        }
        val vertical = Path().apply {
            addRoundRect(RectF(spot.x - arm / 2f, spot.y - size / 2f, spot.x + arm / 2f, spot.y + size / 2f), corner, corner, Path.Direction.CW)
        }
        cross.reset()
        cross.op(horizontal, vertical, Path.Op.UNION)
    }

    /**
     * A thumb sliding in from a side edge onto a stick must not turn into
     * Android's Back gesture. Android honours 200 dp of each edge; this
     * takes it where the thumbs rest.
     */
    private fun excludeEdges(pad: PadLayout) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
        val band = (EXCLUDED_DP * dp / 2f).toInt()
        val edge = (EDGE_DP * dp).toInt()
        val leftY = (pad.spot(Control.LEFT_STICK)?.y ?: height / 2f).toInt()
        val face = pad.spot(Control.FACE)?.y ?: height / 2f
        val rightY = ((face + (pad.spot(Control.RIGHT_STICK)?.y ?: face)) / 2f).toInt()
        systemGestureExclusionRects = listOf(
            Rect(0, leftY - band, edge, leftY + band),
            Rect(width - edge, rightY - band, width, rightY + band),
        )
    }

    // ---- Touch -----------------------------------------------------------

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        val pad = pad
        if (pad == null || !showing || waitingForTurn || suspended) {
            if (fingerCount > 0) release()
            return true
        }
        // A finger that left without its UP must not hold a stick where it was.
        if (event.actionMasked != MotionEvent.ACTION_DOWN) dropMissing(event)
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                // Moves come as the panel samples them, not once a frame.
                requestUnbufferedDispatch(event)
                liftAll(cancelled = true)
                land(pad, event, 0)
            }
            MotionEvent.ACTION_POINTER_DOWN -> land(pad, event, event.actionIndex)
            MotionEvent.ACTION_POINTER_UP -> lift(
                event.getPointerId(event.actionIndex), event.eventTime,
                cancelled = event.flags and MotionEvent.FLAG_CANCELED != 0,
            )
            MotionEvent.ACTION_UP -> {
                lift(event.getPointerId(0), event.eventTime, cancelled = event.flags and MotionEvent.FLAG_CANCELED != 0)
                liftAll(cancelled = true)
            }
            MotionEvent.ACTION_CANCEL -> liftAll(cancelled = true)
        }
        // Every event carries fresh positions for every finger still down.
        if (event.actionMasked != MotionEvent.ACTION_UP && event.actionMasked != MotionEvent.ACTION_CANCEL) {
            for (index in 0 until event.pointerCount) move(pad, event.getPointerId(index), event.getX(index), event.getY(index))
        }
        publish()
        if (fingerCount == 0 && relayoutWaiting) relayout()
        return true
    }

    private fun land(pad: PadLayout, event: MotionEvent, index: Int) {
        // Android reuses a lifted finger's id: one still tracked here lost its UP.
        lift(event.getPointerId(index), event.eventTime, cancelled = true)
        if (fingerCount == MAX_FINGERS) return
        val x = event.getX(index)
        val y = event.getY(index)
        var spot = pad.pick(x, y)
        val stick = spot?.let { stickFor(it.control) }
        // A second thumb on a held stick's half is meant for nothing.
        if (stick != null && stick.held) spot = null
        spot ?: return
        val finger = fingers[fingerCount++]
        finger.id = event.getPointerId(index)
        finger.spot = spot
        finger.x = x; finger.y = y
        finger.downX = x; finger.downY = y
        finger.downTime = event.eventTime
        finger.travel = 0f
        finger.faces = 0
        finger.sector = -1
        stick?.grab(finger.id, x, y, event.eventTime)
    }

    private fun move(pad: PadLayout, id: Int, x: Float, y: Float) {
        val finger = find(id) ?: return
        val spot = finger.spot ?: return
        finger.x = x
        finger.y = y
        finger.travel = max(finger.travel, hypot(x - finger.downX, y - finger.downY))
        when (spot.control.kind) {
            ControlKind.STICK -> stickFor(spot.control)?.drag(x, y, spot.b)
            ControlKind.DPAD -> finger.sector = dpadSector(spot, x, y, finger.sector)
            ControlKind.FACE -> finger.faces = faceButtons(pad, spot, x, y, finger.faces)
            else -> Unit
        }
    }

    private fun lift(id: Int, time: Long, cancelled: Boolean) {
        val index = indexOf(id)
        if (index < 0) return
        val finger = fingers[index]
        val spot = finger.spot
        if (spot != null && spot.control.kind == ControlKind.STICK) {
            val tap = !cancelled && time - finger.downTime <= TAP_MS && finger.travel <= TAP_SLOP_MM * mm
            stickFor(spot.control)?.letGo(time, tap, finger.x, finger.y)
        }
        // Keep the array dense: the last finger takes this one's place.
        val last = fingers[fingerCount - 1]
        fingers[fingerCount - 1] = finger
        fingers[index] = last
        finger.id = -1
        finger.spot = null
        fingerCount--
    }

    /** Lets go of every finger [event] no longer carries: Android dropped its UP. */
    private fun dropMissing(event: MotionEvent) {
        // From the end: a lift moves the last finger into the freed place.
        for (index in fingerCount - 1 downTo 0) {
            val id = fingers[index].id
            if (event.findPointerIndex(id) < 0) lift(id, event.eventTime, cancelled = true)
        }
    }

    private fun liftAll(cancelled: Boolean) {
        while (fingerCount > 0) lift(fingers[fingerCount - 1].id, SystemClock.uptimeMillis(), cancelled)
        leftStick.letGo(0L, false, 0f, 0f)
        rightStick.letGo(0L, false, 0f, 0f)
    }

    private fun indexOf(id: Int): Int {
        for (index in 0 until fingerCount) if (fingers[index].id == id) return index
        return -1
    }

    private fun find(id: Int): Finger? = indexOf(id).takeIf { it >= 0 }?.let { fingers[it] }

    private fun stickFor(control: Control): Stick? = when (control) {
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
    private fun triggerLevel(spot: PadSpot, y: Float): Float {
        if (!analogTriggers) return 1f
        val top = spot.y - spot.h / 2f
        val fraction = ((y - top) / (spot.h * 0.85f)).coerceIn(0f, 1f)
        return 0.2f + 0.8f * fraction
    }

    /** Works out the pad from the fingers, sends it if it changed, and lets the thumbs feel what changed. */
    private fun publish() {
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
            stickClicked -> PadHaptics.Feel.Click
            pressed != 0 || leftTrigger > 0f && leftTriggerBefore == 0f || rightTrigger > 0f && rightTriggerBefore == 0f ->
                PadHaptics.Feel.Press
            hatX * 3 + hatY != hatBefore && (hatX != 0 || hatY != 0) -> PadHaptics.Feel.Step
            grabbed -> PadHaptics.Feel.Grab
            reachedRim -> PadHaptics.Feel.Rim
            else -> null
        }
        stickClicked = false
        grabbed = false
        reachedRim = false
        feel?.let(haptics::play)
        send()
        aim()
    }

    private fun send() {
        if (buttons == sentButtons && axes.contentEquals(sentAxes)) return
        // Only what reached the guest counts: a dropped release goes again with the next event.
        if (!sink.send(buttons, axes)) {
            sentButtons = -1
            return
        }
        sentButtons = buttons
        System.arraycopy(axes, 0, sentAxes, 0, axes.size)
    }

    /** Points every drawn press at the pad as it now is. */
    private fun aim() {
        for (ease in press) ease.target = 0f
        for (index in 0 until fingerCount) {
            val spot = fingers[index].spot ?: continue
            when (spot.control.kind) {
                ControlKind.BUMPER, ControlKind.BUTTON -> press[spot.control.ordinal].target = 1f
                ControlKind.TRIGGER -> {
                    val ease = press[spot.control.ordinal]
                    ease.target = max(ease.target, triggerLevel(spot, fingers[index].y))
                }
                else -> Unit
            }
        }
        // A clicked stick lights its own button too: the game sees one L3.
        if (buttons and PadBits.L3 != 0) press[Control.L3.ordinal].target = 1f
        if (buttons and PadBits.R3 != 0) press[Control.R3.ordinal].target = 1f
        for (index in 0 until 4) facePress[index].target = if (faceMask and (1 shl index) != 0) 1f else 0f
        armPress[0].target = if (hatX > 0) 1f else 0f
        armPress[1].target = if (hatY > 0) 1f else 0f
        armPress[2].target = if (hatX < 0) 1f else 0f
        armPress[3].target = if (hatY < 0) 1f else 0f
        leftStick.hold.target = if (leftStick.held) 1f else 0f
        rightStick.hold.target = if (rightStick.held) 1f else 0f
        invalidate()
    }

    override fun onWindowFocusChanged(hasWindowFocus: Boolean) {
        super.onWindowFocusChanged(hasWindowFocus)
        if (!hasWindowFocus) release()
    }

    override fun onDetachedFromWindow() {
        removeCallbacks(turnTimeout)
        release()
        super.onDetachedFromWindow()
    }

    // A mouse's hover and wheel mean nothing here.
    override fun onGenericMotionEvent(event: MotionEvent): Boolean = true

    // ---- Sticks ----------------------------------------------------------

    /** A stick that floats to where the thumb lands. */
    private inner class Stick(val control: Control) {
        var finger = -1
            private set
        val held: Boolean get() = finger >= 0
        private var originX = 0f
        private var originY = 0f
        private var thumbX = 0f
        private var thumbY = 0f
        var ax = 0f
            private set
        var ay = 0f
            private set
        /** How far it leans, 0 to 1 after the dead zone. */
        var amount = 0f
            private set
        var clicked = false
            private set
        private var atRim = false
        private var lastTap = Long.MIN_VALUE / 2
        private var tapX = 0f
        private var tapY = 0f

        // Drawn: where the base is, where the knob is from it, how held it looks.
        var baseX = Float.NaN
        var baseY = Float.NaN
        var knobX = 0f
        var knobY = 0f
        val hold = Ease(PRESS_IN_MS, 160f)
        val ring = Ease(1f, CLICK_RING_MS)

        fun grab(id: Int, x: Float, y: Float, time: Long) {
            finger = id
            originX = x; originY = y
            thumbX = x; thumbY = y
            ax = 0f; ay = 0f; amount = 0f
            atRim = false
            // A quick tap and then a hold, in the same place, clicks the stick.
            clicked = time - lastTap <= DOUBLE_TAP_MS && hypot(x - tapX, y - tapY) <= DOUBLE_TAP_MM * mm
            lastTap = Long.MIN_VALUE / 2
            if (clicked) {
                stickClicked = true
                ring.value = 1f
                ring.target = 0f
            } else {
                grabbed = true
            }
        }

        fun drag(x: Float, y: Float, travel: Float) {
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
                dx = x - originX
                dy = y - originY
            }
            val lean = min(length, travel) / travel
            amount = ((lean - DEAD_ZONE) / (FULL_AT - DEAD_ZONE)).coerceIn(0f, 1f)
            val reach = hypot(dx, dy)
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

        fun letGo(time: Long, tap: Boolean, x: Float, y: Float) {
            if (!held) return
            finger = -1
            ax = 0f; ay = 0f; amount = 0f
            clicked = false
            atRim = false
            if (tap) {
                lastTap = time
                tapX = x; tapY = y
            }
        }

        /** A new layout: an idle stick's base goes straight home. */
        fun rest(pad: PadLayout) {
            val spot = pad.spot(control) ?: return
            if (!held) {
                baseX = spot.x
                baseY = spot.y
                knobX = 0f
                knobY = 0f
            }
        }

        /** Moves the drawing [dt] ms on; true while it is still moving. */
        fun step(dt: Float, spot: PadSpot): Boolean {
            if (baseX.isNaN()) {
                baseX = spot.x
                baseY = spot.y
            }
            val toX = if (held) originX else spot.x
            val toY = if (held) originY else spot.y
            val glide = 1f - exp(-dt / (if (held) BASE_FOLLOW_MS else BASE_HOME_MS))
            baseX += (toX - baseX) * glide
            baseY += (toY - baseY) * glide
            var moving = abs(toX - baseX) > 0.3f || abs(toY - baseY) > 0.3f
            if (!moving) {
                baseX = toX
                baseY = toY
            }
            if (held) {
                // Right under the thumb, as far as the base lets it go.
                val visual = spot.r - spot.a * 0.5f
                var dx = thumbX - baseX
                var dy = thumbY - baseY
                val length = hypot(dx, dy)
                if (length > visual) {
                    dx *= visual / length
                    dy *= visual / length
                }
                knobX = dx
                knobY = dy
            } else if (knobX != 0f || knobY != 0f) {
                val back = 1f - exp(-dt / KNOB_HOME_MS)
                knobX -= knobX * back
                knobY -= knobY * back
                if (abs(knobX) < 0.3f && abs(knobY) < 0.3f) {
                    knobX = 0f
                    knobY = 0f
                } else {
                    moving = true
                }
            }
            if (hold.step(dt)) moving = true
            if (ring.step(dt)) moving = true
            return moving
        }
    }

    // ---- Drawing ---------------------------------------------------------

    private fun restyle() {
        val ink = colors.ink
        faceTints = IntArray(4) { index ->
            // Xbox's hues, softened toward the page's ink so they sit in Portal's palette.
            mix(FACE_HUES[index], ink, if (colors.dark) 0.42f else 0.2f)
        }
        halo.shader = RadialGradient(
            0f, 0f, 1f,
            intArrayOf(withAlpha(colors.accent, 72), withAlpha(colors.accent, 26), withAlpha(colors.accent, 0)),
            floatArrayOf(0f, 0.6f, 1f),
            Shader.TileMode.CLAMP,
        )
    }

    override fun onDraw(canvas: Canvas) {
        val pad = pad ?: return
        val now = SystemClock.uptimeMillis()
        val dt = if (lastFrame == 0L) 16f else (now - lastFrame).coerceIn(1L, 48L).toFloat()
        lastFrame = now
        var moving = stepAppearance(now)
        for (ease in press) if (ease.step(dt)) moving = true
        for (ease in facePress) if (ease.step(dt)) moving = true
        for (ease in armPress) if (ease.step(dt)) moving = true
        pad.spot(Control.LEFT_STICK)?.let { if (leftStick.step(dt, it)) moving = true }
        pad.spot(Control.RIGHT_STICK)?.let { if (rightStick.step(dt, it)) moving = true }
        outline.strokeWidth = max(1f, 1.1f * dp)
        for (index in pad.spots.indices) drawSpot(canvas, pad, pad.spots[index])
        if (moving) postInvalidateOnAnimation() else lastFrame = 0L
    }

    /** Each control's share of the entrance or exit this frame; true while it runs. */
    private fun stepAppearance(now: Long): Boolean {
        if (showing) {
            if (waitingForTurn) {
                shown.fill(0f)
                return false
            }
            val elapsed = (now - appearStart).toFloat()
            var moving = false
            for (control in CONTROLS) {
                val raw = ((elapsed - entranceDelay(control)) / SHOW_MS).coerceIn(0f, 1f)
                shown[control.ordinal] = DECELERATE.getInterpolation(raw)
                if (raw < 1f) moving = true
            }
            return moving
        }
        val done = onHidden
        if (done == null) {
            shown.fill(0f)
            return false
        }
        val raw = ((now - hideStart).toFloat() / HIDE_MS).coerceIn(0f, 1f)
        val left = 1f - ACCELERATE.getInterpolation(raw)
        for (index in shown.indices) shown[index] = hideFrom[index] * left
        if (raw < 1f) return true
        onHidden = null
        voteFrameRate(high = false)
        post(done)
        return false
    }

    /** The shoulders arrive first, then the clusters, then the middle row. */
    private fun entranceDelay(control: Control): Float = when (control.kind) {
        ControlKind.BUMPER, ControlKind.TRIGGER -> 0f
        ControlKind.STICK, ControlKind.FACE -> 45f
        ControlKind.DPAD -> 70f
        ControlKind.BUTTON -> if (control == Control.L3 || control == Control.R3) 90f else 120f
    }

    private fun drawSpot(canvas: Canvas, pad: PadLayout, spot: PadSpot) {
        val appear = shown[spot.control.ordinal]
        if (appear <= 0.004f) return
        val saved = canvas.save()
        // In from the side its thumb is on; the middle row drops in from the top.
        val rise = (1f - appear) * ENTRY_MM * mm
        when {
            spot.control == Control.BACK || spot.control == Control.GUIDE || spot.control == Control.START -> canvas.translate(0f, -rise)
            spot.x < pad.middle -> canvas.translate(-rise, 0f)
            else -> canvas.translate(rise, 0f)
        }
        val grow = 0.9f + 0.1f * appear
        canvas.scale(grow, grow, spot.x, spot.y)
        when (spot.control.kind) {
            ControlKind.STICK -> stickFor(spot.control)?.let { drawStick(canvas, spot, it, appear) }
            ControlKind.DPAD -> drawDpad(canvas, spot, appear)
            ControlKind.FACE -> drawFace(canvas, pad, spot, appear)
            ControlKind.BUMPER, ControlKind.TRIGGER -> drawShoulder(canvas, spot, appear)
            ControlKind.BUTTON -> drawButton(canvas, spot, appear)
        }
        canvas.restoreToCount(saved)
    }

    private fun drawHalo(canvas: Canvas, x: Float, y: Float, radius: Float, strength: Float) {
        if (strength <= 0.01f || radius <= 0f) return
        haloMatrix.setScale(radius, radius)
        haloMatrix.postTranslate(x, y)
        halo.shader?.setLocalMatrix(haloMatrix)
        halo.alpha = (255 * strength).roundToInt().coerceIn(0, 255)
        canvas.drawCircle(x, y, radius, halo)
    }

    /** A round face: the page's surface, turning to the accent as it is pressed. */
    private fun drawDisc(canvas: Canvas, x: Float, y: Float, radius: Float, pressed: Float, fade: Float) {
        fill.color = faded(mix(colors.surface, colors.accent, pressed), fade)
        canvas.drawCircle(x, y, radius, fill)
        outline.color = faded(edgeColor(), fade * (1f - pressed))
        canvas.drawCircle(x, y, radius, outline)
    }

    private fun edgeColor(): Int = withAlpha(colors.muted, if (colors.dark) 58 else 64)

    private fun drawText(canvas: Canvas, text: String, x: Float, y: Float, size: Float, color: Int) {
        label.textSize = size
        label.color = color
        canvas.drawText(text, x, y - (label.ascent() + label.descent()) / 2f, label)
    }

    private fun drawStick(canvas: Canvas, spot: PadSpot, stick: Stick, fade: Float) {
        val held = stick.hold.value
        val x = stick.baseX.takeUnless { it.isNaN() } ?: spot.x
        val y = stick.baseY.takeUnless { it.isNaN() } ?: spot.y
        val radius = spot.r
        drawHalo(canvas, x, y, radius * 1.3f, held * 0.9f * fade)
        fill.color = faded(colors.surface, fade)
        canvas.drawCircle(x, y, radius, fill)
        outline.color = faded(edgeColor(), fade)
        canvas.drawCircle(x, y, radius, outline)
        // Where full tilt is: a faint ring the knob reaches at the rim.
        outline.color = faded(withAlpha(colors.muted, if (colors.dark) 22 else 26), fade)
        canvas.drawCircle(x, y, radius - spot.a * 0.5f, outline)
        if (stick.amount > 0.02f && stick.held) {
            // An arc on the rim shows where and how far the stick leans.
            val angle = Math.toDegrees(atan2(stick.ay.toDouble(), stick.ax.toDouble())).toFloat()
            stroke.strokeWidth = 3f * dp
            stroke.color = faded(colors.accent, fade * (0.35f + 0.65f * stick.amount))
            val inset = stroke.strokeWidth
            rect.set(x - radius + inset, y - radius + inset, x + radius - inset, y + radius - inset)
            val sweep = 40f + 30f * stick.amount
            canvas.drawArc(rect, angle - sweep / 2f, sweep, false, stroke)
        }
        val knobX = x + stick.knobX
        val knobY = y + stick.knobY
        val ring = stick.ring.value
        if (ring > 0.01f) {
            // A stick click rings out from the knob.
            stroke.strokeWidth = 2f * dp
            stroke.color = faded(colors.accent, fade * ring)
            canvas.drawCircle(knobX, knobY, spot.a * (1.15f + 0.9f * (1f - ring)), stroke)
        }
        fill.color = faded(mix(mix(colors.surface, colors.ink, if (colors.dark) 0.1f else 0.04f), colors.accent, held), fade)
        canvas.drawCircle(knobX, knobY, spot.a, fill)
        outline.color = faded(edgeColor(), fade * (1f - held))
        canvas.drawCircle(knobX, knobY, spot.a, outline)
        val name = if (stick.control == Control.LEFT_STICK) "L" else "R"
        drawText(canvas, if (stick.clicked) name + "3" else name, knobX, knobY, spot.a * 0.62f,
            faded(mix(colors.muted, colors.accentInk, held), fade))
    }

    private fun drawDpad(canvas: Canvas, spot: PadSpot, fade: Float) {
        val pressing = max(max(armPress[0].value, armPress[1].value), max(armPress[2].value, armPress[3].value))
        drawHalo(canvas, spot.x, spot.y, spot.r * 1.25f, pressing * 0.7f * fade)
        fill.color = faded(colors.surface, fade)
        canvas.drawPath(cross, fill)
        val arm = spot.a
        val half = spot.r
        val corner = arm * 0.3f
        for (index in 0 until 4) {
            val lit = armPress[index].value
            if (lit <= 0.01f) continue
            val dx = ARM_X[index]
            val dy = ARM_Y[index]
            // The held arm, from just short of the middle to its end.
            if (dx != 0) {
                rect.set(min(spot.x + dx * arm * 0.5f, spot.x + dx * half), spot.y - arm / 2f,
                    max(spot.x + dx * arm * 0.5f, spot.x + dx * half), spot.y + arm / 2f)
            } else {
                rect.set(spot.x - arm / 2f, min(spot.y + dy * arm * 0.5f, spot.y + dy * half),
                    spot.x + arm / 2f, max(spot.y + dy * arm * 0.5f, spot.y + dy * half))
            }
            fill.color = faded(colors.accent, fade * lit)
            canvas.drawRoundRect(rect, corner, corner, fill)
        }
        outline.color = faded(edgeColor(), fade)
        canvas.drawPath(cross, outline)
        // A chevron near each arm's end.
        val reach = half - arm * 0.55f
        val chevron = arm * 0.2f
        for (index in 0 until 4) {
            val dx = ARM_X[index]
            val dy = ARM_Y[index]
            val tipX = spot.x + dx * (reach + chevron * 0.6f)
            val tipY = spot.y + dy * (reach + chevron * 0.6f)
            val baseX = spot.x + dx * (reach - chevron * 0.6f)
            val baseY = spot.y + dy * (reach - chevron * 0.6f)
            path.reset()
            path.moveTo(tipX, tipY)
            path.lineTo(baseX + dy * chevron, baseY + dx * chevron)
            path.lineTo(baseX - dy * chevron, baseY - dx * chevron)
            path.close()
            fill.color = faded(mix(colors.muted, colors.accentInk, armPress[index].value), fade)
            canvas.drawPath(path, fill)
        }
    }

    private fun drawFace(canvas: Canvas, pad: PadLayout, spot: PadSpot, fade: Float) {
        for (index in 0 until 4) {
            val x = pad.faceButtonX(spot, index)
            val y = pad.faceButtonY(spot, index)
            val lit = facePress[index].value
            drawHalo(canvas, x, y, spot.a * 1.7f, lit * fade)
            val radius = spot.a * (1f - PRESS_SQUEEZE * lit)
            drawDisc(canvas, x, y, radius, lit, fade)
            drawText(canvas, PadLayout.FACE_LABELS[index], x, y, spot.a * 0.92f, faded(mix(faceTints[index], colors.accentInk, lit), fade))
        }
    }

    private fun drawShoulder(canvas: Canvas, spot: PadSpot, fade: Float) {
        val trigger = spot.control.kind == ControlKind.TRIGGER
        val lit = press[spot.control.ordinal].value
        val held = press[spot.control.ordinal].target > 0f
        val squeeze = 1f - PRESS_SQUEEZE * 0.6f * (if (held) 1f else lit)
        val halfW = spot.w / 2f * squeeze
        val halfH = spot.h / 2f * squeeze
        rect.set(spot.x - halfW, spot.y - halfH, spot.x + halfW, spot.y + halfH)
        val corner = if (trigger) rect.height() * 0.36f else rect.height() / 2f
        drawHalo(canvas, spot.x, spot.y, max(halfW, halfH) * 1.15f, lit * 0.6f * fade)
        if (trigger && analogTriggers) {
            // Pressed in from the screen's edge, as far as the trigger is pulled.
            fill.color = faded(colors.surface, fade)
            canvas.drawRoundRect(rect, corner, corner, fill)
            if (lit > 0.01f) {
                val saved = canvas.save()
                canvas.clipRect(rect.left, rect.top, rect.right, rect.top + rect.height() * lit)
                fill.color = faded(colors.accent, fade)
                canvas.drawRoundRect(rect, corner, corner, fill)
                canvas.restoreToCount(saved)
            }
            outline.color = faded(edgeColor(), fade * (1f - lit))
            canvas.drawRoundRect(rect, corner, corner, outline)
            drawText(canvas, spot.control.title, spot.x, spot.y, spot.h * 0.4f,
                faded(if (lit > 0.5f) colors.accentInk else colors.ink, fade))
            return
        }
        fill.color = faded(mix(colors.surface, colors.accent, lit), fade)
        canvas.drawRoundRect(rect, corner, corner, fill)
        outline.color = faded(edgeColor(), fade * (1f - lit))
        canvas.drawRoundRect(rect, corner, corner, outline)
        drawText(canvas, spot.control.title, spot.x, spot.y, spot.h * 0.4f, faded(mix(colors.ink, colors.accentInk, lit), fade))
    }

    private fun drawButton(canvas: Canvas, spot: PadSpot, fade: Float) {
        val lit = press[spot.control.ordinal].value
        val x = spot.x
        val y = spot.y
        val radius = spot.r * (1f - PRESS_SQUEEZE * lit)
        drawHalo(canvas, x, y, spot.r * 1.6f, lit * fade)
        drawDisc(canvas, x, y, radius, lit, fade)
        val ink = faded(mix(colors.ink, colors.accentInk, lit), fade)
        stroke.color = ink
        stroke.strokeWidth = max(1.2f * dp, radius * 0.1f)
        fill.color = ink
        val glyph = radius * 0.42f
        when (spot.control) {
            Control.BACK -> {
                // Two windows, one over the other: the View mark.
                val size = glyph * 0.9f
                val corner = size * 0.25f
                val gap = stroke.strokeWidth
                rect.set(x - size * 0.4f - gap, y - size * 0.4f - gap, x + size + gap, y + size + gap)
                val saved = canvas.save()
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    canvas.clipOutRect(rect)
                } else {
                    @Suppress("DEPRECATION")
                    canvas.clipRect(rect, Region.Op.DIFFERENCE)
                }
                rect.set(x - size, y - size, x + size * 0.4f, y + size * 0.4f)
                canvas.drawRoundRect(rect, corner, corner, stroke)
                canvas.restoreToCount(saved)
                rect.set(x - size * 0.4f, y - size * 0.4f, x + size, y + size)
                canvas.drawRoundRect(rect, corner, corner, stroke)
            }
            Control.START -> {
                // Three lines: the Menu mark.
                for (row in -1..1) {
                    val lineY = y + row * glyph * 0.62f
                    canvas.drawLine(x - glyph * 0.85f, lineY, x + glyph * 0.85f, lineY, stroke)
                }
            }
            Control.GUIDE -> {
                canvas.drawCircle(x, y, glyph * 1.05f, stroke)
                canvas.drawCircle(x, y, glyph * 0.36f, fill)
            }
            else -> drawText(canvas, spot.control.title, x, y, radius * 0.66f, ink)
        }
    }

    /** A value easing toward its target: [rise] ms going up, [fall] ms coming down. */
    private class Ease(private val rise: Float, private val fall: Float) {
        var value = 0f
        var target = 0f

        /** Moves [dt] ms on; true while it is still moving. */
        fun step(dt: Float): Boolean {
            if (value == target) return false
            val tau = if (target > value) rise else fall
            value += (target - value) * (1f - exp(-dt / tau))
            if (abs(target - value) < 0.004f) value = target
            return value != target
        }
    }

    companion object {
        private const val TAG = "PortalController"
        private const val MM_PER_INCH = 25.4f
        private val CONTROLS = Control.entries.toTypedArray()
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
        private val ARM_X = intArrayOf(1, 0, -1, 0)
        private val ARM_Y = intArrayOf(0, 1, 0, -1)

        /** Face buttons, in button radii (the middle in spreads). */
        private const val FACE_MIDDLE = 0.42f
        private const val FACE_KEEP = 0.15f
        private const val FACE_LENS = 0.32f
        private const val FACE_BOTH = 1.75f
        private const val FACE_SLIDE_OFF = 0.5f
        private val FACE_HUES = intArrayOf(
            Color.rgb(0x5E, 0xC2, 0x3C),
            Color.rgb(0xE8, 0x4A, 0x3F),
            Color.rgb(0x3A, 0x8D, 0xE6),
            Color.rgb(0xF2, 0xC2, 0x2E),
        )

        private const val PRESS_IN_MS = 14f
        private const val PRESS_OUT_MS = 90f
        private const val PRESS_SQUEEZE = 0.06f
        private const val BASE_FOLLOW_MS = 32f
        private const val BASE_HOME_MS = 120f
        private const val KNOB_HOME_MS = 55f
        private const val CLICK_RING_MS = 220f

        private const val SHOW_MS = 420f
        private const val HIDE_MS = 170f
        private const val ENTRY_MM = 4f
        private const val TURN_WAIT_MS = 700L
        private const val SUSPENDED_ALPHA = 0.55f

        private const val EXCLUDED_DP = 196f
        private const val EDGE_DP = 40f

        // Portal's emphasized curves.
        private val DECELERATE = PathInterpolator(0.05f, 0.7f, 0.1f, 1f)
        private val ACCELERATE = PathInterpolator(0.3f, 0f, 0.8f, 0.15f)

        private fun withAlpha(color: Int, alpha: Int): Int = (color and 0x00FFFFFF) or (alpha.coerceIn(0, 255) shl 24)

        private fun faded(color: Int, fade: Float): Int =
            withAlpha(color, (Color.alpha(color) * fade.coerceIn(0f, 1f)).roundToInt())

        /** ARGB from [from] toward [to] by [t], alpha included. */
        private fun mix(from: Int, to: Int, t: Float): Int {
            if (t <= 0f) return from
            if (t >= 1f) return to
            fun channel(shift: Int): Int {
                val a = (from ushr shift) and 0xFF
                val b = (to ushr shift) and 0xFF
                return (a + (b - a) * t).roundToInt().coerceIn(0, 255) shl shift
            }
            return channel(24) or channel(16) or channel(8) or channel(0)
        }
    }
}
