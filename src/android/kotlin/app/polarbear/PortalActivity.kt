package app.polarbear

// SPIKE-ONLY (branch compose-setup-spike): native Android launch host.
//
// A minimal android.app.NativeActivity subclass. It preserves the exact
// native-activity/native-library semantics Portal relies on (no
// ComponentActivity, no GameActivity, no rendering moved into Kotlin) and
// owns only window/splash integration: it installs the AndroidX system
// splash before super.onCreate() and holds it until Portal draws its first
// app-owned frame (see ComposeOverlay.isFirstFrameReady).
//
// The splash is released by a real readiness signal, never by a timer.

import android.app.NativeActivity
import android.os.Bundle
import android.util.Log
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen

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
    }

    companion object {
        private const val TAG = "PortalActivity"
    }
}
