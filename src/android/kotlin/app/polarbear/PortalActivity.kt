package app.polarbear

// Native Android launch host.
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
import android.content.res.Configuration
import android.graphics.PixelFormat
import android.media.audiofx.AudioEffect
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
import android.view.PointerIcon
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.ViewGroup
import android.view.WindowManager
import android.widget.FrameLayout
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import com.google.androidgamesdk.GameActivity
import java.io.File
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.abs

open class PortalActivity : GameActivity() {
    internal val portalDisplays by lazy { PortalDisplays(this) }
    private var activityDisplayId: Int? = null

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
        // The native side serves one Activity per process (see onDestroy), and
        // GameActivity.onCreate blocks forever on a second one: a black screen
        // until Android kills the app as not responding. Android can do this
        // when a clear-task launch arrives before the previous Activity is
        // destroyed (OxygenOS relaunches Portal this way during an update).
        // End the process; Android restarts this not-yet-resumed Activity in a
        // fresh one, the same as when onDestroy wins that race.
        if (!processHostsActivity.compareAndSet(false, true)) {
            Log.w(TAG, "second Activity in one process; restarting in a fresh process")
            SessionService.endProcess(this)
            Process.killProcess(Process.myPid())
        }
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
        val preferences = getPreferences(MODE_PRIVATE)
        androidCursorHidden = preferences.getBoolean(PREF_HIDE_ANDROID_CURSOR, true)
        useAndroidScreenTimeout = preferences.getBoolean(PREF_USE_ANDROID_SCREEN_TIMEOUT, true)
        usePortalKeyboard = preferences.getBoolean(PREF_USE_PORTAL_KEYBOARD, false)
        SessionControls.replaceAndroidKeyboard = usePortalKeyboard
        lowLatencyAudio = runCatching { audioOutputFile().readText().trim() == "low-latency" }.getOrDefault(false)
        screenInhibition.start()
        applyScreenTimeout()
        applyDesktopPointerIcon()
        ComposeOverlay.attachContext(this)
        SessionControls.attach(this)
        DesktopControlActivity.bindDesktop(this)
        FileTransfers.attach(this)
        if (intent.getBooleanExtra("portal_open_files", false)) SessionControls.requestFiles(this)
        applyImmersive("onCreate")
        activityDisplayId = currentDisplay()?.displayId
        GamepadBridge.attach(this)
        ClipboardWatch.attach(this)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            window.decorView.setOnCapturedPointerListener { _, event -> dispatchCaptured(event) }
        }
    }

    // Controller keys and sticks go to the guest's evdev pads before the view tree.
    override fun dispatchKeyEvent(event: KeyEvent): Boolean =
        DisplaySetup.dispatchKey(event) || SessionControls.onShortcut(event) || SessionControls.onBackKey(event) || TouchControls.dispatchUtilityKey(event) || dispatchMouseSideButton(event) || TouchControls.onBackKey(event) || GamepadBridge.dispatchKey(event) ||
            super.dispatchKeyEvent(event)

    /**
     * Android sends a mouse's side buttons as Back/Forward keys rather than
     * button presses. Send them on as BTN_SIDE/BTN_EXTRA so browsers go
     * back/forward and games see mouse 4/5; as keys, Back became Esc and
     * Forward was dropped.
     */
    private fun dispatchMouseSideButton(event: KeyEvent): Boolean {
        if (!KeyboardAccessibilityService.isMouseSideButton(event)) return false
        if (event.repeatCount == 0 &&
            (event.action == KeyEvent.ACTION_DOWN || event.action == KeyEvent.ACTION_UP)
        ) {
            val code = if (event.keyCode == KeyEvent.KEYCODE_BACK) 0x113 else 0x114
            nativeCapturedButton(code, event.action == KeyEvent.ACTION_DOWN)
        }
        return true
    }

    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean =
        dispatchCaptured(event) || GamepadBridge.dispatchMotion(event) ||
            super.dispatchGenericMotionEvent(event)

    // A captured mouse reports as SOURCE_MOUSE_RELATIVE, a trackball-class
    // source, which reaches the activity here when no view consumed it.
    override fun dispatchTrackballEvent(event: MotionEvent): Boolean =
        dispatchCaptured(event) || super.dispatchTrackballEvent(event)

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (intent.action == Intent.ACTION_MAIN && intent.hasCategory(Intent.CATEGORY_LAUNCHER) &&
            !intent.getBooleanExtra(PortalDisplays.EXTRA_DISPLAY_MOVE, false)
        ) {
            DesktopControlActivity.showOnTablet(this)
        }
        if (intent.getBooleanExtra("portal_open_files", false)) {
            SessionControls.requestFiles(this)
        }
        // Debug builds only: open the first-run stages on demand.
        //   adb shell am start -n app.polarbear/.PortalActivity --activity-single-top --ez portal_preview_setup true
        //   (add --es portal_preview_install ok|fail to play a scripted install,
        //   --es portal_preview_stage welcome|prepare|look|apps to open on a stage,
        //   --es portal_preview_workspace <name> to set up a new workspace by that name)
        if ((applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE) != 0 &&
            intent.getBooleanExtra(PortalPreview.EXTRA, false)
        ) {
            PortalPreview.show(
                this,
                intent.getFloatExtra(PortalPreview.EXTRA_SLOW, 1f),
                intent.getBooleanExtra(PortalPreview.EXTRA_GATE, false),
                intent.getStringExtra(PortalPreview.EXTRA_INSTALL),
                intent.getStringExtra(PortalPreview.EXTRA_STAGE),
                intent.getStringExtra(PortalPreview.EXTRA_WORKSPACE),
            )
        }
        //   adb shell am start -n app.polarbear/.PortalActivity --activity-single-top --es portal_preview_updates full
        if ((applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE) != 0) {
            intent.getStringExtra(UpdatePreview.EXTRA)?.let(UpdatePreview::play)
        }
        // Debug builds: show the desktop on a display as External display would.
        //   adb shell am start -n app.polarbear/.PortalActivity --activity-single-top --es portal_display 5
        //   (add --ef portal_display_slow 8 to slow the touchpad's transitions down for screenshots,
        //   --ez portal_display_setup true to go through every step of the monitor setup first)
        if ((applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE) != 0) {
            if (intent.hasExtra(EXTRA_DEBUG_DISPLAY_SLOW)) {
                PresentedDesktopPad.slow = intent.getFloatExtra(EXTRA_DEBUG_DISPLAY_SLOW, 1f).coerceIn(1f, 20f)
            }
            // Without portal_display, the setup plays for the next monitor chosen in Portal settings.
            val setup = intent.getBooleanExtra(EXTRA_DEBUG_DISPLAY_SETUP, false)
            when (val display = intent.getStringExtra(EXTRA_DEBUG_DISPLAY)?.toIntOrNull()) {
                null -> if (setup) DisplaySetup.previewNext = true
                else -> if (setup) DisplaySetup.preview(this, display) else portalDisplays.moveTo(display)
            }
        }
        // Return to phone in the notification. A moved activity is already
        // back from the launch display; a presented desktop comes back here.
        if (intent.getBooleanExtra(PortalDisplays.EXTRA_RETURN, false)) endPresentation()
    }

    /** Started, so this screen shows Portal: while presenting, the touchpad page. */
    internal var onScreen = false
        private set

    /** The phone's touchpad page is on screen, with its own Control desktop. */
    internal val showsPresentedPad: Boolean get() = onScreen && presentedPad != null

    override fun onStart() {
        super.onStart()
        onScreen = true
        SessionService.sync(this)
        SetupService.setPortalVisible(this, true)
        DesktopControlActivity.displayChanged()
    }

    override fun onStop() {
        SetupService.setPortalVisible(this, false)
        super.onStop()
        onScreen = false
        // A presented desktop stays on the monitor; the floating controls take over here.
        DesktopControlActivity.displayChanged()
    }

    override fun onResume() {
        super.onResume()
        TouchControls.attach(this)
        AppUpdateInstaller.onActivityResumed(this)
        CapturePermissions.refresh(this)
        // Back from Android's permission pages, or a revoke.
        portalDisplays.refreshPermissions()
        DesktopControlActivity.displayChanged()
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        updateDisplayMode()
        CapturePermissions.reportRotation(this)
    }

    /** Called from native code when a Linux app starts using the mic or a camera. */
    fun requestCapturePermission(kind: Int) {
        runOnUiThread { CapturePermissions.request(this, kind) }
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        CapturePermissions.onResult(this, requestCode, grantResults)
    }

    override fun onPause() {
        SessionControls.reset()
        TouchControls.detach()
        AppUpdateInstaller.onActivityPaused(this)
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
        SessionService.endProcess(this)
        Process.killProcess(Process.myPid())
        super.onDestroy()
    }

    /** Native guest-worker ownership; surface suspension deliberately does not end the session. */
    fun setDesktopSessionActive(owner: Long, active: Boolean) {
        SessionService.setActive(this, owner, active)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        // Android legitimately clears bar visibility on focus changes;
        // reapply without touching geometry.
        if (hasFocus) {
            applyImmersive("focus")
            updateDisplayMode()
            // Android drops pointer capture with window focus; take it back
            // if the guest still holds its pointer lock.
            applyPointerCapture()
            SessionControls.focused()
        } else TouchControls.cancelInput()
    }

    /**
     * Native (any thread): give the desktop a fresh Android surface. Anland
     * connects the window to the CPU buffer API and Android has no public
     * call that disconnects it, so an EGL renderer cannot use that surface
     * afterwards. Hiding the SurfaceView releases its surface and showing it
     * again creates a new one through the usual native window callbacks; the
     * view itself is never detached or recreated.
     */
    fun recycleNativeSurface() {
        runOnUiThread {
            // The presented monitor's view while the desktop is shown there.
            val surface = desktopSurface()
            if (surface == null) {
                Log.w(TAG, "No native SurfaceView to recycle")
                return@runOnUiThread
            }
            surface.visibility = android.view.View.GONE
            surface.postDelayed({ surface.visibility = android.view.View.VISIBLE }, 300)
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

    internal var pointerCaptureWanted = false
        private set
    private val capturedPointer = CapturedDesktopPointer(::nativeCapturedMotion, ::nativeCapturedButton, ::nativeCapturedWheel)

    internal fun moveLockedDesktopPointer(dx: Float, dy: Float) = nativeCapturedMotion(dx, dy)

    /** The view the desktop is drawn in: on a presented monitor, or this activity's own. */
    internal fun desktopSurface(): SurfaceView? = presentation?.desktopView ?: mSurfaceView

    /**
     * Where [overlayHost] takes views drawn over the desktop (touch controls,
     * the capsule): above the native SurfaceView (child 0) and, while the
     * desktop is presented, the touchpad, but below Portal's own screens.
     */
    internal fun desktopLayerTop(): Int {
        val host = overlayHost()
        val base = presentedPad?.let(host::indexOfChild)?.takeIf { it >= 0 } ?: 0
        return (base + 1).coerceAtMost(host.childCount)
    }

    // Where the desktop draws. GameActivity gives native code its own
    // SurfaceView's surface; while the desktop is presented on a monitor
    // ([DesktopPresentation]) native code gets that window's surface instead,
    // through the same callbacks, so every renderer just sees a new window.
    // The phone's SurfaceView stays (it holds the IME connection) under the
    // touchpad.
    @Volatile private var presentation: DesktopPresentation? = null
    private var presentedPad: PresentedDesktopPad? = null
    /** The touchpad page after the desktop came back, until the desktop shows through it here. */
    private var leavingPad: PresentedDesktopPad? = null

    /**
     * Native code keeps drawing here while the touchpad copies the desktop from
     * this screen, so the copy is the desktop and not a surface it has left.
     */
    private var holdingPhoneSurface = false

    /** The holder whose surface native code currently draws into. */
    private var nativeHolder: SurfaceHolder? = null

    private fun desktopHolder(): SurfaceHolder? =
        presentation?.holder?.takeUnless { holdingPhoneSurface } ?: mSurfaceView?.holder

    override fun surfaceCreated(holder: SurfaceHolder) {
        if (holder !== desktopHolder()) return
        releaseNativeSurface()
        nativeHolder = holder
        super.surfaceCreated(holder)
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        if (holder === nativeHolder) super.surfaceChanged(holder, format, width, height)
    }

    override fun surfaceRedrawNeeded(holder: SurfaceHolder) {
        if (holder === nativeHolder) super.surfaceRedrawNeeded(holder)
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        if (holder === nativeHolder) releaseNativeSurface()
    }

    /** Takes the desktop off its surface; native code suspends rendering until the next one. */
    private fun releaseNativeSurface() {
        val holder = nativeHolder ?: return
        nativeHolder = null
        super.surfaceDestroyed(holder)
    }

    /** Hands native code the desktop holder's surface when it already exists; otherwise its creation will. */
    private fun attachNativeSurface() {
        val holder = desktopHolder() ?: return
        if (holder === nativeHolder || holder.surface?.isValid != true) return
        releaseNativeSurface()
        nativeHolder = holder
        super.surfaceCreated(holder)
        val frame = holder.surfaceFrame
        super.surfaceChanged(holder, PixelFormat.RGBA_8888, frame.width(), frame.height())
    }

    /** The display the desktop is presented on, while this activity stays here. */
    internal val presentedDisplay: Display? get() = presentation?.display

    /**
     * Shows the desktop on [display] in a Presentation; this screen becomes
     * its touchpad. False when Android refuses the window.
     */
    internal fun presentDesktop(display: Display): Boolean {
        if (presentation?.display?.displayId == display.displayId) return true
        // Between monitors this screen has no desktop on it to carry over.
        val fromHere = presentation == null
        endPresentation()
        finishLeavingPad()
        val shown = DesktopPresentation(this, display, this) { ended ->
            if (presentation === ended) {
                Log.i(TAG, "Presentation on display ${ended.display.displayId} ended")
                endPresentation()
            }
        }
        presentation = shown
        holdingPhoneSurface = fromHere
        try {
            shown.show()
        } catch (error: RuntimeException) {
            // InvalidDisplayException included: the monitor went, or Android refused the window.
            Log.w(TAG, "Could not present the desktop on display ${display.displayId}", error)
            presentation = null
            holdingPhoneSurface = false
            return false
        }
        Log.i(TAG, "Presenting the desktop on display ${display.displayId} (${display.name})")
        applyScreenTimeout()
        // The touchpad carries the desktop as this screen shows it into its
        // monitor chip. Native code moves to the monitor once that copy is
        // taken, or when the Presentation's surface is created.
        val snapshot = if (fromHere) {
            DesktopSnapshot.take(mSurfaceView) {
                if (presentation === shown && holdingPhoneSurface) {
                    holdingPhoneSurface = false
                    attachNativeSurface()
                }
            }
        } else null
        attachNativeSurface()
        val pad = PresentedDesktopPad(this, portalDisplays.displayName(display), snapshot)
        overlayHost().addView(pad, 1.coerceAtMost(overlayHost().childCount),
            FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        presentedPad = pad
        applyHighRefresh()
        desktopDisplayChanged()
        return true
    }

    /** Brings the desktop back to this activity's own surface. */
    internal fun endPresentation() {
        val ending = presentation ?: return
        presentation = null
        holdingPhoneSurface = false
        presentedPad?.let { pad ->
            finishLeavingPad()
            leavingPad = pad
            // The touchpad stays until the desktop shows here again, then opens onto it.
            pad.leave {
                if (leavingPad === pad) {
                    leavingPad = null
                    overlayHost().removeView(pad)
                }
            }
        }
        presentedPad = null
        if (nativeHolder === ending.holder) releaseNativeSurface()
        try {
            ending.dismiss()
        } catch (error: RuntimeException) {
            Log.w(TAG, "Presentation was already gone", error)
        }
        attachNativeSurface()
        applyHighRefresh()
        desktopDisplayChanged()
    }

    private fun finishLeavingPad() {
        val pad = leavingPad ?: return
        leavingPad = null
        overlayHost().removeView(pad)
    }

    private fun desktopDisplayChanged() {
        DesktopControlActivity.displayChanged()
        SessionService.refreshNotification()
    }

    /** Native (any thread): the display the desktop is shown on. */
    fun desktopDisplay(): Display? = presentation?.display ?: currentDisplay()

    /** Native (any thread): density of the display the desktop is shown on. */
    fun desktopDensityDpi(): Int = presentation?.densityDpi ?: resources.displayMetrics.densityDpi

    /**
     * Native (any thread): refresh rate in millihertz to advertise for the
     * desktop's output; see [highRefreshTargetMillihz].
     */
    fun desktopRefreshTargetMillihz(): Int = presentation?.display?.let { Math.round(it.refreshRate * 1000f) }
        ?: highRefreshTargetMillihz()

    private fun applyPointerCapture() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val view = window.decorView
        if (pointerCaptureWanted && !ComposeOverlay.isShowing() && !SessionControls.isUtilityVisible()) {
            if (hasWindowFocus() && !view.hasPointerCapture()) view.requestPointerCapture()
        } else if (view.hasPointerCapture()) {
            view.releasePointerCapture()
        }
    }

    internal var androidCursorHidden by mutableStateOf(true)
        private set

    internal fun setHideAndroidCursor(hidden: Boolean) {
        androidCursorHidden = hidden
        getPreferences(MODE_PRIVATE).edit().putBoolean(PREF_HIDE_ANDROID_CURSOR, hidden).apply()
        applyDesktopPointerIcon()
    }

    private val screenInhibition = ScreenInhibition { runOnUiThread(::applyScreenTimeout) }

    internal var useAndroidScreenTimeout by mutableStateOf(true)
        private set

    internal fun setAndroidScreenTimeoutEnabled(enabled: Boolean) {
        useAndroidScreenTimeout = enabled
        getPreferences(MODE_PRIVATE).edit().putBoolean(PREF_USE_ANDROID_SCREEN_TIMEOUT, enabled).apply()
        applyScreenTimeout()
    }

    internal var usePortalKeyboard by mutableStateOf(false)
        private set

    internal fun setPortalKeyboardEnabled(enabled: Boolean) {
        usePortalKeyboard = enabled
        getPreferences(MODE_PRIVATE).edit().putBoolean(PREF_USE_PORTAL_KEYBOARD, enabled).apply()
        SessionControls.replaceAndroidKeyboard = enabled
    }

    // The native audio supervisor reads this file when it starts the output.
    private fun audioOutputFile() = File(filesDir, "audio-output")

    internal var lowLatencyAudio by mutableStateOf(false)
        private set

    internal fun setLowLatencyAudioEnabled(enabled: Boolean) {
        lowLatencyAudio = enabled
        runCatching {
            if (enabled) audioOutputFile().writeText("low-latency\n") else audioOutputFile().delete()
        }
        nativeAudioOutputChanged()
    }

    private var audioEffectSession = 0

    /**
     * The audio session Portal plays on, or 0 when it has none. Announced the way media
     * players do, so equalizer and sound-effect apps can attach to Portal's sound.
     */
    @Suppress("unused") // called from native code
    fun setAudioEffectSession(session: Int) = runOnUiThread {
        if (session == audioEffectSession) return@runOnUiThread
        if (audioEffectSession > 0) {
            announceAudioEffectSession(AudioEffect.ACTION_CLOSE_AUDIO_EFFECT_CONTROL_SESSION, audioEffectSession)
        }
        audioEffectSession = session
        if (session > 0) {
            announceAudioEffectSession(AudioEffect.ACTION_OPEN_AUDIO_EFFECT_CONTROL_SESSION, session)
        }
    }

    private fun announceAudioEffectSession(action: String, session: Int) {
        sendBroadcast(
            Intent(action)
                .putExtra(AudioEffect.EXTRA_AUDIO_SESSION, session)
                .putExtra(AudioEffect.EXTRA_PACKAGE_NAME, packageName)
                .putExtra(AudioEffect.EXTRA_CONTENT_TYPE, AudioEffect.CONTENT_TYPE_MUSIC),
        )
    }

    private fun applyScreenTimeout() {
        val flag = WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON
        val keepOn = !useAndroidScreenTimeout || screenInhibition.active
        // A presented desktop's window too: after Home it is the only one on screen.
        for (target in listOfNotNull(window, presentation?.window)) {
            if (keepOn) target.addFlags(flag) else target.clearFlags(flag)
        }
    }

    private fun applyDesktopPointerIcon() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) return
        // Plasma draws the desktop cursor. TYPE_NULL only changes Android's icon.
        mSurfaceView?.pointerIcon = if (androidCursorHidden && !ComposeOverlay.isShowing()) {
            PointerIcon.getSystemIcon(this, PointerIcon.TYPE_NULL)
        } else {
            null
        }
    }

    fun overlayInputChanged() {
        applyDesktopPointerIcon()
        applyPointerCapture()
        if (ComposeOverlay.isShowing() || SessionControls.isUtilityVisible()) {
            DesktopControlActivity.releaseTabletInput()
        }
    }

    override fun onPointerCaptureChanged(hasCapture: Boolean) {
        super.onPointerCaptureChanged(hasCapture)
        Log.i(TAG, "pointer capture=$hasCapture (wanted=$pointerCaptureWanted)")
        capturedPointer.reset()
    }

    /** Captured mouse/touchpad input goes straight to the Anland session. */
    private fun dispatchCaptured(event: MotionEvent): Boolean =
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && window.decorView.hasPointerCapture() &&
            capturedPointer.dispatch(event)

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

    fun setUtilityHighRefresh(enable: Boolean) {
        utilityHighRefresh = enable
        applyHighRefresh()
    }
    private var utilityHighRefresh = false
    private var desktopHighRefresh = false
    private var overlayHighRefresh = false

    @Suppress("DEPRECATION")
    internal fun currentDisplay(): Display? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
        display
    } else {
        windowManager.defaultDisplay
    }

    private fun updateDisplayMode() {
        val displayId = currentDisplay()?.displayId ?: return
        if (activityDisplayId == displayId) return
        activityDisplayId = displayId
        applyHighRefresh()
        DesktopControlActivity.displayChanged()
        SessionService.refreshNotification()
    }

    private fun applyHighRefresh() {
        // A desktop presented on a monitor doesn't draw on this screen.
        val enable = desktopHighRefresh && presentation == null || overlayHighRefresh || utilityHighRefresh
        refreshHandler.removeCallbacks(idleRefreshCheck)
        requestDisplayMode(if (enable) highRefreshModeId() else 0, "enable=$enable")
        if (!enable) {
            refreshHandler.postDelayed(idleRefreshCheck, IDLE_REFRESH_CHECK_MS)
        }
    }

    private fun requestDisplayMode(modeId: Int, reason: String) {
        try {
            val attributes = window.attributes
            if (attributes.preferredDisplayModeId != modeId) {
                window.attributes = attributes.apply { preferredDisplayModeId = modeId }
                Log.i(TAG, "display mode request: $reason modeId=$modeId")
            }
        } catch (e: Exception) {
            Log.e(TAG, "display mode request failed ($reason)", e)
        }
    }

    private val refreshHandler = Handler(Looper.getMainLooper())

    /**
     * Releasing the request normally lets the system drop an idle panel to
     * its low rate. A per-app refresh override (OxygenOS's per-app setting,
     * game lists) instead holds a minimum of the panel's peak for as long as
     * Portal is in front, however idle the desktop is. When the panel is
     * still fast some time after the release, ask for the idle mode
     * explicitly; the next interaction replaces it with the peak mode.
     */
    private val idleRefreshCheck = Runnable {
        if (desktopHighRefresh && presentation == null || overlayHighRefresh || utilityHighRefresh) return@Runnable
        try {
            val modeId = idleRefreshModeId()
            if (modeId != 0) requestDisplayMode(modeId, "idle panel still fast")
        } catch (e: Exception) {
            Log.e(TAG, "idle display mode check failed", e)
        }
    }

    /**
     * The mode to hold on an idle desktop when the system keeps the panel
     * fast by itself: same size as the active mode, seamless, and the slowest
     * rate that still shows 60 fps content. 0 when the panel is already slow,
     * varies its own rate (ARR), or has no such mode.
     */
    private fun idleRefreshModeId(): Int {
        val display = currentDisplay() ?: return 0
        if (display.refreshRate <= IDLE_REFRESH_HZ + 0.5f) return 0
        if (Build.VERSION.SDK_INT >= 36) {
            val arr = try {
                Display::class.java.getMethod("hasArrSupport").invoke(display) as? Boolean
            } catch (e: Exception) {
                null
            }
            if (arr == true) return 0
        }
        val active = display.mode
        val seamless = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            active.alternativeRefreshRates
        } else {
            return 0
        }
        return display.supportedModes
            .filter {
                it.physicalWidth == active.physicalWidth &&
                    it.physicalHeight == active.physicalHeight &&
                    abs(it.refreshRate - IDLE_REFRESH_HZ) <= 0.5f &&
                    seamless.any { rate -> abs(rate - it.refreshRate) < 0.5f }
            }
            .maxByOrNull { it.refreshRate }
            ?.modeId ?: 0
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
        val display = currentDisplay() ?: return null
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
        private const val PREF_HIDE_ANDROID_CURSOR = "hide_android_cursor"
        private const val PREF_USE_ANDROID_SCREEN_TIMEOUT = "use_android_screen_timeout"
        private const val PREF_USE_PORTAL_KEYBOARD = "use_portal_keyboard"
        private const val EXTRA_DEBUG_DISPLAY = "portal_display"
        private const val EXTRA_DEBUG_DISPLAY_SLOW = "portal_display_slow"
        private const val EXTRA_DEBUG_DISPLAY_SETUP = "portal_display_setup"
        private const val IDLE_REFRESH_CHECK_MS = 1000L
        private const val IDLE_REFRESH_HZ = 60f
        private val processHostsActivity = AtomicBoolean(false)

        @JvmStatic private external fun nativeAudioOutputChanged()
        @JvmStatic private external fun nativeCapturedMotion(dx: Float, dy: Float)
        // Also carries mouse side buttons, captured or not.
        @JvmStatic private external fun nativeCapturedButton(code: Int, pressed: Boolean)
        // Android axis values: vertical > 0 is wheel up, horizontal > 0 is right.
        @JvmStatic private external fun nativeCapturedWheel(horizontal: Float, vertical: Float)

    }
}
