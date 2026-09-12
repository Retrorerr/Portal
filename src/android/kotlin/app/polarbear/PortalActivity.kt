package app.polarbear

// SPIKE-ONLY (branch compose-setup-spike): native Android launch host.
//
// A minimal android.app.NativeActivity subclass. It preserves the exact
// native-activity/native-library semantics Portal relies on (no
// ComponentActivity, no GameActivity, no rendering moved into Kotlin) and
// owns window integration:
//   - installs the AndroidX system splash before super.onCreate() and holds
//     it until Portal draws its first app-owned frame (see
//     ComposeOverlay.isFirstFrameReady). The splash is released by a real
//     readiness signal, never by a timer.
//   - owns fullscreen/immersive geometry via the modern window/insets path
//     (edge-to-edge + WindowInsetsController, transient bars by swipe) so
//     the splash, first frame, overlay and Plasma surface all share one
//     stable fullscreen coordinate space from the beginning. No legacy
//     SYSTEM_UI_FLAG_* calls anywhere in this path.
//   - routes MotionEvents to the Compose overlay while one is visible (see
//     dispatchTouchEvent/dispatchGenericMotionEvent); everything else falls
//     through to ordinary NativeActivity behaviour.

import android.app.NativeActivity
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.MotionEvent
import android.view.WindowManager
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

open class PortalActivity : NativeActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        try {
            installSplashScreen().setKeepOnScreenCondition {
                !ComposeOverlay.isFirstFrameReady()
            }
        } catch (e: Exception) {
            Log.e(TAG, "splash install failed; continuing with theme fallback", e)
        }
        super.onCreate(savedInstanceState)
        applyImmersive("onCreate")
    }

    /**
     * Explicit input routing for the setup overlay. NativeActivity delivers
     * MotionEvents to its native input queue, bypassing in-window views, so
     * while the Compose overlay is present it gets first refusal here.
     * Anything it declines falls through to super, preserving native Portal
     * input semantics bit-for-bit. With no overlay this is a straight
     * pass-through to ordinary NativeActivity behaviour.
     */
    override fun dispatchTouchEvent(event: MotionEvent): Boolean {
        if (ComposeOverlay.dispatchTouchEventToOverlay(event)) {
            return true
        }
        return super.dispatchTouchEvent(event)
    }

    /**
     * Same contract for hover, mouse/trackpad pointer movement and scroll
     * axes, which Android delivers through the generic-motion path rather
     * than the touch path.
     */
    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean {
        if (ComposeOverlay.dispatchGenericMotionEventToOverlay(event)) {
            return true
        }
        return super.dispatchGenericMotionEvent(event)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        // Android legitimately clears bar visibility on focus changes;
        // reapply without touching geometry.
        if (hasFocus) {
            applyImmersive("focus")
        }
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
