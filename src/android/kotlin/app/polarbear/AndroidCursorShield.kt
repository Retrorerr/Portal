package app.polarbear

import android.content.Context
import android.graphics.PixelFormat
import android.os.Build
import android.os.SystemClock
import android.util.Log
import android.view.InputDevice
import android.view.MotionEvent
import android.view.PointerIcon
import android.view.View
import android.view.WindowManager
import android.view.WindowManager.LayoutParams
import androidx.annotation.RequiresApi

/**
 * Hides Android's own arrow on the tablet while a desktop on another display
 * holds the mouse and touchpad through Portal's accessibility service.
 *
 * Android moves its arrow before the service takes the events, and only the
 * window under the arrow may change its icon. So on the first plain motion of
 * a grab the service lets the mouse through for a moment, onto this invisible
 * window over the whole tablet: its empty icon hides the arrow, and what it
 * receives meanwhile goes to the desktop as the service's events do. Then the
 * service takes the mouse again and the window lets touch through. With no
 * mouse events reaching any window, Android keeps the empty icon until the
 * grab ends and this window goes; the next window the arrow enters sets its
 * own icon again.
 *
 * Android may keep its arrow on a monitor rather than the tablet (OxygenOS
 * puts it on the first external display, which mirrors the tablet until an
 * app shows something there). Over a mirror this window still gets the
 * event, but over the desktop's own Presentation that window is the one
 * under the arrow: it hands its events here through [offer] while arming and
 * shows the empty icon for the rest of the grab ([hidesArrow]). Where no
 * Portal window is under the arrow, tries soon stop: each hands the mouse to
 * Android for a moment, which would make the desktop cursor stutter.
 */
@RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
internal class AndroidCursorShield(context: Context, private val forward: (MotionEvent) -> Unit) {
    private val manager = context.getSystemService(WindowManager::class.java)
    private val empty = PointerIcon.getSystemIcon(context, PointerIcon.TYPE_NULL)
    private val view = object : View(context) {
        override fun onResolvePointerIcon(event: MotionEvent, pointerIndex: Int): PointerIcon {
            if (arming) Log.i(TAG, "Empty icon requested")
            return empty
        }

        override fun dispatchTouchEvent(event: MotionEvent): Boolean = take(event)

        override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean = take(event)
    }
    private val layout = LayoutParams(
        LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT, LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
        LayoutParams.FLAG_NOT_FOCUSABLE or LayoutParams.FLAG_NOT_TOUCHABLE or
            LayoutParams.FLAG_LAYOUT_IN_SCREEN or LayoutParams.FLAG_LAYOUT_NO_LIMITS,
        PixelFormat.TRANSPARENT,
    ).apply {
        layoutInDisplayCutoutMode = LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS
        fitInsetsTypes = 0
        title = "Portal cursor shield"
    }
    private val giveUp = Runnable {
        if (arming) {
            // No Portal window under the arrow: every try hands the mouse to
            // Android for a moment, so they are spaced out and soon stop.
            failures++
            failedAt = SystemClock.uptimeMillis()
            Log.i(TAG, "No mouse event reached the shield (try $failures of $MAX_TRIES)")
        }
        takeBack()
    }

    /** Tries this grab that found no Portal window under the arrow. */
    private var failures = 0
    private var failedAt = 0L
    private var added = false
    private var arming = false

    /** Android's arrow is hidden. */
    var hidden = false
        private set

    fun show(): Boolean {
        try {
            manager.addView(view, layout)
            added = true
            active = this
        } catch (error: RuntimeException) {
            Log.w(TAG, "Could not add the cursor shield", error)
        }
        return added
    }

    /** A routed event: the first plain motion while the arrow shows lets the mouse through onto this window. */
    fun routed(event: MotionEvent) {
        if (!added || hidden || arming) return
        if (event.actionMasked != MotionEvent.ACTION_HOVER_MOVE || event.buttonState != 0) return
        if (failures >= MAX_TRIES || failures > 0 && SystemClock.uptimeMillis() - failedAt < RETRY_DELAY_MS) return
        arming = true
        setTouchable(true)
        KeyboardAccessibilityService.routeMotion(false)
        view.postDelayed(giveUp, ARMING_TIMEOUT_MS)
        Log.i(TAG, "Letting the mouse through")
    }

    /**
     * The window under Android's arrow when it isn't this one, such as the
     * desktop's Presentation on the monitor the arrow is on: while arming, its
     * events are this window's. True when the event was taken.
     */
    fun offer(event: MotionEvent, from: String): Boolean = arming && take(event, from)

    private fun take(event: MotionEvent, from: String = "the shield"): Boolean {
        if (!arming) return false
        if (event.isFromSource(InputDevice.SOURCE_MOUSE)) {
            forward(event)
            if (!hidden) {
                hidden = true
                Log.i(TAG, "Mouse reached $from (action ${event.actionMasked}, source 0x${Integer.toHexString(event.source)})")
                // After this event, so Android applies the empty icon first.
                view.post(::takeBack)
            }
        } else if (event.actionMasked == MotionEvent.ACTION_DOWN) {
            // A finger in the meantime: Android gets the touchscreen back at
            // once, and the next plain motion tries again.
            Log.i(TAG, "Touch reached the shield")
            takeBack()
        }
        return true
    }

    private fun takeBack() {
        if (!arming) return
        arming = false
        view.removeCallbacks(giveUp)
        setTouchable(false)
        KeyboardAccessibilityService.routeMotion(true)
        Log.i(TAG, if (hidden) "Arrow hidden, mouse taken again" else "Mouse taken again, arrow still shown")
    }

    private fun setTouchable(touchable: Boolean) {
        val flags = if (touchable) layout.flags and LayoutParams.FLAG_NOT_TOUCHABLE.inv()
        else layout.flags or LayoutParams.FLAG_NOT_TOUCHABLE
        if (flags == layout.flags || !added) return
        layout.flags = flags
        try {
            manager.updateViewLayout(view, layout)
        } catch (error: RuntimeException) {
            Log.w(TAG, "Cursor shield was removed", error)
            added = false
        }
    }

    /** The grab ended: the window goes, and the arrow's next window shows it again. */
    fun close() {
        arming = false
        view.removeCallbacks(giveUp)
        if (active === this) active = null
        if (!added) return
        added = false
        try {
            manager.removeView(view)
        } catch (error: IllegalArgumentException) {
            Log.d(TAG, "Cursor shield was already removed", error)
        }
        Log.i(TAG, "Closed")
    }

    companion object {
        private const val TAG = "PortalCursorShield"
        private const val ARMING_TIMEOUT_MS = 250L
        private const val MAX_TRIES = 3
        private const val RETRY_DELAY_MS = 2000L

        /** The shield of the routed grab on now, UI thread only. */
        private var active: AndroidCursorShield? = null

        /** Windows Android's arrow may rest on show the empty icon while a routed grab holds the mouse. */
        fun hidesArrow(): Boolean = active != null

        /** See [offer]; false without a routed grab. */
        fun offerToActive(event: MotionEvent, from: String): Boolean = active?.offer(event, from) ?: false
    }
}
