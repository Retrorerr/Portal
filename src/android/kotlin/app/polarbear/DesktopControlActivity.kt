package app.polarbear

import android.app.ActivityOptions
import android.content.Context
import android.content.Intent
import android.graphics.PixelFormat
import android.hardware.display.DisplayManager
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.Display
import android.view.Gravity
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.WindowManager
import android.view.WindowManager.LayoutParams
import androidx.activity.ComponentActivity
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import androidx.lifecycle.setViewTreeLifecycleOwner
import androidx.savedstate.SavedStateRegistry
import androidx.savedstate.SavedStateRegistryController
import androidx.savedstate.SavedStateRegistryOwner
import androidx.savedstate.setViewTreeSavedStateRegistryOwner
import java.lang.ref.WeakReference

/** Read by Portal's accessibility service: a tablet grab owns the keyboard. */
object TabletGrab {
    @JvmField @Volatile var active = false

    /** A grab Portal's accessibility service feeds; it needs no window focus. */
    internal interface Routed {
        fun motion(event: MotionEvent)
        fun key(event: KeyEvent): Boolean
        fun lost(reason: String)
    }

    /**
     * UI thread, like the accessibility service's callbacks. Observed, so
     * every control shows a grab another one holds.
     */
    internal var routed: Routed? by mutableStateOf(null)

    @JvmStatic fun isRouted(): Boolean = routed != null
    @JvmStatic fun routeMotion(event: MotionEvent) { routed?.motion(event) }
    @JvmStatic fun routeKey(event: KeyEvent): Boolean = routed?.key(event) ?: false
    @JvmStatic fun routeLost() { routed?.lost("accessibility service stopped") }
}

/**
 * The tablet side of a desktop shown on another display.
 *
 * Android grants pointer capture only to the focused window. Focus stays on
 * the monitor after a tap on a non-focusable overlay, so every grab is held
 * by this activity: starting it moves Android's focus to the tablet on every
 * device, with or without per-display focus. The optional floating handle
 * (Display over other apps) only starts it.
 */
class DesktopControlActivity : ComponentActivity() {
    private var control: DesktopControlView? = null
    private var stopObserving: (() -> Unit)? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val host = projectedDesktop()
        val grab = intent.getBooleanExtra(EXTRA_TAKE_CONTROL, false)
        // A grab without focus belongs to the floating handle, which stays on
        // screen while Android apps are used by touch.
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O || host == null || (grab && openTouchpadInstead(host)) ||
            (!grab && showOverlay(host)) || (grab && overlay?.view?.takeRoutedControl() == true)
        ) {
            finish()
            return
        }
        current = WeakReference(this)
        overlay?.setHidden(true)
        setFinishOnTouchOutside(false)
        window.clearFlags(LayoutParams.FLAG_DIM_BEHIND)
        window.addFlags(LayoutParams.FLAG_NOT_TOUCH_MODAL or LayoutParams.FLAG_WATCH_OUTSIDE_TOUCH)
        window.setGravity(controlGravity(host.portalDisplays.tabletEdge))
        control = DesktopControlView(this, host, holdsGrab = true, close = ::finish).also(::setContentView)
        window.setLayout(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT)
        stopObserving = host.portalDisplays.observe(::displayChanged)
        if (grab) control?.takeControl()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (intent.getBooleanExtra(EXTRA_TAKE_CONTROL, false)) control?.takeControl()
    }

    override fun dispatchTouchEvent(event: MotionEvent): Boolean {
        if (event.actionMasked == MotionEvent.ACTION_OUTSIDE) {
            control?.outsideTouch(event)
            return true
        }
        return super.dispatchTouchEvent(event)
    }

    override fun onStop() {
        // A controller left behind another app would only hold stale state,
        // unless it holds a grab that needs no focus: that one lasts while
        // Android apps are used by touch, until Ctrl+Alt or the monitor edge.
        if (control?.holdsRoutedGrab != true) finish()
        super.onStop()
    }

    override fun onDestroy() {
        control?.dispose()
        stopObserving?.invoke()
        if (current.get() === this) {
            current.clear()
            overlay?.let { it.setHidden(handleHidden(it.host)) }
        }
        super.onDestroy()
    }

    internal companion object {
        private const val TAG = "PortalDesktopControl"
        private const val EXTRA_TAKE_CONTROL = "app.polarbear.extra.TAKE_CONTROL"
        private var desktop = WeakReference<PortalActivity>(null)
        private var current = WeakReference<DesktopControlActivity>(null)
        private var overlay: DesktopControlOverlay? = null

        fun bindDesktop(host: PortalActivity) { desktop = WeakReference(host) }

        /** Portal while its desktop is on another display: moved there, or presented there. */
        fun projectedDesktop(): PortalActivity? = desktop.get()?.takeIf { host ->
            !host.isFinishing && !host.isDestroyed && host.desktopDisplay()?.let {
                it.displayId != Display.DEFAULT_DISPLAY && it.isValid
            } == true
        }

        /**
         * UI thread: the floating handle exists exactly while the desktop is on
         * another display and Floating controls is on. It hides while the
         * controller or a phone's touchpad page shows the same controls.
         */
        fun displayChanged() {
            val host = projectedDesktop()
            if (host == null) {
                overlay?.close()
                current.get()?.finish()
            } else if (host.portalDisplays.floatingControls) {
                showOverlay(host)
            } else {
                overlay?.close()
            }
        }

        fun placeHandle() {
            val host = projectedDesktop() ?: return
            overlay?.placeAtEdge()
            current.get()?.window?.setGravity(controlGravity(host.portalDisplays.tabletEdge))
        }

        /** A Portal surface opened on the desktop's own display takes its input back. */
        fun releaseTabletInput() {
            current.get()?.control?.release("Portal controls opened")
            TabletGrab.routed?.lost("Portal controls opened")
        }

        fun controlIntent(context: Context, takeControl: Boolean): Intent =
            Intent(context, DesktopControlActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
                .putExtra(EXTRA_TAKE_CONTROL, takeControl)

        fun tabletOptions(): Bundle? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            ActivityOptions.makeBasic().setLaunchDisplayId(Display.DEFAULT_DISPLAY).toBundle()
        } else null

        /** Hand the mouse, touchpad and keyboard to the desktop. */
        fun startControl(host: PortalActivity) = start(host, takeControl = true)

        /** The floating handle when Android allows it, otherwise the controller itself. */
        fun showOnTablet(host: PortalActivity) {
            // A phone presenting the desktop shows its touchpad page instead.
            if (projectedDesktop() !== host || host.presentedDisplay != null) return
            if (current.get() == null && !showOverlay(host)) start(host, takeControl = false)
        }

        /**
         * A phone presenting the desktop with only its own touchscreen has
         * nothing to hand over: Control desktop brings its touchpad page
         * forward instead. False when a grab should go ahead.
         */
        fun openTouchpadInstead(host: PortalActivity): Boolean {
            if (host.presentedDisplay == null || externalPointerConnected()) return false
            host.portalDisplays.bringPortalForward()
            return true
        }

        /** A mouse or touchpad other than the device's own screen; true where Android can't tell. */
        private fun externalPointerConnected(): Boolean {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return true
            return InputDevice.getDeviceIds().any { id ->
                val device = InputDevice.getDevice(id) ?: return@any false
                device.isExternal && !device.isVirtual &&
                    (device.supportsSource(InputDevice.SOURCE_MOUSE) || device.supportsSource(InputDevice.SOURCE_TOUCHPAD))
            }
        }

        private fun handleHidden(host: PortalActivity) = current.get() != null || host.showsPresentedPad

        private fun start(host: PortalActivity, takeControl: Boolean) {
            if (projectedDesktop() !== host) return
            try {
                host.startActivity(controlIntent(host, takeControl), tabletOptions())
            } catch (error: RuntimeException) {
                Log.w(TAG, "Could not open the tablet controls", error)
            }
        }

        private fun showOverlay(host: PortalActivity): Boolean {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O || !host.portalDisplays.floatingControls) return false
            overlay?.let {
                if (it.host === host) {
                    it.placeAtEdge()
                    it.setHidden(handleHidden(host))
                    return true
                }
                it.close()
            }
            try {
                val tablet = host.getSystemService(DisplayManager::class.java)?.getDisplay(Display.DEFAULT_DISPLAY)
                    ?: return false
                val context = host.applicationContext.createDisplayContext(tablet)
                val windowContext = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    context.createWindowContext(LayoutParams.TYPE_APPLICATION_OVERLAY, null)
                } else context
                val handle = DesktopControlOverlay(host, windowContext) { closed ->
                    if (overlay === closed) overlay = null
                }
                handle.show()
                overlay = handle
                handle.setHidden(handleHidden(host))
                return true
            } catch (error: RuntimeException) {
                Log.w(TAG, "Could not show the floating tablet controls", error)
                return false
            }
        }

        @JvmStatic external fun nativeControlMotion(x: Float, y: Float, timeNs: Long)
        @JvmStatic external fun nativeControlButton(code: Int, pressed: Boolean, timeNs: Long)
        @JvmStatic external fun nativeControlWheel(horizontal: Float, vertical: Float, timeNs: Long)
        @JvmStatic external fun nativeControlFingerScroll(horizontal: Float, vertical: Float, finished: Boolean, timeNs: Long)
        @JvmStatic external fun nativeControlPinch(phase: Int, scale: Float, timeNs: Long)
        @JvmStatic external fun nativeControlKey(code: Int, scan: Int, pressed: Boolean)
    }
}

/** The controls sit on the tablet edge nearest the monitor. */
internal fun controlGravity(edge: TabletEdge): Int =
    (if (edge == TabletEdge.Right) Gravity.LEFT else Gravity.RIGHT) or Gravity.CENTER_VERTICAL

/**
 * The floating handle, using Android's user-granted Display over other apps
 * permission. It never takes focus, so Android apps on the tablet keep their
 * keyboard; Control desktop starts [DesktopControlActivity] for the grab.
 */
private class DesktopControlOverlay(
    val host: PortalActivity,
    context: Context,
    private val onClose: (DesktopControlOverlay) -> Unit,
) {
    private val manager = context.getSystemService(Context.WINDOW_SERVICE) as WindowManager
    private val layout = LayoutParams(
        LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT, LayoutParams.TYPE_APPLICATION_OVERLAY,
        LayoutParams.FLAG_NOT_TOUCH_MODAL or LayoutParams.FLAG_NOT_FOCUSABLE,
        PixelFormat.TRANSLUCENT,
    ).apply {
        gravity = controlGravity(host.portalDisplays.tabletEdge)
        title = "Portal tablet controls"
    }
    // The handle outlives any one Portal lifecycle state (the monitor may
    // sleep while the tablet is in use), so it owns a lifecycle of its own.
    private val owner = OverlayOwner()
    val view = DesktopControlView(context, host, holdsGrab = false, close = ::close).apply {
        setViewTreeLifecycleOwner(owner)
        setViewTreeSavedStateRegistryOwner(owner)
    }
    private var stopObserving: (() -> Unit)? = null
    private var added = false
    private var closed = false

    fun show() {
        manager.addView(view, layout)
        added = true
        owner.resume()
        try {
            stopObserving = host.portalDisplays.observe { DesktopControlActivity.displayChanged() }
        } catch (error: RuntimeException) {
            close()
            throw error
        }
    }

    fun placeAtEdge() {
        val gravity = controlGravity(host.portalDisplays.tabletEdge)
        if (layout.gravity == gravity || closed) return
        layout.gravity = gravity
        updateLayout()
    }

    /** Hidden while the controller holds the tablet; it shows the same controls. */
    fun setHidden(hidden: Boolean) {
        if (closed) return
        val flags = if (hidden) layout.flags or LayoutParams.FLAG_NOT_TOUCHABLE
        else layout.flags and LayoutParams.FLAG_NOT_TOUCHABLE.inv()
        view.visibility = if (hidden) android.view.View.INVISIBLE else android.view.View.VISIBLE
        if (flags == layout.flags) return
        layout.flags = flags
        updateLayout()
    }

    private fun updateLayout() {
        try {
            manager.updateViewLayout(view, layout)
        } catch (error: RuntimeException) {
            Log.w("PortalDesktopControl", "Floating tablet controls were removed", error)
            close()
        }
    }

    fun close() {
        if (closed) return
        closed = true
        stopObserving?.invoke()
        try {
            if (added) manager.removeView(view)
        } catch (error: IllegalArgumentException) {
            Log.d("PortalDesktopControl", "Floating tablet controls were already removed", error)
        } finally {
            owner.destroy()
            onClose(this)
        }
    }
}

private class OverlayOwner : LifecycleOwner, SavedStateRegistryOwner {
    private val registry = LifecycleRegistry(this)
    private val savedState = SavedStateRegistryController.create(this).apply { performRestore(null) }

    override val lifecycle: Lifecycle get() = registry
    override val savedStateRegistry: SavedStateRegistry get() = savedState.savedStateRegistry

    fun resume() { registry.currentState = Lifecycle.State.RESUMED }

    fun destroy() {
        if (registry.currentState.isAtLeast(Lifecycle.State.CREATED)) registry.currentState = Lifecycle.State.DESTROYED
    }
}
