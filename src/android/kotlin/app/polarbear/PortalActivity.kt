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
    }

    // GameActivity forwards only touch input to native code; controller
    // keys and sticks go to the guest's evdev pads before the view tree.
    override fun dispatchKeyEvent(event: KeyEvent): Boolean =
        GamepadBridge.dispatchKey(event) || super.dispatchKeyEvent(event)

    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean =
        GamepadBridge.dispatchMotion(event) || super.dispatchGenericMotionEvent(event)

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
        }
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
    }
}
