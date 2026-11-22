package app.polarbear

import android.content.Context
import android.hardware.input.InputManager
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.Gravity
import android.view.InputDevice
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.platform.ComposeView
import app.polarbear.gaming.Control
import app.polarbear.gaming.ControllerLayout
import app.polarbear.gaming.EditBar
import app.polarbear.gaming.GamePanel
import app.polarbear.gaming.Gamepad
import app.polarbear.gaming.LayoutStore
import app.polarbear.gaming.PadColors
import app.polarbear.gaming.PadFeel
import app.polarbear.gaming.PadHaptics
import app.polarbear.gaming.PadLayout
import app.polarbear.gaming.PadSink
import java.util.concurrent.Executors

/**
 * On-screen controller and the quick game panel.
 *
 * A small Portal capsule recalls the session controls. It remembers where
 * it is dragged and adds controller and game options only when relevant.
 *
 * The controller ([Gamepad]) feeds the "touch pad" through the same evdev
 * path as physical controllers ([GamepadBridge.touchPad]) and steps aside
 * while a physical controller is connected, unless the user asks for it
 * anyway. Over the desktop it floats: each control is its own view above
 * the native surface, so Android splits multitouch between them and a
 * finger outside every control still reaches the desktop.
 *
 * While the desktop is on a monitor, this screen is its touchpad page
 * ([PresentedDesktopPad]) and the floating controls stay away: that page
 * turns the whole screen into the same controller ([PresentedController]),
 * which feeds the same touch pad ([screenPad]).
 */
object TouchControls {
    private const val TAG = "PortalTouchControls"
    private const val PREFS = "portal_touch_controls"
    private const val PREF_ON = "on"
    private const val PREF_WITH_PAD = "show_with_pad"
    private const val PREF_HAPTICS = "haptics"
    private const val PREF_RUMBLE = "rumble"
    private const val PREF_ANALOG = "analog_triggers"
    private const val PREF_SCREEN_SCALE = "screen_scale"
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

    /** The desktop is on a monitor and this screen is its touchpad page. */
    val presenting = mutableStateOf(false)
    /** That page shows the whole-screen controller instead of the touchpad. */
    val screenController = mutableStateOf(false)
    /** The whole-screen controller's Size setting. */
    val screenScale = mutableStateOf(1f)

    private val main = Handler(Looper.getMainLooper())
    private val scanner = Executors.newSingleThreadExecutor { Thread(it, "touch-controls-scan").apply { isDaemon = true } }
    private var activity: PortalActivity? = null
    private var root: FrameLayout? = null
    private var handle: PortalCapsuleView? = null
    private var editBar: ComposeView? = null
    private var panel: ComposeView? = null
    private val retiringViews = mutableSetOf<View>()
    /** The floating controller, while it has been needed. */
    private var gamepad: Gamepad? = null
    private var sharedHaptics: PadHaptics? = null
    private var padPlugged = false
    private var scanning = false
    private var layoutKey: String? = "\u0000"
    private val axes = FloatArray(10)
    private var presentingOwner: Any? = null
    private var screenRelease: (() -> Unit)? = null
    // The whole-screen controller took the pad: it stays plugged in until the
    // desktop leaves the monitor, so a game never sees it go while the touchpad is out.
    private var screenPadUsed = false

    private val rootLayoutListener = View.OnLayoutChangeListener { _, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom ->
        if (right - left != oldRight - oldLeft || bottom - top != oldBottom - oldTop) {
            cancelInput()
            fitGamepad()
            placeHandle()
        }
    }

    private val deviceListener = object : InputManager.InputDeviceListener {
        override fun onInputDeviceAdded(deviceId: Int) = devicesChanged()
        override fun onInputDeviceRemoved(deviceId: Int) = devicesChanged()
        override fun onInputDeviceChanged(deviceId: Int) = devicesChanged()
    }

    /** The controller is wanted on screen right now (a game may still be needed). */
    val controllerVisible: Boolean
        get() = if (presenting.value) screenController.value
            else controllerOn.value && (!physicalPad.value || showWithPad.value)

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
        screenScale.value = prefs.getFloat(PREF_SCREEN_SCALE, 1f).coerceIn(PadLayout.MIN_USER_SCALE, PadLayout.MAX_USER_SCALE)
        padHaptics(host).enabled = haptics.value
        try {
            host.getSystemService(InputManager::class.java)?.registerInputDeviceListener(deviceListener, main)
        } catch (e: Exception) {
            Log.w(TAG, "input device listener unavailable", e)
        }
        physicalPad.value = physicalPadConnected(host)
        reloadLayout(force = true)
        refresh(host)
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
     * under Portal's own uid, so their command lines are readable. The
     * /proc walk is native (steam_processes.rs): this one allocated enough
     * every poll to keep the GC busy on an idle desktop.
     */
    private fun scanProcesses(): Scan {
        val fields = (nativeScanSteam() ?: return Scan(false, false, null, null)).split('\t')
        if (fields.size < 4) return Scan(false, false, null, null)
        return Scan(fields[0] == "1", fields[1] == "1", fields[2].ifEmpty { null }, fields[3].ifEmpty { null })
    }

    @JvmStatic private external fun nativeScanSteam(): String?

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
        gamepad?.let(::arrange)
    }

    // ---- View management -------------------------------------------------

    private enum class Mode { NONE, PLAY, EDIT }

    private var lastMode = Mode.NONE

    private fun mode(): Mode = when {
        // The touchpad page fills the screen and brings its own controller.
        presenting.value -> Mode.NONE
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
        val wantHandle = ComposeOverlay.desktopIsReady() && !editingState.value && panel == null
        if (wantHandle && handle == null) addHandle(host, overlay)
        if (!wantHandle && handle != null) removeView(handle).also { handle = null }
        if (editingState.value && editBar == null) addEditBar(host, overlay)
        if (!editingState.value && editBar != null) retireView(editBar).also { editBar = null }
    }

    /** The floating controller for the current mode, and the guest's touch pad with it. */
    private fun syncControls() {
        val overlay = root ?: return
        val host = activity ?: return
        val mode = mode()
        val modeChanged = mode != lastMode
        lastMode = mode
        if (mode == Mode.NONE) {
            gamepad?.hide()
        } else {
            val pad = gamepad ?: Gamepad(host, PadSink { buttons, axes -> lastMode == Mode.PLAY && sendPad(buttons, axes) },
                padHaptics(host), wholeScreen = false).also {
                it.colors = PadColors.OverGame
                it.onSelect = ::select
                it.onMoved = ::moved
                gamepad = it
            }
            // Above the desktop (and its touchpad), below the handle and Portal's own screens.
            pad.floatIn(overlay) { host.desktopLayerTop() }
            fitGamepad()
            arrange(pad)
            pad.editing = mode == Mode.EDIT
            pad.show()
        }
        when (mode) {
            // Plug the pad in now, so games and Steam see it before a touch.
            Mode.PLAY -> if (!padPlugged) plugNeutral()
            Mode.EDIT -> if (padPlugged && modeChanged) sendNeutral()
            Mode.NONE -> when {
                !presenting.value || !screenPadUsed -> unplug()
                // Back in front: the screen controller's pad comes back at rest.
                !padPlugged -> plugNeutral()
            }
        }
    }

    /** The floating controller fills the desktop, clear of the system bars. */
    private fun fitGamepad() {
        val overlay = root ?: return
        val pad = gamepad ?: return
        pad.setInsets(Gamepad.barInsets(overlay.rootWindowInsets))
        pad.resize(overlay.width, overlay.height)
    }

    /** The floating controller as the player arranged it. */
    private fun arrange(pad: Gamepad) {
        val current = layout.value
        pad.opacity = current.opacity
        pad.userScale = current.scale
        pad.elements = current.elements
        pad.analogTriggers = analog.value
        pad.selected = selectedState.value
    }

    private fun removeView(view: View?) {
        view ?: return
        view.animate().cancel()
        retiringViews.remove(view)
        (view.parent as? ViewGroup)?.removeView(view)
    }

    private fun retireView(view: View?) {
        view ?: return
        view.isEnabled = false
        retiringViews += view
        view.animate().cancel()
        view.animate().alpha(0f).scaleX(0.98f).scaleY(0.98f).setDuration(180)
            .withEndAction { removeView(view) }.start()
    }

    private fun removeAll() {
        retiringViews.toList().forEach(::removeView)
        gamepad?.detach()
        gamepad = null
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
        // At rest first: a program that misses the removal must not keep a held direction.
        sendNeutral()
        GamepadBridge.removeTouchPad()
    }

    private fun sendNeutral() {
        axes.fill(0f)
        GamepadBridge.touchPad(0, axes)
    }

    private fun plugNeutral() {
        padPlugged = true
        sendNeutral()
    }

    /** The guest's touch pad gets [buttons] and [axes], plugged in first if it was not. */
    private fun sendPad(buttons: Int, axes: FloatArray): Boolean {
        if (activity == null) return false
        padPlugged = true
        GamepadBridge.touchPad(buttons, axes)
        return true
    }

    /** The controller's touch feel, one for the app, following the Button vibration setting. */
    internal fun padHaptics(context: Context): PadHaptics =
        sharedHaptics ?: PadHaptics(context.applicationContext) { GamepadBridge.touchRumbling() }
            .also { it.enabled = haptics.value; sharedHaptics = it }

    /** A modal control or lost focus ends every held gesture immediately. */
    fun cancelInput() {
        screenRelease?.invoke()
        gamepad?.release()
        if (padPlugged) sendNeutral()
    }

    // ---- The layout editor -----------------------------------------------

    private fun select(control: Control) {
        selectedState.value = control
        gamepad?.selected = control
    }

    /** [control] was dropped with its centre at ([x], [y]) px in the overlay. */
    private fun moved(control: Control, x: Float, y: Float) {
        val overlay = root ?: return
        if (overlay.width <= 0 || overlay.height <= 0) return
        val state = layout.value.element(control)
        updateLayout(layout.value.with(control, state.copy(x = x / overlay.width, y = y / overlay.height)), persist = true)
    }

    // ---- Actions from the handle, the panel and the edit bar ------------

    fun toggleController() {
        val host = activity ?: return
        if (presenting.value) {
            setScreenController(!screenController.value)
            return
        }
        if (controllerVisible) {
            if (physicalPad.value) setShowWithPad(false) else setControllerOn(false)
        } else {
            setControllerOn(true)
            if (physicalPad.value) setShowWithPad(true)
        }
        refresh(host)
    }

    // ---- The touchpad page's whole-screen controller ---------------------

    /** The touchpad page is up; [release] ends every touch on its controller. */
    fun presentingStarted(owner: Any, release: () -> Unit) {
        presentingOwner = owner
        screenRelease = release
        if (!presenting.value) {
            presenting.value = true
            screenController.value = false
            screenPadUsed = false
        }
        if (editingState.value) stopEditing()
        closePanel(immediately = true)
        activity?.let(::refresh)
    }

    /** That page left: the floating controls may return, and the next page starts as a touchpad. */
    fun presentingEnded(owner: Any) {
        if (presentingOwner !== owner) return
        // Let go while the pad still reaches the guest, or it keeps the last push.
        screenRelease?.invoke()
        presentingOwner = null
        screenRelease = null
        presenting.value = false
        screenController.value = false
        screenPadUsed = false
        activity?.let(::refresh)
    }

    /** Shows the whole-screen controller on the touchpad page, or brings the touchpad back. */
    fun setScreenController(on: Boolean) {
        if (on && !presenting.value) return
        if (screenController.value == on) return
        if (!on) screenRelease?.invoke()
        screenController.value = on
    }

    fun setScreenScale(scale: Float, persist: Boolean) {
        val value = scale.coerceIn(PadLayout.MIN_USER_SCALE, PadLayout.MAX_USER_SCALE)
        screenScale.value = value
        if (persist) activity?.let { prefs(it).edit().putFloat(PREF_SCREEN_SCALE, value).apply() }
    }

    /**
     * How far Size can grow the controller on this screen before its controls
     * would run into each other: the touchpad page's ([wholeScreen], turned
     * sideways) or the floating one over the desktop.
     */
    fun sizeRoom(context: Context, wholeScreen: Boolean): Float {
        val metrics = context.resources.displayMetrics
        val desktop = root?.takeIf { !wholeScreen && it.width > 0 && it.height > 0 }
        val insets = Gamepad.barInsets(desktop?.rootWindowInsets)
        val long = maxOf(metrics.widthPixels, metrics.heightPixels).toFloat()
        val short = minOf(metrics.widthPixels, metrics.heightPixels).toFloat()
        val width = desktop?.width?.toFloat() ?: if (wholeScreen) long else metrics.widthPixels.toFloat()
        val height = desktop?.height?.toFloat() ?: if (wholeScreen) short else metrics.heightPixels.toFloat()
        return PadLayout(width, height, insets.left.toFloat(), insets.top.toFloat(), insets.right.toFloat(), insets.bottom.toFloat(),
            Gamepad.mmOf(metrics), 1f, wholeScreen).room
    }

    /** The whole-screen controller's state, buttons in xpad order. False when dropped: Portal is in the background. */
    fun screenPad(buttons: Int, axes: FloatArray): Boolean {
        if (!presenting.value || activity == null) return false
        screenPadUsed = true
        return sendPad(buttons, axes)
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
        padHaptics(host).enabled = on
        if (on) padHaptics(host).play(PadFeel.Press)
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
        gamepad?.analogTriggers = on
    }

    /** Apply [next] to the controls now; save it when the gesture ends. */
    fun updateLayout(next: ControllerLayout, persist: Boolean) {
        layout.value = next
        gamepad?.let(::arrange)
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
        cancelInput()
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
        handle?.let { it.showGameOptions(); return }
        val host = activity ?: return
        val overlay = root ?: return
        if (panel != null) return
        panelClosing.value = false
        val view = ComposeView(host).apply {
            setContent { app.polarbear.setup.PortalTheme { app.polarbear.setup.components.PortalUiDensity { GamePanel(onClosed = { closePanel(immediately = true) }) } } }
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
        val view = ComposeView(host).apply { setContent { app.polarbear.setup.PortalTheme { app.polarbear.setup.components.PortalUiDensity { EditBar() } } } }
        overlay.addView(
            view,
            FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.TOP or Gravity.CENTER_HORIZONTAL,
            ),
        )
        editBar = view
        view.alpha = 0f; view.translationY = -8 * host.resources.displayMetrics.density
        view.animate().alpha(1f).translationY(0f).setDuration(280).start()
    }

    fun overlayChanged() { activity?.let(::refresh) }

    // The phone's touchpad page stands in for the capsule's tab: where its
    // Portal button is, or null when no touchpad page is up.
    private var padButton: Offset? = null

    /** The touchpad page is up, with its Portal button at ([x], [y]) in the overlay host. */
    fun hideCapsuleForPad(x: Float, y: Float) {
        padButton = Offset(x, y)
        handle?.hideDockAt(x, y)
    }

    /** The touchpad page left; the capsule's tab returns to its edge. */
    fun restoreCapsule() {
        padButton = null
        handle?.showDock()
    }

    /** Opens a capsule page from a touchpad button at ([x], [y]); false while no capsule is up. */
    internal fun openFromPad(page: UtilityPage, x: Float, y: Float): Boolean {
        val view = handle ?: return false
        view.openFrom(page, x, y)
        return true
    }

    fun closeUtilityMenu(): Boolean = handle?.dismiss() ?: false
    /** False only when there is no Portal keyboard to show, so Android's may. */
    fun showTextKeyboard(): Boolean = handle?.also { it.showTextKeyboard() } != null
    fun textKeyboardUnwanted() { handle?.closeTextKeyboard() }
    fun openVeilFromUtility() { handle?.openVeil() }
    fun dispatchUtilityKey(event: KeyEvent): Boolean {
        if (!SessionControls.isUtilityVisible()) return false
        if (SessionControls.desktopKeyboardVisible && event.keyCode != KeyEvent.KEYCODE_BACK) return false
        if (event.keyCode in setOf(KeyEvent.KEYCODE_VOLUME_UP, KeyEvent.KEYCODE_VOLUME_DOWN, KeyEvent.KEYCODE_POWER)) return false
        if (event.keyCode == KeyEvent.KEYCODE_BACK || event.keyCode == KeyEvent.KEYCODE_ESCAPE) {
            if (event.action == KeyEvent.ACTION_UP) closeUtilityMenu()
        } else handle?.dispatchKeyEvent(event)
        return true
    }

    /** Back closes the panel, finishes editing or brings the touchpad back before reaching the guest. */
    fun onBackKey(event: KeyEvent): Boolean {
        if (dispatchUtilityKey(event)) return true
        if (event.keyCode != KeyEvent.KEYCODE_BACK) return false
        val screen = presenting.value && screenController.value
        if (panel == null && !editingState.value && !screen) return false
        if (event.action == KeyEvent.ACTION_UP) {
            when {
                panel != null -> closePanel()
                editingState.value -> stopEditing()
                else -> setScreenController(false)
            }
        }
        return true
    }

    private fun addHandle(host: PortalActivity, overlay: FrameLayout) {
        val view = PortalCapsuleView(host,
            onPositionChanged = { x, y -> saveHandle(x,y) },
            onSizeChanged = { placeHandle() },
            onOpen = { panel -> SessionControls.requestPanel(host,panel) },
            onEditController = { later { startEditing() } },
            onToggleController = { later { toggleController() } },
        )
        padButton?.let { view.hideDockAt(it.x, it.y) }
        val index = gamepad?.topIndex()?.takeIf { it >= 0 }?.plus(1) ?: host.desktopLayerTop()
        handle = view
        overlay.addView(view, index.coerceIn(0, overlay.childCount), FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT, Gravity.TOP or Gravity.START))
    }

    private fun placeHandle() {
        val view = handle ?: return
        if (view.dragging) return
        val overlay = root ?: return
        if (overlay.width <= 0 || view.width <= 0) return
        val preferences = prefs(overlay.context)
        view.place(preferences.getFloat(PREF_HANDLE_X, 0.5f), preferences.getFloat(PREF_HANDLE_Y, -1f))
    }

    private fun saveHandle(x: Float, y: Float) {
        val overlay = root ?: return
        if (overlay.width <= 0 || overlay.height <= 0) return
        prefs(overlay.context).edit()
            .putFloat(PREF_HANDLE_X, x)
            .putFloat(PREF_HANDLE_Y, y)
            .apply()
    }
}
