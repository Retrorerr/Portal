package app.polarbear

import android.os.Build
import android.view.MotionEvent
import androidx.annotation.RequiresApi
import app.polarbear.TouchpadGestures.PinchPhase

/**
 * The mouse and touchpad as Android's own recognizer reports them, through
 * Portal's accessibility service while a desktop on another display holds
 * them: taps, tap-drags and clicks arrive as buttons, pointer motion as
 * relative deltas, two-finger swipes and pinches as classified gesture
 * streams. They are read as the desktop reads them on the tablet itself, so
 * the gestures, speed and scroll direction set in Android apply unchanged.
 */
@RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
internal class RoutedDesktopPointer(
    private val motion: (Float, Float) -> Unit,
    private val button: (Int, Boolean) -> Unit,
    private val wheel: (Float, Float) -> Unit,
    private val fingerScroll: (Float, Float, Boolean) -> Unit,
    private val pinch: (PinchPhase, Float) -> Unit,
) {
    private var scrolling = false
    private var pinchScale = Float.NaN

    fun reset() {
        endScroll()
        endPinch(PinchPhase.Cancel)
    }

    fun dispatch(event: MotionEvent) {
        val action = event.actionMasked
        when (event.classification) {
            MotionEvent.CLASSIFICATION_TWO_FINGER_SWIPE -> {
                endPinch(PinchPhase.End)
                forEachSample(event) { sample ->
                    val h = axis(event, MotionEvent.AXIS_GESTURE_SCROLL_X_DISTANCE, sample)
                    val v = axis(event, MotionEvent.AXIS_GESTURE_SCROLL_Y_DISTANCE, sample)
                    if (h != 0f || v != 0f) {
                        scrolling = true
                        fingerScroll(h, v, false)
                    }
                }
                if (action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_CANCEL) endScroll()
                return
            }
            MotionEvent.CLASSIFICATION_PINCH -> {
                endScroll()
                when (action) {
                    MotionEvent.ACTION_MOVE -> if (event.pointerCount == 2) {
                        var factor = 1f
                        forEachSample(event) { sample ->
                            val step = axis(event, MotionEvent.AXIS_GESTURE_PINCH_SCALE_FACTOR, sample)
                            if (step.isFinite() && step > 0f) factor *= step
                        }
                        if (pinchScale.isNaN()) {
                            pinchScale = 1f
                            pinch(PinchPhase.Begin, 1f)
                        }
                        pinchScale = (pinchScale * factor).coerceIn(0.05f, 20f)
                        pinch(PinchPhase.Update, pinchScale)
                    }
                    MotionEvent.ACTION_POINTER_UP, MotionEvent.ACTION_UP -> endPinch(PinchPhase.End)
                    MotionEvent.ACTION_CANCEL -> endPinch(PinchPhase.Cancel)
                }
                return
            }
            // Three- and four-finger swipes have no desktop counterpart on
            // this path; they are kept from the tablet's apps all the same.
            CLASSIFICATION_MULTI_FINGER_SWIPE -> return
        }
        endScroll()
        endPinch(PinchPhase.End)
        when (action) {
            MotionEvent.ACTION_BUTTON_PRESS, MotionEvent.ACTION_BUTTON_RELEASE ->
                desktopButton(event.actionButton)?.let { button(it, action == MotionEvent.ACTION_BUTTON_PRESS) }
            MotionEvent.ACTION_SCROLL ->
                wheel(event.getAxisValue(MotionEvent.AXIS_HSCROLL), event.getAxisValue(MotionEvent.AXIS_VSCROLL))
            MotionEvent.ACTION_HOVER_MOVE, MotionEvent.ACTION_MOVE -> {
                // Relative axes are per sample; Android's own cursor stops at
                // the tablet's edges, these keep counting.
                var dx = 0f
                var dy = 0f
                forEachSample(event) { sample ->
                    dx += axis(event, MotionEvent.AXIS_RELATIVE_X, sample)
                    dy += axis(event, MotionEvent.AXIS_RELATIVE_Y, sample)
                }
                if ((dx != 0f || dy != 0f) && dx.isFinite() && dy.isFinite()) motion(dx, dy)
            }
        }
    }

    private fun endScroll() {
        if (!scrolling) return
        scrolling = false
        fingerScroll(0f, 0f, true)
    }

    private fun endPinch(phase: PinchPhase) {
        if (pinchScale.isNaN()) return
        pinch(phase, pinchScale)
        pinchScale = Float.NaN
    }

    /** Historical samples oldest first, then the current one (-1). */
    private inline fun forEachSample(event: MotionEvent, block: (Int) -> Unit) {
        for (sample in 0 until event.historySize) block(sample)
        block(-1)
    }

    private fun axis(event: MotionEvent, axis: Int, sample: Int): Float =
        if (sample < 0) event.getAxisValue(axis) else event.getHistoricalAxisValue(axis, sample)

    private companion object {
        /** MotionEvent.CLASSIFICATION_MULTI_FINGER_SWIPE (Android 14), absent from the SDK stubs. */
        const val CLASSIFICATION_MULTI_FINGER_SWIPE = 4
    }
}
