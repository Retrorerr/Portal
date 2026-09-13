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
// At Compose Ready the host becomes transparent while the ambient veil owns
// its controlled final compositing alpha. The final gesture translates that
// complete visual layer, exposing the live SurfaceView directly. Removal
// happens only after the veil is completely offscreen.

import android.app.Activity
import android.util.Log
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.platform.ComposeView
import app.polarbear.setup.PortalLaunchTransition
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

    @JvmStatic external fun nativeOnRevealCommitted()
    @JvmStatic external fun nativeOnOverlayRemoved()
    @JvmStatic external fun nativeOnOverlayShown()
    @JvmStatic external fun nativeOnOverlayShowFailed(reason: String)
    @JvmStatic external fun nativeSetReadyVeilBlur(enabled: Boolean, radiusPx: Float)
    @JvmStatic external fun nativeSetReadyVeilRevealProgress(progress: Float)

    // Native readiness is independent of the fake setup phase. A KWin frame
    // can latch this before the Compose hierarchy has finished presenting.
    private val desktopReadyLatched = AtomicBoolean(false)
    private val desktopReadyState = mutableStateOf(false)
    // First app-owned frame handshake for the system splash: set on the
    // Compose content pre-draw (CONFIGURE and header measured), or when
    // showing fails so the fallback screen can draw instead. PortalActivity
    // holds the system splash until this flips; there is no timed wait
    // anywhere.
    private val firstFrameReady = AtomicBoolean(false)
    // System-splash layer signal: set once by PortalActivity's splash exit
    // listener when the platform splash view is actually removed (or
    // immediately if splash install failed and there is nothing to wait
    // for). The launch intro stays frozen at t=0 until this AND the
    // CONFIGURE/header pre-draw gate both hold. Observed by composition via
    // systemSplashRemovedState; the exit callback runs on the main thread.
    private val systemSplashRemoved = AtomicBoolean(false)
    private val systemSplashRemovedState = mutableStateOf(false)
    private val revealCommitted = AtomicBoolean(false)
    private val removalRequested = AtomicBoolean(false)
    private val readyPreludeActive = AtomicBoolean(false)
    private var container: FrameLayout? = null
    private var composeView: ComposeView? = null
    private var displayDensity = 1f
    private var launchIntroResolved = false

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

    /** Called by PortalActivity when the system splash view is actually removed. */
    fun markSystemSplashRemoved() {
        if (systemSplashRemoved.compareAndSet(false, true)) {
            systemSplashRemovedState.value = true
            Log.i(TAG, "system splash removal confirmed; launch intro may leave t=0")
        }
    }

    private fun markFirstFrameReady() {
        if (firstFrameReady.compareAndSet(false, true)) {
            Log.i(TAG, "first app frame drawn; releasing system splash")
        }
    }

    /** Publish native desktop readiness without changing overlay visibility. */
    @JvmStatic fun updateDesktopReady(ready: Boolean) {
        val changed = desktopReadyLatched.getAndSet(ready) != ready
        if (ready && changed) {
            Log.i(TAG, "native desktop READY latched; Compose overlay remains attached")
        }
        composeView?.post {
            desktopReadyState.value = desktopReadyLatched.get()
            // Recover the intended Ready translucency after an underlying
            // runtime error temporarily restored the safety charcoal.
            if (ready && readyPreludeActive.get()) {
                applyReadyBackdrop(container, active = true)
            }
        }
    }

    /** Retained recovery-state bridge; setup progress itself stays Compose-local. */
    @JvmStatic fun updateState(value: String) {
        when (value) {
            STATE_READY -> updateDesktopReady(true)
            STATE_ERROR -> {
                updateDesktopReady(false)
                revealCommitted.set(false)
                composeView?.post {
                    applyReadyBackdrop(container, active = false)
                }
            }
        }
    }

    private fun doShow(activity: Activity) {
        if (container != null) {
            // Recovery racing a committed reveal: restore a solid host
            // immediately while Compose returns its veil to rest.
            setNativeReadyBlur(false)
            container?.animate()?.cancel()
            container?.alpha = 1f
            container?.setBackgroundColor(PORTAL_CHARCOAL)
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
            revealCommitted.set(false)
            removalRequested.set(false)
            readyPreludeActive.set(false)
            displayDensity = activity.resources.displayMetrics.density
            val frame = FrameLayout(activity).apply {
                layoutParams = FrameLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.MATCH_PARENT,
                )
                // Same charcoal as the splash and the setup UI from pixel one.
                setBackgroundColor(PORTAL_CHARCOAL)
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
            // Lifecycle/SavedState/ViewModel owners resolve from the window
            // decor (real AppCompatActivity owners); nothing is tagged here.
            view.setContent {
                PortalLaunchTransition(
                    playIntro = !launchIntroResolved,
                    splashRemoved = systemSplashRemovedState.value,
                    desktopReady = desktopReadyState.value,
                    onContentPreDraw = { markFirstFrameReady() },
                    onIntroResolved = { launchIntroResolved = true },
                    onReadyPreludeChanged = { active ->
                        val changed = readyPreludeActive.getAndSet(active) != active
                        // Setup Ready owns this transition; native Ready only
                        // controls gesture availability. The ambient layer is
                        // still an opaque blur source and composites at 0.83.
                        // The host becomes transparent for the approved 0.83
                        // veil composite. The live same-window SurfaceView is
                        // blurred in Anland's fenced GPU presentation path.
                        applyReadyBackdrop(frame, active)
                        if (active && changed) {
                            Log.i(TAG, "setup READY prelude; overlay host transparent for live SurfaceView")
                        }
                    },
                    onRevealProgressChanged = { progress ->
                        try {
                            nativeSetReadyVeilRevealProgress(progress)
                        } catch (_: UnsatisfiedLinkError) {
                        }
                    },
                    onRevealCommitted = { acknowledgeRevealCommitted() },
                    onRevealFinished = { removeNow() },
                )
            }
            // Sibling above the native SurfaceView in the SAME window: no
            // second window is created and the SurfaceView is untouched.
            host.addView(
                frame,
                FrameLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.MATCH_PARENT,
                ),
            )
            frame.addView(view)
            container = frame
            composeView = view
            // Catch readiness that arrived before or while the Compose
            // hierarchy was being attached, always from this UI thread.
            desktopReadyState.value = desktopReadyLatched.get()
            frame.alpha = 1f
            Log.i(
                TAG,
                "overlay shown as GameActivity sibling; host solid through CONFIGURE/INSTALLING",
            )
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
            readyPreludeActive.set(false)
            setNativeReadyBlur(false)
            nativeOnOverlayShowFailed(reason)
        } catch (_: UnsatisfiedLinkError) {
        } catch (_: Exception) {
        }
    }

    private fun acknowledgeRevealCommitted() {
        if (!revealCommitted.compareAndSet(false, true)) return
        Log.i(TAG, "final upward reveal committed; native desktop was already ready")
        try {
            nativeOnRevealCommitted()
        } catch (_: UnsatisfiedLinkError) {
            Log.e(TAG, "nativeOnRevealCommitted unavailable")
        } catch (e: Exception) {
            Log.e(TAG, "nativeOnRevealCommitted failed", e)
        }
    }

    private fun removeNow() {
        if (!removalRequested.compareAndSet(false, true)) return
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
            setNativeReadyBlur(false)
            container = null
            composeView = null
            readyPreludeActive.set(false)
            Log.i(TAG, "overlay removed; native surface undisturbed")
            try {
                nativeOnOverlayRemoved()
            } catch (_: UnsatisfiedLinkError) {
            } catch (_: Exception) {
            }
        }
    }

    private fun applyReadyBackdrop(frame: FrameLayout?, active: Boolean) {
        frame?.setBackgroundColor(if (active) 0x00000000 else PORTAL_CHARCOAL)
        setNativeReadyBlur(active)
    }

    private fun setNativeReadyBlur(active: Boolean) {
        try {
            nativeSetReadyVeilBlur(active, READY_BACKDROP_BLUR_DP * displayDensity)
        } catch (_: UnsatisfiedLinkError) {
        } catch (e: Exception) {
            Log.w(TAG, "native READY veil blur state failed", e)
        }
    }

    private val PORTAL_CHARCOAL = 0xFF191B1C.toInt()
    private const val READY_BACKDROP_BLUR_DP = 24f
}
