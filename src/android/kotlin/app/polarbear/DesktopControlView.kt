package app.polarbear

import android.content.Context
import android.hardware.input.InputManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.view.Display
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.SurfaceView
import android.widget.FrameLayout
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.AppearanceMode
import app.polarbear.setup.PortalPalette
import app.polarbear.setup.PortalTheme
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import app.polarbear.setup.components.PortalGlyph
import app.polarbear.setup.components.PortalIcon
import app.polarbear.setup.components.PortalUiDensity
import app.polarbear.setup.components.openAccessibilitySettings
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
    /** False for a holder whose host draws its own button, such as the phone's touchpad page. */
    private val showControls: Boolean = true,
) : FrameLayout(context) {
    private var captured by mutableStateOf(false)
    private var waiting by mutableStateOf(false)
    private var message by mutableStateOf<String?>(null)
    /** Use mouse found Portal's accessibility service off and asks about it first. Compose-observable. */
    private var askingTouch by mutableStateOf(false)
    private var askedAt = 0L

    /** Asking to turn the accessibility service back on, in [TurnOnTouch]. Compose-observable. */
    val asksAboutTouch: Boolean get() = askingTouch
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

    /**
     * A routed grab another control holds, such as the phone's touchpad page
     * before Home; this one shows it and can release it.
     */
    private val grabbedElsewhere: Boolean get() = TabletGrab.routed.let { it != null && it !== routedInput }

    /** The desktop has the mouse and keyboard, through this control or another. Compose-observable. */
    val grabbed: Boolean get() = captured || grabbedElsewhere

    /** Why the last attempt failed, until the next one. Compose-observable. */
    val failure: String? get() = message

    /** The first few grabs show which keys hand control back. Compose-observable. */
    val showsReleaseKeys: Boolean get() = captured && releaseHint != null

    /** The button's action: take control, or hand it back. */
    fun toggle() = primaryAction()

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
        if (showControls) addView(controls(), LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT))
    }

    /**
     * The touchpad's round buttons on the edge facing the monitor, sliding in
     * from it and back out to it. They only grow inward, away from the edge,
     * so nothing already on screen moves.
     */
    private fun controls() = ComposeView(context).apply {
        setContent {
            PortalTheme {
                PortalUiDensity {
                    val palette = resolvePalette(AppearanceMode.System)
                    // TabletEdge is where this device sits beside the monitor.
                    val atLeft = host.portalDisplays.tabletEdge == TabletEdge.Right
                    val presence = remember { Animatable(0f) }
                    LaunchedEffect(present) {
                        if (present) {
                            presence.animateTo(1f, tween(380, easing = PortalEmphasizedDecelerate))
                        } else {
                            presence.animateTo(0f, tween(200, easing = PortalEmphasizedAccelerate))
                            away = true
                            gone?.let { done -> gone = null; done() }
                        }
                    }
                    // While Android grants the grab the button stays as it was: a
                    // second click from the same touchpad press must not undo it.
                    // Leaving, the controls keep what they last showed: Return
                    // ends the grab a moment before they slide off.
                    val shown = remember { booleanArrayOf(false) }
                    if (present && !returning) shown[0] = captured || grabbedElsewhere
                    val grabbed = shown[0]
                    // A tablet or phone with only its touchscreen has no mouse to hand over.
                    val touchpad = !grabbed && host.presentedDisplay != null && !pointerConnected
                    val device = deviceNoun(context)
                    val failure = message
                    val note = when {
                        askingTouch -> ControlNote.Touch
                        failure != null -> ControlNote.Failure(failure)
                        showsReleaseKeys -> ControlNote.Keys
                        else -> null
                    }
                    // The note keeps what it showed while it fades out.
                    val lastNote = remember { arrayOfNulls<ControlNote>(1) }
                    if (note != null) lastNote[0] = note
                    val parts = listOf<@Composable () -> Unit>(
                        {
                            PadButton(
                                when {
                                    !touchpad -> PortalGlyph.Mouse
                                    device == "tablet" -> PortalGlyph.Tablet
                                    else -> PortalGlyph.Phone
                                },
                                when {
                                    grabbed -> "Release"
                                    touchpad -> "Touchpad"
                                    else -> "Use mouse"
                                },
                                palette, ButtonSize,
                                accent = grabbed,
                                description = when {
                                    grabbed -> "Give the mouse and keyboard back to Android"
                                    touchpad -> "Use this $device as the desktop’s touchpad"
                                    else -> "Use the mouse and keyboard on the desktop"
                                },
                                onClick = ::primaryAction,
                            )
                        },
                        {
                            AnimatedVisibility(
                                grabbed,
                                enter = fadeIn(tween(200, delayMillis = 60)) + scaleIn(tween(320, easing = PortalEmphasizedDecelerate), initialScale = 0.8f),
                                exit = fadeOut(tween(120)) + scaleOut(tween(160, easing = PortalEmphasizedAccelerate), targetScale = 0.8f),
                                label = "return button",
                            ) {
                                PadButton(PortalGlyph.Return, "Return", palette, ButtonSize,
                                    description = "Show the desktop on this ${deviceNoun(context)}", onClick = ::returnToTablet)
                            }
                        },
                        {
                            // It grows out of the buttons and goes back into them.
                            val origin = TransformOrigin(if (atLeft) 0f else 1f, 0.5f)
                            AnimatedVisibility(
                                note != null,
                                enter = fadeIn(tween(220, delayMillis = 80)) +
                                    scaleIn(tween(320, easing = PortalEmphasizedDecelerate), initialScale = 0.9f, transformOrigin = origin),
                                exit = fadeOut(tween(140)) +
                                    scaleOut(tween(160, easing = PortalEmphasizedAccelerate), targetScale = 0.9f, transformOrigin = origin),
                                label = "control note",
                            ) {
                                NoteCard(palette, Modifier.widthIn(max = 240.dp)) {
                                    when (val shown = lastNote[0]) {
                                        ControlNote.Touch -> TurnOnTouch(palette)
                                        ControlNote.Keys -> ReleaseKeys(palette)
                                        is ControlNote.Failure ->
                                            Text(shown.text, color = palette.textMuted, fontSize = 12.sp, lineHeight = 16.sp)
                                        null -> Unit
                                    }
                                }
                            }
                        },
                    )
                    Row(
                        Modifier.padding(12.dp).graphicsLayer {
                            val shown = presence.value
                            alpha = shown
                            translationX = (1f - shown) * (size.width + 12.dp.toPx()) * (if (atLeft) -1f else 1f)
                        },
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        (if (atLeft) parts else parts.asReversed()).forEachIndexed { index, part ->
                            key(if (atLeft) index else parts.lastIndex - index) { part() }
                        }
                    }
                }
            }
        }
    }

    /**
     * Use mouse with Portal's accessibility service off: turn it back on, or go
     * ahead without it. Shown beside the controls' buttons, or by a holder that
     * draws its own, such as the phone's touchpad page.
     */
    @Composable
    internal fun TurnOnTouch(palette: PortalPalette) {
        Column {
            Row {
                PortalIcon(PortalGlyph.Accessibility, palette.textSecondary, Modifier.padding(top = 1.dp, end = 10.dp), size = 18.dp)
                Column {
                    Text(
                        DisplayPermission.Accessibility.offTitle, color = palette.textPrimary,
                        fontSize = 13.sp, lineHeight = 17.sp, fontWeight = FontWeight.SemiBold,
                    )
                    Text(
                        DisplayPermission.Accessibility.offReason(deviceNoun(context)),
                        Modifier.padding(top = 2.dp), color = palette.textMuted, fontSize = 12.sp, lineHeight = 16.sp,
                    )
                }
            }
            Row(Modifier.padding(top = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                SetupButton("Turn on", granted = false, palette, Modifier.weight(1f).height(40.dp), onClick = ::turnOnTouch)
                Box(
                    Modifier.heightIn(min = 40.dp).clip(RoundedCornerShape(50))
                        .clickable(role = Role.Button, onClick = ::goWithoutTouch)
                        .padding(horizontal = 14.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Text("Not now", color = palette.textMuted, fontSize = 13.sp, fontWeight = FontWeight.Medium)
                }
            }
        }
    }

    /** The controls are on screen; [slideOut] takes them off to their edge. Compose-observable. */
    private var present by mutableStateOf(true)
    private var away = false
    private var gone: (() -> Unit)? = null

    fun slideIn() {
        gone = null
        away = false
        present = true
    }

    /** Slides the controls off to their edge, then runs [then]; at once if they are already off. */
    fun slideOut(then: () -> Unit) {
        askingTouch = false
        if (!present && away) {
            then()
            return
        }
        gone = then
        present = false
    }

    private var pointerConnected by mutableStateOf(DesktopControlActivity.externalPointerConnected())
    private val inputManager = if (showControls) context.getSystemService(InputManager::class.java) else null
    private val devices = object : InputManager.InputDeviceListener {
        override fun onInputDeviceAdded(deviceId: Int) = devicesChanged()
        override fun onInputDeviceRemoved(deviceId: Int) = devicesChanged()
        override fun onInputDeviceChanged(deviceId: Int) = devicesChanged()
    }

    private fun devicesChanged() {
        pointerConnected = DesktopControlActivity.externalPointerConnected()
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        inputManager?.registerInputDeviceListener(devices, Handler(Looper.getMainLooper()))
        devicesChanged()
    }

    // Sliding off, the window takes no touches; until then Return's pinned
    // buttons must not grab again.
    private fun primaryAction() {
        when {
            waiting || returning -> Unit
            captured -> if (SystemClock.uptimeMillis() - grabbedAt >= CLICK_GUARD_MS) handBack("button")
            grabbedElsewhere -> {
                TabletGrab.routed?.lost("button")
                handedBack?.invoke()
            }
            // Asked about touch, Use mouse again goes ahead without it.
            askingTouch -> if (SystemClock.uptimeMillis() - askedAt >= CLICK_GUARD_MS) goWithoutTouch()
            else -> takeControl()
        }
    }

    /** Turn on: Android's page for Portal's service, on this screen. Use mouse asks again until it's on. */
    private fun turnOnTouch() {
        askingTouch = false
        openAccessibilitySettings(context, DesktopControlActivity.tabletOptions())
    }

    /** Not now: the grab goes ahead without the service. Portal asks again once it has been on. */
    private fun goWithoutTouch() {
        if (!askingTouch) return
        askingTouch = false
        host.portalDisplays.putOffAsk(DisplayPermission.Accessibility)
        takeControl()
    }

    /** Return was tapped: the controls stay as they are until they slide off. Compose-observable. */
    private var returning by mutableStateOf(false)
    private val returnTimeout = Runnable { returning = false }

    private fun returnToTablet() {
        if (returning) return
        returning = true
        relinquish("return")
        TabletGrab.routed?.lost("return")
        if (!host.portalDisplays.moveTo(Display.DEFAULT_DISPLAY)) {
            returning = false
            return
        }
        if (holdsGrab) close()
        // Should the desktop not leave the monitor after all, show what is true.
        removeCallbacks(returnTimeout)
        postDelayed(returnTimeout, RETURN_TIMEOUT_MS)
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
        if ((captured || grabbedElsewhere) && !isDirectTouch(event)) true else super.dispatchTouchEvent(event)

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
        askingTouch = false
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
                handBack("Ctrl+Alt")
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
        if (captured || waiting || askingTouch) return
        if (readySurface() == null) {
            message = "The desktop isn’t ready on the monitor yet."
            return
        }
        message = null
        if (DesktopControlActivity.openTouchpadInstead(host)) {
            if (host.showsPresentedPad) message = "Connect a mouse or touchpad to hand it to the desktop."
            return
        }
        if (takeRoutedControl()) return
        // Without Portal's accessibility service a touch on this screen ends the
        // grab; switched off since it was last on, the controls ask about it first.
        if (host.portalDisplays.asksFor(DisplayPermission.Accessibility)) {
            askingTouch = true
            askedAt = SystemClock.uptimeMillis()
            return
        }
        if (!holdsGrab) {
            DesktopControlActivity.startControl(host)
            return
        }
        capturePointer()
    }

    /**
     * The touchpad page's own grab of a connected mouse or touchpad: through
     * Portal's accessibility service when it runs, otherwise pointer capture.
     * It never asks about the service (the monitor setup just did, and Use
     * mouse still will) and says nothing while the desktop isn't ready yet.
     * False when it didn't start.
     */
    fun takeControlQuietly(): Boolean {
        if (captured || waiting) return true
        if (askingTouch || !holdsGrab || readySurface() == null) return false
        message = null
        if (!takeRoutedControl()) capturePointer()
        return true
    }

    private fun capturePointer() {
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

    /** The user handed the mouse back on purpose: Release, or Ctrl+Alt. */
    var handedBack: (() -> Unit)? = null

    private fun handBack(reason: String) {
        release(reason)
        handedBack?.invoke()
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
        inputManager?.unregisterInputDeviceListener(devices)
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
        const val RETURN_TIMEOUT_MS = 2000L
        const val BUTTON_BACK = 0x113
        const val BUTTON_FORWARD = 0x114
        /** As on the phone's touchpad page, where a label fits on one line. */
        val ButtonSize = 72.dp

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

/** What the note beside the controls' buttons, or above the touchpad's, says. */
internal sealed interface ControlNote {
    class Failure(val text: String) : ControlNote
    object Keys : ControlNote
    object Touch : ControlNote
}

/** The note's card, beside the controls' buttons or above the touchpad's. */
@Composable
internal fun NoteCard(palette: PortalPalette, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    val shape = RoundedCornerShape(20.dp)
    Box(
        modifier
            .shadow(6.dp, shape, ambientColor = palette.pillShadow, spotColor = palette.pillShadow)
            .clip(shape).background(palette.background).background(palette.surfaceTop)
            .border(1.dp, palette.surfaceBorder, shape)
            .padding(horizontal = 14.dp, vertical = 10.dp)
            .semantics { liveRegion = LiveRegionMode.Polite },
    ) {
        content()
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
