package app.polarbear

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.Gravity
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.View
import android.widget.FrameLayout
import java.io.File
import java.util.concurrent.Executors
import kotlin.math.hypot
import kotlin.math.min

/**
 * On-screen controller for games without touch support, shown over the
 * desktop while Steam runs. It feeds the "touch pad" through the same
 * evdev path as physical controllers ([GamepadBridge.touchPad]).
 *
 * The controls are a few separate views sized to their clusters, added as
 * siblings above the native surface: a finger outside them reaches the
 * desktop even while another finger holds a control, which one
 * full-screen view could not allow (Android sends later pointers to a
 * view already being touched).
 */
object TouchControls {
    private const val TAG = "PortalTouchControls"
    private const val PREFS = "portal_touch_controls"
    private const val PREF_ON = "on"
    private const val POLL_MS = 2500L

    // Bits in xpad button order (src/core/gamepad.rs BUTTONS).
    const val A = 1 shl 0
    const val B = 1 shl 1
    const val X = 1 shl 2
    const val Y = 1 shl 3
    const val LB = 1 shl 4
    const val RB = 1 shl 5
    const val BACK = 1 shl 6
    const val START = 1 shl 7
    const val GUIDE = 1 shl 8
    const val L3 = 1 shl 9
    const val R3 = 1 shl 10

    private val main = Handler(Looper.getMainLooper())
    private val scanner = Executors.newSingleThreadExecutor { Thread(it, "touch-controls-scan").apply { isDaemon = true } }
    private var activity: PortalActivity? = null
    private var toggle: View? = null
    private val clusters = ArrayList<View>()
    private var steamRunning = false
    private var scanning = false

    private val model = PadModel()

    fun attach(host: PortalActivity) {
        activity = host
        main.removeCallbacks(poll)
        main.post(poll)
    }

    fun detach() {
        main.removeCallbacks(poll)
        removeViews()
        activity = null
    }

    private val poll = object : Runnable {
        override fun run() {
            val host = activity ?: return
            if (!scanning && !ComposeOverlay.isShowing()) {
                scanning = true
                scanner.execute {
                    val running = isSteamRunning()
                    main.post {
                        scanning = false
                        if (running != steamRunning) Log.i(TAG, "Steam running: $running")
                        steamRunning = running
                        refresh(host)
                    }
                }
            } else if (ComposeOverlay.isShowing()) {
                refresh(host)
            }
            main.postDelayed(this, POLL_MS)
        }
    }

    /** Steam's guest processes run under Portal's own uid. */
    private fun isSteamRunning(): Boolean {
        val processes = File("/proc").listFiles() ?: return false.also { Log.w(TAG, "/proc unreadable") }
        for (entry in processes) {
            val name = entry.name
            if (name.isEmpty() || !name[0].isDigit()) continue
            try {
                val cmdline = File(entry, "cmdline").readBytes()
                val end = cmdline.indexOf(0).let { if (it < 0) cmdline.size else it }
                // The client retitles itself "steam"; its helpers keep their names.
                val arg0 = String(cmdline, 0, end).substringAfterLast('/')
                if (arg0 == "steam" || arg0 == "steamwebhelper") return true
            } catch (_: Exception) {
            }
        }
        return false
    }

    private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    private fun refresh(host: PortalActivity) {
        val visible = steamRunning && !ComposeOverlay.isShowing()
        if (!visible) {
            removeViews()
            return
        }
        if (toggle == null) addToggle(host)
        val on = prefs(host).getBoolean(PREF_ON, false)
        if (on && clusters.isEmpty()) addClusters(host)
        if (!on && clusters.isNotEmpty()) removeClusters()
    }

    private fun removeViews() {
        removeClusters()
        toggle?.let { (it.parent as? FrameLayout)?.removeView(it) }
        toggle = null
    }

    private fun removeClusters() {
        if (clusters.isEmpty()) return
        for (view in clusters) (view.parent as? FrameLayout)?.removeView(view)
        clusters.clear()
        model.reset()
        GamepadBridge.removeTouchPad()
    }

    private fun dp(context: Context, value: Float) = value * context.resources.displayMetrics.density

    /** Above the native surface (child 0), below Portal's own screens. */
    private fun addAboveSurface(host: PortalActivity, view: View, params: FrameLayout.LayoutParams) {
        val root = try {
            host.overlayHost()
        } catch (e: Exception) {
            Log.w(TAG, "overlay host unavailable", e)
            return
        }
        root.addView(view, min(1, root.childCount), params)
    }

    private fun addToggle(host: PortalActivity) {
        val size = dp(host, 46f).toInt()
        val view = ToggleView(host) {
            val on = !prefs(host).getBoolean(PREF_ON, false)
            prefs(host).edit().putBoolean(PREF_ON, on).apply()
            refresh(host)
        }
        val params = FrameLayout.LayoutParams(size, size, Gravity.TOP or Gravity.CENTER_HORIZONTAL).apply {
            topMargin = dp(host, 10f).toInt()
        }
        addAboveSurface(host, view, params)
        toggle = view
    }

    private fun addClusters(host: PortalActivity) {
        val send = { GamepadBridge.touchPad(model.buttons, model.axes) }
        fun add(width: Float, height: Float, gravity: Int, elements: List<Element>, endInset: Float = 0f) {
            val view = ClusterView(host, elements, model, send)
            val params = FrameLayout.LayoutParams(dp(host, width).toInt(), dp(host, height).toInt(), gravity).apply {
                marginEnd = dp(host, endInset).toInt()
            }
            addAboveSurface(host, view, params)
            clusters += view
        }
        // Coordinates are dp inside each cluster.
        add(
            300f, 560f, Gravity.BOTTOM or Gravity.START,
            listOf(
                Element.Trigger("LT", 70f, 50f, 34f, axis = 4),
                Element.Button("LB", 170f, 50f, 34f, LB),
                Element.DPad(110f, 235f, 34f),
                Element.Stick(150f, 425f, 85f, axisX = 0, axisY = 1, thumb = L3),
            ),
        )
        add(
            320f, 560f, Gravity.BOTTOM or Gravity.END,
            listOf(
                Element.Button("RB", 150f, 50f, 34f, RB),
                Element.Trigger("RT", 250f, 50f, 34f, axis = 5),
                Element.Button("Y", 200f, 160f, 31f, Y),
                Element.Button("X", 140f, 220f, 31f, X),
                Element.Button("B", 260f, 220f, 31f, B),
                Element.Button("A", 200f, 280f, 31f, A),
                Element.Stick(150f, 435f, 80f, axisX = 2, axisY = 3, thumb = R3),
            ),
            // Clear of Portal's default side panel on the right edge.
            endInset = 64f,
        )
        add(
            300f, 76f, Gravity.BOTTOM or Gravity.CENTER_HORIZONTAL,
            listOf(
                Element.Button("◁", 60f, 38f, 25f, BACK),
                Element.Button("◎", 150f, 38f, 25f, GUIDE),
                Element.Button("▷", 240f, 38f, 25f, START),
            ),
        )
        // Plug the pad in now, so games and Steam see it before a touch.
        send()
    }

    /** Whole touch-pad state, laid out like GamepadBridge's axes. */
    class PadModel {
        var buttons = 0
        val axes = FloatArray(10)
        fun reset() {
            buttons = 0
            axes.fill(0f)
        }
    }

    sealed class Element(val label: String, val x: Float, val y: Float, val radius: Float) {
        class Button(label: String, x: Float, y: Float, radius: Float, val bit: Int) : Element(label, x, y, radius)
        class Trigger(label: String, x: Float, y: Float, radius: Float, val axis: Int) : Element(label, x, y, radius)
        class DPad(x: Float, y: Float, radius: Float) : Element("", x, y, radius * 2.6f) {
            val arm = radius
        }
        class Stick(x: Float, y: Float, radius: Float, val axisX: Int, val axisY: Int, val thumb: Int) :
            Element("", x, y, radius)
    }

    @SuppressLint("ViewConstructor")
    private class ClusterView(
        context: Context,
        val elements: List<Element>,
        val model: PadModel,
        val send: () -> Unit,
    ) : View(context) {
        private val density = context.resources.displayMetrics.density
        private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = Color.argb(46, 255, 255, 255) }
        private val pressedFill = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = Color.argb(130, 255, 255, 255) }
        private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            style = Paint.Style.STROKE
            strokeWidth = 1.5f * density
            color = Color.argb(120, 255, 255, 255)
        }
        private val text = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = Color.argb(210, 255, 255, 255)
            textAlign = Paint.Align.CENTER
            textSize = 15f * density
            isFakeBoldText = true
        }

        // Pointer id -> element it started on; sticks remember their offset.
        private val owners = HashMap<Int, Element>()
        private val stickOffsets = HashMap<Element.Stick, Pair<Float, Float>>()
        private val lastStickUp = HashMap<Element.Stick, Long>()

        private fun px(value: Float) = value * density

        private fun hit(x: Float, y: Float): Element? = elements.firstOrNull {
            hypot(x - px(it.x), y - px(it.y)) <= px(it.radius) * 1.25f
        }

        @SuppressLint("ClickableViewAccessibility")
        override fun onTouchEvent(event: MotionEvent): Boolean {
            when (event.actionMasked) {
                MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> {
                    val index = event.actionIndex
                    val element = hit(event.getX(index), event.getY(index))
                        ?: return event.actionMasked != MotionEvent.ACTION_DOWN
                    owners[event.getPointerId(index)] = element
                    press(element, event.getX(index), event.getY(index), event.eventTime)
                }
                MotionEvent.ACTION_MOVE -> {
                    for (index in 0 until event.pointerCount) {
                        val element = owners[event.getPointerId(index)] ?: continue
                        move(element, event.getX(index), event.getY(index))
                    }
                }
                MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> {
                    val element = owners.remove(event.getPointerId(event.actionIndex))
                    if (element != null) release(element, event.eventTime)
                }
                MotionEvent.ACTION_CANCEL -> {
                    for (element in owners.values) release(element, event.eventTime)
                    owners.clear()
                }
            }
            send()
            invalidate()
            return true
        }

        private fun press(element: Element, x: Float, y: Float, time: Long) {
            performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY)
            when (element) {
                is Element.Button -> model.buttons = model.buttons or element.bit
                is Element.Trigger -> model.axes[element.axis] = 1f
                is Element.DPad -> move(element, x, y)
                is Element.Stick -> {
                    // A second tap soon after letting go clicks the stick.
                    if (time - (lastStickUp[element] ?: 0L) < 300L) {
                        model.buttons = model.buttons or element.thumb
                    }
                    stickOffsets[element] = 0f to 0f
                    move(element, x, y)
                }
            }
        }

        private fun move(element: Element, x: Float, y: Float) {
            val dx = x - px(element.x)
            val dy = y - px(element.y)
            when (element) {
                is Element.DPad -> {
                    val dead = px(element.arm) * 0.45f
                    model.axes[8] = if (dx > dead) 1f else if (dx < -dead) -1f else 0f
                    model.axes[9] = if (dy > dead) 1f else if (dy < -dead) -1f else 0f
                }
                is Element.Stick -> {
                    val range = px(element.radius)
                    val length = hypot(dx, dy).coerceAtLeast(1f)
                    val scale = if (length > range) range / length else 1f
                    stickOffsets[element] = dx * scale to dy * scale
                    model.axes[element.axisX] = (dx * scale / range).coerceIn(-1f, 1f)
                    model.axes[element.axisY] = (dy * scale / range).coerceIn(-1f, 1f)
                }
                else -> Unit
            }
        }

        private fun release(element: Element, time: Long) {
            when (element) {
                is Element.Button -> model.buttons = model.buttons and element.bit.inv()
                is Element.Trigger -> model.axes[element.axis] = 0f
                is Element.DPad -> {
                    model.axes[8] = 0f
                    model.axes[9] = 0f
                }
                is Element.Stick -> {
                    model.buttons = model.buttons and element.thumb.inv()
                    model.axes[element.axisX] = 0f
                    model.axes[element.axisY] = 0f
                    stickOffsets.remove(element)
                    lastStickUp[element] = time
                }
            }
        }

        override fun onDraw(canvas: Canvas) {
            for (element in elements) {
                val cx = px(element.x)
                val cy = px(element.y)
                val held = owners.containsValue(element)
                when (element) {
                    is Element.Button, is Element.Trigger -> {
                        canvas.drawCircle(cx, cy, px(element.radius), if (held) pressedFill else fill)
                        canvas.drawCircle(cx, cy, px(element.radius), stroke)
                        canvas.drawText(element.label, cx, cy - (text.ascent() + text.descent()) / 2f, text)
                    }
                    is Element.DPad -> {
                        val arm = px(element.arm)
                        for ((ax, ay, active) in listOf(
                            Triple(0f, -1f, model.axes[9] < 0f),
                            Triple(0f, 1f, model.axes[9] > 0f),
                            Triple(-1f, 0f, model.axes[8] < 0f),
                            Triple(1f, 0f, model.axes[8] > 0f),
                        )) {
                            val x = cx + ax * arm * 1.7f
                            val y = cy + ay * arm * 1.7f
                            canvas.drawCircle(x, y, arm, if (active) pressedFill else fill)
                            canvas.drawCircle(x, y, arm, stroke)
                        }
                    }
                    is Element.Stick -> {
                        val (ox, oy) = stickOffsets[element] ?: (0f to 0f)
                        canvas.drawCircle(cx, cy, px(element.radius), fill)
                        canvas.drawCircle(cx, cy, px(element.radius), stroke)
                        canvas.drawCircle(cx + ox, cy + oy, px(element.radius) * 0.45f, pressedFill)
                    }
                }
            }
        }
    }

    @SuppressLint("ViewConstructor")
    private class ToggleView(context: Context, val onToggle: () -> Unit) : View(context) {
        private val density = context.resources.displayMetrics.density
        private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = Color.argb(110, 20, 20, 24) }
        private val ink = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = Color.argb(220, 255, 255, 255)
            style = Paint.Style.STROKE
            strokeWidth = 2f * density
        }
        private val dot = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = Color.argb(220, 255, 255, 255) }

        init {
            contentDescription = "On-screen controller"
            setOnClickListener {
                performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY)
                onToggle()
            }
        }

        override fun onDraw(canvas: Canvas) {
            val w = width.toFloat()
            val h = height.toFloat()
            canvas.drawCircle(w / 2f, h / 2f, min(w, h) / 2f, fill)
            // A small controller outline: body plus two stick dots.
            val bw = w * 0.56f
            val bh = h * 0.32f
            canvas.drawRoundRect(w / 2f - bw / 2f, h / 2f - bh / 2f, w / 2f + bw / 2f, h / 2f + bh / 2f, bh / 2f, bh / 2f, ink)
            canvas.drawCircle(w / 2f - bw * 0.25f, h / 2f, bh * 0.16f, dot)
            canvas.drawCircle(w / 2f + bw * 0.25f, h / 2f, bh * 0.16f, dot)
        }
    }
}
