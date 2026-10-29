package app.polarbear

import android.os.Handler
import android.os.Looper
import android.view.HapticFeedbackConstants
import android.view.KeyEvent
import android.view.View
import androidx.core.view.ViewCompat
import java.lang.ref.WeakReference

/** All recall entry points converge on the native overlay readiness gate. */
object SessionControls {
    @JvmStatic private external fun nativeOpen(): Boolean
    @JvmStatic private external fun nativeUtilityInput(open: Boolean): Boolean
    @Volatile private var utilityVisible = false
    private var activity = WeakReference<PortalActivity>(null)
    private var shortcutHeld = false
    private var pendingFiles = false
    private var backCallback: androidx.activity.OnBackPressedCallback? = null

    fun attach(host: PortalActivity) {
        activity = WeakReference(host)
        host.overlayHost().importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_YES
        backCallback = object : androidx.activity.OnBackPressedCallback(false) {
            override fun handleOnBackPressed() { if (utilityVisible) TouchControls.closeUtilityMenu() else ComposeOverlay.backFromControls() }
        }.also { host.onBackPressedDispatcher.addCallback(host, it) }
        ViewCompat.addAccessibilityAction(host.overlayHost(), "Open Portal controls") { _, _ -> open(host) }
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
    fun setUtilityVisible(visible: Boolean): Boolean {
        if (visible == utilityVisible) return true
        if (!nativeUtilityInput(visible) && visible) return false
        utilityVisible = visible
        backCallback?.isEnabled = visible || ComposeOverlay.controlsVisible()
        activity.get()?.overlayInputChanged()
        return true
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
