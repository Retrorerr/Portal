package app.polarbear

// SPIKE-ONLY (branch compose-setup-spike): minimal Jetpack Compose fullscreen
// overlay hosted in a PopupWindow owned by Portal's existing winit
// android-native-activity.
//
// Architecture: the ComposeView lives in a fullscreen PopupWindow attached to
// the Activity decor (same window token family, same Activity). No
// ComponentActivity, GameActivity, or second Activity is involved, and the
// native SurfaceView underneath is never touched. (v1 attached the view to
// the Activity content root, but motion events never enter in-window views
// of a NativeActivity, so buttons were untappable; a separate window gets
// its own input channel — the proven WebView setup/recovery pattern.)
// Lifecycle/SavedState/ViewModel ownership is supplied manually because
// NativeActivity does not provide it.
//
// Removal dismisses the popup and disposes the composition on the UI thread,
// leaving the native Wayland surface visible and operating normally.

import android.app.Activity
import android.content.Context
import android.os.Build
import android.util.Log
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.ViewModelStoreOwner
import androidx.savedstate.SavedStateRegistry
import androidx.savedstate.SavedStateRegistryController
import androidx.savedstate.SavedStateRegistryOwner

/**
 * Manually supplied owners for the Compose hierarchy. The NativeActivity
 * never leaves RESUMED behind: [resume] follows the Activity's resumed
 * callback and [pause] follows suspend, so recomposition pauses while the
 * app is backgrounded. [destroy] runs once on overlay removal and clears
 * the ViewModelStore.
 */

private class SpikeLifecycleOwner : LifecycleOwner, ViewModelStoreOwner, SavedStateRegistryOwner {
    private val registry = LifecycleRegistry(this)
    private val savedState = SavedStateRegistryController.create(this)
    private val store = ViewModelStore()

    override val lifecycle: Lifecycle get() = registry
    override val savedStateRegistry: SavedStateRegistry get() = savedState.savedStateRegistry
    override val viewModelStore: ViewModelStore get() = store

    fun create() {
        savedState.performAttach()
        savedState.performRestore(null)
        registry.handleLifecycleEvent(Lifecycle.Event.ON_CREATE)
        registry.handleLifecycleEvent(Lifecycle.Event.ON_START)
        registry.handleLifecycleEvent(Lifecycle.Event.ON_RESUME)
    }

    fun resume() {
        try {
            if (registry.currentState == Lifecycle.State.STARTED) {
                registry.handleLifecycleEvent(Lifecycle.Event.ON_RESUME)
            }
        } catch (_: Exception) {
        }
    }

    fun pause() {
        try {
            if (registry.currentState.isAtLeast(Lifecycle.State.RESUMED)) {
                registry.handleLifecycleEvent(Lifecycle.Event.ON_PAUSE)
            }
        } catch (_: Exception) {
        }
    }

    fun destroy() {
        try {
            registry.handleLifecycleEvent(Lifecycle.Event.ON_PAUSE)
            registry.handleLifecycleEvent(Lifecycle.Event.ON_STOP)
            registry.handleLifecycleEvent(Lifecycle.Event.ON_DESTROY)
        } catch (_: Exception) {
        }
        try {
            store.clear()
        } catch (_: Exception) {
        }
    }
}

object ComposeOverlay {
    private const val TAG = "PortalComposeSpike"
    const val STATE_IDLE = "Idle"
    const val STATE_STARTING = "Starting"
    const val STATE_READY = "Desktop ready"
    const val STATE_ERROR = "Error"

    init {
        try {
            System.loadLibrary("localdesktop")
        } catch (_: UnsatisfiedLinkError) {
            // Already loaded by NativeActivity; harmless.
        }
    }

    @JvmStatic external fun nativeOnStartPlasma()
    @JvmStatic external fun nativeOnOverlayRemoved()
    @JvmStatic external fun nativeOnOverlayShown()
    @JvmStatic external fun nativeOnOverlayShowFailed(reason: String)

    private val state = mutableStateOf(STATE_IDLE)
    private var container: FrameLayout? = null
    private var composeView: ComposeView? = null
    private var owner: SpikeLifecycleOwner? = null
    // PopupWindow owned by the same NativeActivity: v1 attached the
    // ComposeView to the Activity content root, but motion events never enter
    // in-window views of a NativeActivity (they go to the native input queue
    // only), so no button was tappable. A separate window gets its own input
    // channel — the same proven pattern as Portal's WebView setup/recovery
    // popup — while the Activity is never recreated.
    private var popup: android.widget.PopupWindow? = null

    /**
     * Show the fullscreen overlay. Fire-and-forget: success or failure is
     * reported back through nativeOnOverlayShown/nativeOnOverlayShowFailed,
     * which is the only signal Rust trusts. Safe to call from any thread.
     */
    @JvmStatic fun show(activity: Activity) {
        activity.runOnUiThread { doShow(activity) }
    }

    /** Update the small state text. "Desktop ready" fades the overlay out. */
    @JvmStatic fun updateState(value: String) {
        val view = composeView
        if (view != null) {
            view.post {
                state.value = value
                if (value == STATE_READY) {
                    fadeAndRemove()
                }
            }
        } else {
            state.value = value
        }
    }

    /** Follow the NativeActivity into the foreground. Safe from any thread. */
    @JvmStatic fun onHostResumed(activity: Activity) {
        activity.runOnUiThread {
            owner?.resume()
            Log.i(TAG, "host resumed")
        }
    }

    /** Follow the NativeActivity into the background. Safe from any thread. */
    @JvmStatic fun onHostSuspended(activity: Activity) {
        activity.runOnUiThread {
            owner?.pause()
            Log.i(TAG, "host suspended")
        }
    }

    /**
     * Android 12+ cross-window blur probe for the future final transition.
     * Investigative only: the spike never depends on the result.
     */
    @JvmStatic fun queryBlur(activity: Activity): Boolean {
        return queryBlurSupport(activity) == true
    }

    private fun queryBlurSupport(activity: Activity): Boolean? {
        if (Build.VERSION.SDK_INT < 31) {
            Log.i(TAG, "crossWindowBlur: unsupported api=${Build.VERSION.SDK_INT}")
            return null
        }
        return try {
            val wm = activity.getSystemService(Context.WINDOW_SERVICE) as android.view.WindowManager
            val method = wm.javaClass.getMethod("isCrossWindowBlurEnabled")
            val enabled = method.invoke(wm) as? Boolean
            Log.i(TAG, "crossWindowBlur: isCrossWindowBlurEnabled=$enabled")
            enabled
        } catch (e: Exception) {
            Log.i(TAG, "crossWindowBlur: probe failed: $e")
            null
        }
    }

    private fun doShow(activity: Activity) {
        if (popup != null) {
            // Re-show during a fade (e.g. recovery racing dismissal):
            // cancel teardown, restore opacity, re-acknowledge.
            container?.animate()?.cancel()
            container?.alpha = 1f
            ackShown()
            return
        }
        try {
            val lifecycleOwner = SpikeLifecycleOwner().also { it.create() }
            val frame = FrameLayout(activity).apply {
                layoutParams = FrameLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.MATCH_PARENT,
                )
                setBackgroundColor(0xFF10131A.toInt())
                isClickable = true
                isFocusable = true
                isFocusableInTouchMode = true
            }
            val view = ComposeView(activity).apply {
                layoutParams = FrameLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.MATCH_PARENT,
                )
            }
            // The ViewTree* owner setters are metadata-less Java facades in the
            // KMP-published lifecycle/savedstate artifacts: visible to javac but
            // not to kotlinc. Route them through the tiny Java bridge below.
            // Compose resolves owners from the composition PARENT, so tag both
            // the ComposeView and its container.
            ComposeOwnerHost.attach(frame, lifecycleOwner, lifecycleOwner, lifecycleOwner)
            ComposeOwnerHost.attach(view, lifecycleOwner, lifecycleOwner, lifecycleOwner)
            view.setContent { SpikeScreen() }
            val content = activity.findViewById<ViewGroup>(android.R.id.content)
            val window = android.widget.PopupWindow(
                frame,
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            ).apply {
                isFocusable = true
                isOutsideTouchable = false
            }
            // Show first with an empty frame: the PopupDecorView does not exist
            // before this call, and Compose installs the window recomposer on
            // the window root (which only sees its own tags), so the decor
            // must be tagged before any ComposeView attaches.
            window.showAtLocation(content, android.view.Gravity.CENTER, 0, 0)
            (frame.parent as? android.view.View)?.let {
                ComposeOwnerHost.attach(it, lifecycleOwner, lifecycleOwner, lifecycleOwner)
            }
            frame.addView(view)
            popup = window
            container = frame
            composeView = view
            owner = lifecycleOwner
            frame.alpha = 1f
            val blur = queryBlurSupport(activity)
            Log.i(TAG, "overlay shown; crossWindowBlur=$blur")
            ackShown()
        } catch (e: Exception) {
            Log.e(TAG, "overlay show failed", e)
            ackShowFailed(e.toString())
        }
    }

    private fun ackShown() {
        try {
            nativeOnOverlayShown()
        } catch (_: UnsatisfiedLinkError) {
        } catch (_: Exception) {
        }
    }

    private fun ackShowFailed(reason: String) {
        try {
            // Best effort: also tear down any half-constructed hierarchy so a
            // later fallback screen is never covered by a stale popup.
            try {
                popup?.dismiss()
            } catch (_: Exception) {
            }
            popup = null
            container = null
            composeView = null
            try {
                owner?.destroy()
            } catch (_: Exception) {
            }
            owner = null
            nativeOnOverlayShowFailed(reason)
        } catch (_: UnsatisfiedLinkError) {
        } catch (_: Exception) {
        }
    }

    private fun fadeAndRemove() {
        val frame = container ?: return
        try {
            frame.animate().alpha(0f).setDuration(600).withEndAction { removeNow() }.start()
        } catch (e: Exception) {
            Log.e(TAG, "overlay fade failed; removing immediately", e)
            removeNow()
        }
    }

    private fun removeNow() {
        try {
            try {
                popup?.dismiss()
            } catch (_: Exception) {
            }
            popup = null
            val frame = container
            (frame?.parent as? ViewGroup)?.removeView(frame)
            try {
                composeView?.disposeComposition()
            } catch (_: Exception) {
            }
        } catch (e: Exception) {
            Log.e(TAG, "overlay remove failed", e)
        } finally {
            container = null
            composeView = null
            try {
                owner?.destroy()
            } catch (_: Exception) {
            }
            owner = null
            Log.i(TAG, "overlay removed; native surface undisturbed")
            try {
                nativeOnOverlayRemoved()
            } catch (_: UnsatisfiedLinkError) {
            } catch (_: Exception) {
            }
        }
    }

    @Composable
    private fun SpikeScreen() {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .background(Color(0xFF10131A)),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
        ) {
            Text(
                text = "Portal Compose integration test",
                color = Color.White,
                fontSize = 20.sp,
            )
            Spacer(modifier = Modifier.height(24.dp))
            Button(onClick = {
                try {
                    nativeOnStartPlasma()
                } catch (e: Exception) {
                    Log.e(TAG, "start callback failed", e)
                }
            }) {
                Text(text = "Start Plasma")
            }
            Spacer(modifier = Modifier.height(16.dp))
            Text(
                text = state.value,
                color = Color(0xFF9AA0AA),
                fontSize = 14.sp,
            )
        }
    }
}
