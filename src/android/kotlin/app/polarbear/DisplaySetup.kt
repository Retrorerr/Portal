package app.polarbear

import android.os.Build
import android.view.HapticFeedbackConstants
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.PointerIcon
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.annotation.RequiresApi
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.RoundRect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.geometry.lerp
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Outline
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.lerp as lerpColor
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.AbstractComposeView
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
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
import app.polarbear.setup.components.openAccessibilitySettings
import app.polarbear.setup.components.openAppInfo
import app.polarbear.setup.resolvePalette
import kotlinx.coroutines.delay
import kotlin.math.abs
import kotlin.math.roundToInt

/** What the desktop on a monitor works better with, and only the user can allow. */
internal enum class DisplayPermission {
    /** Display over other apps: the floating controls stay on this screen over any app. */
    Overlay,

    /** Portal's accessibility service: Control desktop leaves the touchscreen to Android. */
    Accessibility;

    /** Whether this Android version has it at all. */
    val applies: Boolean
        get() = when (this) {
            Overlay -> Build.VERSION.SDK_INT >= Build.VERSION_CODES.O
            // Routing only the mouse through the service needs Android 14.
            Accessibility -> Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE
        }

    /** What it gives, as the setup and Portal settings name it. */
    val title: String
        get() = when (this) {
            Overlay -> "Controls over other apps"
            Accessibility -> "Touch stays with Android"
        }
}

/**
 * Before the desktop goes to a monitor: one short step for each Android
 * permission it works better with there and doesn't have yet. The card grows
 * out of the monitor's row in Portal settings and goes back into it. Every
 * step can be put off; once the last one is answered the desktop moves, unless
 * the setup was opened only to finish what was put off.
 */
internal object DisplaySetup {
    private var shown: DisplaySetupView? = null

    /**
     * Opens the setup over Portal; false when [steps] is empty, and the caller
     * moves now. Without [moved] the desktop stays where it is.
     */
    fun show(
        host: PortalActivity,
        target: PortalDisplays.Target,
        steps: List<DisplayPermission>,
        origin: Rect?,
        moved: (() -> Unit)?,
        preview: Boolean = false,
    ): Boolean {
        if (steps.isEmpty()) return false
        shown?.removeNow()
        val view = DisplaySetupView(host, target, steps, origin, preview, moved)
        // Above everything over the desktop. Android's settings pages close
        // Portal's menu behind them; this stays for when they come back.
        host.overlayHost().addView(view, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        shown = view
        return true
    }

    /** Debug builds: every step this Android has, whatever it already allows. */
    fun preview(host: PortalActivity, displayId: Int) {
        val target = host.portalDisplays.targets().firstOrNull { it.id == displayId && !it.current } ?: return
        show(host, target, everyStep(), origin = null, moved = {}, preview = true)
    }

    /** Debug builds: the next monitor chosen in Portal settings plays every step. */
    @Volatile var previewNext = false

    fun everyStep(): List<DisplayPermission> = DisplayPermission.entries.filter { it.applies }

    internal fun closed(view: DisplaySetupView) {
        if (shown === view) shown = null
    }

    /** Back or Escape closes the setup; no other key reaches the desktop while it is up. */
    fun dispatchKey(event: KeyEvent): Boolean {
        val view = shown?.takeIf { it.isAttachedToWindow } ?: return false
        if (event.keyCode == KeyEvent.KEYCODE_BACK || event.keyCode == KeyEvent.KEYCODE_ESCAPE) {
            if (event.action == KeyEvent.ACTION_UP) view.cancel()
            return true
        }
        // Volume, media and the like still go to Android.
        return !event.isSystem
    }
}

internal class DisplaySetupView(
    private val host: PortalActivity,
    private val target: PortalDisplays.Target,
    private val steps: List<DisplayPermission>,
    /** The monitor's row, in window pixels. */
    private val origin: Rect?,
    private val preview: Boolean,
    /** Null when the desktop stays where it is. */
    private val moved: (() -> Unit)?,
) : AbstractComposeView(host) {
    private enum class Exit { Back, Move }

    private var safe by mutableStateOf(Insets.NONE)
    private var exit by mutableStateOf<Exit?>(null)
    private var stopWatching: (() -> Unit)? = null
    private val device = deviceNoun(host)
    private val appName = host.applicationInfo.loadLabel(host.packageManager).toString()

    init {
        ViewCompat.setOnApplyWindowInsetsListener(this) { _, insets ->
            safe = insets.getInsetsIgnoringVisibility(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
            insets
        }
    }

    fun cancel() {
        if (exit == null) exit = Exit.Back
    }

    fun removeNow() {
        (parent as? ViewGroup)?.removeView(this)
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        ViewCompat.getRootWindowInsets(this)?.let {
            safe = it.getInsetsIgnoringVisibility(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
        }
        // The monitor going away ends the setup.
        stopWatching = host.portalDisplays.observe {
            if (host.portalDisplays.targets().none { it.id == target.id }) cancel()
        }
    }

    override fun onDetachedFromWindow() {
        stopWatching?.invoke()
        stopWatching = null
        DisplaySetup.closed(this)
        super.onDetachedFromWindow()
    }

    @RequiresApi(Build.VERSION_CODES.N)
    override fun onResolvePointerIcon(event: MotionEvent, pointerIndex: Int): PointerIcon =
        super.onResolvePointerIcon(event, pointerIndex) ?: PointerIcon.getSystemIcon(context, PointerIcon.TYPE_ARROW)

    // A mouse wheel over the setup stays off the settings page below it.
    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean {
        super.dispatchGenericMotionEvent(event)
        return true
    }

    @Composable
    override fun Content() {
        PortalTheme {
            PortalUiDensity {
                Setup(resolvePalette(AppearanceMode.System))
            }
        }
    }

    @Composable
    private fun Setup(palette: PortalPalette) {
        // 0 is the monitor's row (or a smaller card), 1 the card.
        val open = remember { Animatable(0f) }
        var rootOffset by remember { mutableStateOf(Offset.Zero) }
        // Android's pages close Portal's menu, and the row with it: from then
        // on the card leaves where it is.
        var fromRow by remember { mutableStateOf(origin != null) }
        var index by remember { mutableIntStateOf(0) }
        val granted = remember { mutableStateListOf<Int>() }
        var asked by remember { mutableStateOf(false) }
        var unanswered by remember { mutableStateOf(false) }
        var resumes by remember { mutableIntStateOf(0) }
        val lifecycle = LocalLifecycleOwner.current.lifecycle
        DisposableEffect(lifecycle) {
            val observer = LifecycleEventObserver { _, event ->
                when (event) {
                    Lifecycle.Event.ON_RESUME -> resumes++
                    Lifecycle.Event.ON_PAUSE -> fromRow = false
                    else -> Unit
                }
            }
            lifecycle.addObserver(observer)
            onDispose { lifecycle.removeObserver(observer) }
        }

        fun advance() {
            if (index < steps.lastIndex) {
                index++
                asked = false
                unanswered = false
            } else if (exit == null) {
                exit = if (moved != null) Exit.Move else Exit.Back
            }
        }

        LaunchedEffect(Unit) { open.animateTo(1f, tween(scaled(460), easing = PortalEmphasized)) }

        // Back from Android's page. Its answer can reach Portal a moment late.
        LaunchedEffect(resumes) {
            val step = index
            if (resumes == 0 || exit != null || step in granted || preview && !asked) return@LaunchedEffect
            for (wait in longArrayOf(0L, 200L, 400L, 800L)) {
                delay(wait)
                if (host.portalDisplays.granted(steps[step])) {
                    granted.add(step)
                    performHapticFeedback(
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) HapticFeedbackConstants.CONFIRM
                        else HapticFeedbackConstants.CONTEXT_CLICK,
                    )
                    delay(scaled(760).toLong())
                    if (index == step) advance()
                    return@LaunchedEffect
                }
            }
            if (asked) unanswered = true
        }

        LaunchedEffect(exit) {
            val leaving = exit ?: return@LaunchedEffect
            // Presented, the desktop leaves this screen under the fading card,
            // and Portal's menu closes, as when a monitor needs nothing.
            if (leaving == Exit.Move && target.presented && host.portalDisplays.moveTo(target.id)) moved?.invoke()
            val back = leaving == Exit.Back && fromRow
            fromRow = back
            open.animateTo(
                0f,
                if (back) tween(scaled(380), easing = PortalEmphasized)
                else tween(scaled(200), easing = PortalEmphasizedAccelerate),
            )
            // Portal itself moves to the monitor: the card is gone first.
            if (leaving == Exit.Move && !target.presented && host.portalDisplays.moveTo(target.id)) moved?.invoke()
            post { removeNow() }
        }

        val compact = LocalConfiguration.current.screenHeightDp < 560
        Layout(
            content = {
                Shell(palette, open) { fromRow }
                Column(
                    Modifier
                        // The card's own space doesn't close it.
                        .pointerInput(Unit) { detectTapGestures { } }
                        .graphicsLayer { alpha = smooth(if (fromRow) 0.4f else 0.25f, 0.85f, open.value) }
                        .padding(start = 22.dp, end = 22.dp, top = 18.dp, bottom = 12.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    MonitorChip(target.name, palette)
                    Spacer(Modifier.height(if (compact) 8.dp else 14.dp))
                    AnimatedContent(
                        index,
                        transitionSpec = {
                            (fadeIn(tween(scaled(260), delayMillis = scaled(90), easing = PortalEmphasizedDecelerate)) +
                                slideInHorizontally(tween(scaled(420), easing = PortalEmphasized)) { it / 6 })
                                .togetherWith(
                                    fadeOut(tween(scaled(120), easing = PortalEmphasizedAccelerate)) +
                                        slideOutHorizontally(tween(scaled(420), easing = PortalEmphasized)) { -it / 6 },
                                )
                                .using(SizeTransform(clip = false))
                        },
                        label = "display setup step",
                    ) { shown ->
                        Step(
                            steps[shown], first = shown == 0, palette, compact,
                            granted = shown in granted,
                            unanswered = unanswered && shown == index,
                            onAllow = {
                                asked = true
                                unanswered = false
                                when (steps[shown]) {
                                    DisplayPermission.Overlay -> PortalDisplays.openOverlaySettings(host)
                                    DisplayPermission.Accessibility -> openAccessibilitySettings(host)
                                }
                            },
                            onLater = {
                                if (!preview) host.portalDisplays.putOff(steps[shown])
                                if (shown == index) advance()
                            },
                        )
                    }
                    if (steps.size > 1) StepDots(steps.size, index, palette)
                }
            },
            modifier = Modifier
                .onGloballyPositioned { rootOffset = it.positionInWindow() }
                .drawBehind { drawRect(palette.scrim, alpha = open.value) }
                .pointerInput(Unit) { detectTapGestures { if (open.value > 0.9f) cancel() } }
                .semantics { paneTitle = "Before ${target.name}" },
        ) { measurables, constraints ->
            val gutter = 16.dp.roundToPx()
            val width = minOf(constraints.maxWidth - 2 * gutter, 360.dp.roundToPx()).coerceAtLeast(1)
            val top = safe.top + gutter
            val bottom = (constraints.maxHeight - safe.bottom - gutter).coerceAtLeast(top + 1)
            val card = measurables[1].measure(Constraints(minWidth = width, maxWidth = width, maxHeight = bottom - top))
            val left = (constraints.maxWidth - card.width) / 2
            val cardTop = top + (bottom - top - card.height) / 2
            val rest = Rect(Offset(left.toFloat(), cardTop.toFloat()), Size(card.width.toFloat(), card.height.toFloat()))
            val start = origin?.takeIf { fromRow }?.translate(-rootOffset) ?: shrink(rest, 0.92f)
            val body = lerp(start, rest, open.value)
            val shell = measurables[0].measure(
                Constraints.fixed(body.width.roundToInt().coerceAtLeast(1), body.height.roundToInt().coerceAtLeast(1)),
            )
            // The card's content rides the surface's top edge and shows only within it.
            val inside = RoundRect(
                Rect(Offset((rest.width - body.width) / 2f, 0f), body.size),
                CornerRadius(shellRadius(open.value, fromRow)),
            )
            layout(constraints.maxWidth, constraints.maxHeight) {
                shell.place(body.left.roundToInt(), body.top.roundToInt())
                card.placeWithLayer(left + (body.center.x - rest.center.x).roundToInt(), body.top.roundToInt()) {
                    clip = true
                    shape = RoundRectShape(inside)
                }
            }
        }
    }

    private class RoundRectShape(private val rect: RoundRect) : Shape {
        override fun createOutline(size: Size, layoutDirection: LayoutDirection, density: Density) = Outline.Rounded(rect)
    }

    /** The card's surface, from the monitor's row to its own size and back. */
    @Composable
    private fun Shell(palette: PortalPalette, open: Animatable<Float, *>, fromRow: () -> Boolean) {
        Box(
            Modifier
                .graphicsLayer {
                    val t = open.value
                    val row = fromRow()
                    shape = RoundedCornerShape(shellRadius(t, row))
                    clip = true
                    alpha = if (row) smooth(0f, 0.18f, t) else smooth(0f, 0.7f, t)
                    shadowElevation = 22.dp.toPx() * t
                    ambientShadowColor = palette.pillShadow
                    spotShadowColor = palette.pillShadow
                }
                .drawBehind {
                    drawRect(palette.background)
                    drawRect(Brush.verticalGradient(listOf(palette.surfaceTop, palette.surfaceBottom)))
                    val edge = 0.5.dp.toPx()
                    drawRoundRect(
                        palette.surfaceBorder, Offset(edge, edge), Size(size.width - 2 * edge, size.height - 2 * edge),
                        CornerRadius(shellRadius(open.value, fromRow()) - edge), style = Stroke(2 * edge),
                    )
                },
        )
    }

    private fun androidx.compose.ui.unit.Density.shellRadius(t: Float, fromRow: Boolean): Float {
        val start = if (fromRow) 16.dp.toPx() else 26.dp.toPx()
        return start + (28.dp.toPx() - start) * t
    }

    @Composable
    private fun Step(
        step: DisplayPermission,
        first: Boolean,
        palette: PortalPalette,
        compact: Boolean,
        granted: Boolean,
        unanswered: Boolean,
        onAllow: () -> Unit,
        onLater: () -> Unit,
    ) {
        // Played once as the step arrives; it rests on what the step gives.
        val play = remember { Animatable(0f) }
        LaunchedEffect(Unit) {
            delay(scaled(if (first) 340 else 220).toLong())
            play.animateTo(1f, tween(scaled(2000), easing = LinearEasing))
        }
        val badge by animateFloatAsState(
            if (granted) 1f else 0f, spring(dampingRatio = 0.6f, stiffness = 500f), label = "display setup granted",
        )
        val cursor = remember { Path() }
        val tablet = device == "tablet"
        Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
            Canvas(
                Modifier.fillMaxWidth().height(if (compact) 72.dp else 112.dp)
                    .semantics { contentDescription = illustration(step) },
            ) {
                drawStage(step, play.value, badge, palette, tablet, cursor)
            }
            Spacer(Modifier.height(if (compact) 10.dp else 16.dp))
            Text(
                step.title, Modifier.semantics { heading() },
                color = palette.textPrimary, fontSize = 18.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(6.dp))
            Text(
                why(step), color = palette.textMuted, fontSize = 13.5.sp, lineHeight = 19.sp,
                textAlign = TextAlign.Center, minLines = 2,
            )
            Spacer(Modifier.height(if (compact) 12.dp else 18.dp))
            SetupButton(if (step == DisplayPermission.Overlay) "Allow" else "Turn on", granted, palette, onAllow)
            // Where to look on Android's page (Android often opens its whole
            // list), or why it didn't take.
            val listed = step == DisplayPermission.Accessibility || Build.VERSION.SDK_INT >= Build.VERSION_CODES.R
            val restricted = step == DisplayPermission.Accessibility && unanswered &&
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
            AnimatedVisibility(
                listed || restricted,
                enter = fadeIn(tween(220, delayMillis = 60)) + expandVertically(spring(dampingRatio = 1f, stiffness = 380f)),
                exit = fadeOut(tween(120)) + shrinkVertically(spring(dampingRatio = 1f, stiffness = 380f)),
                label = "display setup hint",
            ) {
                if (restricted) {
                    Text(
                        "Switch greyed out? Open App info, then ⋮ › Allow restricted settings",
                        Modifier.padding(top = 10.dp).clip(RoundedCornerShape(8.dp))
                            .clickable(role = Role.Button) { openAppInfo(host) }
                            .padding(horizontal = 6.dp, vertical = 4.dp),
                        color = palette.accent, fontSize = 12.sp, lineHeight = 16.sp, textAlign = TextAlign.Center,
                    )
                } else {
                    Text(
                        if (step == DisplayPermission.Overlay) "Pick $appName in Android’s list"
                        else "Find $appName and turn it on",
                        Modifier.padding(top = 10.dp),
                        color = palette.textMuted, fontSize = 12.sp, textAlign = TextAlign.Center,
                    )
                }
            }
            Box(
                Modifier.padding(top = 4.dp).heightIn(min = 44.dp).clip(RoundedCornerShape(50))
                    .clickable(enabled = !granted, role = Role.Button, onClick = onLater)
                    .padding(horizontal = 18.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text("Not now", color = palette.textMuted, fontSize = 13.5.sp, fontWeight = FontWeight.Medium)
            }
        }
    }

    private fun why(step: DisplayPermission) = when (step) {
        DisplayPermission.Overlay -> "Reach the desktop from any app on your $device."
        DisplayPermission.Accessibility -> "While the desktop has the mouse and keyboard, your $device still works by touch."
    }

    private fun illustration(step: DisplayPermission) = when (step) {
        DisplayPermission.Overlay -> "Portal’s control floats over an app on the $device and reaches the desktop on the monitor"
        DisplayPermission.Accessibility -> "A mouse moves the desktop’s pointer while a finger opens an app on the $device"
    }

    private fun scaled(ms: Int): Int = (ms * PresentedDesktopPad.slow).toInt()
}

@Composable
private fun MonitorChip(name: String, palette: PortalPalette) {
    val shape = RoundedCornerShape(50)
    Row(
        Modifier.clip(shape).background(palette.surfaceTop).border(1.dp, palette.surfaceBorder, shape)
            .padding(start = 12.dp, end = 14.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PortalIcon(PortalGlyph.Monitor, palette.textPrimary, size = 16.dp)
        Spacer(Modifier.width(8.dp))
        Text(
            name, color = palette.textPrimary, fontSize = 12.5.sp, fontWeight = FontWeight.Medium,
            maxLines = 1, overflow = TextOverflow.Ellipsis,
        )
    }
}

/** Opens Android's page for the step; once it is allowed, a check. */
@Composable
private fun SetupButton(label: String, granted: Boolean, palette: PortalPalette, onClick: () -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val press by animateFloatAsState(
        if (pressed) 0.97f else 1f, spring(dampingRatio = 0.8f, stiffness = 650f), label = "display setup press",
    )
    val shape = RoundedCornerShape(50)
    Box(
        Modifier.fillMaxWidth().height(48.dp)
            .graphicsLayer { scaleX = press; scaleY = press }
            .clip(shape).background(palette.accent)
            .clickable(interactionSource = interaction, indication = null, enabled = !granted, role = Role.Button, onClick = onClick)
            .semantics { if (granted) contentDescription = "Done" },
        contentAlignment = Alignment.Center,
    ) {
        AnimatedContent(
            granted,
            transitionSpec = {
                (fadeIn(tween(200, delayMillis = 60)) + scaleIn(tween(320, easing = PortalEmphasized), initialScale = 0.7f))
                    .togetherWith(fadeOut(tween(100)))
            },
            label = "display setup button",
        ) { done ->
            val ink = PortalColors.Charcoal
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (done) {
                    Canvas(Modifier.size(20.dp)) {
                        scale(size.width / 24f, size.height / 24f, pivot = Offset.Zero) {
                            drawLine(ink, Offset(5f, 12.5f), Offset(10f, 17.5f), 2.2f, StrokeCap.Round)
                            drawLine(ink, Offset(10f, 17.5f), Offset(19f, 7f), 2.2f, StrokeCap.Round)
                        }
                    }
                } else {
                    Text(label, color = ink, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                    Spacer(Modifier.width(6.dp))
                    // It opens Android's settings.
                    Canvas(Modifier.size(13.dp)) {
                        scale(size.width / 24f, size.height / 24f, pivot = Offset.Zero) {
                            drawLine(ink, Offset(6f, 18f), Offset(18f, 6f), 2.4f, StrokeCap.Round)
                            drawLine(ink, Offset(9f, 6f), Offset(18f, 6f), 2.4f, StrokeCap.Round)
                            drawLine(ink, Offset(18f, 6f), Offset(18f, 15f), 2.4f, StrokeCap.Round)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun StepDots(count: Int, current: Int, palette: PortalPalette) {
    Row(Modifier.padding(top = 4.dp, bottom = 4.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        for (i in 0 until count) {
            val color by animateColorAsState(
                if (i == current) palette.accent else palette.textMuted.copy(alpha = 0.35f), tween(240), label = "setup dot",
            )
            val width by animateDpAsState(if (i == current) 16.dp else 6.dp, spring(dampingRatio = 1f, stiffness = 420f), label = "setup dot width")
            Box(Modifier.size(width = width, height = 6.dp).clip(CircleShape).background(color))
        }
    }
}

/** 0 before [from], 1 after [to], easing in and out between. */
private fun smooth(from: Float, to: Float, value: Float): Float {
    val t = ((value - from) / (to - from)).coerceIn(0f, 1f)
    return t * t * (3f - 2f * t)
}

/** 1 at [center], falling to 0 at [width] either side. */
private fun bump(value: Float, center: Float, width: Float): Float =
    smooth(0f, 1f, 1f - abs(value - center) / width)

private fun shrink(rect: Rect, factor: Float): Rect {
    val half = Size(rect.width * factor / 2f, rect.height * factor / 2f)
    return Rect(rect.center.x - half.width, rect.center.y - half.height, rect.center.x + half.width, rect.center.y + half.height)
}

private const val SCENE_WIDTH = 240f
private const val SCENE_HEIGHT = 112f

/**
 * This device beside the monitor on a 240 × 112 grid, in the strokes of
 * Portal's glyphs, and what the step gives: [t] runs 0 to 1 once; [granted]
 * pins a check to the device.
 */
private fun DrawScope.drawStage(
    step: DisplayPermission,
    t: Float,
    granted: Float,
    palette: PortalPalette,
    tablet: Boolean,
    cursor: Path,
) {
    val fit = minOf(size.width / SCENE_WIDTH, size.height / SCENE_HEIGHT)
    translate((size.width - SCENE_WIDTH * fit) / 2f, (size.height - SCENE_HEIGHT * fit) / 2f) {
        scale(fit, fit, pivot = Offset.Zero) {
            val ink = palette.textPrimary.copy(alpha = 0.8f)
            val muted = palette.textMuted.copy(alpha = 0.6f)
            val outline = Stroke(1.6f, cap = StrokeCap.Round)
            val device = if (tablet) Rect(8f, 24f, 100f, 88f) else Rect(36f, 4f, 88f, 108f)
            val monitor = Rect(130f, 12f, 232f, 78f)

            // The device with its apps.
            drawRoundRect(ink, device.topLeft, device.size, CornerRadius(if (tablet) 8f else 9f), style = outline)
            drawLine(ink, Offset(device.center.x - 6f, device.bottom - 7f), Offset(device.center.x + 6f, device.bottom - 7f), 1.6f, StrokeCap.Round)
            val cols = if (tablet) 4 else 3
            val rows = if (tablet) 2 else 4
            val tile = if (tablet) 9f else 8f
            val gap = if (tablet) 7f else 5f
            val gridLeft = device.center.x - (cols * tile + (cols - 1) * gap) / 2f
            val gridTop = device.top + if (tablet) 12f else 13f
            fun tile(i: Int) = Offset(gridLeft + (i % cols) * (tile + gap), gridTop + (i / cols) * (tile + gap))
            for (i in 0 until cols * rows) {
                drawRoundRect(palette.textPrimary.copy(alpha = 0.14f), tile(i), Size(tile, tile), CornerRadius(2.5f))
            }

            // The monitor with a window on its desktop, linked to the device.
            drawRoundRect(ink, monitor.topLeft, monitor.size, CornerRadius(5f), style = outline)
            drawLine(ink, Offset(monitor.center.x, monitor.bottom), Offset(monitor.center.x, monitor.bottom + 13f), 1.6f, StrokeCap.Round)
            drawLine(ink, Offset(monitor.center.x - 18f, monitor.bottom + 14f), Offset(monitor.center.x + 18f, monitor.bottom + 14f), 1.6f, StrokeCap.Round)
            drawRoundRect(muted, Offset(monitor.left + 12f, monitor.top + 10f), Size(46f, 28f), CornerRadius(3f), style = Stroke(1.2f))
            drawLine(muted, Offset(monitor.left + 8f, monitor.bottom - 8f), Offset(monitor.right - 8f, monitor.bottom - 8f), 1.2f, StrokeCap.Round)
            val linkFrom = if (step == DisplayPermission.Overlay) 0.48f else 2f
            for (i in 0..2) {
                val x = device.right + 8f + (monitor.left - device.right - 16f) * i / 2f
                val pulse = bump(t, linkFrom + i * 0.06f, 0.08f)
                drawCircle(lerpColor(palette.textMuted.copy(alpha = 0.45f), palette.accent, pulse), 1.5f + 0.6f * pulse, Offset(x, monitor.top + monitor.height * 0.6f))
            }

            when (step) {
                DisplayPermission.Overlay -> {
                    // Portal's control slides in over the app, on the side facing
                    // the monitor; a tap on it reaches the desktop.
                    val slide = smooth(0.06f, 0.36f, t)
                    val pill = Rect(Offset(device.right - 14f + (1f - slide) * 7f, device.center.y - 12f), Size(10f, 24f))
                    if (slide > 0f) {
                        drawRoundRect(palette.accent.copy(alpha = slide), pill.topLeft, pill.size, CornerRadius(5f))
                        val chevron = PortalColors.Charcoal.copy(alpha = slide)
                        val c = pill.center
                        drawLine(chevron, Offset(c.x - 1.4f, c.y - 2.8f), Offset(c.x + 1.4f, c.y), 1.3f, StrokeCap.Round)
                        drawLine(chevron, Offset(c.x + 1.4f, c.y), Offset(c.x - 1.4f, c.y + 2.8f), 1.3f, StrokeCap.Round)
                    }
                    drawTap(pill.center, t, 0.40f, palette)
                    val shown = smooth(0.62f, 0.70f, t)
                    if (shown > 0f) {
                        val tip = lerp(Offset(monitor.left + 70f, monitor.top + 44f), Offset(monitor.left + 44f, monitor.top + 22f), smooth(0.64f, 0.96f, t))
                        drawCursor(tip, palette.textPrimary.copy(alpha = shown), cursor)
                    }
                }
                DisplayPermission.Accessibility -> {
                    // The mouse moves the desktop's pointer while a finger opens
                    // an app on the device.
                    val shift = -7f * smooth(0.08f, 0.40f, t) + 4f * smooth(0.40f, 0.62f, t)
                    val mouse = Rect(Offset(206f + shift, 86f), Size(11f, 18f))
                    drawRoundRect(ink, mouse.topLeft, mouse.size, CornerRadius(5.5f), style = outline)
                    drawLine(ink, Offset(mouse.center.x, mouse.top + 2.5f), Offset(mouse.center.x, mouse.top + 6.5f), 1.4f, StrokeCap.Round)
                    drawCursor(Offset(monitor.left + 62f + shift * 3.4f, monitor.top + 34f + shift * 1.4f), palette.textPrimary, cursor)
                    val app = tile(if (tablet) 2 else 4)
                    val lit = smooth(0.56f, 0.68f, t)
                    if (lit > 0f) drawRoundRect(palette.accent.copy(alpha = lit), app, Size(tile, tile), CornerRadius(2.5f))
                    drawTap(Offset(app.x + tile / 2f, app.y + tile / 2f), t, 0.48f, palette)
                }
            }

            if (granted > 0f) {
                val c = Offset(device.right - 1f, device.top + 3f)
                val r = 8f * granted
                drawCircle(palette.background, r + 2f, c)
                drawCircle(palette.accent, r, c)
                if (granted > 0.4f) {
                    val ink2 = PortalColors.Charcoal
                    drawLine(ink2, Offset(c.x - 0.45f * r, c.y + 0.02f * r), Offset(c.x - 0.1f * r, c.y + 0.36f * r), 1.6f, StrokeCap.Round)
                    drawLine(ink2, Offset(c.x - 0.1f * r, c.y + 0.36f * r), Offset(c.x + 0.45f * r, c.y - 0.3f * r), 1.6f, StrokeCap.Round)
                }
            }
        }
    }
}

/** A fingertip landing at [at] from [from] on, with a ripple, then lifting. */
private fun DrawScope.drawTap(at: Offset, t: Float, from: Float, palette: PortalPalette) {
    val ring = smooth(from + 0.06f, from + 0.38f, t)
    if (ring > 0f && ring < 1f) drawCircle(palette.accent.copy(alpha = (1f - ring) * 0.8f), 4f + 9f * ring, at, style = Stroke(1.2f))
    val finger = smooth(from, from + 0.08f, t) * (1f - smooth(from + 0.32f, from + 0.42f, t))
    if (finger > 0f) drawCircle(palette.textPrimary.copy(alpha = 0.85f * finger), 3.4f, at)
}

private fun DrawScope.drawCursor(tip: Offset, color: Color, path: Path) {
    path.reset()
    path.moveTo(tip.x, tip.y)
    path.lineTo(tip.x, tip.y + 12f)
    path.lineTo(tip.x + 3.2f, tip.y + 9.2f)
    path.lineTo(tip.x + 5.6f, tip.y + 14f)
    path.lineTo(tip.x + 7.6f, tip.y + 13f)
    path.lineTo(tip.x + 5.3f, tip.y + 8.4f)
    path.lineTo(tip.x + 9.4f, tip.y + 8.4f)
    path.close()
    drawPath(path, color)
}
