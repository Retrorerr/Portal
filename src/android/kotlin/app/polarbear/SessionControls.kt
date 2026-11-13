package app.polarbear

import android.os.Handler
import android.os.Looper
import android.view.HapticFeedbackConstants
import android.view.KeyEvent
import android.view.View
import androidx.core.view.ViewCompat
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import java.lang.ref.WeakReference

/** All recall entry points converge on the native overlay readiness gate. */
object SessionControls {
    @JvmStatic private external fun nativeOpen(): Boolean
    @JvmStatic private external fun nativeUtilityInput(open: Boolean): Boolean
    @JvmStatic private external fun nativeTextInputActive(): Boolean
    @JvmStatic private external fun nativeUtilityKeyboard(active: Boolean)
    @JvmStatic private external fun nativeUtilityKeyboardInput(): Boolean
    @JvmStatic private external fun nativeKeyboardWanted(): Boolean
    @JvmStatic private external fun nativeDesktopKey(code: Int, down: Boolean)
    @JvmStatic private external fun nativeDesktopFrames(): Long
    /** Frames Portal presented to the desktop surface; 0 (unknown) means sample anyway. */
    internal fun desktopFrames(): Long = try { nativeDesktopFrames() } catch (_: UnsatisfiedLinkError) { 0L }
    var desktopKeyboardVisible by androidx.compose.runtime.mutableStateOf(false)
        private set
    internal var desktopKeyboardRevision by androidx.compose.runtime.mutableIntStateOf(0)
        private set
    internal val desktopKeyboard = DesktopKeyboardState({ code, down -> nativeDesktopKey(code, down) }, { desktopKeyboardRevision++ })
    fun setDesktopKeyboard(active: Boolean) {
        desktopKeyboard.reset()
        desktopKeyboardVisible = active
        nativeUtilityKeyboard(active)
    }
    fun resetDesktopKeyboard() { desktopKeyboard.reset() }
    val textInputFocused = mutableStateOf(false)
    /** Portal's keyboard stands in for Android's wherever that one would appear. */
    @Volatile internal var replaceAndroidKeyboard = false
    private val main = Handler(Looper.getMainLooper())
    // Focus hopping between two fields reports the loss first. Wait for the gain.
    private val closeTextKeyboard = Runnable { if (!nativeKeyboardWanted()) TouchControls.textKeyboardUnwanted() }

    /** Called where Android's keyboard is about to show; true when Portal's took its place. */
    @JvmStatic fun showPortalKeyboard(): Boolean {
        if (!replaceAndroidKeyboard) return false
        main.removeCallbacks(closeTextKeyboard)
        return TouchControls.showTextKeyboard()
    }
    @Volatile private var utilityVisible = false
    private var utilityOwner: Any? = null
    private var activity = WeakReference<PortalActivity>(null)
    private var shortcutHeld = false
    private var pendingFiles = false
    private var backCallback: androidx.activity.OnBackPressedCallback? = null

    fun attach(host: PortalActivity) {
        activity = WeakReference(host)
        refreshTextInput()
        host.overlayHost().importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_YES
        backCallback = object : androidx.activity.OnBackPressedCallback(false) {
            override fun handleOnBackPressed() { if (utilityVisible) TouchControls.closeUtilityMenu() else ComposeOverlay.backFromControls() }
        }.also { host.onBackPressedDispatcher.addCallback(host, it) }
        ViewCompat.addAccessibilityAction(host.overlayHost(), "Open Portal controls") { _, _ -> open(host) }
    }

    /** The guest focus signal includes fields used with a physical keyboard. */
    @JvmStatic fun refreshTextInput() {
        if (Looper.myLooper() != Looper.getMainLooper()) {
            Handler(Looper.getMainLooper()).post { refreshTextInput() }
            return
        }
        textInputFocused.value = nativeTextInputActive()
        if (!replaceAndroidKeyboard) return
        main.removeCallbacks(closeTextKeyboard)
        if (!nativeKeyboardWanted()) main.postDelayed(closeTextKeyboard, 150)
    }

    fun requestFiles(host: PortalActivity) = requestPanel(host, "files")
    fun requestPanel(host: PortalActivity, panel: String?) {
        FileTransfers.filesRequested = panel == "files"
        ComposeOverlay.activePanel = panel
        pendingFiles = panel == "files"
        if (panel == "files") finishPendingFiles(host)
        else if (!ComposeOverlay.isShowing()) open(host)
    }
    @JvmStatic fun isUtilityVisible() = utilityVisible
    fun acquireUtility(owner: Any, keyboard: Boolean = false): Boolean {
        if (utilityOwner === owner) return true
        if (!(if (keyboard) nativeUtilityKeyboardInput() else nativeUtilityInput(true))) return false
        utilityOwner = owner
        publishUtilityVisible(true)
        return true
    }
    fun releaseUtility(owner: Any): Boolean {
        if (utilityOwner !== owner) return false
        utilityOwner = null
        nativeUtilityInput(false)
        publishUtilityVisible(false)
        return true
    }
    private fun publishUtilityVisible(visible: Boolean) {
        utilityVisible = visible
        backCallback?.isEnabled = visible || ComposeOverlay.controlsVisible()
        activity.get()?.overlayInputChanged()
    }
    fun desktopReadyChanged(ready: Boolean) {
        if (ready) activity.get()?.let(::finishPendingFiles)
    }
    fun focused() { activity.get()?.let(::finishPendingFiles) }
    private fun finishPendingFiles(host: PortalActivity) {
        if (!pendingFiles || !host.hasWindowFocus()) return
        if (ComposeOverlay.isShowing()) {
            pendingFiles = false
            ComposeOverlay.activePanel = "files"
            FileTransfers.open()
        } else if (open(host)) pendingFiles = false
    }

    fun overlayChanged() {
        if (ComposeOverlay.isShowing() && ComposeOverlay.activePanel == "files") pendingFiles = false
        backCallback?.isEnabled = utilityVisible || ComposeOverlay.controlsVisible()
        activity.get()?.overlayInputChanged()
        TouchControls.overlayChanged()
    }
    fun onBackKey(event: KeyEvent): Boolean {
        if (!ComposeOverlay.controlsVisible() ||
            event.keyCode !in setOf(KeyEvent.KEYCODE_BACK, KeyEvent.KEYCODE_ESCAPE)) return false
        if (event.action == KeyEvent.ACTION_UP) ComposeOverlay.backFromControls()
        return true
    }

    fun open(host: PortalActivity, requireFocus: Boolean = true): Boolean {
        if (requireFocus && !host.hasWindowFocus() || ComposeOverlay.isShowing()) return false
        val opened = nativeOpen()
        if (opened) host.window.decorView.performHapticFeedback(HapticFeedbackConstants.CONTEXT_CLICK)
        return opened
    }

    @JvmStatic fun onShortcut(event: KeyEvent): Boolean {
        val host = activity.get() ?: return false
        if (!host.hasWindowFocus()) return false
        if (event.keyCode != KeyEvent.KEYCODE_P) return false
        if (event.action == KeyEvent.ACTION_UP && shortcutHeld) { shortcutHeld = false; return true }
        if (event.action != KeyEvent.ACTION_DOWN || !event.isCtrlPressed || !event.isAltPressed) return false
        if (event.repeatCount == 0) {
            shortcutHeld = true
            Handler(Looper.getMainLooper()).post {
                if (utilityVisible) TouchControls.openVeilFromUtility()
                else if (ComposeOverlay.isShowing()) ComposeOverlay.requestReturnToDesktop() else requestPanel(host, null)
            }
        }
        return true
    }

    fun reset() { shortcutHeld = false }
}
