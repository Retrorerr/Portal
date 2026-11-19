package app.polarbear

import android.app.Presentation
import android.graphics.Color
import android.os.Bundle
import android.view.Display
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
            FrameLayout(context).apply {
                setBackgroundColor(Color.BLACK)
                addView(surfaceView, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
            },
        )
        // Also when Android removes the monitor or cancels the window.
        setOnDismissListener { ended(this) }
    }
}
