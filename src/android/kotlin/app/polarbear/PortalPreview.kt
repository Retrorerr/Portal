package app.polarbear

// Debug builds only (PortalActivity checks FLAG_DEBUGGABLE): shows the real
// first-run setup screen above a finished install so its stages and animations
// can be reviewed without reinstalling. It reads no install state, starts no
// installation, and Back closes it.
//
// The optional slow factor stretches every Compose animation inside the preview
// (tweens, springs, shared-element bounds, infinite loops) by giving its own
// Recomposer a MotionDurationScale, the same mechanism as Android's animator
// duration scale but local to this view. No device setting is touched.

import android.graphics.Color
import android.util.Log
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.activity.OnBackPressedCallback
import androidx.compose.runtime.Recomposer
import androidx.compose.ui.Modifier
import androidx.compose.ui.InternalComposeUiApi
import androidx.compose.ui.MotionDurationScale
import androidx.compose.ui.platform.AndroidUiDispatcher
import androidx.compose.ui.platform.ComposeView
import app.polarbear.setup.PortalPrepareGate
import app.polarbear.setup.PortalSetupScreen
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

object PortalPreview {
    const val EXTRA = "portal_preview_setup"
    const val EXTRA_SLOW = "portal_preview_slow"
    // Show the launch gate of an installed desktop instead of the first-run card.
    const val EXTRA_GATE = "portal_preview_gate"
    // Begin Install plays a scripted install: "ok", or "fail" to pause once.
    const val EXTRA_INSTALL = "portal_preview_install"
    private const val TAG = "PortalPreview"

    private var frame: FrameLayout? = null
    private var stopSlowClock: (() -> Unit)? = null
    private var backCallback: OnBackPressedCallback? = null

    fun show(activity: PortalActivity, slow: Float = 1f, gate: Boolean = false, install: String? = null) {
        hide()
        val root = FrameLayout(activity).apply {
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            )
            setBackgroundColor(Color.parseColor("#191B1C"))
            isClickable = true
        }
        val view = ComposeView(activity).apply {
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            )
            if (slow > 1f) stopSlowClock = slowDown(this, slow)
            setContent {
                if (gate) {
                    PortalPrepareGate(launchMarkModifier = Modifier, onCleared = { hide() }, preview = true)
                } else {
                    PortalSetupScreen(previewOnly = true, previewInstall = install)
                }
            }
        }
        root.addView(view)
        activity.overlayHost().addView(root)
        frame = root
        val callback = object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                hide()
            }
        }
        backCallback = callback
        activity.onBackPressedDispatcher.addCallback(activity, callback)
        Log.i(TAG, "setup preview shown (animations x$slow slower)")
    }

    private fun hide() {
        backCallback?.remove()
        backCallback = null
        stopSlowClock?.invoke()
        stopSlowClock = null
        frame?.let { (it.parent as? ViewGroup)?.removeView(it) }
        if (frame != null) Log.i(TAG, "setup preview closed")
        frame = null
    }

    /** Give [view] a Recomposer whose animations run [factor] times slower. */
    @OptIn(InternalComposeUiApi::class)
    private fun slowDown(view: ComposeView, factor: Float): () -> Unit {
        val scale = object : MotionDurationScale {
            override val scaleFactor: Float = factor
        }
        val context = AndroidUiDispatcher.CurrentThread + scale
        val recomposer = Recomposer(context)
        val scope = CoroutineScope(context)
        scope.launch(start = CoroutineStart.UNDISPATCHED) { recomposer.runRecomposeAndApplyChanges() }
        view.setParentCompositionContext(recomposer)
        return {
            recomposer.cancel()
            scope.cancel()
        }
    }
}
