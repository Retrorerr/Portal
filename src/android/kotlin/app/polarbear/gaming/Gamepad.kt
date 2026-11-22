package app.polarbear.gaming

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.DashPathEffect
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
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import android.view.WindowInsets
import android.view.animation.PathInterpolator
import android.widget.FrameLayout
import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.exp
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt
import kotlin.math.sqrt

/** Where the controller's state goes: the guest's touch pad. */
internal fun interface PadSink {
    /**
     * The whole pad: [buttons] in xpad order ([PadBits]), [axes] laid out as
     * GamepadBridge's. False when it could not go to the guest.
     */
    fun send(buttons: Int, axes: FloatArray): Boolean
}

/**
 * Portal's palette as the controller paints it (ARGB): the surface, outline,
 * ink and orange accent of Portal's own buttons.
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
        /** Portal's dark reading, until the touchpad page hands over its live palette. */
        val Dark = PadColors(
            surface = 0x16F1EBDD,
            edge = 0x3ABDB7AA,
            ink = 0xFFF1EBDD.toInt(),
            muted = 0xC4BDB7AA.toInt(),
            accent = 0xFFF07949.toInt(),
            accentInk = 0xFF191B1C.toInt(),
            dark = true,
        )

        /** Over a game frame: Portal's charcoal glass and ivory, legible on any picture. */
        val OverGame = PadColors(
            surface = 0x9C191B1C.toInt(),
            edge = 0x5CF1EBDD,
            ink = 0xF2F1EBDD.toInt(),
            muted = 0xB8F1EBDD.toInt(),
            accent = 0xFFF07949.toInt(),
            accentInk = 0xFF191B1C.toInt(),
            dark = true,
        )
    }
}

/**
 * Portal's on-screen game controller: an Xbox pad sized in millimetres for
 * thumbs on any screen ([PadLayout]), with the feel of [PadInput], painted
 * in Portal's palette and felt through [PadHaptics]. It has two homes:
 *
 * - [wholeScreen]: the touchpad page's controller while the desktop is on a
 *   monitor. One [GamepadView] takes every finger, so a thumb may land
 *   anywhere and a stick takes any landing on its half.
 * - Floating over the desktop: one [GamepadView] per control ([floatIn]), so
 *   Android splits fingers between them and a finger that misses every
 *   control still reaches the game. The player may move, resize and hide
 *   controls ([elements], [editing]) and set their [opacity].
 *
 * Touches arrive unbuffered and the state goes out on the event that changed
 * it; only the views whose controls changed redraw. UI thread.
 */
internal class Gamepad(
    private val context: Context,
    private val sink: PadSink,
    private val haptics: PadHaptics,
    val wholeScreen: Boolean,
) {
    private val dp = context.resources.displayMetrics.density

    /** Physical px per millimetre: thumbs are the same size on every screen. */
    private val mm: Float = context.resources.displayMetrics.let { metrics ->
        val physical = (metrics.xdpi + metrics.ydpi) / 2f
        val nominal = metrics.densityDpi.toFloat()
        // Some panels report a placeholder; Android's own density is the better guess then.
        (if (physical > nominal * 0.6f && physical < nominal * 1.6f) physical else nominal) / MM_PER_INCH
    }
    // Only the layout editor needs it (and a bare process has no window manager to ask).
    private val slop by lazy { ViewConfiguration.get(context).scaledTouchSlop.toFloat() }
    private val main = Handler(Looper.getMainLooper())
    private val input = PadInput(mm)

    // ---- What it is laid out for -------------------------------------------

    private var width = 0
    private var height = 0
    private val insets = Rect()

    private var layout: PadLayout? = null
    private var relayoutWaiting = false

    /** The player's Size setting. */
    var userScale = 1f
        set(value) {
            if (field == value) return
            field = value
            relayout()
        }

    /** Lowest edge of the chip at the top centre, px; the middle row stays below it. */
    var chipBottom = 0f
        set(value) {
            if (field == value) return
            field = value
            relayout()
        }

    /** Floating: where the player moved, resized or hid each control. */
    var elements: Map<Control, ElementState> = emptyMap()
        set(value) {
            if (field == value) return
            field = value
            relayout()
        }

    var colors: PadColors = PadColors.Dark
        set(value) {
            if (field == value) return
            field = value
            restyle()
            invalidateAll()
        }

    var analogTriggers: Boolean
        get() = input.analogTriggers
        set(value) {
            if (input.analogTriggers == value) return
            input.analogTriggers = value
            invalidateAll()
        }

    /** Floating: how much of the game shows through the controls. */
    var opacity = 1f
        set(value) {
            if (field == value) return
            field = value
            applyAlpha(animate = false)
        }

    /** A Portal page is open over the controller: every touch ends and the controls dim until it closes. */
    var suspended = false
        set(value) {
            if (field == value) return
            field = value
            if (value) release()
            applyAlpha(animate = true)
        }

    // ---- Layout editor (floating) --------------------------------------------

    /** The player is arranging the controls: touches move them, and hidden ones show faintly. */
    var editing = false
        set(value) {
            if (field == value) return
            field = value
            // Touches held in one mode mean nothing in the other.
            release()
            syncViews()
            invalidateAll()
        }

    var selected: Control? = null
        set(value) {
            if (field == value) return
            field = value
            invalidateAll()
        }

    var onSelect: (Control) -> Unit = {}

    /** A control was dropped with its centre at ([x], [y]) px. */
    var onMoved: (Control, Float, Float) -> Unit = { _, _, _ -> }

    /** The view whose touch moves a control, and the control's own view that moves. */
    private var dragSource: GamepadView? = null
    private var dragView: GamepadView? = null
    private var dragId = -1
    private var dragStartX = 0f
    private var dragStartY = 0f
    private var dragging = false

    // ---- Views -----------------------------------------------------------------

    private val views = ArrayList<GamepadView>()
    /** The view that draws and catches each control, by [Control.ordinal]. */
    private val owners = arrayOfNulls<GamepadView>(CONTROLS.size)
    private var parent: FrameLayout? = null
    private var index: () -> Int = { 0 }

    // ---- What went to the guest --------------------------------------------------

    private var sentButtons = -1
    private val sentAxes = FloatArray(10)

    // ---- Drawing state -----------------------------------------------------------

    /** How pressed each control looks, by [Control.ordinal]; a trigger's is its level. */
    private val press = Array(CONTROLS.size) { Ease(PRESS_IN_MS, PRESS_OUT_MS) }
    private val facePress = Array(4) { Ease(PRESS_IN_MS, PRESS_OUT_MS) }
    /** D-pad arms: east, south, west, north. */
    private val armPress = Array(4) { Ease(PRESS_IN_MS, PRESS_OUT_MS) }
    private val leftLook = StickLook(input.leftStick)
    private val rightLook = StickLook(input.rightStick)
    private val dirty = BooleanArray(CONTROLS.size)

    private var showing = false
    private var waitingForTurn = false
    private var appearStart = 0L
    private var hideStart = 0L
    private var onHidden: (() -> Unit)? = null
    private val hideFrom = FloatArray(CONTROLS.size)

    private val fill = Paint(Paint.ANTI_ALIAS_FLAG)
    private val outline = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.STROKE }
    private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
    }
    private val frame = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.STROKE }
    private val dashes = DashPathEffect(floatArrayOf(6f * dp, 5f * dp), 0f)
    private val label = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        textAlign = Paint.Align.CENTER
        typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
    }
    private val halo = Paint(Paint.ANTI_ALIAS_FLAG)
    private val haloMatrix = Matrix()
    private val rect = RectF()
    private val path = Path()
    private val cross = Path()
    private val crossFrame = Path()
    private var faceTints = IntArray(4)

    private val turnTimeout = Runnable { if (waitingForTurn) startEntrance() }
    private val finishHide = Runnable {
        val done = onHidden ?: return@Runnable
        onHidden = null
        views.forEach { it.awake(false) }
        syncViews()
        done()
    }

    init {
        restyle()
    }

    // ---- Homes ---------------------------------------------------------------------

    /** The whole-screen controller's one view; the caller adds it over its page, full size. */
    fun screenView(): GamepadView {
        check(wholeScreen)
        views.firstOrNull()?.let { return it }
        val view = GamepadView(context, this, CONTROLS.toList())
        view.contentDescription = "Controller"
        adopt(view)
        return view
    }

    /**
     * Floating: the controls' views go into [parent] at [index] (above the
     * desktop, below Portal's own buttons) while the controller shows.
     */
    fun floatIn(parent: FrameLayout, index: () -> Int) {
        check(!wholeScreen)
        if (this.parent !== parent) {
            for (control in CONTROLS) drop(control)
            this.parent = parent
        }
        this.index = index
        syncViews()
    }

    /** Floating: gone at once, every view with it, until [floatIn] again. */
    fun detach() {
        release()
        main.removeCallbacks(turnTimeout)
        main.removeCallbacks(finishHide)
        showing = false
        waitingForTurn = false
        onHidden = null
        for (control in CONTROLS) drop(control)
        parent = null
    }

    /** Floating: the highest place a control's view holds in its parent, or -1; Portal's own buttons go above. */
    fun topIndex(): Int = views.maxOfOrNull { view -> (view.parent as? FrameLayout)?.indexOfChild(view) ?: -1 } ?: -1

    private fun adopt(view: GamepadView) {
        views += view
        for (control in view.controls) owners[control.ordinal] = view
        view.alpha = alphaTarget()
        view.awake(showing || onHidden != null)
    }

    private fun drop(control: Control) {
        val view = owners[control.ordinal] ?: return
        owners[control.ordinal] = null
        if (view.controls.none { owners[it.ordinal] === view }) {
            views.remove(view)
            if (dragView === view || dragSource === view) endDrag()
            view.animate().cancel()
            (view.parent as? FrameLayout)?.removeView(view)
        }
    }

    /** Floating: a view for each control that shows now, each where its control is. */
    private fun syncViews() {
        if (wholeScreen) return
        val parent = parent ?: return
        val pad = layout
        val present = showing || onHidden != null
        for (control in CONTROLS) {
            val spot = pad?.spot(control)
            val wanted = present && spot != null && (!spot.hidden || editing)
            val view = owners[control.ordinal]
            if (!wanted) {
                drop(control)
            } else if (view == null) {
                val added = GamepadView(context, this, listOf(control))
                added.contentDescription = control.title
                adopt(added)
                parent.addView(added, index().coerceIn(0, parent.childCount), FrameLayout.LayoutParams(1, 1, Gravity.TOP or Gravity.START))
            }
        }
        if (pad != null) for (view in views) place(view, pad)
    }

    /** Floating: [view] covers everything its control may draw, and every touch it takes. */
    private fun place(view: GamepadView, pad: PadLayout) {
        val spot = pad.spot(view.controls.first()) ?: return
        extent(spot, pad, rect)
        val params = view.layoutParams as? FrameLayout.LayoutParams ?: FrameLayout.LayoutParams(1, 1, Gravity.TOP or Gravity.START)
        val left = rect.left.roundToInt()
        val top = rect.top.roundToInt()
        val w = rect.width().roundToInt()
        val h = rect.height().roundToInt()
        if (dragView !== view) {
            view.translationX = 0f
            view.translationY = 0f
        }
        if (params.leftMargin == left && params.topMargin == top && params.width == w && params.height == h) return
        params.gravity = Gravity.TOP or Gravity.START
        params.leftMargin = left
        params.topMargin = top
        params.width = w
        params.height = h
        view.layoutParams = params
    }

    /** Everything [spot] may draw, px: its glow, a stick's drifting base, the edit frame, the entrance's slide. */
    private fun extent(spot: PadSpot, pad: PadLayout, out: RectF) {
        touchBounds(spot, out)
        val glow = when (spot.control.kind) {
            ControlKind.STICK -> pad.drift(spot) + max(spot.r * 1.3f, spot.r + spot.a * 1.6f)
            ControlKind.DPAD -> spot.r * 1.25f
            ControlKind.FACE -> spot.b + spot.a * 1.7f
            ControlKind.BUTTON -> spot.r * 1.6f
            ControlKind.BUMPER, ControlKind.TRIGGER -> max(spot.w, spot.h) / 2f * 1.15f
        }
        out.union(spot.x - glow, spot.y - glow, spot.x + glow, spot.y + glow)
        val margin = ENTRY_MM * mm + 2f * dp
        out.inset(-margin, -margin)
    }

    /** Where [spot] takes a landing finger, px. */
    private fun touchBounds(spot: PadSpot, out: RectF) {
        val halfW = when (spot.control.kind) {
            ControlKind.BUMPER, ControlKind.TRIGGER -> spot.w / 2f
            ControlKind.FACE -> spot.b + spot.a
            else -> spot.r
        } + spot.reach
        val halfH = if (spot.control.kind == ControlKind.BUMPER || spot.control.kind == ControlKind.TRIGGER) spot.h / 2f + spot.reach else halfW
        out.set(spot.x - halfW, spot.y - halfH, spot.x + halfW, spot.y + halfH)
    }

    // ---- Coming and going ------------------------------------------------------------

    /**
     * Comes in: the controls rise into place, the shoulders first. With
     * [waitForTurn] while the screen is upright, Android is turning it
     * sideways, so they wait for that (or a moment) and arrive in place.
     */
    fun show(waitForTurn: Boolean = false) {
        if (showing) return
        main.removeCallbacks(finishHide)
        onHidden = null
        showing = true
        syncViews()
        views.forEach { it.awake(true) }
        // Plug the pad in now, so the game sees it before the first touch.
        release()
        sentButtons = -1
        send()
        if (waitForTurn && height > width) {
            waitingForTurn = true
            main.removeCallbacks(turnTimeout)
            main.postDelayed(turnTimeout, TURN_WAIT_MS)
            invalidateAll()
        } else {
            startEntrance()
        }
    }

    private fun startEntrance() {
        waitingForTurn = false
        main.removeCallbacks(turnTimeout)
        appearStart = SystemClock.uptimeMillis()
        invalidateAll()
    }

    /** Goes: every touch ends now; [done] runs once the controls have faded. */
    fun hide(done: () -> Unit = {}) {
        release()
        main.removeCallbacks(turnTimeout)
        if (!showing) {
            if (onHidden != null) onHidden = done else done()
            return
        }
        val now = SystemClock.uptimeMillis()
        for (control in CONTROLS) hideFrom[control.ordinal] = appear(control, now)
        showing = false
        waitingForTurn = false
        onHidden = done
        hideStart = now
        main.removeCallbacks(finishHide)
        main.postDelayed(finishHide, HIDE_MS.toLong())
        invalidateAll()
    }

    /** Every finger lets go and the pad goes back to rest. */
    fun release() {
        cancelDrag()
        input.liftAll(null, SystemClock.uptimeMillis())
        publish()
    }

    private fun alphaTarget(): Float = (if (wholeScreen) 1f else opacity) * (if (suspended) SUSPENDED_ALPHA else 1f)

    private fun applyAlpha(animate: Boolean) {
        val target = alphaTarget()
        for (view in views) {
            view.animate().cancel()
            if (animate) {
                view.animate().alpha(target)
                    .setDuration(if (suspended) 160L else 260L)
                    .setInterpolator(if (suspended) ACCELERATE else DECELERATE)
                    .start()
            } else {
                view.alpha = target
            }
        }
    }

    // ---- Layout ----------------------------------------------------------------------

    /** The screen (whole screen) or the desktop (floating) is [width] x [height] px. */
    fun resize(width: Int, height: Int) {
        if (width == this.width && height == this.height) return
        this.width = width
        this.height = height
        // A turn mid-gesture: nothing held means the same on the new screen.
        release()
        relayout()
        if (waitingForTurn && width >= height) startEntrance()
    }

    /** The edges the system bars keep, px. */
    fun setInsets(next: Rect) {
        if (next == insets) return
        insets.set(next)
        relayout()
    }

    /** Lays the controls out for the screen as it is; while fingers are down, once they lift. */
    private fun relayout() {
        if (width <= 0 || height <= 0) return
        if (input.fingerCount > 0 || dragView != null) {
            relayoutWaiting = true
            return
        }
        relayoutWaiting = false
        val next = PadLayout(
            width.toFloat(), height.toFloat(),
            insets.left.toFloat(), insets.top.toFloat(), insets.right.toFloat(), insets.bottom.toFloat(),
            mm, userScale, wholeScreen, chipBottom, elements,
        )
        layout = next
        input.layout = next
        // One line per layout, for bug reports and the device check: where each control sits, px.
        Log.i(TAG, "${if (wholeScreen) "screen" else "floating"} layout ${width}x$height at ${(mm * 100).roundToInt() / 100f} px/mm, " +
            "size ${(userScale * 100).roundToInt()}%: " +
            next.spots.joinToString(" ") { "${it.control.name}@${it.x.roundToInt()},${it.y.roundToInt()},r${it.r.roundToInt()},b${it.b.roundToInt()}${if (it.hidden) ",hidden" else ""}" })
        next.spot(Control.DPAD)?.let { spot ->
            crossPath(cross, spot, 0f)
            crossPath(crossFrame, spot, frameGap(next))
        }
        leftLook.rest(next)
        rightLook.rest(next)
        syncViews()
        if (wholeScreen) views.firstOrNull()?.let { excludeEdges(it, next) }
        invalidateAll()
    }

    /** The d-pad's cross into [out], grown by [grow] px all round. */
    private fun crossPath(out: Path, spot: PadSpot, grow: Float) {
        val half = spot.r + grow
        val arm = spot.a / 2f + grow
        val corner = spot.a * 0.3f + grow
        val horizontal = Path().apply {
            addRoundRect(RectF(spot.x - half, spot.y - arm, spot.x + half, spot.y + arm), corner, corner, Path.Direction.CW)
        }
        val vertical = Path().apply {
            addRoundRect(RectF(spot.x - arm, spot.y - half, spot.x + arm, spot.y + half), corner, corner, Path.Direction.CW)
        }
        out.reset()
        out.op(horizontal, vertical, Path.Op.UNION)
    }

    /**
     * On the whole screen a thumb sliding in from a side edge onto a stick
     * must not turn into Android's Back gesture. Android honours 200 dp of
     * each edge; this takes it where the thumbs rest. Floating controls sit
     * clear of the edges' gesture strips, so they need none.
     */
    private fun excludeEdges(view: View, pad: PadLayout) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
        val band = (EXCLUDED_DP * dp / 2f).toInt()
        val edge = (EDGE_DP * dp).toInt()
        val leftY = (pad.spot(Control.LEFT_STICK)?.y ?: height / 2f).toInt()
        val face = pad.spot(Control.FACE)?.y ?: (height / 2f)
        val rightY = ((face + (pad.spot(Control.RIGHT_STICK)?.y ?: face)) / 2f).toInt()
        view.systemGestureExclusionRects = listOf(
            Rect(0, leftY - band, edge, leftY + band),
            Rect(width - edge, rightY - band, width, rightY + band),
        )
    }

    // ---- Touch ---------------------------------------------------------------------

    /** [view]'s touch; false when a first finger lands on no control and belongs to what is below. */
    internal fun touch(view: GamepadView, event: MotionEvent): Boolean {
        if (editing) return edit(view, event)
        if (layout == null || !showing || waitingForTurn || suspended) {
            if (input.fingerCount > 0) release()
            return wholeScreen
        }
        // A view sees its own corner of the screen; the controller works in the whole screen's.
        val dx = view.x
        val dy = view.y
        val time = event.eventTime
        val action = event.actionMasked
        // A finger that left without its UP must not hold a stick where it was.
        if (action != MotionEvent.ACTION_DOWN) input.dropMissing(view, time) { event.findPointerIndex(it) >= 0 }
        var took = true
        when (action) {
            MotionEvent.ACTION_DOWN -> {
                input.liftAll(view, time)
                took = input.land(view, event.getPointerId(0), event.getX(0) + dx, event.getY(0) + dy, time)
                // Moves come as the panel samples them, not once a frame.
                if (took || wholeScreen) view.requestUnbufferedDispatch(event)
            }
            MotionEvent.ACTION_POINTER_DOWN -> {
                val index = event.actionIndex
                input.land(view, event.getPointerId(index), event.getX(index) + dx, event.getY(index) + dy, time)
            }
            MotionEvent.ACTION_POINTER_UP -> input.lift(
                event.getPointerId(event.actionIndex), time,
                cancelled = event.flags and MotionEvent.FLAG_CANCELED != 0,
            )
            MotionEvent.ACTION_UP -> {
                input.lift(event.getPointerId(0), time, cancelled = event.flags and MotionEvent.FLAG_CANCELED != 0)
                input.liftAll(view, time)
            }
            MotionEvent.ACTION_CANCEL -> input.liftAll(view, time)
        }
        // Every event carries fresh positions for every finger still down.
        if (action != MotionEvent.ACTION_UP && action != MotionEvent.ACTION_CANCEL) {
            for (index in 0 until event.pointerCount) input.move(event.getPointerId(index), event.getX(index) + dx, event.getY(index) + dy)
        }
        publish()
        if (input.fingerCount == 0 && relayoutWaiting) relayout()
        return took || wholeScreen
    }

    /**
     * The layout editor: a touch selects the control nearest it, and a drag
     * moves that control's view. Views overlap where their controls are
     * close, so the one that catches a touch may move a neighbour.
     */
    private fun edit(view: GamepadView, event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                cancelDrag()
                // The whole screen's coordinates: a view moves with the finger dragging it.
                val x = view.x + event.getX(0)
                val y = view.y + event.getY(0)
                val control = layout?.pick(x, y, hidden = true)?.control ?: return false
                dragView = owners[control.ordinal] ?: return false
                dragSource = view
                dragId = event.getPointerId(0)
                dragStartX = x
                dragStartY = y
                dragging = false
                onSelect(control)
            }
            MotionEvent.ACTION_MOVE -> {
                if (dragSource !== view) return true
                val moving = dragView ?: return true
                val index = event.findPointerIndex(dragId)
                if (index < 0) return true
                val x = view.x + event.getX(index)
                val y = view.y + event.getY(index)
                if (!dragging && hypot(x - dragStartX, y - dragStartY) > slop) dragging = true
                val spot = layout?.spot(moving.controls.first())
                if (dragging && spot != null) {
                    // The control stays whole on screen.
                    val halfW = spot.w / 2f
                    val halfH = spot.h / 2f
                    val cx = (spot.x + x - dragStartX).coerceIn(halfW, max(halfW, width - halfW))
                    val cy = (spot.y + y - dragStartY).coerceIn(halfH, max(halfH, height - halfH))
                    moving.translationX = cx - spot.x
                    moving.translationY = cy - spot.y
                }
            }
            MotionEvent.ACTION_UP -> if (dragSource === view) {
                val moving = dragView
                val moved = dragging
                val spot = moving?.let { layout?.spot(it.controls.first()) }
                if (moving != null && moved && spot != null) {
                    // The new layout puts the view where it was dropped.
                    dragSource = null
                    dragView = null
                    dragging = false
                    onMoved(spot.control, spot.x + moving.translationX, spot.y + moving.translationY)
                }
                cancelDrag()
            }
            MotionEvent.ACTION_CANCEL -> if (dragSource === view) cancelDrag()
        }
        return true
    }

    /** Ends a drag in the editor; the control goes back where it was. */
    private fun cancelDrag() {
        endDrag()
        if (relayoutWaiting) relayout()
    }

    private fun endDrag() {
        val view = dragView
        dragSource = null
        dragView = null
        dragging = false
        view?.translationX = 0f
        view?.translationY = 0f
    }

    /** Sends the pad if it changed, lets the thumbs feel what changed, and redraws what moved. */
    private fun publish() {
        input.publish()?.let(haptics::play)
        send()
        aim()
    }

    private fun send() {
        if (input.buttons == sentButtons && input.axes.contentEquals(sentAxes)) return
        // Only what reached the guest counts: a dropped release goes again with the next event.
        if (!sink.send(input.buttons, input.axes)) {
            sentButtons = -1
            return
        }
        sentButtons = input.buttons
        System.arraycopy(input.axes, 0, sentAxes, 0, sentAxes.size)
    }

    /** Points every drawn press at the pad as it now is, and redraws the controls that changed. */
    private fun aim() {
        dirty.fill(false)
        val targets = FloatArray(CONTROLS.size)
        for (index in 0 until input.fingerCount) {
            val finger = input.finger(index)
            val spot = finger.spot ?: continue
            when (spot.control.kind) {
                ControlKind.BUMPER, ControlKind.BUTTON -> targets[spot.control.ordinal] = 1f
                ControlKind.TRIGGER -> targets[spot.control.ordinal] = max(targets[spot.control.ordinal], input.triggerLevel(spot, finger.y))
                else -> Unit
            }
        }
        // A clicked stick lights its own button too: the game sees one L3.
        if (input.buttons and PadBits.L3 != 0) targets[Control.L3.ordinal] = 1f
        if (input.buttons and PadBits.R3 != 0) targets[Control.R3.ordinal] = 1f
        for (control in CONTROLS) aimAt(press[control.ordinal], targets[control.ordinal], control)
        for (index in 0 until 4) aimAt(facePress[index], if (input.faceMask and (1 shl index) != 0) 1f else 0f, Control.FACE)
        aimAt(armPress[0], if (input.hatX > 0) 1f else 0f, Control.DPAD)
        aimAt(armPress[1], if (input.hatY > 0) 1f else 0f, Control.DPAD)
        aimAt(armPress[2], if (input.hatX < 0) 1f else 0f, Control.DPAD)
        aimAt(armPress[3], if (input.hatY < 0) 1f else 0f, Control.DPAD)
        leftLook.aim()
        rightLook.aim()
        for (control in CONTROLS) if (dirty[control.ordinal]) owners[control.ordinal]?.invalidate()
    }

    private fun aimAt(ease: Ease, target: Float, control: Control) {
        if (ease.target == target) return
        ease.target = target
        dirty[control.ordinal] = true
    }

    // ---- Sticks, as drawn ------------------------------------------------------------

    /** A stick's drawing: its base glides after the thumb, its knob sits under it. */
    private inner class StickLook(val stick: PadInput.Stick) {
        // Where the base is, where the knob is from it, how held it looks.
        var baseX = Float.NaN
        var baseY = Float.NaN
        var knobX = 0f
        var knobY = 0f
        val hold = Ease(PRESS_IN_MS, 160f)
        val ring = Ease(1f, CLICK_RING_MS)
        private var clicks = 0

        fun aim() {
            hold.target = if (stick.held) 1f else 0f
            if (stick.clicks != clicks) {
                // A stick click rings out from the knob.
                clicks = stick.clicks
                ring.value = 1f
                ring.target = 0f
            }
            if (stick.held || hold.value != hold.target || ring.value != ring.target) dirty[stick.control.ordinal] = true
        }

        /** A new layout: an idle stick's base goes straight home. */
        fun rest(pad: PadLayout) {
            val spot = pad.spot(stick.control) ?: return
            if (!stick.held) {
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
            val held = stick.held
            val toX = if (held) stick.originX else spot.x
            val toY = if (held) stick.originY else spot.y
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
                var dx = stick.thumbX - baseX
                var dy = stick.thumbY - baseY
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

    private fun lookFor(control: Control): StickLook? = when (control) {
        Control.LEFT_STICK -> leftLook
        Control.RIGHT_STICK -> rightLook
        else -> null
    }

    // ---- Drawing -----------------------------------------------------------------------

    private fun restyle() {
        val ink = colors.ink
        faceTints = IntArray(4) { index ->
            // Xbox's hues, softened toward the ink so they sit in Portal's palette.
            mix(FACE_HUES[index], ink, if (colors.dark) 0.42f else 0.2f)
        }
        halo.shader = RadialGradient(
            0f, 0f, 1f,
            intArrayOf(withAlpha(colors.accent, 72), withAlpha(colors.accent, 26), withAlpha(colors.accent, 0)),
            floatArrayOf(0f, 0.6f, 1f),
            Shader.TileMode.CLAMP,
        )
    }

    private fun invalidateAll() {
        for (view in views) view.invalidate()
    }

    /** [view] draws its controls, in the whole screen's coordinates. */
    internal fun draw(view: GamepadView, canvas: Canvas) {
        val pad = layout ?: return
        val now = SystemClock.uptimeMillis()
        val dt = if (view.lastFrame == 0L) 16f else (now - view.lastFrame).coerceIn(1L, 48L).toFloat()
        view.lastFrame = now
        var moving = false
        outline.strokeWidth = max(1f, 1.1f * dp)
        val saved = canvas.save()
        canvas.translate(-view.left.toFloat(), -view.top.toFloat())
        for (spot in pad.spots) {
            if (owners[spot.control.ordinal] !== view) continue
            if (spot.hidden && !editing) continue
            if (step(spot, dt, now)) moving = true
            drawSpot(canvas, pad, spot, now)
        }
        canvas.restoreToCount(saved)
        if (moving) view.postInvalidateOnAnimation() else view.lastFrame = 0L
    }

    /** Moves [spot]'s drawing [dt] ms on; true while it is still moving. */
    private fun step(spot: PadSpot, dt: Float, now: Long): Boolean {
        val control = spot.control
        var moving = appearing(control, now)
        if (press[control.ordinal].step(dt)) moving = true
        when (control.kind) {
            ControlKind.FACE -> for (ease in facePress) if (ease.step(dt)) moving = true
            ControlKind.DPAD -> for (ease in armPress) if (ease.step(dt)) moving = true
            ControlKind.STICK -> if (lookFor(control)?.step(dt, spot) == true) moving = true
            else -> Unit
        }
        return moving
    }

    /** [control]'s share of the entrance or exit at [now]. */
    private fun appear(control: Control, now: Long): Float {
        if (showing) {
            if (waitingForTurn) return 0f
            val raw = ((now - appearStart - entranceDelay(control)) / SHOW_MS).coerceIn(0f, 1f)
            return DECELERATE.getInterpolation(raw)
        }
        if (onHidden == null) return 0f
        val raw = ((now - hideStart).toFloat() / HIDE_MS).coerceIn(0f, 1f)
        return hideFrom[control.ordinal] * (1f - ACCELERATE.getInterpolation(raw))
    }

    private fun appearing(control: Control, now: Long): Boolean =
        if (showing) !waitingForTurn && now - appearStart - entranceDelay(control) < SHOW_MS
        else onHidden != null && now - hideStart < HIDE_MS

    /** The shoulders arrive first, then the clusters, then the middle row. */
    private fun entranceDelay(control: Control): Float = when (control.kind) {
        ControlKind.BUMPER, ControlKind.TRIGGER -> 0f
        ControlKind.STICK, ControlKind.FACE -> 45f
        ControlKind.DPAD -> 70f
        ControlKind.BUTTON -> if (control == Control.L3 || control == Control.R3) 90f else 120f
    }

    private fun drawSpot(canvas: Canvas, pad: PadLayout, spot: PadSpot, now: Long) {
        val appear = appear(spot.control, now)
        if (appear <= 0.004f) return
        // In the editor a hidden control shows faintly, where it would be.
        val fade = if (spot.hidden) appear * GHOST_ALPHA else appear
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
            ControlKind.STICK -> lookFor(spot.control)?.let { drawStick(canvas, spot, it, fade) }
            ControlKind.DPAD -> drawDpad(canvas, spot, fade)
            ControlKind.FACE -> drawFace(canvas, pad, spot, fade)
            ControlKind.BUMPER, ControlKind.TRIGGER -> drawShoulder(canvas, spot, fade)
            ControlKind.BUTTON -> drawButton(canvas, spot, fade)
        }
        if (editing) drawFrame(canvas, pad, spot, appear)
        canvas.restoreToCount(saved)
    }

    /**
     * The editor's outline, a little outside the control's own shape, so
     * neighbours' outlines stay apart: dashed, or the accent once selected.
     */
    private fun drawFrame(canvas: Canvas, pad: PadLayout, spot: PadSpot, fade: Float) {
        val chosen = selected == spot.control
        frame.strokeWidth = (if (chosen) 2f else 1.5f) * dp
        frame.pathEffect = if (chosen) null else dashes
        frame.color = faded(if (chosen) colors.accent else withAlpha(colors.ink, 120), fade)
        val gap = frameGap(pad)
        when (spot.control.kind) {
            ControlKind.STICK, ControlKind.BUTTON -> canvas.drawCircle(spot.x, spot.y, spot.r + gap, frame)
            ControlKind.DPAD -> canvas.drawPath(crossFrame, frame)
            ControlKind.FACE -> {
                // Around the four buttons: a square on its corner, rounded like them.
                val half = spot.b / SQRT2 + spot.a + gap
                val saved = canvas.save()
                canvas.rotate(45f, spot.x, spot.y)
                rect.set(spot.x - half, spot.y - half, spot.x + half, spot.y + half)
                canvas.drawRoundRect(rect, spot.a + gap, spot.a + gap, frame)
                canvas.restoreToCount(saved)
            }
            ControlKind.BUMPER, ControlKind.TRIGGER -> {
                rect.set(spot.x - spot.w / 2f - gap, spot.y - spot.h / 2f - gap, spot.x + spot.w / 2f + gap, spot.y + spot.h / 2f + gap)
                val corner = shoulderCorner(spot, spot.h) + gap
                canvas.drawRoundRect(rect, corner, corner, frame)
            }
        }
    }

    private fun frameGap(pad: PadLayout): Float = FRAME_GAP_MM * pad.s * mm

    private fun shoulderCorner(spot: PadSpot, height: Float): Float =
        if (spot.control.kind == ControlKind.TRIGGER) height * 0.36f else height / 2f

    private fun drawHalo(canvas: Canvas, x: Float, y: Float, radius: Float, strength: Float) {
        if (strength <= 0.01f || radius <= 0f) return
        haloMatrix.setScale(radius, radius)
        haloMatrix.postTranslate(x, y)
        halo.shader?.setLocalMatrix(haloMatrix)
        halo.alpha = (255 * strength).roundToInt().coerceIn(0, 255)
        canvas.drawCircle(x, y, radius, halo)
    }

    /** A round face: the surface, turning to the accent as it is pressed. */
    private fun drawDisc(canvas: Canvas, x: Float, y: Float, radius: Float, pressed: Float, fade: Float) {
        fill.color = faded(mix(colors.surface, colors.accent, pressed), fade)
        canvas.drawCircle(x, y, radius, fill)
        outline.color = faded(colors.edge, fade * (1f - pressed))
        canvas.drawCircle(x, y, radius, outline)
    }

    private fun drawText(canvas: Canvas, text: String, x: Float, y: Float, size: Float, color: Int) {
        label.textSize = size
        label.color = color
        canvas.drawText(text, x, y - (label.ascent() + label.descent()) / 2f, label)
    }

    private fun drawStick(canvas: Canvas, spot: PadSpot, look: StickLook, fade: Float) {
        val stick = look.stick
        val held = look.hold.value
        val x = look.baseX.takeUnless { it.isNaN() } ?: spot.x
        val y = look.baseY.takeUnless { it.isNaN() } ?: spot.y
        val radius = spot.r
        drawHalo(canvas, x, y, radius * 1.3f, held * 0.9f * fade)
        fill.color = faded(colors.surface, fade)
        canvas.drawCircle(x, y, radius, fill)
        outline.color = faded(colors.edge, fade)
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
        val knobX = x + look.knobX
        val knobY = y + look.knobY
        val ring = look.ring.value
        if (ring > 0.01f) {
            stroke.strokeWidth = 2f * dp
            stroke.color = faded(colors.accent, fade * ring)
            canvas.drawCircle(knobX, knobY, spot.a * (1.15f + 0.9f * (1f - ring)), stroke)
        }
        fill.color = faded(mix(mix(colors.surface, colors.ink, if (colors.dark) 0.1f else 0.04f), colors.accent, held), fade)
        canvas.drawCircle(knobX, knobY, spot.a, fill)
        outline.color = faded(colors.edge, fade * (1f - held))
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
        outline.color = faded(colors.edge, fade)
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
        val corner = shoulderCorner(spot, rect.height())
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
            outline.color = faded(colors.edge, fade * (1f - lit))
            canvas.drawRoundRect(rect, corner, corner, outline)
            drawText(canvas, spot.control.title, spot.x, spot.y, spot.h * 0.4f,
                faded(if (lit > 0.5f) colors.accentInk else colors.ink, fade))
            return
        }
        fill.color = faded(mix(colors.surface, colors.accent, lit), fade)
        canvas.drawRoundRect(rect, corner, corner, fill)
        outline.color = faded(colors.edge, fade * (1f - lit))
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
        private const val GHOST_ALPHA = 0.35f
        /** The editor's outline, this far outside a control's shape. */
        private const val FRAME_GAP_MM = 0.7f
        private val SQRT2 = sqrt(2f)

        private const val EXCLUDED_DP = 196f
        private const val EDGE_DP = 40f

        private val ARM_X = intArrayOf(1, 0, -1, 0)
        private val ARM_Y = intArrayOf(0, 1, 0, -1)
        private val FACE_HUES = intArrayOf(
            Color.rgb(0x5E, 0xC2, 0x3C),
            Color.rgb(0xE8, 0x4A, 0x3F),
            Color.rgb(0x3A, 0x8D, 0xE6),
            Color.rgb(0xF2, 0xC2, 0x2E),
        )

        // Portal's emphasized curves.
        private val DECELERATE = PathInterpolator(0.05f, 0.7f, 0.1f, 1f)
        private val ACCELERATE = PathInterpolator(0.3f, 0f, 0.8f, 0.15f)

        /** The system bars as they are now: hidden, the controls may use their room. */
        fun barInsets(windowInsets: WindowInsets?): Rect {
            windowInsets ?: return Rect()
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                val bars = windowInsets.getInsets(WindowInsets.Type.systemBars())
                return Rect(bars.left, bars.top, bars.right, bars.bottom)
            }
            @Suppress("DEPRECATION")
            return Rect(windowInsets.systemWindowInsetLeft, windowInsets.systemWindowInsetTop,
                windowInsets.systemWindowInsetRight, windowInsets.systemWindowInsetBottom)
        }

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

/**
 * Where [Gamepad] draws and catches touches: the whole screen (all its
 * controls) or one floating control. Fingers go to the gamepad, which works
 * in the whole screen's coordinates.
 */
@SuppressLint("ViewConstructor")
internal class GamepadView(
    context: Context,
    private val gamepad: Gamepad,
    val controls: List<Control>,
) : View(context) {
    internal var lastFrame = 0L

    init {
        isHapticFeedbackEnabled = false
        isSoundEffectsEnabled = false
    }

    override fun onDraw(canvas: Canvas) = gamepad.draw(this, canvas)

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean = gamepad.touch(this, event)

    // On the touchpad page a mouse's hover and wheel mean nothing; over the desktop they are the desktop's.
    override fun onGenericMotionEvent(event: MotionEvent): Boolean = gamepad.wholeScreen

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        if (gamepad.wholeScreen) requestApplyInsets()
    }

    override fun onApplyWindowInsets(insets: WindowInsets): WindowInsets {
        if (gamepad.wholeScreen) gamepad.setInsets(Gamepad.barInsets(insets))
        return insets
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        if (gamepad.wholeScreen) gamepad.resize(w, h)
    }

    override fun onWindowFocusChanged(hasWindowFocus: Boolean) {
        super.onWindowFocusChanged(hasWindowFocus)
        if (!hasWindowFocus) gamepad.release()
    }

    override fun onDetachedFromWindow() {
        gamepad.release()
        super.onDetachedFromWindow()
    }

    /**
     * While the controller shows: the whole-screen one keeps the screen on,
     * and Android 15, which counts a vote from each view that draws (OxygenOS
     * holds an app to 60 Hz by default), lets a moving thumb draw at the
     * panel's full rate. Idle, a view draws nothing and asks nothing.
     */
    internal fun awake(on: Boolean) {
        keepScreenOn = on && gamepad.wholeScreen
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.VANILLA_ICE_CREAM) {
            requestedFrameRate = if (on) REQUESTED_FRAME_RATE_CATEGORY_HIGH else REQUESTED_FRAME_RATE_CATEGORY_NO_PREFERENCE
        }
    }
}
