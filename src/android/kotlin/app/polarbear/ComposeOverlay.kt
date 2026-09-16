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
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
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
    /** Debug-automation broadcast action. Debug builds only; release ignores it. */
    const val ACTION_DEBUG_DISMISS_VEIL = "app.polarbear.DEBUG_DISMISS_VEIL"
    /** Debug-automation pointer action (extras: op, x, y, button, pressed). */
    const val ACTION_DEBUG_POINTER = "app.polarbear.DEBUG_POINTER"

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
    @JvmStatic external fun nativeBeginInstall(): Boolean
    /** Explicit repair action for an already-installed QPainter/legacy runtime. */
    @JvmStatic external fun nativeRepairEnableAnland(): Boolean
    /** Return to the existing committed-runtime recovery page after a repair failure. */
    @JvmStatic external fun nativeRequestAnlandRepairRecovery(): Boolean

    // Native readiness is independent of installation progress. A KWin frame
    // can latch this before the Compose hierarchy has finished presenting.
    private val desktopReadyLatched = AtomicBoolean(false)
    private val desktopReadyState = mutableStateOf(false)
    data class InstallUiState(
        val phase: String,
        val progress: Int,
        val message: String,
        val error: String?,
        val complete: Boolean,
    ) {
        val failed: Boolean get() = error != null || phase == "Failed"
        val running: Boolean get() = !complete && !failed && phase != "Idle"
    }

    const val ANLAND_REPAIR_UNAVAILABLE = 0
    const val ANLAND_REPAIR_AVAILABLE = 1
    const val ANLAND_REPAIR_RUNNING = 2
    const val ANLAND_REPAIR_FAILED = 3
    const val ANLAND_REPAIR_COMPLETE = 4

    data class AnlandRepairUiState(
        val status: Int,
        val progress: Int,
        val message: String,
        val error: String?,
    ) {
        val available: Boolean get() = status == ANLAND_REPAIR_AVAILABLE
        val running: Boolean get() = status == ANLAND_REPAIR_RUNNING
        val failed: Boolean get() = status == ANLAND_REPAIR_FAILED
        val complete: Boolean get() = status == ANLAND_REPAIR_COMPLETE
    }

    private val defaultInstallState = InstallUiState(
        phase = "Idle",
        progress = 0,
        message = "Portal setup is ready to begin.",
        error = null,
        complete = false,
    )
    // The native worker may publish while the overlay is absent (or while an
    // Activity is being recreated). Keep the latest snapshot outside the
    // Compose hierarchy, then apply it on the main thread when available.
    @Volatile private var installStateSnapshot = defaultInstallState
    private val installStateValue = mutableStateOf(defaultInstallState)
    private val defaultAnlandRepairState = AnlandRepairUiState(
        status = ANLAND_REPAIR_UNAVAILABLE,
        progress = 0,
        message = "",
        error = null,
    )
    @Volatile private var anlandRepairStateSnapshot = defaultAnlandRepairState
    private val anlandRepairStateValue = mutableStateOf(defaultAnlandRepairState)
    // First app-owned frame handshake for the system splash: set on the
    // Compose content pre-draw (CONFIGURE and launch destination measured), or when
    // showing fails so the fallback screen can draw instead. PortalActivity
    // holds the system splash until this flips; there is no timed wait
    // anywhere.
    private val firstFrameReady = AtomicBoolean(false)
    // System-splash layer signal: set once by PortalActivity's splash exit
    // listener when the platform splash view is actually removed (or
    // immediately if splash install failed and there is nothing to wait
    // for). The launch intro stays frozen at t=0 until this AND the
    // CONFIGURE/destination pre-draw gate both hold. Observed by composition via
    // systemSplashRemovedState; the exit callback runs on the main thread.
    private val systemSplashRemoved = AtomicBoolean(false)
    private val systemSplashRemovedState = mutableStateOf(false)
    private val revealCommitted = AtomicBoolean(false)
    private val removalRequested = AtomicBoolean(false)
    private val recoveryDismissRequested = AtomicBoolean(false)
    private val readyPreludeActive = AtomicBoolean(false)
    private var container: FrameLayout? = null
    private var composeView: ComposeView? = null
    private var launchIntroResolved = false
    // Return-to-Plasma mode for already-installed launches: same overlay
    // host, same transition, same veil — only the content screen differs.
    // Decided fresh by each show() / showReturn() call.
    private var returnMode = false

    /**
     * Show the fullscreen overlay. Fire-and-forget: success or failure is
     * reported back through nativeOnOverlayShown/nativeOnOverlayShowFailed,
     * which is the only signal Rust trusts. Safe to call from any thread.
     */
    @JvmStatic fun show(activity: Activity) {
        activity.runOnUiThread { doShow(activity, returnMode = false) }
    }

    /**
     * Show the overlay in Return-to-Plasma mode (already installed): same
     * host, same launch intro, READY prelude and veil — the minimal return
     * screen replaces the installer as the final destination.
     */
    @JvmStatic fun showReturn(activity: Activity) {
        activity.runOnUiThread { doShow(activity, returnMode = true) }
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
                applyReadyHost(container, active = true)
            }
        }
    }

    /**
     * Native provisioning bridge. This is a snapshot, not a timer tick: the
     * Rust coordinator only advances it when real work advances, and it sends
     * `complete=true` after the durable installation marker validates.
     */
    @JvmStatic fun updateInstallState(
        phase: String,
        progress: Int,
        message: String,
        error: String?,
        complete: Boolean,
    ) {
        val next = InstallUiState(
            phase = phase,
            progress = progress.coerceIn(0, 100),
            message = message,
            error = error,
            complete = complete,
        )
        installStateSnapshot = next
        composeView?.post { installStateValue.value = installStateSnapshot }
    }

    /** Subscribe to the process-lifetime native provisioning snapshot. */
    @Composable
    fun installState(): State<InstallUiState> = installStateValue

    /** Subscribe to the process-lifetime explicit Anland repair state. */
    @Composable
    fun anlandRepairState(): State<AnlandRepairUiState> = anlandRepairStateValue

    /** Publish native repair availability/progress without coupling it to install completion. */
    @JvmStatic fun updateAnlandRepairState(
        status: Int,
        progress: Int,
        message: String,
        error: String?,
    ) {
        val next = AnlandRepairUiState(
            status = status.coerceIn(ANLAND_REPAIR_UNAVAILABLE, ANLAND_REPAIR_COMPLETE),
            progress = progress.coerceIn(0, 100),
            message = message,
            error = error,
        )
        anlandRepairStateSnapshot = next
        composeView?.post { anlandRepairStateValue.value = anlandRepairStateSnapshot }
    }

    /** Start or attach to the one native provisioning operation. */
    @JvmStatic fun beginInstall(): Boolean = try {
        nativeBeginInstall()
    } catch (_: UnsatisfiedLinkError) {
        Log.e(TAG, "nativeBeginInstall unavailable")
        false
    } catch (e: Exception) {
        Log.e(TAG, "nativeBeginInstall failed", e)
        false
    }

    /** Ask the process-lifetime native coordinator to migrate graphics only. */
    @JvmStatic fun repairEnableAnland(): Boolean = try {
        nativeRepairEnableAnland()
    } catch (_: UnsatisfiedLinkError) {
        Log.e(TAG, "nativeRepairEnableAnland unavailable")
        false
    } catch (e: Exception) {
        Log.e(TAG, "nativeRepairEnableAnland failed", e)
        false
    }

    /** Ask native recovery to show the existing Retry Plasma page. */
    @JvmStatic fun requestAnlandRepairRecovery(): Boolean = try {
        nativeRequestAnlandRepairRecovery()
    } catch (_: UnsatisfiedLinkError) {
        Log.e(TAG, "nativeRequestAnlandRepairRecovery unavailable")
        false
    } catch (e: Exception) {
        Log.e(TAG, "nativeRequestAnlandRepairRecovery failed", e)
        false
    }

    /** Retained recovery-state bridge for the launch transition. */
    @JvmStatic fun updateState(value: String) {
        when (value) {
            STATE_READY -> updateDesktopReady(true)
            STATE_ERROR -> {
                updateDesktopReady(false)
                revealCommitted.set(false)
                composeView?.post {
                    applyReadyHost(container, active = false)
                }
            }
        }
    }

    /**
     * Remove the Compose veil immediately so the existing runtime-error page
     * can present its Retry Plasma action after a committed install fails to
     * bind or resume Wayland. No native surface is recreated or touched.
     */
    @JvmStatic fun dismissForRuntimeRecovery(activity: Activity) {
        recoveryDismissRequested.set(true)
        activity.runOnUiThread {
            if (!recoveryDismissRequested.compareAndSet(true, false)) return@runOnUiThread
            revealCommitted.set(false)
            val frame = container
            if (frame == null) {
                acknowledgeOverlayRemoved()
                return@runOnUiThread
            }
            frame.animate()?.cancel()
            frame.alpha = 1f
            // removeNow is guarded so the recovery path cannot race a final
            // reveal callback that was queued on this same UI thread.
            removalRequested.set(false)
            removeNow()
        }
    }

    private fun doShow(activity: Activity, returnMode: Boolean) {
        // A recovery dismissal may have been requested before a queued show
        // runnable reached the UI thread. Consume it rather than attaching a
        // new veil after the runtime error page was selected.
        if (recoveryDismissRequested.compareAndSet(true, false)) {
            if (container == null) {
                acknowledgeOverlayRemoved()
            } else {
                removalRequested.set(false)
                removeNow()
            }
            return
        }
        if (container != null) {
            // Recovery racing a committed reveal: restore a solid host
            // immediately while Compose returns its veil to rest.
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
            this.returnMode = returnMode
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
                        // controls gesture availability. The ambient layer
                        // owns the fragment blur and composites at 0.83; the
                        // transparent host lets the live SurfaceView show through.
                        applyReadyHost(frame, active)
                        if (active && changed) {
                            Log.i(TAG, "setup READY prelude; overlay host transparent for live SurfaceView")
                        }
                    },
                    onRevealCommitted = { acknowledgeRevealCommitted() },
                    onRevealFinished = { removeNow() },
                    isReturn = returnMode,
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
            installStateValue.value = installStateSnapshot
            anlandRepairStateValue.value = anlandRepairStateSnapshot
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
            returnMode = false
            recoveryDismissRequested.set(false)
            nativeOnOverlayShowFailed(reason)
        } catch (_: UnsatisfiedLinkError) {
        } catch (_: Exception) {
        }
    }

    /**
     * True only on debuggable (Debug) builds. Used to gate automation hooks
     * without depending on the generated BuildConfig class.
     */
    @JvmStatic fun isDebuggableBuild(): Boolean {
        return try {
            val flags = container?.context?.applicationInfo?.flags ?: return false
            (flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) != 0
        } catch (_: Exception) {
            false
        }
    }

    /**
     * DEBUG-ONLY automation hook: complete the READY veil exactly as a
     * committed human reveal does, so ADB-driven UI tests never interact
     * with Plasma through the veil. Release builds ignore every call.
     * Requires the desktop-ready latch (same eligibility as the reveal
     * affordance) and an attached veil; otherwise a no-op returning false.
     * Runs the identical native callbacks as the gesture path
     * (reveal-committed, then overlay-removed via removeNow), minus the
     * fling animation. Must be called on the UI thread.
     */
    @JvmStatic fun debugDismissVeilForAutomation(): Boolean {
        if (!isDebuggableBuild()) return false
        if (!desktopReadyState.value) {
            Log.i(TAG, "debug veil dismiss refused: desktop not ready")
            return false
        }
        val frame = container
        if (frame == null) {
            Log.i(TAG, "debug veil dismiss refused: no veil attached")
            return false
        }
        Log.i(TAG, "debug veil dismiss: completing reveal for automation")
        acknowledgeRevealCommitted()
        removeNow()
        return true
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
            container = null
            composeView = null
            readyPreludeActive.set(false)
            returnMode = false
            recoveryDismissRequested.set(false)
            Log.i(TAG, "overlay removed; native surface undisturbed")
            acknowledgeOverlayRemoved()
        }
    }

    private fun acknowledgeOverlayRemoved() {
        try {
            nativeOnOverlayRemoved()
        } catch (_: UnsatisfiedLinkError) {
        } catch (_: Exception) {
        }
    }

    private fun applyReadyHost(frame: FrameLayout?, active: Boolean) {
        frame?.setBackgroundColor(if (active) 0x00000000 else PORTAL_CHARCOAL)
    }

    private val PORTAL_CHARCOAL = 0xFF191B1C.toInt()
}
