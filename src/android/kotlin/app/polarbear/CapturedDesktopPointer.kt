package app.polarbear

import android.view.InputDevice
import android.view.MotionEvent

/** Decodes Android's two pointer-capture sources for the desktop and control windows. */
internal class CapturedDesktopPointer(
    private val motion: (Float, Float) -> Unit,
    private val button: (Int, Boolean) -> Unit,
    private val wheel: (Float, Float) -> Unit,
    private val touchpad: CapturedTouchpadGestures? = null,
) {
    private var padX = Float.NaN
    private var padY = Float.NaN

    fun reset() {
        padX = Float.NaN
        padY = Float.NaN
        touchpad?.reset()
    }

    fun dispatch(event: MotionEvent): Boolean {
        val mouse = event.isFromSource(InputDevice.SOURCE_MOUSE_RELATIVE)
        if (!mouse && !event.isFromSource(InputDevice.SOURCE_TOUCHPAD)) return false
        when (event.actionMasked) {
            MotionEvent.ACTION_BUTTON_PRESS, MotionEvent.ACTION_BUTTON_RELEASE -> {
                val code = desktopButton(event.actionButton) ?: return true
                val pressed = event.actionMasked == MotionEvent.ACTION_BUTTON_PRESS
                if (!mouse) touchpad?.physicalButton(code, pressed)
                button(code, pressed)
            }
            MotionEvent.ACTION_SCROLL -> wheel(
                event.getAxisValue(MotionEvent.AXIS_HSCROLL),
                event.getAxisValue(MotionEvent.AXIS_VSCROLL),
            )
            else -> if (!mouse && touchpad != null) {
                touchpad.dispatch(event)
            } else dispatchMotion(event, mouse)
        }
        return true
    }

    private fun dispatchMotion(event: MotionEvent, mouse: Boolean) {
        when (event.actionMasked) {
            MotionEvent.ACTION_MOVE, MotionEvent.ACTION_HOVER_MOVE -> when {
                mouse -> {
                    var dx = event.x
                    var dy = event.y
                    for (i in 0 until event.historySize) {
                        dx += event.getHistoricalX(i)
                        dy += event.getHistoricalY(i)
                    }
                    if (dx != 0f || dy != 0f) motion(dx, dy)
                }
                event.pointerCount == 1 -> {
                    if (!padX.isNaN()) motion(event.x - padX, event.y - padY)
                    padX = event.x
                    padY = event.y
                }
                else -> reset()
            }
            MotionEvent.ACTION_DOWN -> {
                padX = event.x
                padY = event.y
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> reset()
        }
    }
}

/** The Linux button code for an Android mouse button. */
internal fun desktopButton(androidButton: Int): Int? = when (androidButton) {
    MotionEvent.BUTTON_PRIMARY -> 0x110
    MotionEvent.BUTTON_SECONDARY -> 0x111
    MotionEvent.BUTTON_TERTIARY -> 0x112
    MotionEvent.BUTTON_BACK -> 0x113
    MotionEvent.BUTTON_FORWARD -> 0x114
    else -> null
}
