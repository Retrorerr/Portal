package app.polarbear

// SPIKE-ONLY (branch compose-setup-spike): minimal Jetpack Compose fullscreen
// overlay hosted inside Portal's own NativeActivity window.
//
// Architecture: the ComposeView lives in a MATCH_PARENT container added on
// top of the Activity content (same window, same Activity, no PopupWindow,
// no Dialog, no second Activity). The native SurfaceView underneath is never
// touched. Because NativeActivity routes MotionEvents to its native input
// queue instead of in-window views, PortalActivity explicitly forwards
// events to the overlay first (see dispatchTouchEventToOverlay /
// dispatchGenericMotionEventToOverlay); anything the overlay declines falls
// through to super, preserving native input semantics exactly.
// Lifecycle/SavedState/ViewModel ownership is supplied manually because
// NativeActivity does not provide it.
//
// Removal detaches the container and disposes the composition on the UI
// thread, leaving the native Wayland surface visible and operating normally.

import android.app.Activity
import android.content.Context
import android.os.Build
import android.util.Log
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.platform.ComposeView
import app.polarbear.setup.PortalSetupScreen
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.ViewModelStoreOwner
import androidx.savedstate.SavedStateRegistry
import androidx.savedstate.SavedStateRegistryController
import androidx.savedstate.SavedStateRegistryOwner
import java.util.concurrent.atomic.AtomicBoolean

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
    // First app-owned frame handshake for the system splash: set on the
    // overlay frame's first pre-draw (measured and laid out), or when
    // showing fails so the fallback screen can draw instead. PortalActivity
    // holds the system splash until this flips; there is no timed wait
    // anywhere.
    private val firstFrameReady = AtomicBoolean(false)
    private var container: FrameLayout? = null
    private var composeView: ComposeView? = null
    private var owner: SpikeLifecycleOwner? = null

    /**
     * Route one MotionEvent to the overlay. Called by PortalActivity before
     * NativeActivity sees the event. Returns true iff the overlay is present
     * and consumed it; otherwise PortalActivity falls through to super, so
     * native Portal input is bit-for-bit unchanged. All Activity dispatch
     * callbacks run on the UI thread, same as every other method here, and
     * the event object itself is forwarded untouched (never re-synthesized).
     */
    fun dispatchTouchEventToOverlay(event: android.view.MotionEvent): Boolean {
        val frame = container ?: return false
        return try {
            frame.dispatchTouchEvent(event)
        } catch (_: Exception) {
            false
        }
    }

    /**
     * Same routing contract for non-touch motion: hover, mouse/trackpad
     * pointer movement and scroll axes all arrive through the generic-motion
     * dispatch path.
     */
    fun dispatchGenericMotionEventToOverlay(event: android.view.MotionEvent): Boolean {
        val frame = container ?: return false
        return try {
            frame.dispatchGenericMotionEvent(event)
        } catch (_: Exception) {
            false
        }
    }

    /**
     * Show the fullscreen overlay. Fire-and-forget: success or failure is
     * reported back through nativeOnOverlayShown/nativeOnOverlayShowFailed,
     * which is the only signal Rust trusts. Safe to call from any thread.
     */
    @JvmStatic fun show(activity: Activity) {
        activity.runOnUiThread { doShow(activity) }
    }

    /** System-splash gate: true once an app-owned frame (or the fallback path) exists. */
    @JvmStatic fun isFirstFrameReady(): Boolean = firstFrameReady.get()

    private fun markFirstFrameReady() {
        if (firstFrameReady.compareAndSet(false, true)) {
            Log.i(TAG, "first app frame drawn; releasing system splash")
        }
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
        if (container != null) {
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
                // Same charcoal as the splash and the setup UI from pixel one.
                setBackgroundColor(0xFF191B1C.toInt())
                isClickable = true
                isFocusable = true
                isFocusableInTouchMode = true
            }
            // Static launch mark: the first app-owned frame is charcoal plus
            // this centred mark, continuing the system splash seamlessly.
            // Compose attaches afterwards (see below).
            val markSizePx = (144f * activity.resources.displayMetrics.density).toInt()
            val mark = android.widget.ImageView(activity).apply {
                layoutParams = FrameLayout.LayoutParams(markSizePx, markSizePx).apply {
                    gravity = android.view.Gravity.CENTER
                }
                val markId = activity.resources.getIdentifier(
                    "portal_mark", "drawable", activity.packageName,
                )
                if (markId != 0) {
                    setImageResource(markId)
                } else {
                    Log.e(TAG, "launch mark drawable missing")
                }
            }
            frame.addView(mark)
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
            view.setContent { PortalSetupScreen() }
            val content = activity.findViewById<ViewGroup>(android.R.id.content)
            // Tag the Activity window root as well: Compose installs the
            // window recomposer on the root, which only sees its own tags.
            // (The root already exists, unlike a popup decor, so this runs
            // before anything attaches.)
            try {
                val decor = activity.window?.decorView
                if (decor != null) {
                    ComposeOwnerHost.attach(decor, lifecycleOwner, lifecycleOwner, lifecycleOwner)
                }
            } catch (_: Exception) {
            }
            ComposeOwnerHost.attach(content, lifecycleOwner, lifecycleOwner, lifecycleOwner)
            // Attach the overlay container on top of the native surface view.
            // MATCH_PARENT in the already-edge-to-edge window: same geometry
            // as everything else, no second window, no resize.
            content.addView(
                frame,
                FrameLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.MATCH_PARENT,
                ),
            )
            // First-draw handshake: the system splash is released only once
            // this measured, laid-out frame is about to render. No timers.
            frame.viewTreeObserver.addOnPreDrawListener(
                object : android.view.ViewTreeObserver.OnPreDrawListener {
                    override fun onPreDraw(): Boolean {
                        frame.viewTreeObserver.removeOnPreDrawListener(this)
                        markFirstFrameReady()
                        return true
                    }
                },
            )
            frame.addView(view)
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
        // Release the system splash too: the fallback screen owns the next frame.
        markFirstFrameReady()
        try {
            // Best effort: also tear down any half-constructed hierarchy so a
            // later fallback screen is never covered by a stale overlay.
            val frame = container
            (frame?.parent as? ViewGroup)?.removeView(frame)
            try {
                composeView?.disposeComposition()
            } catch (_: Exception) {
            }
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
}
