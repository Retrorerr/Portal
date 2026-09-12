package app.polarbear

// SPIKE-ONLY (branch spike/game-activity-host): minimal Jetpack Compose
// fullscreen setup UI hosted as a normal sibling View in the GameActivity
// window.
//
// Architecture: the ComposeView lives in a MATCH_PARENT FrameLayout added to
// GameActivity's own root FrameLayout (see PortalActivity.overlayHost),
// directly above the native InputEnabledSurfaceView. Same window, same
// Activity, no PopupWindow, no Dialog, no second Activity, no second window,
// no Window.takeSurface(). The native SurfaceView is never reparented,
// detached or recreated: showing and removing the overlay only adds/removes
// the sibling frame.
//
// Lifecycle/SavedState/ViewModel ownership comes from the real
// AppCompatActivity owners: GameActivity derives from AppCompatActivity, so
// the window decor already carries them and the ComposeView resolves them by
// walking up the tree. There is no manual owner and no ViewTree tagging.
//
// Input needs no custom routing: the overlay frame is the topmost View while
// visible, so normal Android hit-testing delivers touch, mouse, hover,
// scroll and generic motion to Compose; once removed,
// GameActivity/android-activity/Winit receive native input normally.
//
// Removal removes the frame and disposes the composition on the UI thread,
// leaving the native surface visible and operating normally.

import android.app.Activity
import android.content.Context
import android.os.Build
import android.util.Log
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.platform.ComposeView
import app.polarbear.setup.PortalSetupScreen
import java.util.concurrent.atomic.AtomicBoolean

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
            // Already loaded by GameActivity; harmless.
        }
    }

    @JvmStatic external fun nativeOnStartPlasma()
    @JvmStatic external fun nativeOnOverlayRemoved()
    @JvmStatic external fun nativeOnOverlayShown()
    @JvmStatic external fun nativeOnOverlayShowFailed(reason: String)

    private val state = mutableStateOf(STATE_IDLE)
    // First app-owned frame handshake for the system splash: set on the
    // sibling frame's first pre-draw (measured and laid out), or when
    // showing fails so the fallback screen can draw instead. PortalActivity
    // holds the system splash until this flips; there is no timed wait
    // anywhere.
    private val firstFrameReady = AtomicBoolean(false)
    // Begin Install guard: the Start signal is accepted exactly once per
    // overlay session so a double-tap cannot double-start the runtime.
    private val startRequested = AtomicBoolean(false)
    private var container: FrameLayout? = null
    private var composeView: ComposeView? = null

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

    /**
     * SPIKE-ONLY host-validation entry: the CONFIGURE Begin Install button
     * calls this (via PortalSetupScreen) to start Portal/Plasma beneath the
     * overlay, bypassing provisioning. Accepted once; the overlay itself is
     * NOT removed here — removal still happens only on the existing
     * STATE_READY / "Desktop ready" signal.
     */
    private fun onBeginInstallPressed() {
        if (!startRequested.compareAndSet(false, true)) {
            Log.i(TAG, "Begin Install ignored: start already requested")
            return
        }
        Log.i(TAG, "spike host validation: Begin Install bypasses provisioning; starting Portal beneath overlay")
        try {
            nativeOnStartPlasma()
        } catch (_: UnsatisfiedLinkError) {
            Log.e(TAG, "nativeOnStartPlasma unavailable")
        } catch (e: Exception) {
            Log.e(TAG, "nativeOnStartPlasma failed", e)
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
        val host = (activity as? PortalActivity)?.overlayHost()
        if (host == null) {
            Log.e(TAG, "overlay show failed: host Activity is not a PortalActivity")
            ackShowFailed("GameActivity root unavailable")
            return
        }
        try {
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
            // Lifecycle/SavedState/ViewModel owners resolve from the window
            // decor (real AppCompatActivity owners); nothing is tagged here.
            view.setContent { PortalSetupScreen(onBeginInstall = { onBeginInstallPressed() }) }
            // Sibling above the native SurfaceView in the SAME window: no
            // second window is created and the SurfaceView is untouched.
            host.addView(
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
            frame.alpha = 1f
            val blur = queryBlurSupport(activity)
            Log.i(TAG, "overlay shown as GameActivity sibling; crossWindowBlur=$blur")
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
            // later fallback screen is never covered by a stale frame.
            val frame = container
            (frame?.parent as? ViewGroup)?.removeView(frame)
            container = null
            composeView = null
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
            Log.i(TAG, "overlay removed; native surface undisturbed")
            try {
                nativeOnOverlayRemoved()
            } catch (_: UnsatisfiedLinkError) {
            } catch (_: Exception) {
            }
        }
    }
}
