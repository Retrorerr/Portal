package app.polarbear

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.hardware.input.InputManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.util.Log
import android.view.Gravity
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.platform.ComposeView
import app.polarbear.gaming.Control
import app.polarbear.gaming.ControlHost
import app.polarbear.gaming.ControlKind
import app.polarbear.gaming.ControlView
import app.polarbear.gaming.ControllerLayout
import app.polarbear.gaming.EditBar
import app.polarbear.gaming.GamePanel
import app.polarbear.gaming.LayoutStore
import java.io.File
import java.util.concurrent.Executors
import kotlin.math.min

/**
 * On-screen controller and the quick game panel.
 *
 * While Steam runs, a small handle sits at the top of the screen (drag it
 * anywhere; it remembers where): one side shows or hides the controller
 * (only while a game runs), the other opens the game panel (controller options, layout editing and the settings
 * Portal's compatibility tools read at launch, see [app.polarbear.gaming.GameSettings]).
 *
 * The controller feeds the "touch pad" through the same evdev path as
 * physical controllers ([GamepadBridge.touchPad]) and steps aside while a
 * physical controller is connected, unless the user asks for it anyway.
 * Each control is its own view above the native surface, so Android splits
 * multitouch between them and a finger outside every control still
 * reaches the desktop.
 */
object TouchControls : ControlHost {
    private const val TAG = "PortalTouchControls"
    private const val PREFS = "portal_touch_controls"
    private const val PREF_ON = "on"
    private const val PREF_WITH_PAD = "show_with_pad"
    private const val PREF_HAPTICS = "haptics"
    private const val PREF_RUMBLE = "rumble"
    private const val PREF_ANALOG = "analog_triggers"
    private const val PREF_HANDLE_X = "handle_x"
    private const val PREF_HANDLE_Y = "handle_y"
    private const val POLL_MS = 2500L

    // Compose-observable state, read by the game panel and the edit bar.
    val gameRunning = mutableStateOf(false)
    val steamRunning = mutableStateOf(false)
    val appId = mutableStateOf<String?>(null)
    val gameName = mutableStateOf<String?>(null)
    val physicalPad = mutableStateOf(false)
    val controllerOn = mutableStateOf(false)
    val showWithPad = mutableStateOf(false)
    val haptics = mutableStateOf(true)
    val rumble = mutableStateOf(true)
    val analog = mutableStateOf(false)
    val editingState = mutableStateOf(false)
    val selectedState = mutableStateOf<Control?>(null)
    val layout = mutableStateOf(ControllerLayout())
    val appLayout = mutableStateOf(false)
    val panelClosing = mutableStateOf(false)

    private val main = Handler(Looper.getMainLooper())
    private val scanner = Executors.newSingleThreadExecutor { Thread(it, "touch-controls-scan").apply { isDaemon = true } }
    private var activity: PortalActivity? = null
    private var root: FrameLayout? = null
    private var handle: HandleView? = null
    private var editBar: ComposeView? = null
    private var panel: ComposeView? = null
    private val views = LinkedHashMap<Control, ControlView>()
    private var padPlugged = false
    private var scanning = false
    private var layoutKey: String? = "\u0000"
    private var vibrator: Vibrator? = null
    private val axes = FloatArray(10)

    private val rootLayoutListener = View.OnLayoutChangeListener { _, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom ->
        if (right - left != oldRight - oldLeft || bottom - top != oldBottom - oldTop) {
            placeAll()
            placeHandle()
        }
    }

    private val deviceListener = object : InputManager.InputDeviceListener {
        override fun onInputDeviceAdded(deviceId: Int) = devicesChanged()
        override fun onInputDeviceRemoved(deviceId: Int) = devicesChanged()
        override fun onInputDeviceChanged(deviceId: Int) = devicesChanged()
    }

    override val editing: Boolean get() = editingState.value
    override val selected: Control? get() = selectedState.value
    override val analogTriggers: Boolean get() = analog.value

    /** The controller is wanted on screen right now (a game may still be needed). */
    val controllerVisible: Boolean
        get() = controllerOn.value && (!physicalPad.value || showWithPad.value)

    /** Read by [GamepadBridge.rumble]: vibrate the tablet for the touch pad's rumble. */
    @JvmStatic fun rumbleEnabled(): Boolean = rumble.value

    fun attach(host: PortalActivity) {
        activity = host
        val prefs = prefs(host)
        controllerOn.value = prefs.getBoolean(PREF_ON, false)
        showWithPad.value = prefs.getBoolean(PREF_WITH_PAD, false)
        haptics.value = prefs.getBoolean(PREF_HAPTICS, true)
        rumble.value = prefs.getBoolean(PREF_RUMBLE, true)
        analog.value = prefs.getBoolean(PREF_ANALOG, false)
        vibrator = ownVibrator(host)
        try {
            host.getSystemService(InputManager::class.java)?.registerInputDeviceListener(deviceListener, main)
        } catch (e: Exception) {
            Log.w(TAG, "input device listener unavailable", e)
        }
        physicalPad.value = physicalPadConnected(host)
        reloadLayout(force = true)
        main.removeCallbacks(poll)
        main.post(poll)
    }

    fun detach() {
        main.removeCallbacks(poll)
        activity?.let { host ->
            try {
                host.getSystemService(InputManager::class.java)?.unregisterInputDeviceListener(deviceListener)
            } catch (_: Exception) {
            }
        }
        if (editingState.value) stopEditing()
        removeAll()
        activity = null
    }

    private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    private val poll = object : Runnable {
        override fun run() {
            val host = activity ?: return
            if (!scanning && !ComposeOverlay.isShowing()) {
                scanning = true
                scanner.execute {
                    val scan = scanProcesses()
                    main.post {
                        scanning = false
                        applyScan(scan)
                        activity?.let { refresh(it) }
                    }
                }
            } else if (ComposeOverlay.isShowing()) {
                refresh(host)
            }
            physicalPad.value = physicalPadConnected(host)
            main.postDelayed(this, POLL_MS)
        }
    }

    private class Scan(val steam: Boolean, val game: Boolean, val appId: String?, val name: String?)

    private fun applyScan(scan: Scan) {
        if (scan.game != gameRunning.value) Log.i(TAG, "Steam game running: ${scan.game} (app ${scan.appId})")
        gameRunning.value = scan.game
        steamRunning.value = scan.steam
        appId.value = scan.appId
        gameName.value = scan.name
        reloadLayout(force = false)
    }

    /**
     * Steam starts every game as `reaper SteamLaunch AppId=<id> -- ...`,
     * which lives exactly as long as the game; the client itself is
     * `steam` (with its `steamwebhelper` children). Guest processes run
     * under Portal's own uid, so their command lines are readable.
     */
    private fun scanProcesses(): Scan {
        val processes = File("/proc").listFiles() ?: return Scan(false, false, null, null).also { Log.w(TAG, "/proc unreadable") }
        var steam = false
        var game = false
        var app: String? = null
        var name: String? = null
        for (entry in processes) {
            val pid = entry.name
            if (pid.isEmpty() || !pid[0].isDigit()) continue
            try {
                val cmdline = File(entry, "cmdline").readBytes()
                if (cmdline.isEmpty()) continue
                val end = cmdline.indexOf(0).let { if (it < 0) cmdline.size else it }
                val arg0 = String(cmdline, 0, end).substringAfterLast('/')
                if (arg0 == "steam" || arg0 == "steamwebhelper") {
                    steam = true
                } else if (arg0 == "reaper") {
                    val args = String(cmdline).split('\u0000')
                    if ("SteamLaunch" !in args) continue
                    game = true
                    steam = true
                    app = args.firstOrNull { it.startsWith("AppId=") }?.removePrefix("AppId=")?.takeIf { id ->
                        id.isNotEmpty() && id.all { it.isDigit() } && id != "0"
                    } ?: app
                    // The game's folder under steamapps/common is its readable name.
                    name = args.firstNotNullOfOrNull { arg ->
                        arg.substringAfter("/steamapps/common/", "").substringBefore('/').takeIf { it.isNotEmpty() }
                    } ?: name
                }
            } catch (_: Exception) {
            }
        }
        return Scan(steam, game, if (game) app else null, if (game) name else null)
    }

    private fun physicalPadConnected(context: Context): Boolean = try {
        val input = context.getSystemService(InputManager::class.java)
        input?.inputDeviceIds?.any { id ->
            val device = InputDevice.getDevice(id)
            // Sticks make a controller; keyboards may claim gamepad keys.
            device != null && !device.isVirtual &&
                (device.sources and InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK
        } ?: false
    } catch (_: Exception) {
        false
    }

    private fun devicesChanged() {
        val host = activity ?: return
        val connected = physicalPadConnected(host)
        if (connected != physicalPad.value) {
            Log.i(TAG, "physical controller connected: $connected")
            physicalPad.value = connected
            refresh(host)
        }
    }

    private fun reloadLayout(force: Boolean) {
        val host = activity ?: return
        val key = appId.value
        if (!force && key == layoutKey) return
        layoutKey = key
        layout.value = LayoutStore.load(host, key)
        appLayout.value = LayoutStore.hasAppLayout(host, key)
        selectedState.value = null
        if (views.isNotEmpty()) syncControls(place = true)
    }

    // ---- View management -------------------------------------------------

    private enum class Mode { NONE, PLAY, EDIT }

    private var lastMode = Mode.NONE

    private fun mode(): Mode = when {
        editingState.value -> Mode.EDIT
        gameRunning.value && controllerVisible -> Mode.PLAY
        else -> Mode.NONE
    }

    private fun refresh(host: PortalActivity) {
        if (ComposeOverlay.isShowing()) {
            if (editingState.value) stopEditing()
            removeAll()
            return
        }
        val overlay = try {
            host.overlayHost()
        } catch (e: Exception) {
            Log.w(TAG, "overlay host unavailable", e)
            return
        }
        if (root !== overlay) {
            root?.removeOnLayoutChangeListener(rootLayoutListener)
            removeAll()
            root = overlay
            overlay.addOnLayoutChangeListener(rootLayoutListener)
        }
        syncControls()
        val wantHandle = (steamRunning.value || gameRunning.value) && !editingState.value && panel == null
        if (wantHandle && handle == null) addHandle(host, overlay)
        if (!wantHandle && handle != null) removeView(handle).also { handle = null }
        handle?.update()
        if (editingState.value && editBar == null) addEditBar(host, overlay)
        if (!editingState.value && editBar != null) removeView(editBar).also { editBar = null }
    }

    /** Add and remove control views for the current mode; place them when anything changed. */
    private fun syncControls(place: Boolean = false) {
        val overlay = root ?: return
        val host = activity ?: return
        val mode = mode()
        val modeChanged = mode != lastMode
        lastMode = mode
        var changed = false
        val wanted: Set<Control> = when (mode) {
            Mode.NONE -> emptySet()
            Mode.EDIT -> Control.values().toSet()
            Mode.PLAY -> Control.values().filter { layout.value.element(it).visible }.toSet()
        }
        val iterator = views.entries.iterator()
        while (iterator.hasNext()) {
            val (control, view) = iterator.next()
            if (control !in wanted) {
                view.release()
                removeView(view)
                iterator.remove()
                changed = true
            }
        }
        for (control in Control.values()) {
            if (control in wanted && control !in views) {
                val view = ControlView(host, control, this)
                // Above the native surface (child 0), below the handle and Portal's own screens.
                overlay.addView(view, min(1, overlay.childCount), FrameLayout.LayoutParams(1, 1, Gravity.TOP or Gravity.START))
                views[control] = view
                changed = true
            }
        }
        // Touches held in one mode mean nothing in the other.
        if (modeChanged) views.values.forEach { it.release() }
        if (changed || modeChanged || place) placeAll()
        when (mode) {
            Mode.PLAY -> {
                // Plug the pad in now, so games and Steam see it before a touch.
                if (!padPlugged) {
                    padPlugged = true
                    padChanged()
                }
            }
            Mode.EDIT -> if (padPlugged && modeChanged) sendNeutral()
            Mode.NONE -> unplug()
        }
    }

    private fun placeAll() {
        val overlay = root ?: return
        val width = overlay.width
        val height = overlay.height
        if (width <= 0 || height <= 0) {
            overlay.post { if (root === overlay && overlay.width > 0) placeAll() }
            return
        }
        val density = overlay.resources.displayMetrics.density
        val current = layout.value
        for ((control, view) in views) {
            // A control being dragged keeps its offset until it is dropped.
            if (view.isDragging) continue
            val state = current.element(control)
            val scale = current.scale * state.scale
            val drawnWidth = control.width * scale * density
            val drawnHeight = control.height * scale * density
            val margin = if (control.kind == ControlKind.STICK) drawnWidth * 0.16f else 12f * density * scale
            val viewWidth = drawnWidth + margin * 2f
            val viewHeight = drawnHeight + margin * 2f
            var cx: Float
            var cy: Float
            if (state.placed) {
                cx = state.x * width
                cy = state.y * height
            } else {
                val (dx, dy) = ControllerLayout.defaultCenter(control, width / density, height / density)
                cx = dx * density
                cy = dy * density
            }
            // Keep the drawn control on screen.
            cx = cx.coerceIn(drawnWidth / 2f, (width - drawnWidth / 2f).coerceAtLeast(drawnWidth / 2f))
            cy = cy.coerceIn(drawnHeight / 2f, (height - drawnHeight / 2f).coerceAtLeast(drawnHeight / 2f))
            val params = (view.layoutParams as? FrameLayout.LayoutParams)
                ?: FrameLayout.LayoutParams(1, 1, Gravity.TOP or Gravity.START)
            params.width = viewWidth.toInt()
            params.height = viewHeight.toInt()
            params.gravity = Gravity.TOP or Gravity.START
            params.leftMargin = (cx - viewWidth / 2f).toInt()
            params.topMargin = (cy - viewHeight / 2f).toInt()
            view.layoutParams = params
            view.translationX = 0f
            view.translationY = 0f
            view.setGeometry(drawnWidth, drawnHeight, margin)
            view.alpha = current.opacity
            view.ghost = editingState.value && !state.visible
        }
    }

    private fun removeView(view: View?) {
        view ?: return
        (view.parent as? ViewGroup)?.removeView(view)
    }

    private fun removeAll() {
        for (view in views.values) {
            view.release()
            removeView(view)
        }
        views.clear()
        unplug()
        removeView(handle)
        handle = null
        removeView(editBar)
        editBar = null
        removeView(panel)
        panel = null
        panelClosing.value = false
        lastMode = Mode.NONE
    }

    private fun unplug() {
        if (!padPlugged) return
        padPlugged = false
        GamepadBridge.removeTouchPad()
    }

    private fun sendNeutral() {
        axes.fill(0f)
        GamepadBridge.touchPad(0, axes)
    }

    // ---- ControlHost -----------------------------------------------------

    override fun padChanged() {
        if (editingState.value || !padPlugged) return
        var buttons = 0
        axes.fill(0f)
        for (view in views.values) {
            buttons = buttons or view.buttons
            view.writeAxes(axes)
        }
        GamepadBridge.touchPad(buttons, axes)
    }

    override fun feedback(strong: Boolean) {
        if (!haptics.value) return
        val vibrator = vibrator ?: return
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                vibrator.vibrate(
                    VibrationEffect.createPredefined(
                        if (strong) VibrationEffect.EFFECT_CLICK else VibrationEffect.EFFECT_TICK,
                    ),
                )
            } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                vibrator.vibrate(VibrationEffect.createOneShot(if (strong) 14L else 8L, VibrationEffect.DEFAULT_AMPLITUDE))
            } else {
                @Suppress("DEPRECATION")
                vibrator.vibrate(if (strong) 14L else 8L)
            }
        } catch (_: Exception) {
        }
    }

    override fun select(control: Control) {
        selectedState.value = control
        views.values.forEach { it.invalidate() }
    }

    override fun moved(control: Control, x: Float, y: Float) {
        val overlay = root ?: return
        if (overlay.width <= 0 || overlay.height <= 0) return
        val state = layout.value.element(control)
        updateLayout(layout.value.with(control, state.copy(x = x / overlay.width, y = y / overlay.height)), persist = true)
    }

    override fun clampDrag(view: View, dx: Float, dy: Float): Pair<Float, Float> {
        val overlay = root ?: return dx to dy
        val control = view as? ControlView ?: return dx to dy
        val minX = -control.pad
        val maxX = overlay.width - view.width + control.pad
        val minY = -control.pad
        val maxY = overlay.height - view.height + control.pad
        val x = (view.x + dx).coerceIn(minX, maxOf(minX, maxX))
        val y = (view.y + dy).coerceIn(minY, maxOf(minY, maxY))
        return (x - view.x) to (y - view.y)
    }

    // ---- Actions from the handle, the panel and the edit bar ------------

    fun toggleController() {
        val host = activity ?: return
        if (controllerVisible) {
            if (physicalPad.value) setShowWithPad(false) else setControllerOn(false)
        } else {
            setControllerOn(true)
            if (physicalPad.value) setShowWithPad(true)
        }
        refresh(host)
    }

    fun setControllerOn(on: Boolean) {
        val host = activity ?: return
        controllerOn.value = on
        prefs(host).edit().putBoolean(PREF_ON, on).apply()
        refresh(host)
    }

    fun setShowWithPad(on: Boolean) {
        val host = activity ?: return
        showWithPad.value = on
        prefs(host).edit().putBoolean(PREF_WITH_PAD, on).apply()
        refresh(host)
    }

    fun setHaptics(on: Boolean) {
        val host = activity ?: return
        haptics.value = on
        prefs(host).edit().putBoolean(PREF_HAPTICS, on).apply()
        if (on) feedback(strong = true)
    }

    fun setRumble(on: Boolean) {
        val host = activity ?: return
        rumble.value = on
        prefs(host).edit().putBoolean(PREF_RUMBLE, on).apply()
    }

    fun setAnalogTriggers(on: Boolean) {
        val host = activity ?: return
        analog.value = on
        prefs(host).edit().putBoolean(PREF_ANALOG, on).apply()
    }

    /** Apply [next] to the controls now; save it when the gesture ends. */
    fun updateLayout(next: ControllerLayout, persist: Boolean) {
        layout.value = next
        placeAll()
        if (persist) activity?.let { LayoutStore.save(it, appId.value, next) }
    }

    fun setElementScale(control: Control, scale: Float, persist: Boolean) {
        val state = layout.value.element(control)
        val value = scale.coerceIn(ControllerLayout.MIN_ELEMENT_SCALE, ControllerLayout.MAX_ELEMENT_SCALE)
        updateLayout(layout.value.with(control, state.copy(scale = value)), persist)
    }

    fun setElementVisible(control: Control, visible: Boolean) {
        val state = layout.value.element(control)
        updateLayout(layout.value.with(control, state.copy(visible = visible)), persist = true)
    }

    fun resetLayout() {
        selectedState.value = null
        updateLayout(ControllerLayout(), persist = true)
    }

    fun setAppLayout(own: Boolean) {
        val host = activity ?: return
        val id = appId.value ?: return
        LayoutStore.setAppLayout(host, id, own)
        reloadLayout(force = true)
    }

    fun startEditing() {
        if (activity == null) return
        selectedState.value = null
        editingState.value = true
        closePanel(immediately = true)
    }

    fun stopEditing() {
        editingState.value = false
        selectedState.value = null
        activity?.let { host -> LayoutStore.save(host, appId.value, layout.value) }
        // Usually called from the edit bar's own click: drop it after the event.
        main.post { activity?.let { refresh(it) } }
    }

    fun openPanel() {
        val host = activity ?: return
        val overlay = root ?: return
        if (panel != null) return
        panelClosing.value = false
        val view = ComposeView(host).apply {
            setContent { GamePanel(onClosed = { closePanel(immediately = true) }) }
        }
        overlay.addView(view, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        panel = view
        refresh(host)
    }

    /**
     * Start the panel's exit animation, or drop it at once. Removal waits
     * for the next main-loop turn: callers are the panel's own clicks and
     * effects, which must not detach their view mid-dispatch.
     */
    fun closePanel(immediately: Boolean = false) {
        if (panel == null) return
        if (!immediately) {
            panelClosing.value = true
            return
        }
        main.post {
            removeView(panel)
            panel = null
            panelClosing.value = false
            activity?.let { refresh(it) }
        }
    }

    /** From the handle: act after its touch event finishes dispatching. */
    private fun later(action: () -> Unit) {
        main.post(action)
    }

    private fun addEditBar(host: PortalActivity, overlay: FrameLayout) {
        val view = ComposeView(host).apply { setContent { EditBar() } }
        overlay.addView(
            view,
            FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.TOP or Gravity.CENTER_HORIZONTAL,
            ),
        )
        editBar = view
    }

    /** Back closes the panel or finishes editing before reaching the guest. */
    fun onBackKey(event: KeyEvent): Boolean {
        if (event.keyCode != KeyEvent.KEYCODE_BACK) return false
        if (panel == null && !editingState.value) return false
        if (event.action == KeyEvent.ACTION_UP) {
            if (panel != null) closePanel() else stopEditing()
        }
        return true
    }

    private fun ownVibrator(context: Context): Vibrator? = try {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            context.getSystemService(VibratorManager::class.java)?.defaultVibrator
        } else {
            @Suppress("DEPRECATION")
            context.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
        }
    } catch (_: Exception) {
        null
    }?.takeIf { it.hasVibrator() }

    private fun addHandle(host: PortalActivity, overlay: FrameLayout) {
        val density = host.resources.displayMetrics.density
        val view = HandleView(host)
        // Placed by translation from the top-left corner, see [placeHandle].
        val params = FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.WRAP_CONTENT,
            (36f * density).toInt(),
            Gravity.TOP or Gravity.START,
        )
        // Above the controls, below Portal's own screens and the panel.
        val index = views.values.maxOfOrNull { overlay.indexOfChild(it) }?.plus(1) ?: min(1, overlay.childCount)
        overlay.addView(view, index.coerceIn(0, overlay.childCount), params)
        handle = view
    }

    /**
     * Put the handle where the user left it: its centre as a fraction of the
     * overlay's width and its top as a fraction of the height, so it keeps its
     * spot across rotation and folding. Unmoved, it sits top centre.
     */
    private fun placeHandle() {
        val view = handle ?: return
        val overlay = root ?: return
        if (view.dragging) return
        val width = overlay.width.toFloat()
        val height = overlay.height.toFloat()
        if (width <= 0f || view.width <= 0) return
        val density = overlay.resources.displayMetrics.density
        val prefs = prefs(overlay.context)
        val cx = prefs.getFloat(PREF_HANDLE_X, 0.5f) * width
        val top = prefs.getFloat(PREF_HANDLE_Y, -1f).let { if (it < 0f) 8f * density else it * height }
        view.translationX = (cx - view.width / 2f).coerceIn(0f, (width - view.width).coerceAtLeast(0f))
        view.translationY = top.coerceIn(0f, (height - view.height).coerceAtLeast(0f))
    }

    private fun saveHandle(view: View) {
        val overlay = root ?: return
        if (overlay.width <= 0 || overlay.height <= 0) return
        prefs(overlay.context).edit()
            .putFloat(PREF_HANDLE_X, (view.translationX + view.width / 2f) / overlay.width)
            .putFloat(PREF_HANDLE_Y, view.translationY / overlay.height)
            .apply()
    }

    /**
     * The top-of-screen handle: a controller segment (while a game runs)
     * and a settings segment that opens the panel. Long-press also opens it;
     * dragging moves it.
     */
    private const val IDLE_ALPHA = 0.82f

    @SuppressLint("ViewConstructor")
    private class HandleView(context: Context) : View(context) {
        private val density = context.resources.displayMetrics.density
        private val segment = 46f * density
        private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = ControlView.withAlpha(ControlView.CHARCOAL, 170) }
        private val border = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            style = Paint.Style.STROKE
            strokeWidth = 1f * density
            color = ControlView.withAlpha(ControlView.IVORY, 52)
        }
        private val ink = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            style = Paint.Style.STROKE
            strokeWidth = 1.8f * density
            strokeCap = Paint.Cap.ROUND
            color = ControlView.withAlpha(ControlView.IVORY, 225)
        }
        private val dot = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = ControlView.withAlpha(ControlView.IVORY, 225) }
        private val rect = RectF()
        private val slop = android.view.ViewConfiguration.get(context).scaledTouchSlop.toFloat()
        private var downX = 0f
        private var downRawX = 0f
        private var downRawY = 0f
        private var startX = 0f
        private var startY = 0f
        private var longPressed = false
        var dragging = false
            private set
        private val longPress = Runnable {
            longPressed = true
            performHapticFeedback(android.view.HapticFeedbackConstants.LONG_PRESS)
            later { openPanel() }
        }
        private var shownSegments = 0

        init {
            contentDescription = "Game controls"
            alpha = IDLE_ALPHA
        }

        private val segments: Int get() = if (gameRunning.value) 2 else 1

        override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
            setMeasuredDimension((segment * segments).toInt(), MeasureSpec.getSize(heightMeasureSpec))
        }

        override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
            // Growing a segment keeps the handle's centre where it was.
            if (w > 0) placeHandle()
        }

        /** Grow or shrink with the controller segment, and redraw. */
        fun update() {
            if (shownSegments != segments) {
                shownSegments = segments
                requestLayout()
            }
            invalidate()
        }

        @SuppressLint("ClickableViewAccessibility")
        override fun onTouchEvent(event: MotionEvent): Boolean {
            when (event.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    downX = event.x
                    downRawX = event.rawX
                    downRawY = event.rawY
                    startX = translationX
                    startY = translationY
                    longPressed = false
                    dragging = false
                    postDelayed(longPress, 450L)
                }
                MotionEvent.ACTION_MOVE -> {
                    val dx = event.rawX - downRawX
                    val dy = event.rawY - downRawY
                    if (!dragging && !longPressed && dx * dx + dy * dy > slop * slop) {
                        dragging = true
                        removeCallbacks(longPress)
                        parent?.requestDisallowInterceptTouchEvent(true)
                        animate().scaleX(1.12f).scaleY(1.12f).alpha(1f).setDuration(120L).start()
                    }
                    if (dragging) {
                        val parentView = parent as? View
                        val maxX = ((parentView?.width ?: 0) - width).toFloat().coerceAtLeast(0f)
                        val maxY = ((parentView?.height ?: 0) - height).toFloat().coerceAtLeast(0f)
                        translationX = (startX + dx).coerceIn(0f, maxX)
                        translationY = (startY + dy).coerceIn(0f, maxY)
                    }
                }
                MotionEvent.ACTION_UP -> {
                    removeCallbacks(longPress)
                    if (dragging) {
                        dragging = false
                        animate().scaleX(1f).scaleY(1f).alpha(IDLE_ALPHA).setDuration(160L).start()
                        saveHandle(this)
                    } else if (!longPressed) {
                        performHapticFeedback(android.view.HapticFeedbackConstants.VIRTUAL_KEY)
                        if (segments == 2 && downX < segment) later { toggleController() } else later { openPanel() }
                    }
                }
                MotionEvent.ACTION_CANCEL -> {
                    removeCallbacks(longPress)
                    if (dragging) {
                        dragging = false
                        animate().scaleX(1f).scaleY(1f).alpha(IDLE_ALPHA).setDuration(160L).start()
                        saveHandle(this)
                    }
                }
            }
            return true
        }

        override fun onDraw(canvas: Canvas) {
            val w = width.toFloat()
            val h = height.toFloat()
            rect.set(0.5f * density, 0.5f * density, w - 0.5f * density, h - 0.5f * density)
            canvas.drawRoundRect(rect, h / 2f, h / 2f, fill)
            canvas.drawRoundRect(rect, h / 2f, h / 2f, border)
            var cx = segment / 2f
            val cy = h / 2f
            if (segments == 2) {
                // Controller outline: body plus two stick dots, orange while shown.
                val color = if (controllerVisible) ControlView.ORANGE else ControlView.withAlpha(ControlView.IVORY, 225)
                ink.color = color
                dot.color = color
                val bw = segment * 0.5f
                val bh = h * 0.36f
                rect.set(cx - bw / 2f, cy - bh / 2f, cx + bw / 2f, cy + bh / 2f)
                canvas.drawRoundRect(rect, bh / 2f, bh / 2f, ink)
                canvas.drawCircle(cx - bw * 0.22f, cy, bh * 0.14f, dot)
                canvas.drawCircle(cx + bw * 0.22f, cy, bh * 0.14f, dot)
                ink.color = ControlView.withAlpha(ControlView.IVORY, 60)
                canvas.drawLine(segment, h * 0.28f, segment, h * 0.72f, ink)
                cx += segment
            }
            // Settings: three slider lines with knobs.
            ink.color = ControlView.withAlpha(ControlView.IVORY, 225)
            dot.color = ControlView.withAlpha(ControlView.IVORY, 225)
            val half = segment * 0.2f
            for ((row, knob) in listOf(-1 to 0.45f, 0 to -0.4f, 1 to 0.15f)) {
                val y = cy + row * h * 0.17f
                canvas.drawLine(cx - half, y, cx + half, y, ink)
                canvas.drawCircle(cx + knob * half, y, 2.6f * density, dot)
            }
        }
    }
}
