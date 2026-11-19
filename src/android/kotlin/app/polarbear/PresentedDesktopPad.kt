package app.polarbear

import android.hardware.input.InputManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.Display
import android.view.MotionEvent
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
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

/**
 * The phone's screen while the desktop is shown on a monitor through
 * [DesktopPresentation]: the whole screen is a touchpad for the desktop's
 * cursor (one finger moves it, a tap clicks, two fingers scroll or pinch, as
 * on a laptop). A chip at the top names the monitor; the buttons below open
 * Portal's keyboard and menu, which grow out of them and close back into them,
 * and bring the desktop back. With a mouse connected, one more hands it and the
 * keyboard to the desktop.
 */
internal class PresentedDesktopPad(private val host: PortalActivity, private val monitor: String) : FrameLayout(host) {
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
        ViewCompat.setOnApplyWindowInsetsListener(this) { _, insets ->
            safe = insets.getInsetsIgnoringVisibility(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
            insets
        }
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        inputManager?.registerInputDeviceListener(devices, Handler(Looper.getMainLooper()))
        devicesChanged()
        ViewCompat.getRootWindowInsets(this)?.let {
            safe = it.getInsetsIgnoringVisibility(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
        }
    }

    private fun devicesChanged() {
        pointerConnected = DesktopControlActivity.externalPointerConnected()
    }

    @Composable
    private fun PadContent() {
        PortalTheme {
            PortalUiDensity {
                val palette = resolvePalette(AppearanceMode.System)
                val density = LocalDensity.current
                val top = with(density) { safe.top.toDp() }
                val bottom = with(density) { safe.bottom.toDp() }
                // The chip drops in from the top edge and the buttons rise from the bottom one.
                val arrival = remember { Animatable(0f) }
                LaunchedEffect(Unit) { arrival.animateTo(1f, tween(560, easing = PortalEmphasizedDecelerate)) }
                // A capsule page (menu, keyboard, settings) grows out of a button
                // here: the buttons and the guide step aside until it closes back.
                val chrome = animateFloatAsState(
                    if (SessionControls.utilityOpen.value) 0f else 1f,
                    if (SessionControls.utilityOpen.value) tween(160, easing = PortalEmphasizedAccelerate)
                    else tween(260, easing = PortalEmphasizedDecelerate),
                    label = "touchpad chrome",
                )
                // Read in draw only: a touch redraws the pad without recomposing it.
                val glow = animateFloatAsState(
                    if (fingers.down) 1f else 0f,
                    if (fingers.down) tween(90) else tween(320, easing = PortalEmphasizedAccelerate),
                    label = "touchpad finger glow",
                )
                Box(
                    Modifier.fillMaxSize().background(palette.background)
                        .drawBehind { fingers.draw(this, palette.accent, glow.value) },
                ) {
                    MonitorChip(
                        palette,
                        Modifier.align(Alignment.TopCenter).padding(top = top + 20.dp, start = 24.dp, end = 24.dp)
                            .graphicsLayer {
                                alpha = arrival.value
                                translationY = (1f - arrival.value) * -14.dp.toPx()
                            },
                    )
                    AnimatedVisibility(
                        guide && !touched,
                        Modifier.align(Alignment.Center).padding(horizontal = 24.dp)
                            .graphicsLayer { alpha = chrome.value },
                        enter = fadeIn(tween(420, delayMillis = 240, easing = PortalEmphasizedDecelerate)) +
                            scaleIn(tween(520, delayMillis = 240, easing = PortalEmphasized), initialScale = 0.96f),
                        exit = fadeOut(tween(200, easing = PortalEmphasizedAccelerate)) +
                            scaleOut(tween(240, easing = PortalEmphasizedAccelerate), targetScale = 0.97f),
                        label = "touchpad gestures",
                    ) {
                        GestureGuide(palette)
                    }
                    Column(
                        Modifier.align(Alignment.BottomCenter).padding(bottom = bottom + 28.dp)
                            .graphicsLayer {
                                alpha = arrival.value * chrome.value
                                translationY = (1f - arrival.value) * 22.dp.toPx()
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

    /** Above the buttons: why taking the mouse failed, or which keys hand it back. */
    @Composable
    private fun GrabNote(palette: PortalPalette) {
        val failure = grab.failure
        // A failure fades after a while; trying again shows it again.
        var shownFailure by remember { mutableStateOf<String?>(null) }
        LaunchedEffect(failure) {
            shownFailure = failure
            if (failure != null) { delay(4_000); shownFailure = null }
        }
        AnimatedVisibility(
            shownFailure != null || grab.showsReleaseKeys,
            enter = fadeIn(tween(220)) + androidx.compose.animation.expandVertically(spring(dampingRatio = 1f, stiffness = 380f)),
            exit = fadeOut(tween(160)) + androidx.compose.animation.shrinkVertically(spring(dampingRatio = 1f, stiffness = 380f)),
            label = "touchpad note",
        ) {
            Box(Modifier.padding(bottom = 16.dp).widthIn(max = 320.dp), contentAlignment = Alignment.Center) {
                val text = shownFailure
                if (text != null) {
                    Text(text, color = palette.textMuted, fontSize = 12.sp, lineHeight = 16.sp, textAlign = TextAlign.Center)
                } else {
                    ReleaseKeys(palette)
                }
            }
        }
    }

    @Composable
    private fun PadButtons(palette: PortalPalette) {
        // Read when pressed, so each page starts from where its button is now.
        val placed = remember { arrayOfNulls<LayoutCoordinates>(2) }
        fun center(index: Int): Offset? = placed[index]?.takeIf { it.isAttached }?.boundsInRoot()?.center
        BoxWithConstraints {
            val count = if (pointerConnected) 4 else 3
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
                PadButton(PortalGlyph.Keyboard, "Keyboard", palette, size, onPlaced = { placed[0] = it }) {
                    center(0)?.let { TouchControls.openFromPad(UtilityPage.Keyboard, it.x, it.y) }
                }
                PadButton(PortalGlyph.Portal, "Portal", palette, size, description = "Portal menu",
                    onPlaced = { coordinates ->
                        placed[1] = coordinates
                        center(1)?.let { TouchControls.hideCapsuleForPad(it.x, it.y) }
                    }) {
                    center(1)?.let { TouchControls.openFromPad(UtilityPage.Menu, it.x, it.y) }
                }
                PadButton(PortalGlyph.Return, "Return", palette, size,
                    description = "Show the desktop on this ${deviceNoun(host)}") {
                    host.portalDisplays.moveTo(Display.DEFAULT_DISPLAY)
                }
            }
        }
    }

    // Touches the buttons don't take land here. Only a finger or pen works
    // the pad: a mouse goes to the desktop through Use mouse.
    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (!DesktopControlView.isDirectTouch(event)) return true
        timeNs = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) event.eventTimeNanos
        else event.eventTime * 1_000_000L
        if (event.actionMasked == MotionEvent.ACTION_DOWN) touched = true
        fingers.track(event)
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
        inputManager?.unregisterInputDeviceListener(devices)
        removeCallbacks(gestureTimer)
        gestures.reset()
        for (code in heldButtons) DesktopControlActivity.nativeControlButton(code, false, 0L)
        heldButtons.clear()
        TouchControls.restoreCapsule()
        super.onDetachedFromWindow()
    }

    private companion object {
        const val MM_PER_INCH = 25.4f
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

@Composable
private fun PadButton(
    glyph: PortalGlyph,
    label: String,
    palette: PortalPalette,
    size: Dp,
    accent: Boolean = false,
    description: String = label,
    onPlaced: (LayoutCoordinates) -> Unit = {},
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
    Column(
        Modifier.size(size)
            .onGloballyPositioned(onPlaced)
            .graphicsLayer { scaleX = scale; scaleY = scale }
            .shadow(6.dp, CircleShape, ambientColor = palette.pillShadow, spotColor = palette.pillShadow)
            .clip(CircleShape).background(palette.background).background(fill)
            .border(1.dp, palette.surfaceBorder, CircleShape)
            .clickable(interactionSource = interaction, indication = null, role = Role.Button, onClickLabel = description, onClick = onClick)
            .semantics(mergeDescendants = true) { contentDescription = description }
            .padding(6.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(3.dp, Alignment.CenterVertically),
    ) {
        PortalIcon(glyph, ink, size = 20.dp)
        Text(label, color = ink, fontSize = 11.sp, fontWeight = FontWeight.Medium, textAlign = TextAlign.Center,
            lineHeight = 13.sp, maxLines = 2)
    }
}

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
        delay(560L + index * 220L)
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
