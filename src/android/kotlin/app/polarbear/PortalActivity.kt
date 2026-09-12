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

import android.graphics.PixelFormat
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.WindowManager
import android.widget.FrameLayout
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import com.google.androidgamesdk.GameActivity

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
        applyImmersive("onCreate")
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
