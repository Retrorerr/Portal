package app.polarbear.gaming

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.DashPathEffect
import android.graphics.LinearGradient
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RadialGradient
import android.graphics.RectF
import android.graphics.Shader
import android.graphics.Typeface
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/** What a control needs from the controller that owns it. */
interface ControlHost {
    val editing: Boolean
    val selected: Control?
    val analogTriggers: Boolean

    /** The pad state changed; send it to the guest. */
    fun padChanged()

    /** A control was pressed: [strong] for buttons, light for sticks and d-pad steps. */
    fun feedback(strong: Boolean)

    fun select(control: Control)

    /** Edit mode: [control]'s centre moved to ([x], [y]) px in the overlay. */
    fun moved(control: Control, x: Float, y: Float)

    /** Edit mode: keep a dragged control inside the overlay. */
    fun clampDrag(view: View, dx: Float, dy: Float): Pair<Float, Float>
}

/**
 * One on-screen control as its own view, sized to the control plus a
 * forgiving touch margin. Separate views let Android split multitouch
 * between them: a thumb on the stick, another on A, and a third finger on
 * the desktop all reach the right place.
 */
@SuppressLint("ViewConstructor")
class ControlView(
    context: Context,
    val control: Control,
    private val host: ControlHost,
) : View(context) {
    private val density = context.resources.displayMetrics.density
    private val slop = ViewConfiguration.get(context).scaledTouchSlop.toFloat()

    /** The drawn control, px, inside the margin [pad]. */
    var drawnWidth = 0f
        private set
    var drawnHeight = 0f
        private set
    var pad = 0f
        private set

    /** Shown greyed in edit mode: the user hid it from play. */
    var ghost = false
        set(value) {
            field = value
            invalidate()
        }

    // Output, read by the controller after every change.
    var buttons = 0
        private set
    private var axisX = 0f
    private var axisY = 0f
    private var trigger = 0f

    private class Touch(var x: Float, var y: Float, val downTime: Long)

    private val touches = HashMap<Int, Touch>()
    private var stickPointer = -1
    private var stickClick = false
    private var lastStickUp = 0L
    private var hatX = 0
    private var hatY = 0

    // Edit-mode drag.
    private var dragPointer = -1
    private var dragStartX = 0f
    private var dragStartY = 0f
    private var dragging = false

    /** Edit mode: the user is moving this control right now. */
    val isDragging: Boolean get() = dragging

    // Paints, in Portal's palette.
    private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = withAlpha(CHARCOAL, 150) }
    private val sheen = Paint(Paint.ANTI_ALIAS_FLAG)
    private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = 1.5f * density
        color = withAlpha(IVORY, 92)
    }
    private val faint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = 1f * density
        color = withAlpha(IVORY, 34)
    }
    private val knobRing = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = 1f * density
        color = withAlpha(CHARCOAL, 70)
    }
    private val glow = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = withAlpha(ORANGE, 48) }
    private val pressedFill = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = withAlpha(ORANGE, 220) }
    private val pressedStroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = 1.5f * density
        color = ORANGE
    }
    private val arc = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeCap = Paint.Cap.ROUND
        strokeWidth = 3.5f * density
        color = ORANGE
    }
    private val halo = Paint(Paint.ANTI_ALIAS_FLAG)
    private val ink = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = withAlpha(IVORY, 236)
        textAlign = Paint.Align.CENTER
        typeface = Typeface.create(Typeface.DEFAULT, Typeface.BOLD)
    }
    private val glyph = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = 2f * density
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
        color = withAlpha(IVORY, 236)
    }
    private val glyphFill = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = withAlpha(IVORY, 236) }
    private val editOutline = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = 1.5f * density
        color = withAlpha(IVORY, 120)
        pathEffect = DashPathEffect(floatArrayOf(6f * density, 5f * density), 0f)
    }
    private val selectedOutline = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = 2.5f * density
        color = ORANGE
    }
    private val rect = RectF()
    private val path = Path()
    private val dpadPath = Path()

    init {
        isClickable = false
        contentDescription = control.title
        // Split touches only reach sibling views; never let a parent steal them.
        isHapticFeedbackEnabled = false
    }

    /** Size the control: [width] x [height] px drawn, [margin] px of touch margin around it. */
    fun setGeometry(width: Float, height: Float, margin: Float) {
        drawnWidth = width
        drawnHeight = height
        pad = margin
        rebuildShaders()
        invalidate()
    }

    private fun rebuildShaders() {
        if (drawnHeight <= 0f) return
        sheen.shader = LinearGradient(
            0f, pad, 0f, pad + drawnHeight,
            withAlpha(IVORY, 30), withAlpha(IVORY, 0), Shader.TileMode.CLAMP,
        )
        val cx = pad + drawnWidth / 2f
        val cy = pad + drawnHeight / 2f
        val radius = max(drawnWidth, drawnHeight) / 2f + pad
        halo.shader = RadialGradient(
            cx, cy, radius,
            intArrayOf(withAlpha(ORANGE, 96), withAlpha(ORANGE, 40), withAlpha(ORANGE, 0)),
            floatArrayOf(0f, 0.62f, 1f),
            Shader.TileMode.CLAMP,
        )
        if (control.kind == ControlKind.DPAD) {
            val size = min(drawnWidth, drawnHeight)
            val arm = size * 0.36f
            val corner = arm * 0.28f
            val horizontal = Path().apply {
                addRoundRect(RectF(cx - size / 2f, cy - arm / 2f, cx + size / 2f, cy + arm / 2f), corner, corner, Path.Direction.CW)
            }
            val vertical = Path().apply {
                addRoundRect(RectF(cx - arm / 2f, cy - size / 2f, cx + arm / 2f, cy + size / 2f), corner, corner, Path.Direction.CW)
            }
            dpadPath.reset()
            dpadPath.op(horizontal, vertical, Path.Op.UNION)
        }
    }

    /** Forget every touch (hidden, edit mode, pad removed). */
    fun release() {
        touches.clear()
        stickPointer = -1
        stickClick = false
        dragPointer = -1
        dragging = false
        translationX = 0f
        translationY = 0f
        buttons = 0
        axisX = 0f
        axisY = 0f
        trigger = 0f
        hatX = 0
        hatY = 0
        invalidate()
    }

    /** Write the axes this control owns into the pad state. */
    fun writeAxes(axes: FloatArray) {
        when (control.kind) {
            ControlKind.STICK -> {
                axes[control.axisX] = axisX
                axes[control.axisY] = axisY
            }
            ControlKind.TRIGGER -> axes[control.axisX] = trigger
            ControlKind.DPAD -> {
                axes[8] = hatX.toFloat()
                axes[9] = hatY.toFloat()
            }
            else -> Unit
        }
    }

    private val centerX get() = pad + drawnWidth / 2f
    private val centerY get() = pad + drawnHeight / 2f
    private val radius get() = min(drawnWidth, drawnHeight) / 2f

    /** Whether a finger landing at ([x], [y]) is meant for this control. */
    private fun accepts(x: Float, y: Float): Boolean {
        val distance = hypot(x - centerX, y - centerY)
        return when (control.kind) {
            ControlKind.STICK -> distance <= radius * 1.3f
            ControlKind.DPAD, ControlKind.FACE -> distance <= radius * 1.12f
            ControlKind.BUTTON -> distance <= radius + pad
            ControlKind.BUMPER, ControlKind.TRIGGER -> true
        }
    }

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (host.editing) return editTouch(event)
        val before = buttons
        val beforeHat = hatX to hatY
        val beforeTrigger = trigger > 0f
        var stickDown = false
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> {
                val index = event.actionIndex
                val x = event.getX(index)
                val y = event.getY(index)
                if (!accepts(x, y)) {
                    // A first finger off the control falls through to the desktop.
                    return event.actionMasked != MotionEvent.ACTION_DOWN
                }
                val id = event.getPointerId(index)
                touches[id] = Touch(x, y, event.eventTime)
                if (control.kind == ControlKind.STICK && stickPointer < 0) {
                    stickPointer = id
                    // A second tap soon after letting go clicks the stick.
                    stickClick = event.eventTime - lastStickUp < STICK_CLICK_MS
                    stickDown = true
                }
            }
            MotionEvent.ACTION_MOVE -> {
                for (index in 0 until event.pointerCount) {
                    val touch = touches[event.getPointerId(index)] ?: continue
                    touch.x = event.getX(index)
                    touch.y = event.getY(index)
                }
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> {
                val id = event.getPointerId(event.actionIndex)
                touches.remove(id)
                if (id == stickPointer) {
                    stickPointer = -1
                    stickClick = false
                    lastStickUp = event.eventTime
                }
            }
            MotionEvent.ACTION_CANCEL -> {
                touches.clear()
                stickPointer = -1
                stickClick = false
            }
        }
        recompute()
        val pressed = buttons and before.inv()
        when {
            pressed != 0 -> host.feedback(strong = true)
            control.kind == ControlKind.TRIGGER && trigger > 0f && !beforeTrigger -> host.feedback(strong = true)
            stickDown -> host.feedback(strong = false)
            control.kind == ControlKind.DPAD && (hatX to hatY) != beforeHat && (hatX != 0 || hatY != 0) ->
                host.feedback(strong = false)
        }
        host.padChanged()
        invalidate()
        return true
    }

    private fun recompute() {
        when (control.kind) {
            ControlKind.STICK -> {
                val touch = touches[stickPointer]
                if (touch == null) {
                    axisX = 0f
                    axisY = 0f
                    buttons = 0
                } else {
                    val range = radius
                    val dx = touch.x - centerX
                    val dy = touch.y - centerY
                    val length = hypot(dx, dy)
                    val scale = if (length > range) range / length else 1f
                    axisX = (dx * scale / range).coerceIn(-1f, 1f)
                    axisY = (dy * scale / range).coerceIn(-1f, 1f)
                    buttons = if (stickClick) control.bit else 0
                }
            }
            ControlKind.DPAD -> {
                var x = 0
                var y = 0
                for (touch in touches.values) {
                    val dx = touch.x - centerX
                    val dy = touch.y - centerY
                    if (hypot(dx, dy) < radius * 0.2f) continue
                    // Eight 45-degree sectors, so diagonals are easy to hold.
                    val sector = ((Math.toDegrees(atan2(dy.toDouble(), dx.toDouble())) + 360.0 + 22.5) % 360.0 / 45.0).toInt()
                    val (sx, sy) = SECTORS[sector]
                    if (sx != 0) x = sx
                    if (sy != 0) y = sy
                }
                hatX = x
                hatY = y
            }
            ControlKind.FACE -> {
                var mask = 0
                val r = radius * FACE_BUTTON
                val offset = radius * FACE_OFFSET
                for (touch in touches.values) {
                    var nearest = -1
                    var nearestDistance = Float.MAX_VALUE
                    var hits = 0
                    for ((index, face) in FACE_BUTTONS.withIndex()) {
                        val distance = hypot(touch.x - (centerX + face.dx * offset), touch.y - (centerY + face.dy * offset))
                        // Between two buttons presses both, like a thumb across A and B.
                        if (distance <= r * 1.08f) hits = hits or face.bit
                        if (distance < nearestDistance) {
                            nearestDistance = distance
                            nearest = index
                        }
                    }
                    if (hits == 0 && nearest >= 0 && nearestDistance <= r * 1.8f) hits = FACE_BUTTONS[nearest].bit
                    mask = mask or hits
                }
                buttons = mask
            }
            ControlKind.TRIGGER -> {
                var value = 0f
                for (touch in touches.values) {
                    val level = if (host.analogTriggers) {
                        // Pressure follows the finger: light at the top, full near the bottom.
                        val fraction = ((touch.y - pad) / (drawnHeight * 0.85f)).coerceIn(0f, 1f)
                        0.2f + 0.8f * fraction
                    } else {
                        1f
                    }
                    value = max(value, level)
                }
                trigger = value
            }
            ControlKind.BUMPER, ControlKind.BUTTON -> buttons = if (touches.isEmpty()) 0 else control.bit
        }
    }

    private fun editTouch(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                dragPointer = event.getPointerId(0)
                dragStartX = x + event.getX(0)
                dragStartY = y + event.getY(0)
                dragging = false
                host.select(control)
            }
            MotionEvent.ACTION_MOVE -> {
                val index = event.findPointerIndex(dragPointer)
                if (index >= 0) {
                    // Overlay coordinates: x/y already include the drag offset.
                    val dx = x + event.getX(index) - dragStartX
                    val dy = y + event.getY(index) - dragStartY
                    if (!dragging && hypot(dx, dy) > slop) dragging = true
                    if (dragging) {
                        val (cx, cy) = host.clampDrag(this, dx, dy)
                        translationX += cx
                        translationY += cy
                        dragStartX += cx
                        dragStartY += cy
                    }
                }
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                val dropped = dragging
                dragPointer = -1
                dragging = false
                if (dropped) host.moved(control, x + width / 2f, y + height / 2f)
            }
        }
        return true
    }

    override fun onDraw(canvas: Canvas) {
        if (drawnWidth <= 0f) return
        val editing = host.editing
        val saved = if (ghost) canvas.saveLayerAlpha(0f, 0f, width.toFloat(), height.toFloat(), 90) else -1
        when (control.kind) {
            ControlKind.STICK -> drawStick(canvas)
            ControlKind.DPAD -> drawDpad(canvas)
            ControlKind.FACE -> drawFace(canvas)
            ControlKind.BUMPER, ControlKind.TRIGGER -> drawShoulder(canvas)
            ControlKind.BUTTON -> drawButton(canvas)
        }
        if (saved >= 0) canvas.restoreToCount(saved)
        if (editing) {
            val inset = 2f * density
            rect.set(inset, inset, width - inset, height - inset)
            val corner = 14f * density
            canvas.drawRoundRect(rect, corner, corner, if (host.selected == control) selectedOutline else editOutline)
        }
    }

    private fun drawGlassCircle(canvas: Canvas, cx: Float, cy: Float, r: Float, pressed: Boolean) {
        if (pressed) {
            canvas.drawCircle(cx, cy, r, pressedFill)
            canvas.drawCircle(cx, cy, r, pressedStroke)
        } else {
            canvas.drawCircle(cx, cy, r, fill)
            canvas.drawCircle(cx, cy, r, sheen)
            canvas.drawCircle(cx, cy, r, stroke)
        }
    }

    private fun drawLabel(canvas: Canvas, text: String, cx: Float, cy: Float, size: Float, color: Int) {
        ink.textSize = size
        ink.color = color
        canvas.drawText(text, cx, cy - (ink.ascent() + ink.descent()) / 2f, ink)
    }

    private fun drawStick(canvas: Canvas) {
        val cx = centerX
        val cy = centerY
        val r = radius
        val held = stickPointer >= 0
        if (held) canvas.drawCircle(cx, cy, r + pad, halo)
        canvas.drawCircle(cx, cy, r, fill)
        canvas.drawCircle(cx, cy, r, sheen)
        canvas.drawCircle(cx, cy, r, stroke)
        canvas.drawCircle(cx, cy, r * 0.62f, faint)
        val deflection = hypot(axisX, axisY)
        if (held && deflection > 0.08f) {
            // An arc on the rim points where the stick leans.
            val angle = Math.toDegrees(atan2(axisY.toDouble(), axisX.toDouble())).toFloat()
            val inset = arc.strokeWidth
            rect.set(cx - r + inset, cy - r + inset, cx + r - inset, cy + r - inset)
            arc.alpha = (90 + 165 * deflection).roundToInt().coerceAtMost(255)
            canvas.drawArc(rect, angle - 32f, 64f, false, arc)
        }
        val knob = r * 0.44f
        val kx = cx + axisX * r * 0.62f
        val ky = cy + axisY * r * 0.62f
        drawGlassCircle(canvas, kx, ky, knob, held)
        canvas.drawCircle(kx, ky, knob * 0.7f, if (held) knobRing else faint)
        val label = if (control == Control.LEFT_STICK) "L" else "R"
        drawLabel(canvas, if (stickClick) label + "3" else label, kx, ky, knob * 0.62f, if (held) CHARCOAL else withAlpha(IVORY, 170))
    }

    private fun drawDpad(canvas: Canvas) {
        val cx = centerX
        val cy = centerY
        val size = radius * 2f
        val arm = size * 0.36f
        if (hatX != 0 || hatY != 0) canvas.drawCircle(cx, cy, radius + pad, halo)
        canvas.drawPath(dpadPath, fill)
        canvas.drawPath(dpadPath, sheen)
        // Light the held arm(s).
        val corner = arm * 0.28f
        if (hatX != 0) {
            val sx = hatX.toFloat()
            rect.set(
                min(cx + sx * arm / 2f, cx + sx * size / 2f), cy - arm / 2f,
                max(cx + sx * arm / 2f, cx + sx * size / 2f), cy + arm / 2f,
            )
            canvas.drawRoundRect(rect, corner, corner, pressedFill)
        }
        if (hatY != 0) {
            val sy = hatY.toFloat()
            rect.set(
                cx - arm / 2f, min(cy + sy * arm / 2f, cy + sy * size / 2f),
                cx + arm / 2f, max(cy + sy * arm / 2f, cy + sy * size / 2f),
            )
            canvas.drawRoundRect(rect, corner, corner, pressedFill)
        }
        canvas.drawPath(dpadPath, stroke)
        canvas.drawCircle(cx, cy, arm * 0.22f, faint)
        // Chevrons near each arm's end.
        val reach = size / 2f - arm * 0.52f
        val chevron = arm * 0.2f
        for ((dx, dy) in listOf(0 to -1, 0 to 1, -1 to 0, 1 to 0)) {
            val active = (dx != 0 && dx == hatX) || (dy != 0 && dy == hatY)
            glyphFill.color = if (active) CHARCOAL else withAlpha(IVORY, 200)
            val tipX = cx + dx * (reach + chevron * 0.6f)
            val tipY = cy + dy * (reach + chevron * 0.6f)
            val baseX = cx + dx * (reach - chevron * 0.6f)
            val baseY = cy + dy * (reach - chevron * 0.6f)
            path.reset()
            path.moveTo(tipX, tipY)
            path.lineTo(baseX + dy * chevron, baseY + dx * chevron)
            path.lineTo(baseX - dy * chevron, baseY - dx * chevron)
            path.close()
            canvas.drawPath(path, glyphFill)
        }
        glyphFill.color = withAlpha(IVORY, 236)
    }

    private fun drawFace(canvas: Canvas) {
        val r = radius * FACE_BUTTON
        val offset = radius * FACE_OFFSET
        for (face in FACE_BUTTONS) {
            val bx = centerX + face.dx * offset
            val by = centerY + face.dy * offset
            val pressed = buttons and face.bit != 0
            if (pressed) canvas.drawCircle(bx, by, r * 1.32f, glow)
            drawGlassCircle(canvas, bx, by, r, pressed)
            drawLabel(canvas, face.label, bx, by, r * 0.86f, if (pressed) CHARCOAL else face.tint)
        }
    }

    private fun drawShoulder(canvas: Canvas) {
        val left = pad
        val top = pad
        val right = pad + drawnWidth
        val bottom = pad + drawnHeight
        val trigger = control.kind == ControlKind.TRIGGER
        val corner = if (trigger) drawnHeight * 0.34f else drawnHeight / 2f
        val held = touches.isNotEmpty()
        if (held) {
            val spread = pad * 0.7f
            rect.set(left - spread, top - spread, right + spread, bottom + spread)
            canvas.drawRoundRect(rect, corner + spread, corner + spread, glow)
        }
        rect.set(left, top, right, bottom)
        if (trigger && held && this.trigger < 1f) {
            // Analog: fill from the bottom as far as the trigger is pressed.
            canvas.drawRoundRect(rect, corner, corner, fill)
            canvas.save()
            canvas.clipRect(left, bottom - drawnHeight * this.trigger, right, bottom)
            canvas.drawRoundRect(rect, corner, corner, pressedFill)
            canvas.restore()
            canvas.drawRoundRect(rect, corner, corner, pressedStroke)
        } else if (held) {
            canvas.drawRoundRect(rect, corner, corner, pressedFill)
            canvas.drawRoundRect(rect, corner, corner, pressedStroke)
        } else {
            canvas.drawRoundRect(rect, corner, corner, fill)
            canvas.drawRoundRect(rect, corner, corner, sheen)
            canvas.drawRoundRect(rect, corner, corner, stroke)
        }
        drawLabel(
            canvas, control.title, centerX, centerY, drawnHeight * 0.36f,
            if (held && (!trigger || this.trigger > 0.5f)) CHARCOAL else withAlpha(IVORY, 236),
        )
    }

    private fun drawButton(canvas: Canvas) {
        val cx = centerX
        val cy = centerY
        val r = radius
        val held = touches.isNotEmpty()
        if (held) canvas.drawCircle(cx, cy, r + pad, halo)
        drawGlassCircle(canvas, cx, cy, r, held)
        val color = if (held) CHARCOAL else withAlpha(IVORY, 236)
        glyph.color = color
        glyphFill.color = color
        val g = r * 0.42f
        when (control) {
            Control.BACK -> {
                // Two overlapping windows, the Xbox "View" mark.
                val s = g * 0.9f
                val corner = s * 0.25f
                rect.set(cx - s, cy - s, cx + s * 0.4f, cy + s * 0.4f)
                canvas.drawRoundRect(rect, corner, corner, glyph)
                rect.set(cx - s * 0.4f, cy - s * 0.4f, cx + s, cy + s)
                canvas.drawRoundRect(rect, corner, corner, if (held) pressedFill else fill)
                canvas.drawRoundRect(rect, corner, corner, glyph)
            }
            Control.START -> {
                // Three lines, the "Menu" mark.
                for (row in -1..1) {
                    val ly = cy + row * g * 0.62f
                    canvas.drawLine(cx - g * 0.85f, ly, cx + g * 0.85f, ly, glyph)
                }
            }
            Control.GUIDE -> {
                canvas.drawCircle(cx, cy, g * 1.05f, glyph)
                canvas.drawCircle(cx, cy, g * 0.36f, glyphFill)
            }
            else -> drawLabel(canvas, control.title, cx, cy, r * 0.62f, color)
        }
        glyph.color = withAlpha(IVORY, 236)
        glyphFill.color = withAlpha(IVORY, 236)
    }

    private class Face(val label: String, val dx: Int, val dy: Int, val bit: Int, val tint: Int)

    companion object {
        private const val STICK_CLICK_MS = 280L
        private const val FACE_BUTTON = 0.4f
        private const val FACE_OFFSET = 0.6f

        val CHARCOAL = Color.rgb(0x19, 0x1B, 0x1C)
        val IVORY = Color.rgb(0xF1, 0xEB, 0xDD)
        val ORANGE = Color.rgb(0xF0, 0x79, 0x49)

        fun withAlpha(color: Int, alpha: Int) = (color and 0x00FFFFFF) or (alpha.coerceIn(0, 255) shl 24)

        /** Xbox's button hues, softened toward Portal's ivory. */
        private fun tint(rgb: Int): Int {
            val r = ((Color.red(rgb) + Color.red(IVORY)) / 2)
            val g = ((Color.green(rgb) + Color.green(IVORY)) / 2)
            val b = ((Color.blue(rgb) + Color.blue(IVORY)) / 2)
            return Color.argb(240, r, g, b)
        }

        private val FACE_BUTTONS = listOf(
            Face("A", 0, 1, PadBits.A, tint(Color.rgb(0x5E, 0xC2, 0x3C))),
            Face("B", 1, 0, PadBits.B, tint(Color.rgb(0xE8, 0x4A, 0x3F))),
            Face("X", -1, 0, PadBits.X, tint(Color.rgb(0x3A, 0x8D, 0xE6))),
            Face("Y", 0, -1, PadBits.Y, tint(Color.rgb(0xF2, 0xC2, 0x2E))),
        )

        /** Hat direction per 45-degree sector, clockwise from east (screen y grows down). */
        private val SECTORS = arrayOf(
            1 to 0, 1 to 1, 0 to 1, -1 to 1, -1 to 0, -1 to -1, 0 to -1, 1 to -1,
        )
    }
}
