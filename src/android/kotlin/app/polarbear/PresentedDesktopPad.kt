package app.polarbear

import android.os.Build
import android.os.SystemClock
import android.util.TypedValue
import android.view.Display
import android.view.Gravity
import android.view.MotionEvent
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.Text
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.AppearanceMode
import app.polarbear.setup.PortalTheme
import app.polarbear.setup.components.PortalUiDensity
import app.polarbear.setup.resolvePalette

/**
 * The phone's screen while the desktop is shown on a monitor through
 * [DesktopPresentation]: the whole screen is a touchpad for the desktop's
 * cursor (one finger moves it, a tap clicks, two fingers scroll or pinch, as
 * on a laptop), Control desktop above it hands over a mouse and keyboard, and
 * the button below brings the desktop back to this screen.
 */
internal class PresentedDesktopPad(private val host: PortalActivity, monitor: String) : FrameLayout(host) {
    private var timeNs = 0L
    private val heldButtons = mutableSetOf<Int>()
    private val gestures = CapturedTouchpadGestures(
        move = ::move,
        button = ::button,
        scroll = { h, v, finished -> DesktopControlActivity.nativeControlFingerScroll(h, v, finished, timeNs) },
        pinch = { phase, scale -> DesktopControlActivity.nativeControlPinch(phase.ordinal, scale, timeNs) },
        screenUnitsPerMm = resources.displayMetrics.let { (it.xdpi + it.ydpi) / 2f } / MM_PER_INCH,
    )

    // A tap holds its press briefly, waiting for a drag or double click.
    private val gestureTimer = Runnable {
        val now = SystemClock.uptimeMillis()
        timeNs = now * 1_000_000L
        gestures.timeout(now)
        scheduleGestureTimer()
    }

    init {
        addView(
            ComposeView(host).apply { setContent { PadContent(monitor) } },
            LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT),
        )
        addView(
            DesktopControlView(host, host, holdsGrab = true, close = {}),
            LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.TOP or Gravity.CENTER_HORIZONTAL).apply { topMargin = dp(36f) },
        )
    }

    @androidx.compose.runtime.Composable
    private fun PadContent(monitor: String) {
        PortalTheme {
            PortalUiDensity {
                val palette = resolvePalette(AppearanceMode.System)
                Box(Modifier.fillMaxSize().background(palette.background).padding(horizontal = 28.dp, vertical = 44.dp)) {
                    Column(
                        Modifier.align(Alignment.Center).widthIn(max = 420.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.Center,
                    ) {
                        Text("Touchpad", fontSize = 20.sp, fontWeight = FontWeight.SemiBold, color = palette.textPrimary)
                        Spacer(Modifier.height(8.dp))
                        Text(
                            "The desktop is on $monitor. Move the pointer with one finger, tap to click, " +
                                "and scroll with two fingers.",
                            fontSize = 14.sp, lineHeight = 20.sp, color = palette.textMuted, textAlign = TextAlign.Center,
                        )
                    }
                    Box(Modifier.align(Alignment.BottomCenter)) {
                        QuietButton("Show desktop on this ${deviceNoun(host)}", palette) {
                            host.portalDisplays.moveTo(Display.DEFAULT_DISPLAY)
                        }
                    }
                }
            }
        }
    }

    // Touches the controls don't take land here. Only a finger or pen works
    // the pad: a mouse goes to the desktop through Control desktop.
    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (!DesktopControlView.isDirectTouch(event)) return true
        timeNs = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) event.eventTimeNanos
        else event.eventTime * 1_000_000L
        gestures.dispatch(event)
        scheduleGestureTimer()
        return true
    }

    private fun scheduleGestureTimer() {
        removeCallbacks(gestureTimer)
        val deadline = gestures.nextDeadline
        if (deadline > 0L) postDelayed(gestureTimer, (deadline - SystemClock.uptimeMillis()).coerceAtLeast(0L))
    }

    private fun move(dx: Float, dy: Float) {
        val surface = host.desktopSurface()?.takeIf {
            ComposeOverlay.desktopIsReady() && it.width > 1 && it.height > 1
        } ?: return
        DesktopCursor.move(host, surface, dx, dy, timeNs)
    }

    private fun button(code: Int, pressed: Boolean) {
        if (if (pressed) heldButtons.add(code) else heldButtons.remove(code)) {
            DesktopControlActivity.nativeControlButton(code, pressed, timeNs)
        }
    }

    override fun onDetachedFromWindow() {
        removeCallbacks(gestureTimer)
        gestures.reset()
        for (code in heldButtons) DesktopControlActivity.nativeControlButton(code, false, 0L)
        heldButtons.clear()
        super.onDetachedFromWindow()
    }

    private fun dp(value: Float): Int =
        TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, value, resources.displayMetrics).toInt()

    private companion object {
        const val MM_PER_INCH = 25.4f
    }
}
