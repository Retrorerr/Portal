package app.polarbear.setup

// Return-to-Plasma screen for already-installed launches (cold start after
// force-close included). This is intentionally minimal: the EXACT shared
// PortalAmbientBackground (same charcoal, same seven blurred fragments, same
// drift, same 720 ms scatter, same 1.0 -> 0.83 alpha), the official Portal
// mark, and two lines of text. A quiet inline graphics affordance is added
// only when native state says an explicit Anland repair is available; it is
// part of this layout rather than a separate control. Pending Debian updates
// use the same slot (ReturnUpdatePanel.kt).
//
// READY uses the SAME path as the installer: a simple 3 second timer,
// started once this screen is presented, sets the shared readyPrelude, which
// drives the identical fragment scatter + translucency + PortalRevealVeil +
// swipe affordance through PortalLaunchTransition. No second animation
// implementation anywhere.

import android.util.Log
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.fadeIn
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.animateDp
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.core.updateTransition
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.material3.Text
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.unit.coerceAtLeast
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import app.polarbear.setup.components.PortalPanelStiffness
import kotlin.math.roundToInt
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.animation.expandIn
import androidx.compose.animation.shrinkOut
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.platform.LocalContext
import app.polarbear.setup.components.LocalPortalVeil
import app.polarbear.setup.components.SetupChecklist
import app.polarbear.setup.components.UsageTips
import app.polarbear.setup.components.usageTipsPending
import app.polarbear.setup.components.setupChecklistPending
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalPanelSizeSpring
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import app.polarbear.setup.components.dissolveBlur
import app.polarbear.ComposeOverlay
import kotlinx.coroutines.delay

private const val TAG = "PortalReturn"
private const val RETURN_READY_DELAY_MS = 3_000L
private const val UNBOUNDED_CARD_HEIGHT = 1_000_000

// The swipe instruction and its handle, with their margins.
private val VEIL_HINT_RESERVE = 60.dp
private val SCROLL_FADE = 24.dp
private val STACKED_CARD_GAP = 28.dp
private val SHORT_SCREEN_HEIGHT = 560.dp
// Wide screens too short to stack the header over a card put them side by side.
private val SIDE_BY_SIDE_MIN_WIDTH = 680.dp
private val SIDE_BY_SIDE_MAX_HEIGHT = 960.dp
private val SIDE_BY_SIDE_GUTTER = 36.dp
// An open panel scrolls inside what the screen has left, down to this much.
private val PANEL_HEADER_MIN_HEIGHT = 240.dp
// The gap between the header and the row of pills (ReturnAppsPanel).
private val PILL_ROW_GAP = 14.dp
// The least an open panel is given, however little the screen has.
private val PANEL_MIN_ROOM = 160.dp
// The least a card keeps with the keyboard up; below this the page scrolls.
private val PANEL_MIN_ROOM_WITH_KEYBOARD = 96.dp

/**
 * The height an open Apps, Files, Workspaces or report card may take: what the
 * screen has left under the header, or all of it where the header folds away.
 * A card taller than this scrolls inside it.
 */
internal val LocalVeilPanelRoom = compositionLocalOf { Dp.Unspecified }

/** [LocalVeilPanelRoom], up to [most]; [most] where there is none. */
@Composable
internal fun veilPanelRoom(most: Dp): Dp {
    val room = LocalVeilPanelRoom.current
    return if (room == Dp.Unspecified) most else minOf(room, most)
}

/** How much of [LocalVeilPanelRoom] the keyboard takes, in pixels; read while measuring. */
internal val LocalVeilKeyboardLift = staticCompositionLocalOf<Density.() -> Int> { { 0 } }

/**
 * At most [LocalVeilPanelRoom] tall, less whatever the keyboard covers, so a
 * card with a text field ends above the keyboard as it slides.
 */
@Composable
internal fun Modifier.veilPanelHeight(): Modifier {
    val room = LocalVeilPanelRoom.current
    if (room == Dp.Unspecified) return this
    val lift = LocalVeilKeyboardLift.current
    return layout { measurable, constraints ->
        val most = (room.roundToPx() - lift()).coerceAtLeast(PANEL_MIN_ROOM_WITH_KEYBOARD.roundToPx())
        val maxHeight = minOf(constraints.maxHeight, most)
        val placeable = measurable.measure(
            constraints.copy(minHeight = minOf(constraints.minHeight, maxHeight), maxHeight = maxHeight),
        )
        layout(placeable.width, placeable.height) { placeable.placeRelative(0, 0) }
    }
}

private enum class ReturnRepairPhase {
    Hidden,
    Available,
    Running,
    Failed,
    RecoveryRequested,
}

@Composable
private fun ReturnAnlandAffordance(
    phase: ReturnRepairPhase,
    progress: Int,
    palette: PortalPalette,
    onEnable: () -> Unit,
    onRecovery: () -> Unit,
) {
    val visible = phase != ReturnRepairPhase.Hidden
    val phaseTransition = updateTransition(
        targetState = phase,
        label = "return graphics affordance",
    )
    val contentAlpha by phaseTransition.animateFloat(
        transitionSpec = { tween(260) },
        label = "return graphics opacity",
    ) { target ->
        if (target == ReturnRepairPhase.RecoveryRequested) 0.78f else 1f
    }
    val contentLift by phaseTransition.animateDp(
        transitionSpec = {
            spring(
                dampingRatio = Spring.DampingRatioNoBouncy,
                stiffness = 520f,
            )
        },
        label = "return graphics movement",
    ) { target ->
        if (target == ReturnRepairPhase.Available) 0.dp else (-1).dp
    }
    val glowAlpha by phaseTransition.animateFloat(
        transitionSpec = { tween(380, easing = FastOutSlowInEasing) },
        label = "return graphics glow",
    ) { target ->
        when (target) {
            ReturnRepairPhase.Available -> 0.14f
            ReturnRepairPhase.Running -> 0.42f
            ReturnRepairPhase.Failed -> 0.08f
            else -> 0f
        }
    }
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val interactive = phase == ReturnRepairPhase.Available || phase == ReturnRepairPhase.Failed
    val pressScale by animateFloatAsState(
        targetValue = if (interactive && pressed) 0.985f else 1f,
        animationSpec = tween(110),
        label = "return graphics press",
    )

    AnimatedVisibility(
        visible = visible,
        enter = expandVertically(tween(420, easing = FastOutSlowInEasing), expandFrom = Alignment.Top) +
            fadeIn(tween(420, delayMillis = 60, easing = PortalEmphasizedDecelerate)) +
            slideInVertically(tween(560, easing = PortalEmphasized)) { it / 3 } +
            scaleIn(tween(560, easing = PortalEmphasized), initialScale = 0.97f),
        exit = fadeOut(tween(220, easing = PortalEmphasizedAccelerate)) +
            slideOutVertically(tween(260, easing = PortalEmphasizedAccelerate)) { -it / 5 } +
            scaleOut(tween(260), targetScale = 0.98f) +
            shrinkVertically(tween(420, easing = FastOutSlowInEasing), shrinkTowards = Alignment.Top),
    ) {
        AnimatedContent(
            targetState = phase,
            transitionSpec = {
                (fadeIn(tween(380, delayMillis = 70, easing = PortalEmphasizedDecelerate)) +
                    slideInVertically(tween(520, easing = PortalEmphasized)) { it / 4 } +
                    scaleIn(tween(520, easing = PortalEmphasized), initialScale = 0.975f))
                    .togetherWith(
                        fadeOut(tween(190, easing = PortalEmphasizedAccelerate)) +
                            slideOutVertically(tween(240, easing = PortalEmphasizedAccelerate)) { -it / 5 } +
                            scaleOut(tween(240), targetScale = 0.985f),
                    )
                    .using(
                        SizeTransform(clip = false) { _, _ ->
                            tween(360, easing = FastOutSlowInEasing)
                        },
                    )
            },
            contentAlignment = Alignment.Center,
            label = "return graphics state",
        ) { target ->
            val click = when (target) {
                ReturnRepairPhase.Available -> onEnable
                ReturnRepairPhase.Failed -> onRecovery
                else -> ({ })
            }
            Column(
                modifier = Modifier
                    .dissolveBlur(this, radius = 10.dp)
                    .animateContentSize(
                        animationSpec = tween(360, easing = FastOutSlowInEasing),
                    )
                    .graphicsLayer {
                        alpha = contentAlpha
                        translationY = contentLift.toPx()
                        scaleX = pressScale
                        scaleY = pressScale
                    }
                    .padding(top = 18.dp, start = 10.dp, end = 10.dp, bottom = 2.dp)
                    .then(
                        if (interactive) {
                            Modifier.clickable(
                                interactionSource = tap,
                                indication = null,
                                role = Role.Button,
                                onClick = click,
                            )
                        } else {
                            Modifier
                        },
                    ),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                when (target) {
                    ReturnRepairPhase.Available -> Text(
                        text = "Accelerated graphics available →",
                        fontSize = 12.sp,
                        fontWeight = FontWeight.Medium,
                        color = palette.textMuted.copy(alpha = 0.88f),
                        textAlign = TextAlign.Center,
                    )

                    ReturnRepairPhase.Running -> Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        Text(
                            text = "Enabling accelerated graphics…",
                            fontSize = 14.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textPrimary,
                            textAlign = TextAlign.Center,
                        )
                        Spacer(modifier = Modifier.height(5.dp))
                        Text(
                            text = "Preparing Anland and GPU acceleration",
                            fontSize = 11.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textMuted,
                            textAlign = TextAlign.Center,
                        )
                        ReturnRepairProgress(
                            progress = progress,
                            palette = palette,
                            glowAlpha = glowAlpha,
                        )
                    }

                    ReturnRepairPhase.Failed -> Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        Text(
                            text = "Acceleration could not be enabled",
                            fontSize = 14.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textPrimary,
                            textAlign = TextAlign.Center,
                        )
                        Spacer(modifier = Modifier.height(5.dp))
                        Text(
                            text = "Your desktop is unchanged",
                            fontSize = 11.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textMuted,
                            textAlign = TextAlign.Center,
                        )
                        Spacer(modifier = Modifier.height(10.dp))
                        Text(
                            text = "Retry Plasma →",
                            fontSize = 12.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textMuted.copy(alpha = 0.9f),
                            textAlign = TextAlign.Center,
                        )
                    }

                    ReturnRepairPhase.RecoveryRequested -> Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        Text(
                            text = "Returning to Plasma…",
                            fontSize = 14.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textPrimary,
                            textAlign = TextAlign.Center,
                        )
                        Spacer(modifier = Modifier.height(5.dp))
                        Text(
                            text = "Keeping your desktop safe",
                            fontSize = 11.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textMuted,
                            textAlign = TextAlign.Center,
                        )
                    }

                    ReturnRepairPhase.Hidden -> Spacer(modifier = Modifier.size(0.dp))
                }
            }
        }
    }
}

@Composable
private fun ReturnRepairProgress(
    progress: Int,
    palette: PortalPalette,
    glowAlpha: Float,
) {
    val target = (progress.coerceIn(0, 99) / 100f).coerceIn(0f, 0.99f)
    val animatedProgress by animateFloatAsState(
        targetValue = target,
        animationSpec = tween(360, easing = FastOutSlowInEasing),
        label = "native graphics progress",
    )
    Canvas(
        modifier = Modifier
            .padding(top = 14.dp)
            .size(width = 112.dp, height = 4.dp),
    ) {
        val radius = size.height / 2f
        drawRoundRect(
            color = palette.textPrimary.copy(alpha = 0.12f),
            cornerRadius = androidx.compose.ui.geometry.CornerRadius(radius, radius),
        )
        val progressWidth = size.width * animatedProgress
        if (progressWidth > 0f) {
            drawRoundRect(
                color = palette.accent.copy(alpha = glowAlpha),
                topLeft = androidx.compose.ui.geometry.Offset.Zero,
                size = androidx.compose.ui.geometry.Size(progressWidth, size.height),
                cornerRadius = androidx.compose.ui.geometry.CornerRadius(radius, radius),
                style = Stroke(width = 3.dp.toPx()),
            )
            drawRoundRect(
                color = palette.accent.copy(alpha = 0.84f),
                topLeft = androidx.compose.ui.geometry.Offset.Zero,
                size = androidx.compose.ui.geometry.Size(progressWidth, size.height),
                cornerRadius = androidx.compose.ui.geometry.CornerRadius(radius, radius),
            )
        }
    }
}

/**
 * A scroller inside the veil: edges fade where more content waits, and while it
 * can scroll it keeps upward swipes, which would otherwise lift the veil.
 */
@Composable
internal fun Modifier.veilScrollRegion(state: ScrollState): Modifier {
    val veil = LocalPortalVeil.current
    val key = remember { Any() }
    val bounds = remember { arrayOfNulls<Rect>(1) }
    // Derived: the limit moves every frame while a panel or card resizes.
    val scrollable by remember(state) { derivedStateOf { state.maxValue > 0 } }
    DisposableEffect(veil) { onDispose { veil.gestureExclusions.remove(key) } }
    LaunchedEffect(scrollable) {
        val known = bounds[0]
        if (scrollable && known != null) veil.gestureExclusions[key] = known
        else veil.gestureExclusions.remove(key)
    }
    return onGloballyPositioned {
        val inRoot = it.boundsInRoot()
        bounds[0] = inRoot
        if (scrollable) veil.gestureExclusions[key] = inRoot
    }
        .graphicsLayer {
            compositingStrategy = if (scrollable) CompositingStrategy.Offscreen else CompositingStrategy.Auto
        }
        .drawWithContent {
            drawContent()
            if (!scrollable) return@drawWithContent
            val edge = SCROLL_FADE.toPx()
            val top = (state.value / edge).coerceIn(0f, 1f)
            val bottom = ((state.maxValue - state.value) / edge).coerceIn(0f, 1f)
            drawRect(
                brush = Brush.verticalGradient(
                    0f to Color.Black.copy(alpha = 1f - top),
                    edge / size.height to Color.Black,
                    1f - edge / size.height to Color.Black,
                    1f to Color.Black.copy(alpha = 1f - bottom),
                ),
                blendMode = BlendMode.DstIn,
            )
        }
}

@Composable
internal fun PortalReturnScreen(
    launchMarkModifier: Modifier,
    sessionControls: Boolean = false,
    onReturnReady: () -> Unit = {},
    onRepairBlockedChanged: (Boolean) -> Unit = {},
) {
    val palette = resolvePalette(AppearanceMode.System)
    val veil = LocalPortalVeil.current
    SideEffect { veil.ink = palette.textPrimary }
    var returnReady by remember { mutableStateOf(sessionControls) }
    var repairRequested by remember { mutableStateOf(false) }
    var recoveryRequested by remember { mutableStateOf(false) }
    val repairState by ComposeOverlay.anlandRepairState()
    val currentReturnReady by rememberUpdatedState(onReturnReady)
    val currentRepairBlocked by rememberUpdatedState(onRepairBlockedChanged)
    val repairPhase = when {
        recoveryRequested -> ReturnRepairPhase.RecoveryRequested
        repairState.failed -> ReturnRepairPhase.Failed
        repairState.complete -> ReturnRepairPhase.Hidden
        repairState.running -> ReturnRepairPhase.Running
        repairRequested -> ReturnRepairPhase.Running
        repairState.available -> ReturnRepairPhase.Available
        else -> ReturnRepairPhase.Hidden
    }
    val repairBlocked = repairPhase == ReturnRepairPhase.Running ||
        repairPhase == ReturnRepairPhase.Failed ||
        repairPhase == ReturnRepairPhase.RecoveryRequested
    var updateBlocked by remember { mutableStateOf(false) }
    val context = LocalContext.current
    // Decided once per screen: ticking a step off keeps the card (showing it
    // done) instead of yanking it away mid-read.
    val checklistPending = remember { !sessionControls && setupChecklistPending(context) }
    val tipsPending = remember { !sessionControls && !checklistPending && usageTipsPending(context) }
    LaunchedEffect(repairBlocked, updateBlocked) {
        currentRepairBlocked(repairBlocked || updateBlocked)
    }
    LaunchedEffect(Unit) {
        if (!sessionControls) delay(RETURN_READY_DELAY_MS)
        returnReady = true
        Log.i(TAG, "return READY; starting ambient scatter and translucent veil prelude")
        currentReturnReady()
    }
    Box(modifier = Modifier.fillMaxSize().dismissPortalPanelOutside(veil)) {
        PortalAmbientBackground(
            palette = palette,
            cardBounds = { Rect.Zero },
            readyPrelude = returnReady && !repairBlocked && !updateBlocked,
        )
        WorkspaceSwitchBackdrop()
        // The swipe hint keeps its own band at the bottom; nothing scrolls under it.
        BoxWithConstraints(
            modifier = Modifier
                .fillMaxSize()
                .windowInsetsPadding(WindowInsets.systemBars.union(WindowInsets.displayCutout))
                .padding(bottom = VEIL_HINT_RESERVE),
        ) {
            val shortScreen = maxHeight < SHORT_SCREEN_HEIGHT
            val sideBySide = maxWidth >= SIDE_BY_SIDE_MIN_WIDTH && maxHeight < SIDE_BY_SIDE_MAX_HEIGHT
            val verticalPadding = if (shortScreen) 12.dp else 24.dp
            val pageScroll = rememberScrollState()
            val cardScroll = rememberScrollState()
            val panelOpen = ComposeOverlay.activePanel != null
            val viewportHeight = constraints.maxHeight
            val density = LocalDensity.current
            // How far the keyboard reaches above the band kept for the swipe
            // hint, which it covers. Read while measuring, so its slide only
            // re-measures the page.
            val keyboardInsets = WindowInsets.ime
            val barInsets = WindowInsets.systemBars.union(WindowInsets.displayCutout)
            val keyboardLift: Density.() -> Int = remember(keyboardInsets, barInsets) {
                {
                    (keyboardInsets.getBottom(this) - barInsets.getBottom(this) - VEIL_HINT_RESERVE.roundToPx())
                        .coerceAtLeast(0)
                }
            }
            val keyboardUp by remember(keyboardLift, density) { derivedStateOf { density.keyboardLift() > 0 } }
            // A short screen (a phone on its side), or the keyboard, gives an
            // open card the whole height: the mark, title and status fold away
            // above it and come back as it closes.
            val foldHeader = panelOpen && (shortScreen || keyboardUp)
            val headerShown by animateFloatAsState(
                if (foldHeader) 0f else 1f,
                spring(dampingRatio = Spring.DampingRatioNoBouncy, stiffness = PortalPanelStiffness),
                label = "return header fold",
            )
            var brandHeight by remember { mutableIntStateOf(0) }
            val panelRoom = (maxHeight - verticalPadding * 2 - PILL_ROW_GAP -
                (if (foldHeader) 0.dp else with(density) { brandHeight.toDp() })).coerceAtLeast(PANEL_MIN_ROOM)
            Layout(
                modifier = Modifier
                    .fillMaxSize()
                    // A workspace name field stays above the keyboard.
                    .layout { measurable, constraints ->
                        val lift = keyboardLift()
                        val placeable = measurable.measure(
                            constraints.copy(
                                minHeight = (constraints.minHeight - lift).coerceAtLeast(0),
                                maxHeight = (constraints.maxHeight - lift).coerceAtLeast(0),
                            ),
                        )
                        layout(placeable.width, placeable.height + lift) { placeable.placeRelative(0, 0) }
                    }
                    .veilScrollRegion(pageScroll)
                    .verticalScroll(pageScroll)
                    .padding(
                        horizontal = if (maxWidth < PortalDimens.CompactBreakpoint) 20.dp else 32.dp,
                        vertical = verticalPadding,
                    ),
                content = {
            // No animateContentSize here: it anchors top-start and clips, which
            // cut off the update card's bottom-right edge while it grew. The
            // affordances below animate their own size instead.
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Column(
                    modifier = Modifier
                        .layout { measurable, constraints ->
                            // Folds from the top down to the pills, which stay put.
                            val placeable = measurable.measure(constraints)
                            val shown = headerShown
                            val height = (placeable.height * shown).roundToInt()
                            layout(placeable.width, height) {
                                if (shown > 0.001f) placeable.placeRelativeWithLayer(0, height - placeable.height) {
                                    alpha = shown * shown
                                    val scale = 0.94f + 0.06f * shown
                                    scaleX = scale
                                    scaleY = scale
                                    transformOrigin = androidx.compose.ui.graphics.TransformOrigin(0.5f, 1f)
                                }
                            }
                        }
                        .onSizeChanged { brandHeight = it.height },
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Image(
                        painter = portalMarkPainter(
                            main = palette.logoMain,
                            threshold = palette.logoThreshold,
                        ),
                        contentDescription = "Portal logo",
                        modifier = Modifier
                            .size(if (shortScreen) 56.dp else PortalDimens.LogoSize)
                            .then(launchMarkModifier),
                    )
                    Spacer(modifier = Modifier.height(if (shortScreen) 10.dp else 20.dp))
                    Text(
                        text = if (sessionControls) "Portal" else "Returning to Plasma",
                        fontSize = if (shortScreen) 24.sp else PortalDimens.TitleSize,
                        fontWeight = FontWeight.SemiBold,
                        color = palette.textPrimary,
                        textAlign = TextAlign.Center,
                    )
                    Spacer(modifier = Modifier.height(if (shortScreen) 4.dp else 8.dp))
                    // The line settles from "restoring" to "ready" the moment the
                    // veil may be lifted, dissolving through a soft blur.
                    val workspaces by app.polarbear.Workspaces.state
                    val openingId by app.polarbear.Workspaces.opening
                    val opening = (openingId ?: workspaces.switching)?.let { workspaces.name(it) }
                    AnimatedContent(
                        targetState = veil.eligible to opening,
                        transitionSpec = {
                            (fadeIn(tween(420, delayMillis = 90, easing = PortalEmphasizedDecelerate)) +
                                slideInVertically(tween(560, easing = PortalEmphasized)) { it / 2 })
                                .togetherWith(
                                    fadeOut(tween(200, easing = PortalEmphasizedAccelerate)) +
                                        slideOutVertically(tween(260, easing = PortalEmphasizedAccelerate)) { -it / 3 },
                                )
                                .using(SizeTransform(clip = false))
                        },
                        contentAlignment = Alignment.Center,
                        label = "return subtitle",
                    ) { (desktopReady, openingName) ->
                        Text(
                            text = when {
                                openingName != null -> "Opening $openingName…"
                                sessionControls -> "Your desktop is running"
                                desktopReady -> "Your desktop is ready"
                                else -> "Restoring your desktop"
                            },
                            modifier = Modifier.dissolveBlur(this, radius = 8.dp),
                            fontSize = 14.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textMuted,
                            textAlign = TextAlign.Center,
                        )
                    }
                    ReturnAnlandAffordance(
                        phase = repairPhase,
                        progress = repairState.progress,
                        palette = palette,
                        onEnable = {
                            if (ComposeOverlay.repairEnableAnland()) {
                                repairRequested = true
                            }
                        },
                        onRecovery = {
                            if (ComposeOverlay.requestAnlandRepairRecovery()) {
                                recoveryRequested = true
                            }
                        },
                    )
                    // Debian updates share this slot; the graphics repair wins.
                    ReturnUpdatePanel(
                        palette = palette,
                        desktopReady = veil.eligible,
                        suppressed = repairPhase != ReturnRepairPhase.Hidden,
                        onBlockedChanged = { updateBlocked = it },
                        immediate = sessionControls,
                    )
                }
                // Apps, Files and Workspaces, one row of pills. Hidden while a
                // repair or Debian upgrade owns the runtime; while a workspace
                // opens, only its pill stays.
                CompositionLocalProvider(
                    LocalVeilPanelRoom provides panelRoom,
                    LocalVeilKeyboardLift provides keyboardLift,
                ) {
                    ReturnAppsPanel(
                        palette = palette,
                        suppressed = repairPhase != ReturnRepairPhase.Hidden || updateBlocked,
                        immediate = sessionControls,
                    )
                }
            }
            // Installs from before the checklist existed, or that skipped a
            // step, see their card here until every step is done. Optional
            // tips wait until then, so the screen never stacks two cards.
            Box {
                // The gap belongs to the card so it grows and shrinks with it.
                val cardModifier = Modifier
                    .padding(
                        start = if (sideBySide) SIDE_BY_SIDE_GUTTER else 0.dp,
                        top = if (sideBySide) 0.dp else STACKED_CARD_GAP,
                    )
                    .widthIn(max = PortalDimens.PickerMaxWidth)
                    // One width for the card's life, however few tips remain.
                    .fillMaxWidth()
                    .veilScrollRegion(cardScroll)
                    .verticalScroll(cardScroll)
                AnimatedVisibility(checklistPending && !panelOpen,
                    enter = expandIn(PortalPanelSizeSpring, expandFrom = Alignment.Center) + fadeIn(tween(300)),
                    exit = shrinkOut(PortalPanelSizeSpring, shrinkTowards = Alignment.Center) + fadeOut(tween(140))) {
                    SetupChecklist(
                        palette = palette,
                        title = "Finish setting up Android",
                        modifier = cardModifier,
                    )
                }
                AnimatedVisibility(tipsPending && !panelOpen,
                    enter = expandIn(PortalPanelSizeSpring, expandFrom = Alignment.Center) + fadeIn(tween(300)),
                    exit = shrinkOut(PortalPanelSizeSpring, shrinkTowards = Alignment.Center) + fadeOut(tween(140))) {
                    UsageTips(
                        palette = palette,
                        modifier = cardModifier,
                    )
                }
            }
                },
            ) { measurables, outer ->
                val width = outer.maxWidth
                val available = (viewportHeight - keyboardLift() - 2 * verticalPadding.roundToPx()).coerceAtLeast(0)
                val gutter = SIDE_BY_SIDE_GUTTER.roundToPx()
                val cardWidth = if (sideBySide) minOf(width * 56 / 100, PortalDimens.PickerMaxWidth.roundToPx()) + gutter else width
                // A panel scrolls inside the space left instead of overflowing the
                // page, which would stop the logo dead against the top on the way.
                val headerHeight = if (panelOpen) maxOf(available, PANEL_HEADER_MIN_HEIGHT.roundToPx()) else Constraints.Infinity
                val card = measurables[1].measure(
                    // A finite height the card's scroller can measure against
                    // when the whole page scrolls instead; the page has no limit.
                    Constraints.fitPrioritizingWidth(
                        minWidth = 0,
                        maxWidth = cardWidth,
                        minHeight = 0,
                        maxHeight = if (sideBySide) available else UNBOUNDED_CARD_HEIGHT,
                    ),
                )
                if (sideBySide) {
                    // An open panel takes the card's place, so it may use the whole
                    // width, and so does the header when there is no card beside it.
                    val headerWidth = if (panelOpen) width else (width - card.width).coerceAtLeast(0)
                    val header = measurables[0].measure(Constraints(maxWidth = headerWidth, maxHeight = headerHeight))
                    val height = maxOf(available, header.height, card.height)
                    layout(width, height) {
                        val x = ((width - header.width - card.width) / 2).coerceAtLeast(0)
                        header.placeRelative(x, (height - header.height) / 2)
                        card.placeRelative(x + header.width, (height - card.height) / 2)
                    }
                } else {
                    val header = measurables[0].measure(Constraints(maxWidth = width, maxHeight = headerHeight))
                    val total = header.height + card.height
                    val height = maxOf(available, total)
                    layout(width, height) {
                        val y = (height - total) / 2
                        header.placeRelative((width - header.width) / 2, y)
                        card.placeRelative((width - card.width) / 2, y + header.height)
                    }
                }
            }
        }
    }
}
