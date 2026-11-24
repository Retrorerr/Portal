package app.polarbear

import android.app.Presentation
import android.content.Context
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import android.view.Display
import android.view.MotionEvent
import android.view.PointerIcon
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.ViewGroup
import android.widget.FrameLayout

/**
 * The desktop on a monitor Portal's activity can't move to.
 *
 * Phones mirror a monitor and keep activities off it unless Android's desktop
 * mode is on (OxygenOS forces mirroring and reports the display as unable to
 * host tasks), but any app may show a Presentation there. The activity stays
 * on the phone as the touchpad, and the desktop's native window is this
 * window's surface: [PortalActivity] routes GameActivity's surface callbacks
 * to it, so the renderers see an ordinary new window of the monitor's size.
 */
internal class DesktopPresentation(
    host: PortalActivity,
    display: Display,
    private val callback: SurfaceHolder.Callback2,
    private val ended: (DesktopPresentation) -> Unit,
) : Presentation(host, display) {
    private val surfaceView = SurfaceView(context)

    val holder: SurfaceHolder get() = surfaceView.holder

    /** The view the desktop's cursor and size are measured against. */
    val desktopView: SurfaceView get() = surfaceView

    val densityDpi: Int get() = context.resources.displayMetrics.densityDpi

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        surfaceView.holder.addCallback(callback)
        setContentView(
            ArrowShield(context).apply {
                setBackgroundColor(Color.BLACK)
                addView(surfaceView, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
            },
        )
        // Also when Android removes the monitor or cancels the window.
        setOnDismissListener { ended(this) }
    }

    /**
     * Android may keep its own arrow on this monitor (OxygenOS puts it on the
     * first external display). While the desktop holds the mouse, KWin draws
     * the only cursor here: this window shows Android's empty icon, and while
     * [AndroidCursorShield] arms, the mouse events Android delivers here are
     * the shield's.
     */
    private class ArrowShield(context: Context) : FrameLayout(context) {
        private val empty = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            PointerIcon.getSystemIcon(context, PointerIcon.TYPE_NULL)
        } else null

        private fun shielding(): Boolean = Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE &&
            AndroidCursorShield.hidesArrow()

        override fun onResolvePointerIcon(event: MotionEvent, pointerIndex: Int): PointerIcon? =
            if (shielding()) empty else super.onResolvePointerIcon(event, pointerIndex)

        override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean =
            offer(event) || super.dispatchGenericMotionEvent(event)

        override fun dispatchTouchEvent(event: MotionEvent): Boolean =
            offer(event) || super.dispatchTouchEvent(event)

        private fun offer(event: MotionEvent): Boolean = Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE &&
            event.isFromSource(android.view.InputDevice.SOURCE_MOUSE) && shielding() &&
            AndroidCursorShield.offerToActive(event, "the desktop on display ${display?.displayId}")
    }
}
