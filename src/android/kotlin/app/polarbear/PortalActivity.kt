package app.polarbear

// SPIKE-ONLY (branch spike/game-activity-host): native Android launch host.
//
// A com.google.androidgamesdk.GameActivity (4.4.0) subclass. It preserves
// Portal's Android-native shell behaviour while giving the app a normal
// AndroidX/AppCompat Activity window instead of a NativeActivity:
//
//   - GameActivity builds its own root FrameLayout holding the native
//     InputEnabledSurfaceView and reads the launcher Activity's
//     android.app.lib_name metadata (emitted by xbuild as "localdesktop")
//     to load the native library; android-activity 0.6.1 supplies the
//     matching Rust JNI glue, so no Games SDK C++ Prefab code is linked.
//     Rust android_main() is unchanged.
//   - installs the AndroidX system splash before super.onCreate(), holds
//     it until Portal draws its first app-owned frame (see
//     ComposeOverlay.isFirstFrameReady), and owns the exit boundary via
//     setOnExitAnimationListener: the splash view is removed atomically
//     with no platform exit animation, and Compose is told the layer is
//     actually gone before the launch intro leaves t=0. The splash is
//     released by a real readiness signal, never by a timer. (GameActivity.onCreate runs its
//     surface/lib/native setup before delegating to AppCompatActivity, so
//     the install still precedes the effective super init.)
//   - owns fullscreen/immersive geometry via the modern window/insets path
//     (edge-to-edge + WindowInsetsController, transient bars by swipe) so
//     the splash, first frame, Compose sibling and native surface all share
//     one stable fullscreen coordinate space from the beginning. No legacy
//     SYSTEM_UI_FLAG_* calls anywhere in this path.
//   - overrides GameActivity.onSetUpWindow policy: the default pins the
//     window to RGB_565 with SOFT_INPUT_ADJUST_RESIZE, which would cap host
//     quality and resize the KDE desktop coordinate space when Android IME
//     appears. Portal keeps full RGBA_8888 quality (Anland configures the
//     SurfaceView BufferQueue geometry itself later) and an overlay-style
//     non-resizing IME policy.
//   - exposes the GameActivity root FrameLayout as the host for the Compose
//     setup sibling view. Compose is a normal topmost View above the native
//     SurfaceView, so normal Android hit-testing delivers input; no custom
//     event routing, no PopupWindow, no second window.

import android.content.Intent
import android.content.pm.ApplicationInfo
import android.graphics.PixelFormat
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.Process
import android.provider.Settings
import android.util.Log
import android.view.Display
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.WindowManager
import android.widget.FrameLayout
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import com.google.androidgamesdk.GameActivity
import kotlin.math.abs

open class PortalActivity : GameActivity() {
    override fun onSetUpWindow() {
        super.onSetUpWindow()
        // Portal window policy, applied after the GameActivity defaults:
        // full-quality window format (the SurfaceView BufferQueue itself is
        // configured to RGBA_8888 by Anland later) and a non-resizing IME
        // policy so the desktop coordinate space stays stable.
        try {
            window.setFormat(PixelFormat.RGBA_8888)
        } catch (e: Exception) {
            Log.e(TAG, "window format override failed", e)
        }
        try {
            window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_NOTHING)
        } catch (e: Exception) {
            Log.e(TAG, "soft input mode override failed", e)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        try {
            installSplashScreen().apply {
                setKeepOnScreenCondition {
                    !ComposeOverlay.isFirstFrameReady()
                }
                // Own the splash boundary explicitly: no platform exit
                // animation. The moment Android is ready to dismiss the
                // splash, remove its view atomically (suppressing the
                // icon-fade/radial-reveal transition) and tell Compose the
                // layer is actually gone, so the launch intro starts from
                // its t=0 mark with no blink and no progress hidden behind
                // the exiting splash.
                setOnExitAnimationListener { provider ->
                    try {
                        provider.remove()
                    } catch (e: Exception) {
                        Log.e(TAG, "splash exit removal failed", e)
                    }
                    try {
                        val icon = provider.iconView
                        Log.i(TAG, "system splash removed; iconView=${icon.width}x${icon.height} at (${icon.x},${icon.y})")
                    } catch (e: Exception) {
                        Log.i(TAG, "system splash removed; icon bounds unreadable", e)
                    }
                    ComposeOverlay.markSystemSplashRemoved()
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "splash install failed; continuing with theme fallback", e)
            // No splash to wait for: never freeze the Compose intro on it.
            ComposeOverlay.markSystemSplashRemoved()
        }
        super.onCreate(savedInstanceState)
        ComposeOverlay.attachContext(this)
        applyImmersive("onCreate")
        GamepadBridge.attach(this)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            window.decorView.setOnCapturedPointerListener { _, event -> dispatchCaptured(event) }
        }
    }

    // GameActivity forwards only touch input to native code; controller
    // keys and sticks go to the guest's evdev pads before the view tree.
    override fun dispatchKeyEvent(event: KeyEvent): Boolean =
        dispatchMouseSideButton(event) || TouchControls.onBackKey(event) || GamepadBridge.dispatchKey(event) ||
            super.dispatchKeyEvent(event)

    /**
     * Android sends a mouse's side buttons as Back/Forward keys rather than
     * button presses. Send them on as BTN_SIDE/BTN_EXTRA so browsers go
     * back/forward and games see mouse 4/5; as keys, Back became Esc and
     * Forward was dropped.
     */
    private fun dispatchMouseSideButton(event: KeyEvent): Boolean {
        if (!KeyboardAccessibilityService.isMouseSideButton(event)) return false
        if (event.repeatCount == 0 && event.action != KeyEvent.ACTION_MULTIPLE) {
            val code = if (event.keyCode == KeyEvent.KEYCODE_BACK) 0x113 else 0x114
            nativeCapturedButton(code, event.action == KeyEvent.ACTION_DOWN)
        }
        return true
    }

    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean =
        dispatchCaptured(event) || dispatchMouseWheel(event) || GamepadBridge.dispatchMotion(event) ||
            super.dispatchGenericMotionEvent(event)

    // A captured mouse reports as SOURCE_MOUSE_RELATIVE, a trackball-class
    // source, which reaches the activity here when no view consumed it.
    override fun dispatchTrackballEvent(event: MotionEvent): Boolean =
        dispatchCaptured(event) || super.dispatchTrackballEvent(event)

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        // Debug builds only: open the first-run stages on demand.
        //   adb shell am start -n app.polarbear/.PortalActivity --activity-single-top --ez portal_preview_setup true
        //   (add --es portal_preview_install ok|fail to play a scripted install)
        if ((applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE) != 0 &&
            intent.getBooleanExtra(PortalPreview.EXTRA, false)
        ) {
            PortalPreview.show(
                this,
                intent.getFloatExtra(PortalPreview.EXTRA_SLOW, 1f),
                intent.getBooleanExtra(PortalPreview.EXTRA_GATE, false),
                intent.getStringExtra(PortalPreview.EXTRA_INSTALL),
            )
        }
    }

    override fun onStart() {
        super.onStart()
        SetupService.setPortalVisible(this, true)
    }

    override fun onStop() {
        SetupService.setPortalVisible(this, false)
        super.onStop()
    }

    override fun onResume() {
        super.onResume()
        TouchControls.attach(this)
    }

    override fun onPause() {
        TouchControls.detach()
        super.onPause()
    }

    /**
     * GameActivity's native onDestroy blocks this thread until Rust
     * android_main returns, and Portal's never does: the event loop and the
     * Plasma session live for the whole process. If the process outlives the
     * Activity (swiped from recents while the session keeps it alive), the
     * UI thread deadlocks here and the next launch sits on the splash
     * forever. End the process instead, like closeAfterLogout; the next
     * launch starts clean. Config changes are handled in place, so this
     * only runs when the Activity is really going away.
     */
    override fun onDestroy() {
        Log.i(TAG, "Activity destroyed (finishing=$isFinishing); ending the process")
        Process.killProcess(Process.myPid())
        super.onDestroy()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        // Android legitimately clears bar visibility on focus changes;
        // reapply without touching geometry.
        if (hasFocus) {
            applyImmersive("focus")
            // Android drops pointer capture with window focus; take it back
            // if the guest still holds its pointer lock.
            applyPointerCapture()
        }
    }

    /**
     * Native (any thread): a guest client took (or released) a pointer lock,
     * see src/android/utils/pointer_capture.rs. While it is held, Android
     * pointer capture hides the system cursor and turns the mouse into raw
     * relative motion, so a game's view keeps turning at the screen edges.
     */
    fun setPointerCaptureWanted(capture: Boolean) {
        runOnUiThread {
            pointerCaptureWanted = capture
            applyPointerCapture()
        }
    }

    private var pointerCaptureWanted = false
    private var padX = Float.NaN
    private var padY = Float.NaN

    private fun applyPointerCapture() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val view = window.decorView
        if (pointerCaptureWanted) {
            if (hasWindowFocus() && !view.hasPointerCapture()) view.requestPointerCapture()
        } else if (view.hasPointerCapture()) {
            view.releasePointerCapture()
        }
    }

    override fun onPointerCaptureChanged(hasCapture: Boolean) {
        super.onPointerCaptureChanged(hasCapture)
        Log.i(TAG, "pointer capture=$hasCapture (wanted=$pointerCaptureWanted)")
        padX = Float.NaN
        padY = Float.NaN
    }

    /**
     * Mouse wheel, straight to the guest: GameActivity's native glue never
     * sees ACTION_SCROLL here, so the wheel did nothing. Touchpad scrolling
     * (finger tool) keeps its own path, and the setup screen keeps the wheel.
     */
    private fun dispatchMouseWheel(event: MotionEvent): Boolean {
        if (event.actionMasked != MotionEvent.ACTION_SCROLL || ComposeOverlay.isShowing()) return false
        if (!event.isFromSource(InputDevice.SOURCE_MOUSE)) return false
        if (event.getToolType(0) == MotionEvent.TOOL_TYPE_FINGER) return false
        nativeCapturedWheel(
            event.getAxisValue(MotionEvent.AXIS_HSCROLL),
            event.getAxisValue(MotionEvent.AXIS_VSCROLL),
        )
        return true
    }

    /** Captured mouse/touchpad input goes straight to the Anland session. */
    private fun dispatchCaptured(event: MotionEvent): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O || !window.decorView.hasPointerCapture()) {
            return false
        }
        val mouse = event.isFromSource(InputDevice.SOURCE_MOUSE_RELATIVE)
        if (!mouse && !event.isFromSource(InputDevice.SOURCE_TOUCHPAD)) return false
        when (event.actionMasked) {
            MotionEvent.ACTION_BUTTON_PRESS, MotionEvent.ACTION_BUTTON_RELEASE -> {
                evdevButton(event.actionButton)?.let {
                    nativeCapturedButton(it, event.actionMasked == MotionEvent.ACTION_BUTTON_PRESS)
                }
            }
            MotionEvent.ACTION_SCROLL -> nativeCapturedWheel(
                event.getAxisValue(MotionEvent.AXIS_HSCROLL),
                event.getAxisValue(MotionEvent.AXIS_VSCROLL),
            )
            MotionEvent.ACTION_MOVE, MotionEvent.ACTION_HOVER_MOVE ->
                if (mouse) {
                    // x/y are this sample's motion; batched samples come first.
                    var dx = event.x
                    var dy = event.y
                    for (i in 0 until event.historySize) {
                        dx += event.getHistoricalX(i)
                        dy += event.getHistoricalY(i)
                    }
                    if (dx != 0f || dy != 0f) nativeCapturedMotion(dx, dy)
                } else if (event.pointerCount == 1) {
                    // A captured touchpad reports absolute pad positions.
                    if (!padX.isNaN()) nativeCapturedMotion(event.x - padX, event.y - padY)
                    padX = event.x
                    padY = event.y
                } else {
                    padX = Float.NaN
                    padY = Float.NaN
                }
            MotionEvent.ACTION_DOWN -> {
                padX = event.x
                padY = event.y
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                padX = Float.NaN
                padY = Float.NaN
            }
        }
        return true
    }

    /**
     * The GameActivity root FrameLayout (created in super.onCreate): first
     * child is the native InputEnabledSurfaceView, and the Compose setup UI
     * attaches as a normal sibling above it. Never used to reparent, detach
     * or recreate the SurfaceView.
     */
    fun overlayHost(): FrameLayout = findViewById(contentViewId)

    /**
     * Native (any thread): the user logged out of Plasma (or chose Restart /
     * Shut down). Leave the task, then end the process so its guest
     * processes go with it and the next launch starts a fresh session.
     */
    fun closeAfterLogout() {
        runOnUiThread {
            Log.i(TAG, "Plasma session ended by the user; closing Portal")
            finishAndRemoveTask()
            Handler(Looper.getMainLooper()).postDelayed({ Process.killProcess(Process.myPid()) }, 500)
        }
    }

    /**
     * Native (any thread): ask for the fastest display mode the user allows
     * while the desktop is actively presenting, and drop the request when it
     * goes idle.
     *
     * OxygenOS votes unrecognised GameActivity windows down to 60 Hz at the
     * window/app-request level, which the SurfaceView's
     * ANativeWindow_setFrameRate hint cannot outrank. A window
     * preferredDisplayModeId is the supported app-level request that the
     * platform (and OxygenOS's "app request first" policy) honours; the
     * user's peak-refresh setting still caps it. Releasing it when idle
     * returns the panel to the system's normal low-rate policy.
     */
    fun setHighRefreshPreferred(enable: Boolean) {
        runOnUiThread {
            desktopHighRefresh = enable
            applyHighRefresh()
        }
    }

    /**
     * UI thread: Portal's own animated screens (setup, install progress, the
     * launch and return transitions) also want the panel's full rate; without
     * this request they ran at OxygenOS's 60 Hz app vote.
     */
    fun setOverlayHighRefresh(enable: Boolean) {
        overlayHighRefresh = enable
        applyHighRefresh()
    }

    private var desktopHighRefresh = false
    private var overlayHighRefresh = false

    private fun applyHighRefresh() {
        val enable = desktopHighRefresh || overlayHighRefresh
        try {
            val modeId = if (enable) highRefreshModeId() else 0
            val attributes = window.attributes
            if (attributes.preferredDisplayModeId != modeId) {
                window.attributes = attributes.apply { preferredDisplayModeId = modeId }
                Log.i(TAG, "display mode request: enable=$enable modeId=$modeId")
            }
        } catch (e: Exception) {
            Log.e(TAG, "display mode request failed (enable=$enable)", e)
        }
    }

    /**
     * Native (any thread): refresh rate in millihertz of the mode
     * [setHighRefreshPreferred] requests, re-resolved against the current
     * peak-refresh setting, so the output rate advertised to KWin follows a
     * setting changed while Portal runs. 0 when unavailable.
     */
    fun highRefreshTargetMillihz(): Int = try {
        highRefreshMode()?.let { Math.round(it.refreshRate * 1000f) } ?: 0
    } catch (e: Exception) {
        0
    }

    private fun highRefreshModeId(): Int = highRefreshMode()?.modeId ?: 0

    /**
     * Same physical size as the active mode, highest refresh within the
     * user's peak-refresh setting (absent/unreadable = no cap). On API 31+
     * only rates the panel can switch to seamlessly qualify: the request
     * follows every interaction burst, and a preferredDisplayModeId outranks
     * the surface's ONLY_IF_SEAMLESS hint, so a non-seamless mode would blank
     * the panel each time it toggles.
     */
    private fun highRefreshMode(): Display.Mode? {
        @Suppress("DEPRECATION")
        val display = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            display
        } else {
            windowManager.defaultDisplay
        } ?: return null
        val active = display.mode
        val peak = try {
            Settings.System.getFloat(contentResolver, "peak_refresh_rate", Float.POSITIVE_INFINITY)
        } catch (e: Exception) {
            Float.POSITIVE_INFINITY
        }
        val seamless = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            active.alternativeRefreshRates
        } else {
            null
        }
        return display.supportedModes
            .filter {
                it.physicalWidth == active.physicalWidth &&
                    it.physicalHeight == active.physicalHeight &&
                    it.refreshRate <= peak + 0.5f &&
                    (
                        seamless == null ||
                            it.modeId == active.modeId ||
                            seamless.any { rate -> abs(rate - it.refreshRate) < 0.5f }
                        )
            }
            .maxByOrNull { it.refreshRate }
    }

    private fun applyImmersive(reason: String) {
        try {
            // Content always lays out in the full display space; transient
            // system bars overlay it instead of resizing it.
            WindowCompat.setDecorFitsSystemWindows(window, false)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                window.attributes = window.attributes.apply {
                    layoutInDisplayCutoutMode =
                        WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
                }
            }
            WindowInsetsControllerCompat(window, window.decorView).apply {
                hide(
                    WindowInsetsCompat.Type.statusBars() or
                        WindowInsetsCompat.Type.navigationBars(),
                )
                systemBarsBehavior =
                    WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            }
        } catch (e: Exception) {
            Log.e(TAG, "immersive apply failed ($reason)", e)
        }
    }

    companion object {
        private const val TAG = "PortalActivity"

        @JvmStatic private external fun nativeCapturedMotion(dx: Float, dy: Float)
        // Also carries mouse side buttons, captured or not.
        @JvmStatic private external fun nativeCapturedButton(code: Int, pressed: Boolean)
        // Android axis values: vertical > 0 is wheel up, horizontal > 0 is right.
        @JvmStatic private external fun nativeCapturedWheel(horizontal: Float, vertical: Float)

        /** Android mouse button to Linux evdev code (BTN_LEFT...BTN_EXTRA). */
        private fun evdevButton(button: Int): Int? = when (button) {
            MotionEvent.BUTTON_PRIMARY -> 0x110
            MotionEvent.BUTTON_SECONDARY -> 0x111
            MotionEvent.BUTTON_TERTIARY -> 0x112
            MotionEvent.BUTTON_BACK -> 0x113
            MotionEvent.BUTTON_FORWARD -> 0x114
            else -> null
        }
    }
}
