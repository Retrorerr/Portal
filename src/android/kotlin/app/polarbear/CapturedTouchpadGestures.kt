package app.polarbear

import android.os.Build
import android.view.InputDevice
import android.view.MotionEvent
import app.polarbear.TouchpadGestures.Point

/**
 * Android's raw pointer capture bypasses its touchpad gesture recognizer. This
 * feeds those contacts to [TouchpadGestures] in millimetres, so tap slop and
 * motion stay consistent across touchpads. Mouse capture still uses Android's
 * processed events. A touchscreen used as the touchpad gives
 * [screenUnitsPerMm], its pixels per millimetre.
 */
internal class CapturedTouchpadGestures(
    move: (Float, Float) -> Unit,
    button: (Int, Boolean) -> Unit,
    scroll: (Float, Float, Boolean) -> Unit,
    pinch: (TouchpadGestures.PinchPhase, Float) -> Unit,
    private val screenUnitsPerMm: Float = 0f,
) {
    private val gestures = TouchpadGestures(move, button, scroll, pinch)
    // Null until a contact arrives: injected input (adb, accessibility) has device id -1.
    private var primaryDevice: Int? = null
    private var xUnitsPerMm = 1f
    private var yUnitsPerMm = 1f

    /** Uptime when [timeout] must run next, or 0. */
    val nextDeadline: Long get() = gestures.nextDeadline

    fun timeout(now: Long) = gestures.timeout(now)

    fun startCapture(now: Long) {
        primaryDevice = null
        gestures.startCapture(now)
    }

    fun reset() {
        primaryDevice = null
        gestures.reset()
    }

    fun physicalButton(code: Int, pressed: Boolean) = gestures.physicalButton(code, pressed)

    fun dispatch(event: MotionEvent) {
        if (event.actionMasked != MotionEvent.ACTION_DOWN && event.deviceId != primaryDevice) return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            event.flags and MotionEvent.FLAG_CANCELED != 0
        ) {
            gestures.cancel()
            return
        }
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                if (primaryDevice != event.deviceId) {
                    primaryDevice = event.deviceId
                    if (screenUnitsPerMm > 0f && event.isFromSource(InputDevice.SOURCE_TOUCHSCREEN)) {
                        xUnitsPerMm = screenUnitsPerMm
                        yUnitsPerMm = screenUnitsPerMm
                    } else {
                        xUnitsPerMm = unitsPerMm(event.device, MotionEvent.AXIS_X, 110f)
                        yUnitsPerMm = unitsPerMm(event.device, MotionEvent.AXIS_Y, 70f)
                    }
                }
                gestures.down(event.eventTime, position(event, 0), event.buttonState != 0)
            }
            MotionEvent.ACTION_POINTER_DOWN ->
                if (event.pointerCount == 2) {
                    gestures.secondFinger(event.eventTime, position(event, 0), position(event, 1))
                } else {
                    gestures.extraFinger()
                }
            MotionEvent.ACTION_MOVE -> {
                for (sample in 0 until event.historySize) track(event, sample)
                track(event, null)
            }
            MotionEvent.ACTION_POINTER_UP ->
                if (event.pointerCount == 2) {
                    gestures.fingerLifted(position(event, 1 - event.actionIndex))
                } else {
                    gestures.extraFinger()
                }
            MotionEvent.ACTION_UP -> gestures.up(event.eventTime)
            MotionEvent.ACTION_CANCEL -> gestures.cancel()
        }
    }

    private fun track(event: MotionEvent, history: Int?) {
        val time = if (history != null) event.getHistoricalEventTime(history) else event.eventTime
        when (event.pointerCount) {
            1 -> gestures.moveOne(position(event, 0, history))
            2 -> gestures.moveTwo(time, position(event, 0, history), position(event, 1, history))
        }
    }

    private fun position(event: MotionEvent, index: Int, history: Int? = null): Point {
        val x = if (history != null) event.getHistoricalX(index, history) else event.getX(index)
        val y = if (history != null) event.getHistoricalY(index, history) else event.getY(index)
        return Point(x / xUnitsPerMm, y / yUnitsPerMm)
    }

    private fun unitsPerMm(device: InputDevice?, axis: Int, fallbackMm: Float): Float {
        val range = device?.getMotionRange(axis, InputDevice.SOURCE_TOUCHPAD) ?: return 1f
        return when {
            range.resolution > 0f -> range.resolution
            range.range > 0f -> range.range / fallbackMm
            else -> 1f
        }
    }
}
