package app.polarbear

import android.graphics.Bitmap
import android.hardware.input.InputManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.view.Display
import android.view.InputDevice
import android.view.MotionEvent
import android.view.PixelCopy
import android.view.PointerIcon
import android.view.SurfaceView
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.LinearOutSlowInEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandHorizontally
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.shrinkHorizontally
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.RoundRect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.geometry.lerp
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.coerceIn
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import app.polarbear.setup.AppearanceMode
import app.polarbear.setup.PortalColors
import app.polarbear.setup.PortalPalette
import app.polarbear.setup.PortalTheme
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import app.polarbear.setup.components.PortalGlyph
import app.polarbear.setup.components.PortalIcon
import app.polarbear.setup.components.PortalUiDensity
import app.polarbear.setup.resolvePalette
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/**
 * The phone's screen while the desktop is shown on a monitor through
 * [DesktopPresentation]: the whole screen is a touchpad for the desktop's
 * cursor (one finger moves it, a tap clicks, two fingers scroll or pinch, as
 * on a laptop). A chip at the top names the monitor; the buttons below open
 * Portal's keyboard and menu, which grow out of them and close back into them,
 * turn the screen into a game controller ([PresentedController]), and bring
 * the desktop back. With a mouse connected, one more hands it and the
 * keyboard to the desktop.
 *
 * The desktop leaves this screen by shrinking into the monitor chip ([snapshot]
 * is its picture) and comes back out of it ([leave]).
 */
internal class PresentedDesktopPad(
    private val host: PortalActivity,
    private val monitor: String,
    private val snapshot: DesktopSnapshot?,
) : FrameLayout(host) {
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

    /** Holds Control desktop for a connected mouse and keyboard; the pad draws its button. */
    private val grab = DesktopControlView(host, host, holdsGrab = true, close = {}, showControls = false)

    private var safe by mutableStateOf(Insets.NONE)
    private var pointerConnected by mutableStateOf(DesktopControlActivity.externalPointerConnected())
    private val guide = host.portalDisplays.takeTouchpadHint()
    private var touched by mutableStateOf(false)
    private val fingers = Fingers()

    /**
     * A connected mouse or touchpad goes to the desktop by itself until it is
     * handed back here; coming back to the front, a newly connected one or Use
     * mouse wants that again. Compose-observable.
     */
    private var mouseWanted by mutableStateOf(true)

    /** This screen's window has focus: Portal is in front. Compose-observable. */
    private var focused by mutableStateOf(false)

    /** Where the monitor chip sits; the desktop leaves into it and comes back out of it. */
    private var chipBounds by mutableStateOf<Rect?>(null)
    private var leaving by mutableStateOf(false)
    private var left: (() -> Unit)? = null
    private var leaveBase = 0L

    /** The Controller button's whole-screen game controller. */
    private val controller = PresentedController(host, this) { safe.top }

    private val inputManager = host.getSystemService(InputManager::class.java)
    private val devices = object : InputManager.InputDeviceListener {
        override fun onInputDeviceAdded(deviceId: Int) = devicesChanged()
        override fun onInputDeviceRemoved(deviceId: Int) = devicesChanged()
        override fun onInputDeviceChanged(deviceId: Int) = devicesChanged()
    }

    init {
        addView(
            ComposeView(host).apply { setContent { PadContent() } },
            LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT),
        )
        addView(grab, LayoutParams(1, 1))
        grab.handedBack = { mouseWanted = false }
        // Until the desktop takes a mouse, it works this screen, with Android's arrow.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) pointerIcon = PointerIcon.getSystemIcon(host, PointerIcon.TYPE_ARROW)
        ViewCompat.setOnApplyWindowInsetsListener(this) { _, insets ->
            safe = insets.getInsetsIgnoringVisibility(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
            insets
        }
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        inputManager?.registerInputDeviceListener(devices, Handler(Looper.getMainLooper()))
        devicesChanged()
        focused = hasWindowFocus()
        ViewCompat.getRootWindowInsets(this)?.let {
            safe = it.getInsetsIgnoringVisibility(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
        }
        TouchControls.presentingStarted(this, controller::release)
    }

    private fun devicesChanged() {
        val connected = DesktopControlActivity.externalPointerConnected()
        // A mouse or touchpad connected now goes to the desktop.
        if (connected && !pointerConnected) mouseWanted = true
        pointerConnected = connected
    }

    @Composable
    private fun PadContent() {
        PortalTheme {
            PortalUiDensity {
                val palette = resolvePalette(AppearanceMode.System)
                val density = LocalDensity.current
                val top = with(density) { safe.top.toDp() }
                val bottom = with(density) { safe.bottom.toDp() }
                // Arriving: the desktop as this screen last showed it shrinks into
                // the monitor chip while the pad fills in around it (with no copy
                // of it, the pad fades in and the chip drops in); then the buttons
                // rise. Until the copy settles, the pad stays clear over the desktop.
                val enter = remember { Animatable(0f) }
                val rise = remember { Animatable(0f) }
                val settled = snapshot?.settled ?: true
                val carried = remember(settled) { settled && snapshot?.image != null }
                LaunchedEffect(settled) {
                    if (!settled || leaving) return@LaunchedEffect
                    if (carried) {
                        launch { delay(scaled(180).toLong()); rise.animateTo(1f, tween(scaled(520), easing = PortalEmphasizedDecelerate)) }
                        // A softer tail than the emphasized curve: the picture lands in the chip.
                        enter.animateTo(1f, tween(scaled(620), easing = FastOutSlowInEasing))
                        snapshot?.drop()
                    } else {
                        launch { rise.animateTo(1f, tween(scaled(560), easing = PortalEmphasizedDecelerate)) }
                        enter.animateTo(1f, tween(scaled(220), easing = PortalEmphasizedDecelerate))
                    }
                }
                // Once the pad has arrived, a connected mouse or touchpad goes to the
                // desktop as if Use mouse had been tapped, and again whenever Portal
                // comes back in front, until it is handed back here.
                LaunchedEffect(Unit) {
                    snapshotFlow {
                        mouseWanted && pointerConnected && focused && rise.value > 0f && !leaving &&
                            !SessionControls.utilityOpen.value && !grab.grabbed && !grab.asksAboutTouch
                    }.collectLatest { take ->
                        if (!take) return@collectLatest
                        // The monitor's surface may still be on its way.
                        repeat(40) {
                            if (grab.takeControlQuietly()) return@collectLatest
                            delay(250)
                        }
                    }
                }
                // Leaving: the buttons go at once. Once the desktop shows on this
                // screen again, the chip opens out over the whole screen onto it.
                val out = remember { Animatable(0f) }
                val reveal = remember { Animatable(0f) }
                LaunchedEffect(leaving) {
                    if (!leaving) return@LaunchedEffect
                    snapshot?.drop()
                    enter.snapTo(1f)
                    val started = SystemClock.uptimeMillis()
                    val chromeGone = launch { out.animateTo(1f, tween(scaled(150), easing = LinearOutSlowInEasing)) }
                    // Plasma goes on redrawing for its new size for a second or two
                    // after this; waiting for that would make every return feel slow.
                    val back = withTimeoutOrNull(REVEAL_WAIT_MS) {
                        while (SessionControls.desktopWindowChanges() <= leaveBase) withFrameNanos { }
                    }
                    Log.i(TAG, if (back != null) "desktop back on this screen after ${SystemClock.uptimeMillis() - started}ms"
                        else "no desktop frame here after ${REVEAL_WAIT_MS}ms; opening anyway")
                    chromeGone.join()
                    reveal.animateTo(1f, tween(scaled(520), easing = PortalEmphasized))
                    left?.let { done -> left = null; post { done() } }
                }
                // A capsule page (menu, keyboard, settings) grows out of a button
                // here, and that button is the capsule until the page has closed
                // back into it (PadButtons). The others and the guide step aside
                // while the page shows and come back as it closes, as the veil's
                // other pills do.
                val utilityShown = SessionControls.utilityShown.value
                val chrome = animateFloatAsState(
                    if (utilityShown) 0f else 1f,
                    if (utilityShown) tween(140, easing = PortalEmphasizedAccelerate)
                    else tween(300, delayMillis = 110, easing = PortalEmphasizedDecelerate),
                    label = "touchpad chrome",
                )
                // Read in draw only: a touch redraws the pad without recomposing it.
                val glow = animateFloatAsState(
                    if (fingers.down) 1f else 0f,
                    if (fingers.down) tween(90) else tween(320, easing = PortalEmphasizedAccelerate),
                    label = "touchpad finger glow",
                )
                // Controller, from its button below or Game settings: the whole screen
                // becomes a game controller, and the touchpad steps back until it goes.
                val controllerShown = TouchControls.screenController.value && !leaving
                LaunchedEffect(controllerShown) { if (controllerShown) controller.show() else controller.hide() }
                val touchpad = animateFloatAsState(
                    if (controllerShown) 0f else 1f,
                    if (controllerShown) tween(160, easing = PortalEmphasizedAccelerate)
                    else tween(320, delayMillis = 120, easing = PortalEmphasizedDecelerate),
                    label = "touchpad under the controller",
                )
                val hole = remember { Path() }
                val frame = remember { Path() }
                Box(
                    Modifier.fillMaxSize().drawBehind {
                        val screen = Rect(Offset.Zero, size)
                        val chip = chipBounds ?: screen
                        // Carried, the picture covers the screen at first: the pad shows
                        // only where it has shrunk away, never the surface behind it.
                        val fill = if (carried) 1f else enter.value
                        val open = reveal.value
                        if (open > 0f) {
                            // The chip opening out onto the desktop behind the pad.
                            val opening = lerp(chip, screen, open)
                            val radius = CornerRadius(chip.height / 2f * (1f - open))
                            hole.reset()
                            hole.fillType = PathFillType.EvenOdd
                            hole.addRect(screen)
                            hole.addRoundRect(RoundRect(opening, radius))
                            drawPath(hole, palette.background, alpha = fill)
                            val thinning = 1f - ease(0f, 0.4f, open)
                            if (thinning > 0f) drawChipShape(opening, radius, palette, thinning)
                        } else {
                            drawRect(palette.background, alpha = fill)
                        }
                        val picture = snapshot?.image
                        if (picture != null && !leaving) drawCarried(picture, screen, chip, enter.value, palette, frame)
                        fingers.draw(this, palette.accent, glow.value)
                    },
                ) {
                    MonitorChip(
                        palette,
                        Modifier.align(Alignment.TopCenter).padding(top = top + 20.dp, start = 24.dp, end = 24.dp)
                            .onGloballyPositioned { chipBounds = it.boundsInRoot() }
                            .graphicsLayer {
                                // Carried in, its name shows once the picture has become the chip.
                                val shown = if (carried) ease(0.9f, 1f, enter.value) else rise.value
                                alpha = shown * (1f - ease(0f, 0.12f, reveal.value)) * touchpad.value
                                translationY = if (carried) 0f else (1f - rise.value) * -14.dp.toPx()
                            },
                    )
                    AnimatedVisibility(
                        guide && !touched && settled && !leaving && !controllerShown,
                        Modifier.align(Alignment.Center).padding(horizontal = 24.dp)
                            .graphicsLayer { alpha = chrome.value },
                        enter = fadeIn(tween(scaled(420), delayMillis = scaled(if (carried) 520 else 240), easing = PortalEmphasizedDecelerate)) +
                            scaleIn(tween(scaled(520), delayMillis = scaled(if (carried) 520 else 240), easing = PortalEmphasized), initialScale = 0.96f),
                        // Leaving, it clears out before the desktop opens up behind it.
                        exit = if (leaving) fadeOut(tween(scaled(110), easing = LinearOutSlowInEasing))
                        else fadeOut(tween(scaled(200), easing = PortalEmphasizedAccelerate)) +
                            scaleOut(tween(scaled(240), easing = PortalEmphasizedAccelerate), targetScale = 0.97f),
                        label = "touchpad gestures",
                    ) {
                        GestureGuide(palette)
                    }
                    Column(
                        Modifier.align(Alignment.BottomCenter).padding(bottom = bottom + 28.dp)
                            .graphicsLayer {
                                val gone = out.value
                                alpha = rise.value * chrome.value * (1f - gone) * touchpad.value
                                translationY = (1f - rise.value) * 22.dp.toPx() + gone * 18.dp.toPx() +
                                    (1f - touchpad.value) * 18.dp.toPx()
                                val settle = 0.94f + 0.06f * chrome.value
                                scaleX = settle; scaleY = settle
                            },
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        GrabNote(palette)
                        PadButtons(palette)
                    }
                }
            }
        }
    }

    @Composable
    private fun MonitorChip(palette: PortalPalette, modifier: Modifier) {
        val shape = RoundedCornerShape(50)
        Row(
            modifier.clip(shape).background(palette.surfaceTop).border(1.dp, palette.surfaceBorder, shape)
                .padding(start = 14.dp, end = 18.dp, top = 9.dp, bottom = 9.dp)
                .semantics(mergeDescendants = true) { contentDescription = "Desktop on $monitor" },
            verticalAlignment = Alignment.CenterVertically,
        ) {
            PortalIcon(PortalGlyph.Monitor, palette.textPrimary, size = 18.dp)
            Spacer(Modifier.width(10.dp))
            Text(monitor, color = palette.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium,
                maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }

    /**
     * Above the buttons: whether to turn the accessibility service back on, why
     * taking the mouse failed, or which keys hand it back.
     */
    @Composable
    private fun GrabNote(palette: PortalPalette) {
        val failure = grab.failure
        // A failure fades after a while; trying again shows it again.
        var shownFailure by remember { mutableStateOf<String?>(null) }
        LaunchedEffect(failure) {
            shownFailure = failure
            if (failure != null) { delay(4_000); shownFailure = null }
        }
        val text = shownFailure
        val note = when {
            grab.asksAboutTouch -> ControlNote.Touch
            text != null -> ControlNote.Failure(text)
            grab.showsReleaseKeys -> ControlNote.Keys
            else -> null
        }
        // The note keeps what it showed while it folds away.
        val lastNote = remember { arrayOfNulls<ControlNote>(1) }
        if (note != null) lastNote[0] = note
        AnimatedVisibility(
            note != null,
            enter = fadeIn(tween(220)) + androidx.compose.animation.expandVertically(spring(dampingRatio = 1f, stiffness = 380f)),
            exit = fadeOut(tween(160)) + androidx.compose.animation.shrinkVertically(spring(dampingRatio = 1f, stiffness = 380f)),
            label = "touchpad note",
        ) {
            Box(Modifier.padding(bottom = 16.dp).widthIn(max = 320.dp), contentAlignment = Alignment.Center) {
                when (val shown = lastNote[0]) {
                    ControlNote.Touch -> NoteCard(palette) { grab.TurnOnTouch(palette) }
                    ControlNote.Keys -> ReleaseKeys(palette)
                    is ControlNote.Failure ->
                        Text(shown.text, color = palette.textMuted, fontSize = 12.sp, lineHeight = 16.sp, textAlign = TextAlign.Center)
                    null -> Unit
                }
            }
        }
    }

    /**
     * Out of sight while the capsule wears this button's face: from the moment
     * a page grows out of it until that page has closed back into it. Read in
     * draw, the same frame the capsule hands over, so neither shows twice.
     */
    private fun Modifier.standingIn(glyph: PortalGlyph) =
        graphicsLayer { alpha = if (TouchControls.padStandIn.value == glyph) 0f else 1f }

    @Composable
    private fun PadButtons(palette: PortalPalette) {
        // Read when pressed, so each page starts from where its button is now.
        val placed = remember { arrayOfNulls<LayoutCoordinates>(2) }
        fun center(index: Int): Offset? = placed[index]?.takeIf { it.isAttached }?.boundsInRoot()?.center
        BoxWithConstraints {
            val count = if (pointerConnected) 5 else 4
            val gap = 14.dp
            val size = ((maxWidth - 32.dp - gap * (count - 1)) / count).coerceIn(56.dp, 74.dp)
            Row(horizontalArrangement = Arrangement.spacedBy(gap), verticalAlignment = Alignment.CenterVertically) {
                AnimatedVisibility(
                    pointerConnected,
                    enter = fadeIn(tween(220, delayMillis = 80)) + expandHorizontally(spring(dampingRatio = 1f, stiffness = 380f)),
                    exit = fadeOut(tween(140)) + shrinkHorizontally(spring(dampingRatio = 1f, stiffness = 380f)),
                    label = "use mouse",
                ) {
                    PadButton(
                        PortalGlyph.Mouse, if (grab.grabbed) "Release" else "Use mouse", palette, size,
                        accent = grab.grabbed,
                        description = if (grab.grabbed) "Give the mouse and keyboard back to Android"
                        else "Use the connected mouse and keyboard on the desktop",
                        onClick = grab::toggle,
                    )
                }
                PadButton(PortalGlyph.Keyboard, "Keyboard", palette, size, onPlaced = { placed[0] = it },
                    modifier = Modifier.standingIn(PortalGlyph.Keyboard)) {
                    center(0)?.let { TouchControls.openFromPad(UtilityPage.Keyboard, it.x, it.y, PadFace(PortalGlyph.Keyboard, "Keyboard", size)) }
                }
                PadButton(PortalGlyph.Controller, "Controller", palette, size,
                    description = "Use this ${deviceNoun(host)} as a game controller") {
                    TouchControls.setScreenController(true)
                }
                val portalFace = PadFace(PortalGlyph.Portal, "Portal", size)
                PadButton(PortalGlyph.Portal, "Portal", palette, size, description = "Portal menu",
                    onPlaced = { coordinates ->
                        placed[1] = coordinates
                        center(1)?.let { TouchControls.hideCapsuleForPad(it.x, it.y, portalFace) }
                    }, modifier = Modifier.standingIn(PortalGlyph.Portal)) {
                    center(1)?.let { TouchControls.openFromPad(UtilityPage.Menu, it.x, it.y, portalFace) }
                }
                PadButton(PortalGlyph.Return, "Return", palette, size,
                    description = "Show the desktop on this ${deviceNoun(host)}") {
                    host.portalDisplays.moveTo(Display.DEFAULT_DISPLAY)
                }
            }
        }
    }

    /**
     * The desktop went back to this screen: the buttons go, and once the desktop
     * shows here again the monitor chip opens out onto it; then [done] removes the pad.
     */
    fun leave(done: () -> Unit) {
        if (leaving) return
        leaveBase = SessionControls.desktopWindowChanges()
        left = done
        leaving = true
        removeCallbacks(gestureTimer)
        gestures.reset()
        releaseButtons()
        fingers.lift()
        // A held mouse and keyboard go back to Android at once.
        removeView(grab)
        // The controller goes with the buttons; the floating controls may come back.
        TouchControls.presentingEnded(this)
    }

    // While it leaves, the pad still covers the desktop: it keeps every touch
    // until it has opened onto it.
    override fun onInterceptTouchEvent(event: MotionEvent): Boolean = leaving || super.onInterceptTouchEvent(event)

    // Touches the buttons don't take land here. Only a finger or pen works
    // the pad: a mouse goes to the desktop through Use mouse.
    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (leaving || !DesktopControlView.isDirectTouch(event)) return true
        timeNs = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) event.eventTimeNanos
        else event.eventTime * 1_000_000L
        if (event.actionMasked == MotionEvent.ACTION_DOWN) touched = true
        fingers.track(event)
        gestures.dispatch(event)
        scheduleGestureTimer()
        return true
    }

    // A mouse or touchpad's hover and scroll stop here too: below the pad lies
    // the desktop's own surface, which would take them as positions on this
    // screen rather than on the monitor.
    override fun onHoverEvent(event: MotionEvent): Boolean = true

    override fun onGenericMotionEvent(event: MotionEvent): Boolean =
        event.isFromSource(InputDevice.SOURCE_CLASS_POINTER) || super.onGenericMotionEvent(event)

    override fun onWindowFocusChanged(hasWindowFocus: Boolean) {
        super.onWindowFocusChanged(hasWindowFocus)
        focused = hasWindowFocus
    }

    override fun onWindowVisibilityChanged(visibility: Int) {
        super.onWindowVisibilityChanged(visibility)
        // Back in front, a connected mouse goes to the desktop again.
        if (visibility == VISIBLE) mouseWanted = true
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

    private fun releaseButtons() {
        for (code in heldButtons) DesktopControlActivity.nativeControlButton(code, false, 0L)
        heldButtons.clear()
    }

    override fun onDetachedFromWindow() {
        inputManager?.unregisterInputDeviceListener(devices)
        removeCallbacks(gestureTimer)
        gestures.reset()
        releaseButtons()
        snapshot?.drop()
        controller.detach()
        TouchControls.presentingEnded(this)
        TouchControls.restoreCapsule()
        super.onDetachedFromWindow()
    }

    companion object {
        private const val TAG = "PortalTouchpad"
        private const val MM_PER_INCH = 25.4f
        /** Longest the desktop's return waits for its first frame on this screen. */
        private const val REVEAL_WAIT_MS = 1_500L

        /** Debug builds: stretches the transitions (portal_display_slow) for screenshots. */
        @Volatile internal var slow = 1f

        private fun scaled(ms: Int): Int = (ms * slow).toInt()
    }
}

/** 0 before [from], 1 after [to], easing in and out between. */
private fun ease(from: Float, to: Float, value: Float): Float {
    val t = ((value - from) / (to - from)).coerceIn(0f, 1f)
    return t * t * (3f - 2f * t)
}

/** The monitor chip's pill, filled and outlined, at [alpha]. */
private fun DrawScope.drawChipShape(bounds: Rect, radius: CornerRadius, palette: PortalPalette, alpha: Float) {
    drawRoundRect(palette.surfaceTop, bounds.topLeft, bounds.size, radius, alpha = alpha)
    drawRoundRect(palette.surfaceBorder, bounds.topLeft, bounds.size, radius, alpha = alpha, style = Stroke(1.dp.toPx()))
}

/**
 * The desktop's picture at [progress] of its way from the whole screen into the
 * monitor chip: it takes the chip's shape and fill as it goes and fades into it.
 */
private fun DrawScope.drawCarried(
    picture: ImageBitmap,
    screen: Rect,
    chip: Rect,
    progress: Float,
    palette: PortalPalette,
    frame: Path,
) {
    if (progress >= 1f) return
    val bounds = lerp(screen, chip, progress)
    val radius = CornerRadius(chip.height / 2f * progress)
    // The picture stays until it is almost the chip, then the chip's fill takes over.
    val fill = ease(0.55f, 0.9f, progress)
    if (fill > 0f) drawRoundRect(palette.surfaceTop, bounds.topLeft, bounds.size, radius, alpha = fill)
    val shown = 1f - ease(0.6f, 0.95f, progress)
    if (shown > 0f) {
        frame.reset()
        frame.addRoundRect(RoundRect(bounds, radius))
        clipPath(frame) {
            // Cropped to fill the shape, as the chip narrows it.
            val fit = maxOf(bounds.width / picture.width, bounds.height / picture.height)
            translate(bounds.center.x - picture.width * fit / 2f, bounds.center.y - picture.height * fit / 2f) {
                scale(fit, fit, pivot = Offset.Zero) { drawImage(picture, alpha = shown) }
            }
        }
    }
    if (fill > 0f) {
        drawRoundRect(palette.surfaceBorder, bounds.topLeft, bounds.size, radius, alpha = fill, style = Stroke(1.dp.toPx()))
    }
}

/**
 * The desktop as this screen showed it just before it left for a monitor; the
 * touchpad shrinks it into its monitor chip. Main thread.
 */
internal class DesktopSnapshot private constructor() {
    /** The copy once it landed; dropped once the touchpad has arrived. */
    var image by mutableStateOf<ImageBitmap?>(null)
        private set

    /** The copy landed, failed or took too long: the touchpad arrives now either way. */
    var settled by mutableStateOf(false)
        private set

    /** Lets the picture go. Never recycled: a frame being drawn may still hold it. */
    fun drop() {
        image = null
    }

    companion object {
        private const val TAG = "PortalTouchpad"
        /** How long the move to the monitor waits for the copy. */
        private const val WAIT_MS = 250L

        /** Copies [surface]; [then] runs on the main thread once the copy has settled, either way. */
        fun take(surface: SurfaceView?, then: () -> Unit): DesktopSnapshot {
            val snapshot = DesktopSnapshot()
            val main = Handler(Looper.getMainLooper())
            val started = SystemClock.uptimeMillis()
            fun settle(bitmap: Bitmap?, why: String) {
                if (snapshot.settled) return
                if (bitmap != null) {
                    // The desktop is opaque whatever its buffer's alpha channel holds.
                    bitmap.setHasAlpha(false)
                    bitmap.prepareToDraw()
                    snapshot.image = bitmap.asImageBitmap()
                }
                snapshot.settled = true
                Log.i(TAG, "desktop copy for the monitor move: $why after ${SystemClock.uptimeMillis() - started}ms")
                then()
            }
            val width = surface?.width ?: 0
            val height = surface?.height ?: 0
            if (surface == null || width < 2 || height < 2 || surface.holder.surface?.isValid != true ||
                SessionControls.desktopFrames() == 0L
            ) {
                main.post { settle(null, "no desktop to copy") }
                return snapshot
            }
            val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
            val giveUp = Runnable { settle(null, "gave up") }
            main.postDelayed(giveUp, WAIT_MS)
            try {
                PixelCopy.request(surface, bitmap, { code ->
                    main.removeCallbacks(giveUp)
                    settle(bitmap.takeIf { code == PixelCopy.SUCCESS }, "code $code")
                }, main)
            } catch (_: IllegalArgumentException) {
                main.removeCallbacks(giveUp)
                main.post { settle(null, "surface refused") }
            }
            return snapshot
        }
    }
}

/** Where fingers rest on the touchpad, drawn as a soft glow under each. UI thread. */
private class Fingers {
    private val xs = FloatArray(MAX)
    private val ys = FloatArray(MAX)
    private var count = 0
    // Read in draw only, so a moving finger redraws the pad without recomposing it.
    private var revision by mutableIntStateOf(0)
    var down by mutableStateOf(false)
        private set

    fun track(event: MotionEvent) {
        when (event.actionMasked) {
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> down = false
            else -> {
                val lifted = if (event.actionMasked == MotionEvent.ACTION_POINTER_UP) event.actionIndex else -1
                var n = 0
                for (i in 0 until event.pointerCount) {
                    if (i == lifted || n == MAX) continue
                    xs[n] = event.getX(i); ys[n] = event.getY(i); n++
                }
                count = n
                down = n > 0
            }
        }
        revision++
    }

    /** The pad stopped taking touches; any glow fades. */
    fun lift() {
        down = false
        revision++
    }

    fun draw(scope: DrawScope, accent: Color, glow: Float) {
        if (revision < 0 || glow <= 0.01f) return
        val radius = with(scope) { 46.dp.toPx() }
        for (i in 0 until count) {
            val center = Offset(xs[i], ys[i])
            scope.drawCircle(
                Brush.radialGradient(listOf(accent.copy(alpha = 0.20f * glow), accent.copy(alpha = 0f)), center, radius),
                radius, center,
            )
        }
    }

    private companion object {
        const val MAX = 3
    }
}

/** A round button with its glyph and a word: the touchpad's, and the floating controls'. */
@Composable
internal fun PadButton(
    glyph: PortalGlyph,
    label: String,
    palette: PortalPalette,
    size: Dp,
    accent: Boolean = false,
    description: String = label,
    onPlaced: (LayoutCoordinates) -> Unit = {},
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val scale by animateFloatAsState(if (pressed) 0.91f else 1f, spring(dampingRatio = 0.82f, stiffness = 650f), label = "pad button press")
    val fill by animateColorAsState(
        when {
            accent -> palette.accent
            pressed -> palette.selectionHighlight
            else -> palette.surfaceTop
        },
        tween(180, easing = PortalEmphasized), label = "pad button fill",
    )
    val ink = if (accent) PortalColors.Charcoal else palette.textPrimary
    PadButtonFace(glyph, label, ink,
        modifier.size(size)
            .onGloballyPositioned(onPlaced)
            .graphicsLayer { scaleX = scale; scaleY = scale }
            .shadow(PadButtonElevation, CircleShape, ambientColor = palette.pillShadow, spotColor = palette.pillShadow)
            .clip(CircleShape).background(palette.background).background(fill)
            .border(1.dp, palette.surfaceBorder, CircleShape)
            .clickable(interactionSource = interaction, indication = null, role = Role.Button, onClickLabel = description, onClick = onClick)
            .semantics(mergeDescendants = true) { contentDescription = description })
}

/** A touchpad button's shadow; the capsule standing in for one casts the same. */
internal val PadButtonElevation = 6.dp

/** Ctrl + Alt as key caps: the keys that give the mouse back to Android. */
@Composable
internal fun ReleaseKeys(palette: PortalPalette, modifier: Modifier = Modifier) {
    Row(modifier.semantics(mergeDescendants = true) { contentDescription = "Ctrl plus Alt releases" },
        verticalAlignment = Alignment.CenterVertically) {
        KeyCap("Ctrl", palette)
        Text("+", Modifier.padding(horizontal = 6.dp), color = palette.textMuted, fontSize = 12.sp)
        KeyCap("Alt", palette)
        Spacer(Modifier.width(10.dp))
        Text("releases", color = palette.textMuted, fontSize = 12.sp)
    }
}

@Composable
private fun KeyCap(label: String, palette: PortalPalette) {
    val shape = RoundedCornerShape(6.dp)
    Box(
        Modifier.clip(shape).background(palette.surfaceTop).border(1.dp, palette.surfaceBorder, shape)
            .padding(horizontal = 8.dp, vertical = 3.dp),
    ) {
        Text(label, color = palette.textPrimary, fontSize = 11.5.sp, fontWeight = FontWeight.Medium)
    }
}

private enum class PadGesture(val label: String, val description: String) {
    Point("Point", "One finger moves the pointer"),
    Click("Click", "Tap to click"),
    RightClick("Right-click", "Tap with two fingers to right-click"),
    Scroll("Scroll", "Slide two fingers to scroll"),
}

/** One finger points, a tap clicks, two fingers right-click and scroll: each drawn, each played once. */
@Composable
private fun GestureGuide(palette: PortalPalette) {
    BoxWithConstraints {
        val wide = maxWidth >= 320.dp
        val tiles = PadGesture.entries
        if (wide) {
            Row(horizontalArrangement = Arrangement.spacedBy(18.dp)) {
                tiles.forEachIndexed { index, gesture -> GestureTile(gesture, index, palette) }
            }
        } else {
            Column(verticalArrangement = Arrangement.spacedBy(18.dp)) {
                tiles.chunked(2).forEachIndexed { row, pair ->
                    Row(horizontalArrangement = Arrangement.spacedBy(18.dp)) {
                        pair.forEachIndexed { index, gesture -> GestureTile(gesture, row * 2 + index, palette) }
                    }
                }
            }
        }
    }
}

@Composable
private fun GestureTile(gesture: PadGesture, index: Int, palette: PortalPalette) {
    val play = remember { Animatable(0f) }
    LaunchedEffect(Unit) {
        delay(360L + index * 220L)
        play.animateTo(1f, tween(1100, easing = FastOutSlowInEasing))
    }
    Column(
        Modifier.width(64.dp).semantics(mergeDescendants = true) { contentDescription = gesture.description },
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Canvas(Modifier.size(52.dp)) { drawGesture(gesture, play.value, palette.accent, palette.textMuted) }
        Spacer(Modifier.height(8.dp))
        Text(gesture.label, color = palette.textMuted, fontSize = 11.5.sp, textAlign = TextAlign.Center, maxLines = 1)
    }
}

/** A gesture on the 24-unit grid of Portal's glyphs; [t] runs 0 to 1 once and the drawing settles at rest. */
private fun DrawScope.drawGesture(gesture: PadGesture, t: Float, finger: Color, ink: Color) {
    scale(size.width / 24f, size.height / 24f, pivot = Offset.Zero) {
        val stroke = Stroke(1.2f, cap = StrokeCap.Round)
        fun line(x: Float, y: Float, x2: Float, y2: Float, color: Color = ink) =
            drawLine(color, Offset(x, y), Offset(x2, y2), 1.2f, StrokeCap.Round)
        // Out and back: 0 at rest, 1 at the far end of the motion.
        val swing = if (t <= 0f || t >= 1f) 0f else kotlin.math.sin(t * Math.PI).toFloat()
        when (gesture) {
            PadGesture.Point -> {
                line(9f, 15f, 16.5f, 7.5f)
                line(16.5f, 7.5f, 12.8f, 7.6f); line(16.5f, 7.5f, 16.4f, 11.2f)
                drawCircle(finger, 2.6f, Offset(7.5f + 7f * swing, 16.5f - 7f * swing))
            }
            PadGesture.Click -> {
                // A ripple leaves the fingertip as the tap lands.
                val ring = if (t <= 0f || t >= 1f) 0f else t
                if (ring > 0f) drawCircle(ink.copy(alpha = (1f - ring) * 0.9f), 3f + 7f * ring, Offset(12f, 12f), style = stroke)
                drawCircle(ink.copy(alpha = 0.35f), 6.5f, Offset(12f, 12f), style = stroke)
                drawCircle(finger, 2.6f, Offset(12f, 12f))
            }
            PadGesture.RightClick -> {
                val ring = if (t <= 0f || t >= 1f) 0f else t
                if (ring > 0f) {
                    drawOval(ink.copy(alpha = (1f - ring) * 0.9f), Offset(4.5f - 3f * ring, 7.5f - 3f * ring),
                        Size(15f + 6f * ring, 9f + 6f * ring), style = stroke)
                }
                drawOval(ink.copy(alpha = 0.35f), Offset(3f, 6f), Size(18f, 12f), style = stroke)
                drawCircle(finger, 2.3f, Offset(8.6f, 12f))
                drawCircle(finger, 2.3f, Offset(15.4f, 12f))
            }
            PadGesture.Scroll -> {
                line(9f, 5.5f, 12f, 2.5f); line(12f, 2.5f, 15f, 5.5f)
                line(9f, 18.5f, 12f, 21.5f); line(12f, 21.5f, 15f, 18.5f)
                val y = 12f - 4f * swing
                drawCircle(finger, 2.3f, Offset(8.6f, y))
                drawCircle(finger, 2.3f, Offset(15.4f, y))
            }
        }
    }
}
