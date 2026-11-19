package app.polarbear

import android.content.Context
import android.os.Build
import android.os.SystemClock
import android.util.Log
import android.view.Display
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.SurfaceView
import android.widget.FrameLayout
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.AppearanceMode
import app.polarbear.setup.PortalTheme
import app.polarbear.setup.components.PortalUiDensity
import app.polarbear.setup.resolvePalette

/**
 * The tablet controls for a desktop on another display.
 *
 * With Portal's accessibility service on (Android 14+), Control desktop takes
 * the mouse, touchpad and keyboard through it: no window focus is involved, so
 * the touchscreen keeps working for Android apps, and Android's own touchpad
 * gestures apply. Otherwise [DesktopControlActivity] ([holdsGrab]) takes
 * Android pointer capture, which Android ends as soon as another app is used;
 * the floating handle then only starts that activity.
 *
 * The desktop cursor is tracked here in the desktop surface's pixels. Only a
 * deliberate act hands control back: Ctrl+Alt, or a finger or pen on Release.
 */
internal class DesktopControlView(
    context: Context,
    private val host: PortalActivity,
    private val holdsGrab: Boolean,
    private val close: () -> Unit,
) : FrameLayout(context) {
    private var captured by mutableStateOf(false)
    private var waiting by mutableStateOf(false)
    private var message by mutableStateOf<String?>(null)
    private var releaseHint by mutableStateOf<String?>(null)
    private var grabbedAt = 0L
    /** The grab comes through the accessibility service rather than pointer capture. */
    private var routed = false
    private val heldKeys = mutableSetOf<Pair<Int, Int>>()
    private val heldButtons = mutableSetOf<Int>()
    /** Ctrl and Alt are down together with nothing else pressed since. */
    private var releaseChord = false
    private var timeNs = 0L
    private val captureTimeout = Runnable {
        if (waiting) {
            relinquish("capture timeout")
            message = "Couldn’t take the pointer. Try again."
        }
    }
    private val touchpad = CapturedTouchpadGestures(
        move = ::move,
        button = ::button,
        scroll = { h, v, finished -> DesktopControlActivity.nativeControlFingerScroll(h, v, finished, timeNs) },
        pinch = { phase, scale -> DesktopControlActivity.nativeControlPinch(phase.ordinal, scale, timeNs) },
    )
    private val pointer = CapturedDesktopPointer(
        ::move, ::button, { h, v -> DesktopControlActivity.nativeControlWheel(h, v, timeNs) }, touchpad,
    )
    private val routedPointer = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
        RoutedDesktopPointer(
            ::move, ::button,
            wheel = { h, v -> DesktopControlActivity.nativeControlWheel(h, v, timeNs) },
            fingerScroll = { h, v, finished -> DesktopControlActivity.nativeControlFingerScroll(h, v, finished, timeNs) },
            pinch = { phase, scale -> DesktopControlActivity.nativeControlPinch(phase.ordinal, scale, timeNs) },
        )
    } else null
    private val routedInput = object : TabletGrab.Routed {
        override fun motion(event: MotionEvent) {
            if (!routedMotion(event)) return
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) shield?.routed(event)
        }

        override fun key(event: KeyEvent): Boolean = captured && routed && grabKey(event)

        override fun lost(reason: String) = release(reason)
    }

    /** Hides Android's arrow during a routed grab. */
    private var shield: AndroidCursorShield? = null

    /** Holds the desktop's input without needing this window's focus. */
    val holdsRoutedGrab: Boolean get() = captured && routed

    private fun routedMotion(event: MotionEvent): Boolean {
        if (!captured || !routed) return false
        timeNs = eventTimeNs(event)
        routedPointer?.dispatch(event)
        return true
    }

    // A tap holds its press briefly, waiting for a drag or double click.
    private val touchpadTimer = Runnable {
        val now = SystemClock.uptimeMillis()
        timeNs = now * 1_000_000L
        touchpad.timeout(now)
        scheduleTouchpadTimer()
    }

    init {
        isFocusableInTouchMode = holdsGrab
        addView(controls(), LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT))
    }

    private fun controls() = ComposeView(context).apply {
        setContent {
            PortalTheme {
                PortalUiDensity {
                    val palette = resolvePalette(AppearanceMode.System)
                    val shape = RoundedCornerShape(26.dp)
                    Column(
                        Modifier.padding(10.dp).widthIn(max = 360.dp)
                            .shadow(10.dp, shape, clip = false)
                            .background(palette.background, shape)
                            .border(1.dp, palette.surfaceBorder, shape)
                            .padding(5.dp),
                    ) {
                        Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                            // While Android grants the grab the button stays as it
                            // was: a second click from the same touchpad press
                            // must not undo it.
                            QuietButton(if (captured) "Release" else "Control desktop", palette, accent = true,
                                animateLabel = false, onClick = ::primaryAction)
                            if (captured) QuietButton("Return to ${deviceNoun(context)}", palette, animateLabel = false,
                                onClick = ::returnToTablet)
                        }
                        val note = message ?: releaseHint.takeIf { captured }
                        if (note != null) {
                            Text(note, Modifier.padding(start = 12.dp, end = 12.dp, top = 6.dp, bottom = 5.dp),
                                color = palette.textMuted, fontSize = 12.sp, lineHeight = 16.sp)
                        }
                    }
                }
            }
        }
    }

    private fun primaryAction() {
        when {
            waiting -> Unit
            captured -> if (SystemClock.uptimeMillis() - grabbedAt >= CLICK_GUARD_MS) release("button")
            else -> takeControl()
        }
    }

    private fun returnToTablet() {
        relinquish("return")
        if (host.portalDisplays.moveTo(Display.DEFAULT_DISPLAY) && holdsGrab) close()
    }

    override fun onWindowFocusChanged(hasWindowFocus: Boolean) {
        super.onWindowFocusChanged(hasWindowFocus)
        if (hasWindowFocus) requestCaptureWhenFocused()
        else if (captured && !routed) release("focus left")
    }

    /**
     * A finger or pen on the tablet outside the controls hands a captured
     * grab back to Android, which would end it anyway. A routed grab stays.
     */
    fun outsideTouch(event: MotionEvent) {
        if (!captured && !waiting) {
            close()
        } else if (!routed && isDirectTouch(event)) {
            release("touch outside, source 0x${Integer.toHexString(event.source)} tool ${event.getToolType(0)}")
        }
    }

    // While the desktop holds the pointer only a finger or pen works these
    // controls: no click of the mouse or touchpad may hand it back.
    override fun dispatchTouchEvent(event: MotionEvent): Boolean =
        if (captured && !isDirectTouch(event)) true else super.dispatchTouchEvent(event)

    // Captured events follow the focus path; taking them here keeps them
    // even when the Compose child holds focus.
    override fun dispatchCapturedPointerEvent(event: MotionEvent): Boolean =
        dispatchCaptured(event) || super.dispatchCapturedPointerEvent(event)

    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean =
        dispatchCaptured(event) || super.dispatchGenericMotionEvent(event)

    override fun dispatchTrackballEvent(event: MotionEvent): Boolean =
        dispatchCaptured(event) || super.dispatchTrackballEvent(event)

    private fun dispatchCaptured(event: MotionEvent): Boolean {
        if (!captured || routed) return false
        timeNs = eventTimeNs(event)
        val handled = pointer.dispatch(event)
        scheduleTouchpadTimer()
        return handled
    }

    private fun scheduleTouchpadTimer() {
        removeCallbacks(touchpadTimer)
        val deadline = touchpad.nextDeadline
        if (captured && deadline > 0L) postDelayed(touchpadTimer, (deadline - SystemClock.uptimeMillis()).coerceAtLeast(0L))
    }

    override fun onPointerCaptureChange(hasCapture: Boolean) {
        super.onPointerCaptureChange(hasCapture)
        if (!hasCapture) {
            if (captured) release("capture lost")
            return
        }
        val surface = readySurface()
        if (!waiting || routed || !hasWindowFocus() || surface == null) {
            release("desktop unavailable")
            return
        }
        beginGrab(surface)
    }

    private fun beginGrab(surface: SurfaceView) {
        removeCallbacks(captureTimeout)
        grabbedAt = SystemClock.uptimeMillis()
        releaseHint = if (host.portalDisplays.takeReleaseHint()) "Ctrl+Alt hands control back" else null
        waiting = false
        captured = true
        TabletGrab.active = true
        DesktopCursor.fit(surface.width, surface.height)
        touchpad.startCapture(SystemClock.uptimeMillis())
        timeNs = 0L
        if (!host.pointerCaptureWanted) DesktopControlActivity.nativeControlMotion(DesktopCursor.x, DesktopCursor.y, timeNs)
        val via = if (routed) "accessibility" else "pointer capture"
        Log.i(TAG, "Grabbed input ($via) for display ${host.desktopDisplay()?.displayId} at ${surface.width}x${surface.height}")
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean =
        captured && !routed && grabKey(event) || super.dispatchKeyEvent(event)

    /** Sends a key to the desktop; false leaves it to Android (power and volume). */
    private fun grabKey(event: KeyEvent): Boolean {
        when (event.keyCode) {
            KeyEvent.KEYCODE_POWER, KeyEvent.KEYCODE_VOLUME_UP, KeyEvent.KEYCODE_VOLUME_DOWN,
            KeyEvent.KEYCODE_VOLUME_MUTE -> return false
        }
        val down = when (event.action) {
            KeyEvent.ACTION_DOWN -> true
            KeyEvent.ACTION_UP -> false
            else -> return true
        }
        if (KeyboardAccessibilityService.isMouseSideButton(event)) {
            if (event.repeatCount == 0) button(if (event.keyCode == KeyEvent.KEYCODE_BACK) BUTTON_BACK else BUTTON_FORWARD, down)
            return true
        }
        val key = event.keyCode to event.scanCode
        if (down) {
            // Plasma repeats held keys itself.
            if (event.repeatCount > 0 || !heldKeys.add(key)) return true
            DesktopControlActivity.nativeControlKey(key.first, key.second, true)
            // A bare Ctrl+Alt tap releases control; Ctrl+Alt+T and the like
            // still reach Plasma because their third key disarms it.
            releaseChord = heldKeys.all { isCtrl(it.first) || isAlt(it.first) } &&
                heldKeys.any { isCtrl(it.first) } && heldKeys.any { isAlt(it.first) }
        } else {
            if (releaseChord && (isCtrl(event.keyCode) || isAlt(event.keyCode))) {
                release("Ctrl+Alt")
                return true
            }
            if (heldKeys.remove(key)) DesktopControlActivity.nativeControlKey(key.first, key.second, false)
            if (heldKeys.isEmpty()) releaseChord = false
        }
        return true
    }

    private fun readySurface(): SurfaceView? = host.desktopSurface()?.takeIf {
        ComposeOverlay.desktopIsReady() && !ComposeOverlay.isShowing() && !SessionControls.isUtilityVisible() &&
            it.width > 1 && it.height > 1 && it.holder.surface?.isValid == true
    }

    fun takeControl() {
        if (captured || waiting) return
        if (readySurface() == null) {
            message = "The desktop isn’t ready on the monitor yet."
            return
        }
        message = null
        if (takeRoutedControl()) return
        if (!holdsGrab) {
            DesktopControlActivity.startControl(host)
            return
        }
        waiting = true
        requestCaptureWhenFocused()
        postDelayed(captureTimeout, CAPTURE_TIMEOUT_MS)
    }

    /** Takes the input through Portal's accessibility service; false when it can't. */
    fun takeRoutedControl(): Boolean {
        if (captured || waiting) return holdsRoutedGrab
        if (!KeyboardAccessibilityService.canRouteMotion()) return false
        val surface = readySurface() ?: return false
        TabletGrab.routed?.lost("another control took over")
        TabletGrab.routed = routedInput
        if (!KeyboardAccessibilityService.routeMotion(true)) {
            TabletGrab.routed = null
            return false
        }
        routed = true
        message = null
        shield = KeyboardAccessibilityService.overlayContext()?.let { service ->
            AndroidCursorShield(service) { routedMotion(it) }.takeIf { it.show() }
        }
        beginGrab(surface)
        return true
    }

    private fun eventTimeNs(event: MotionEvent): Long =
        // Android's event clock is CLOCK_MONOTONIC, the clock KWin's input times use.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) event.eventTimeNanos
        else event.eventTime * 1_000_000L

    private fun requestCaptureWhenFocused() {
        if (!waiting || routed || !hasWindowFocus() || hasPointerCapture()) return
        // Captured events and keys reach the focused view's ancestors only.
        if (!hasFocus()) requestFocus()
        requestPointerCapture()
    }

    /** End a grab and close the controller. */
    fun release(reason: String) {
        if (relinquish(reason) && holdsGrab) close()
    }

    /** Returns true when a grab or pending grab was active. */
    private fun relinquish(reason: String): Boolean {
        val active = captured || waiting || hasPointerCapture()
        if (!active) return false
        val state = if (captured) "grab" else "pending grab"
        val heldMs = if (captured) SystemClock.uptimeMillis() - grabbedAt else 0L
        Log.i(TAG, "Released $state ($reason) after $heldMs ms")
        captured = false
        waiting = false
        TabletGrab.active = false
        removeCallbacks(captureTimeout)
        removeCallbacks(touchpadTimer)
        if (hasPointerCapture()) releasePointerCapture()
        if (routed) {
            routed = false
            shield?.close()
            shield = null
            if (TabletGrab.routed === routedInput) {
                TabletGrab.routed = null
                KeyboardAccessibilityService.routeMotion(false)
            }
            routedPointer?.reset()
        }
        pointer.reset()
        for ((code, scan) in heldKeys) DesktopControlActivity.nativeControlKey(code, scan, false)
        for (code in heldButtons) DesktopControlActivity.nativeControlButton(code, false, 0L)
        heldKeys.clear()
        heldButtons.clear()
        releaseChord = false
        return true
    }

    fun dispose() {
        relinquish("controls closed")
    }

    override fun onDetachedFromWindow() {
        dispose()
        super.onDetachedFromWindow()
    }

    private fun button(code: Int, pressed: Boolean) {
        if (if (pressed) heldButtons.add(code) else heldButtons.remove(code)) {
            DesktopControlActivity.nativeControlButton(code, pressed, timeNs)
        }
    }

    private fun move(dx: Float, dy: Float) {
        val surface = readySurface()
        if (surface == null) {
            release("desktop unavailable")
            return
        }
        DesktopCursor.move(host, surface, dx, dy, timeNs)
    }

    internal companion object {
        const val TAG = "PortalDesktopControl"
        const val CAPTURE_TIMEOUT_MS = 2000L
        const val CLICK_GUARD_MS = 500L
        const val BUTTON_BACK = 0x113
        const val BUTTON_FORWARD = 0x114

        fun isCtrl(code: Int) = code == KeyEvent.KEYCODE_CTRL_LEFT || code == KeyEvent.KEYCODE_CTRL_RIGHT
        fun isAlt(code: Int) = code == KeyEvent.KEYCODE_ALT_LEFT || code == KeyEvent.KEYCODE_ALT_RIGHT

        /**
         * A finger or pen on the screen itself. Sources are bit sets: Samsung
         * DeX reports a desktop mouse as mouse | touchscreen, so any mouse or
         * touchpad bit rules the event out whatever else it carries.
         */
        fun isDirectTouch(event: MotionEvent): Boolean {
            if (event.isFromSource(InputDevice.SOURCE_MOUSE) || event.isFromSource(InputDevice.SOURCE_TOUCHPAD) ||
                event.isFromSource(InputDevice.SOURCE_MOUSE_RELATIVE)
            ) return false
            val tool = event.getToolType(0)
            return (tool == MotionEvent.TOOL_TYPE_FINGER || tool == MotionEvent.TOOL_TYPE_STYLUS ||
                tool == MotionEvent.TOOL_TYPE_ERASER) &&
                (event.isFromSource(InputDevice.SOURCE_TOUCHSCREEN) || event.isFromSource(InputDevice.SOURCE_STYLUS))
        }
    }
}

/**
 * The desktop cursor as Portal's own controls place it, in the desktop
 * surface's pixels. Shared by every control so the cursor carries on from
 * where the last one left it. UI thread.
 */
internal object DesktopCursor {
    var x = 0f
        private set
    var y = 0f
        private set
    private var width = 0
    private var height = 0

    /** Keeps the cursor at the same relative place when the desktop's size changes. */
    fun fit(newWidth: Int, newHeight: Int) {
        if (newWidth == width && newHeight == height) return
        x = if (width > 1) x * (newWidth - 1) / (width - 1) else (newWidth - 1) / 2f
        y = if (height > 1) y * (newHeight - 1) / (height - 1) else (newHeight - 1) / 2f
        width = newWidth
        height = newHeight
    }

    fun move(host: PortalActivity, surface: SurfaceView, dx: Float, dy: Float, timeNs: Long) {
        if (host.pointerCaptureWanted) {
            // A guest pointer lock (games) takes raw relative motion.
            host.moveLockedDesktopPointer(dx, dy)
            return
        }
        fit(surface.width, surface.height)
        x = (x + dx).coerceIn(0f, (width - 1).toFloat())
        y = (y + dy).coerceIn(0f, (height - 1).toFloat())
        DesktopControlActivity.nativeControlMotion(x, y, timeNs)
    }
}

/** What Portal calls the device it runs on, for labels such as Return to phone. */
internal fun deviceNoun(context: Context): String =
    if (context.resources.configuration.smallestScreenWidthDp >= 600) "tablet" else "phone"
