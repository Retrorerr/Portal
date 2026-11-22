package app.polarbear.setup

// Portal first-run setup. Compose owns only the presentation and the local
// choices; the process-lifetime Rust coordinator owns installation and
// publishes the durable provisioning snapshot read below.
//
// Setup is a short walk through one card: a welcome, the Android setting the
// desktop depends on (only while it is off), how the desktop looks, which apps
// come with it, and the install itself. The same walk sets up a new
// workspace, introduced by name. Each stage keeps one job; the card, the
// Portal mark and the primary button stay where they are while the stage
// changes inside it (see SetupFrame.kt for how the card fits each screen).

import android.animation.ValueAnimator
import android.content.Context
import android.content.res.Configuration
import android.os.SystemClock
import android.util.Log
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.ExperimentalSharedTransitionApi
import androidx.compose.animation.SharedTransitionLayout
import androidx.compose.animation.SharedTransitionScope
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.graphics.vector.rememberVectorPainter
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.util.lerp
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import app.polarbear.ComposeOverlay
import app.polarbear.Workspaces
import app.polarbear.setup.components.INCLUDED_APPS
import app.polarbear.setup.components.LocalPortalVeil
import app.polarbear.setup.components.PlasmaFrostBackdrop
import app.polarbear.setup.components.PortalAgslGlow
import app.polarbear.setup.components.PortalAmbientFragments
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import app.polarbear.setup.components.PortalFrostStyle
import app.polarbear.setup.components.portalBloom
import app.polarbear.setup.components.rememberAppIcons
import app.polarbear.setup.components.rememberPlasmaSnapshot
import app.polarbear.setup.components.rememberPrepareState
import app.polarbear.setup.components.rememberStorageCapacity
import app.polarbear.setup.components.rememberThemeReveal
import app.polarbear.setup.components.setupChecklistApplies
import app.polarbear.setup.components.setupPrepareNeeded
import app.polarbear.setup.components.themeReveal
import kotlinx.coroutines.delay
import kotlin.math.roundToInt

private const val PREVIEW_TAG = "PortalComposeSetup"
private const val READY_BACKGROUND_ALPHA = 0.83f
internal enum class SetupPhase { Configure, Installing, Failed, Ready }

// Official Portal aperture geometry (assets/portal-icon-foreground.svg),
// translated into the tight visible artwork bounds: the mark occupies
// x[336,681] y[245,759] at 54-unit stroke, i.e. a 345x514 box once the
// stroke is included. Coordinates below are exactly the SVG values shifted
// by (-336,-245); relative geometry and stroke widths are untouched, so the
// mark fills its allocation instead of floating in empty viewport.
internal const val APERTURE_PATH =
    "M188,473 C149,487 112,473 85,438 C27,364 38,221 87,126 " +
        "C124,54 186,27 236,57 C302,97 318,208 287,308"
internal const val THRESHOLD_PATH = "M281,353 C267,390 249,419 225,438"

@Composable
fun portalMarkPainter(main: Color, threshold: Color) = rememberVectorPainter(
    image = remember(main, threshold) {
        ImageVector.Builder(
            name = "portal",
            defaultWidth = 56.dp,
            defaultHeight = 84.dp,
            viewportWidth = 345f,
            viewportHeight = 514f,
        )
            .addPath(
                pathData = addPathNodes(APERTURE_PATH),
                stroke = SolidColor(main),
                strokeLineWidth = 54f,
                strokeLineCap = StrokeCap.Butt,
            )
            .addPath(
                pathData = addPathNodes(THRESHOLD_PATH),
                stroke = SolidColor(threshold),
                strokeLineWidth = 54f,
                strokeLineCap = StrokeCap.Butt,
            )
            .build()
    },
)

/** The workspace a setup belongs to, when it is not Portal's first one. */
internal data class WorkspaceSetup(val name: String, val backName: String)

@OptIn(ExperimentalSharedTransitionApi::class)
@Composable
fun PortalSetupScreen(
    desktopReady: Boolean = false,
    onSetupReady: () -> Unit = {},
    launchMarkModifier: Modifier = Modifier,
    // Debug preview: shows the first-run stages over a finished install and
    // never reads or starts a real installation.
    previewOnly: Boolean = false,
    // Debug preview only: the install button plays a scripted install instead
    // of the message above ("ok", or "fail" to stop once and offer Try again).
    previewInstall: String? = null,
    // Debug preview only: open on this stage (welcome, prepare, look, apps).
    previewStage: String? = null,
    // Debug preview only: present setup as that of a new workspace so named.
    previewWorkspace: String? = null,
) {
    var appearance by remember { mutableStateOf(AppearanceMode.System) }
    // What is painted. It trails [appearance] by the capture of one frame so
    // the theme reveal can freeze the old theme before the repaint.
    var paintedAppearance by remember { mutableStateOf(AppearanceMode.System) }
    var interfaceSize by remember { mutableStateOf(InterfaceSize.Balanced) }
    val cardBounds = remember { mutableStateOf(Rect.Zero) }
    var rootOrigin by remember { mutableStateOf(Offset.Zero) }
    var optionalAppIds by remember { mutableStateOf(DEFAULT_OPTIONAL_APP_IDS) }
    var lastPress by remember { mutableStateOf(Offset.Unspecified) }
    val realInstallState by ComposeOverlay.installState()
    val diagnosticsExport by ComposeOverlay.diagnosticsExportState()
    var scriptedInstall by remember { mutableStateOf<ComposeOverlay.InstallUiState?>(null) }
    var scriptedRun by remember { mutableIntStateOf(0) }
    var scriptedDesktopReady by remember { mutableStateOf(false) }
    if (previewOnly && previewInstall != null && scriptedRun > 0) {
        LaunchedEffect(scriptedRun) {
            playScriptedInstall(
                fail = previewInstall == "fail" && scriptedRun == 1,
                resume = scriptedRun > 1,
                update = { scriptedInstall = it },
            )
            if (scriptedInstall?.complete == true) {
                delay(2_400)
                scriptedDesktopReady = true
            }
        }
    }
    val nativeInstallState = scriptedInstall ?: realInstallState
    val phase = when {
        previewOnly && scriptedInstall == null -> SetupPhase.Configure
        nativeInstallState.complete -> SetupPhase.Ready
        nativeInstallState.failed -> SetupPhase.Failed
        nativeInstallState.running -> SetupPhase.Installing
        else -> SetupPhase.Configure
    }
    var beginAccepted by remember { mutableStateOf(false) }
    // Only a native-persisted plan survives process death. Keep this local
    // snapshot solely for truthful in-process progress presentation; never
    // regenerate a plan from the visual defaults after a restart.
    var acceptedPlan by remember { mutableStateOf<InstallPlan?>(null) }
    // When the install began on this screen; 0 when it was already running
    // before it (a relaunch), so no duration is claimed.
    var installStartedAt by remember { mutableLongStateOf(0L) }
    var installSubmissionError by remember { mutableStateOf<String?>(null) }
    val installProgressState: State<Float> = animateFloatAsState(
        targetValue = nativeInstallState.progress / 100f,
        animationSpec = tween(360),
        label = "native installation progress",
    )

    val context = LocalContext.current
    // The Android setting that keeps the desktop's processes alive. Until it
    // is on (or the user has chosen to go on) setup asks for it once, after the
    // welcome; an install already under way asks before anything else.
    val prepareNeeded = remember { previewOnly || setupPrepareNeeded(context) }
    var prepareDone by rememberSaveable { mutableStateOf(false) }
    val showPrepare = prepareNeeded && !prepareDone
    val prepare = rememberPrepareState(preview = previewOnly)

    // Setting up a new workspace rather than Portal itself: named throughout,
    // and it can be given up for the workspace Portal came from.
    val workspaces by Workspaces.state
    LaunchedEffect(Unit) { if (!previewOnly) Workspaces.refresh() }
    val workspace = if (previewOnly) {
        previewWorkspace?.let { WorkspaceSetup(it, "Main") }
    } else {
        workspaces.setupReturn?.let { back -> workspaces.name(workspaces.active)?.let { WorkspaceSetup(it, back.name) } }
    }

    var stage by rememberSaveable {
        mutableStateOf(
            previewStage?.let { name -> SetupStage.entries.firstOrNull { it.name.equals(name, ignoreCase = true) } }
                ?.takeIf { previewOnly && it != SetupStage.Install }
                ?: SetupStage.Welcome,
        )
    }
    val shown = when {
        phase != SetupPhase.Configure && showPrepare -> SetupStage.Prepare
        phase != SetupPhase.Configure -> SetupStage.Install
        // The setting came on by itself (or setup was told to go on).
        stage == SetupStage.Prepare && !showPrepare -> SetupStage.Look
        else -> stage
    }
    // The stages this walk passes through, for the dots in the corner.
    val steps = remember(prepareNeeded) {
        listOfNotNull(
            SetupStage.Prepare.takeIf { prepareNeeded },
            SetupStage.Look,
            SetupStage.Apps,
            SetupStage.Install,
        )
    }

    val currentSetupReady by rememberUpdatedState(onSetupReady)
    LaunchedEffect(phase, showPrepare) {
        if (phase == SetupPhase.Ready && !showPrepare) {
            Log.i(PREVIEW_TAG, "setup READY; starting ambient scatter and translucent veil prelude")
            currentSetupReady()
        }
    }

    val beginLocalInstall: () -> Unit = {
        if (previewOnly && previewInstall != null) {
            if (phase == SetupPhase.Configure) installStartedAt = SystemClock.elapsedRealtime()
            if (phase == SetupPhase.Configure || phase == SetupPhase.Failed) scriptedRun++
        } else if (previewOnly) {
            installSubmissionError = "Preview only: nothing is installed."
        } else if (phase == SetupPhase.Configure && !beginAccepted) {
            installSubmissionError = null
            val planResult = runCatching {
                InstallPlan.fromSelections(
                    appearance = appearance,
                    systemIsDark = androidSystemIsDark(context),
                    interfaceSize = interfaceSize,
                    displayMetrics = currentInstallDisplayMetrics(context),
                    selectedAppIds = optionalAppIds,
                )
            }
            val plan = planResult.getOrNull()
            if (plan == null) {
                Log.e(PREVIEW_TAG, "Unable to create first-run install plan", planResult.exceptionOrNull())
                installSubmissionError =
                    "Current display settings are outside Portal’s supported range. Adjust them in Android Settings and try again."
            } else if (ComposeOverlay.beginInstall(plan)) {
                app.polarbear.SetupService.requestNotificationPermission(context)
                beginAccepted = true
                acceptedPlan = plan
                installStartedAt = SystemClock.elapsedRealtime()
            } else {
                installSubmissionError = "Portal didn’t accept these install settings. Please try again."
            }
        } else if (phase == SetupPhase.Failed) {
            // Retry re-attaches only to the durable native plan. In particular,
            // a recreated Compose tree must not turn its visual defaults into a
            // new installation request.
            installSubmissionError = null
            if (!ComposeOverlay.retryInstall()) {
                installSubmissionError = "Portal couldn’t resume the saved installation. Please try again."
            }
        }
    }
    val palette = resolvePalette(paintedAppearance)
    val veil = LocalPortalVeil.current
    SideEffect { veil.ink = palette.textPrimary }
    val capacity = rememberStorageCapacity()
    val checklistApplies = remember { setupChecklistApplies(context) }
    val icons by rememberAppIcons(APP_ICON_IDS)
    val reveal = rememberThemeReveal()
    val revealScope = rememberCoroutineScope()
    val systemDark = isSystemInDarkTheme()
    val selectAppearance: (AppearanceMode) -> Unit = { mode ->
        appearance = mode
        val becomesDark = when (mode) {
            AppearanceMode.System -> systemDark
            AppearanceMode.Dark -> true
            AppearanceMode.Light -> false
        }
        if (becomesDark == palette.isDark) {
            paintedAppearance = mode
        } else {
            reveal.play(revealScope, from = lastPress) { paintedAppearance = mode }
        }
    }
    val previewDark = when (appearance) {
        AppearanceMode.System -> systemDark
        AppearanceMode.Dark -> true
        AppearanceMode.Light -> false
    }

    // Which way the stages are moving, for the direction they slide.
    var lastShown by remember { mutableStateOf(shown) }
    val forward = shown.ordinal >= lastShown.ordinal
    SideEffect { lastShown = shown }

    val canGoBack = shown == SetupStage.Look || shown == SetupStage.Apps ||
        (shown == SetupStage.Prepare && phase == SetupPhase.Configure)
    val goBack: () -> Unit = {
        stage = when (shown) {
            SetupStage.Apps -> SetupStage.Look
            SetupStage.Look, SetupStage.Prepare -> SetupStage.Welcome
            else -> stage
        }
        installSubmissionError = null
    }
    val onPrepareContinue: () -> Unit = {
        prepareDone = true
        if (stage == SetupStage.Prepare || stage == SetupStage.Welcome) stage = SetupStage.Look
    }
    SetupBackHandler(enabled = canGoBack, onBack = goBack)

    Box(
        modifier = Modifier
            .fillMaxSize()
            .onGloballyPositioned { rootOrigin = it.positionInRoot() }
            .themeReveal(reveal)
            .pointerInput(Unit) {
                awaitEachGesture {
                    // Where a theme change blooms from.
                    lastPress = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial).position
                }
            },
    ) {
        PortalAmbientBackground(
            palette = palette,
            cardBounds = { cardBounds.value.translate(-rootOrigin) },
            readyPrelude = phase == SetupPhase.Ready && !showPrepare,
        )
        SetupCard(palette = palette, cardBounds = cardBounds) { metrics ->
            Column(Modifier.fillMaxSize()) {
                SetupTopBar(
                    palette = palette,
                    showBack = canGoBack,
                    onBack = goBack,
                    stepCount = steps.size,
                    stepIndex = steps.indexOf(shown).coerceAtLeast(0),
                    showSteps = shown != SetupStage.Welcome,
                    height = metrics.topBarHeight,
                    modifier = Modifier.fillMaxWidth(),
                )
                SharedTransitionLayout(Modifier.weight(1f)) {
                    AnimatedContent(
                        targetState = shown,
                        transitionSpec = { stageTransition(forward) },
                        contentAlignment = Alignment.TopCenter,
                        label = "setup stage",
                    ) { current ->
                        val stageScope = StageScope(
                            shared = this@SharedTransitionLayout,
                            visibility = this,
                            metrics = metrics,
                            palette = palette,
                            launchMarkModifier = launchMarkModifier,
                        )
                        when (current) {
                            SetupStage.Welcome -> stageScope.WelcomeStage(
                                workspace = workspace,
                                previewDark = previewDark,
                                interfaceSize = interfaceSize,
                                icons = icons,
                                preview = previewOnly,
                                onContinue = {
                                    stage = if (showPrepare) SetupStage.Prepare else SetupStage.Look
                                },
                            )
                            SetupStage.Prepare -> stageScope.PrepareStage(
                                state = prepare,
                                onContinue = onPrepareContinue,
                                installing = phase != SetupPhase.Configure,
                            )
                            SetupStage.Look -> stageScope.LookStage(
                                appearance = appearance,
                                onAppearance = selectAppearance,
                                interfaceSize = interfaceSize,
                                onInterfaceSize = { interfaceSize = it },
                                previewDark = previewDark,
                                icons = icons,
                                onContinue = { stage = SetupStage.Apps },
                            )
                            SetupStage.Apps -> stageScope.AppsStage(
                                selectedIds = optionalAppIds,
                                onToggle = { id ->
                                    optionalAppIds = if (id in optionalAppIds) optionalAppIds - id else optionalAppIds + id
                                },
                                capacity = capacity,
                                icons = icons,
                                error = installSubmissionError,
                                workspace = workspace,
                                onInstall = beginLocalInstall,
                            )
                            SetupStage.Install -> stageScope.InstallStage(
                                phase = phase,
                                workspace = workspace,
                                progressState = installProgressState,
                                nativeProgress = nativeInstallState.progress,
                                message = nativeInstallState.message,
                                errorMessage = installSubmissionError
                                    ?: nativeInstallState.error?.takeIf { it.isNotBlank() },
                                summary = acceptedPlan?.let(::planSummary),
                                startedAt = installStartedAt,
                                desktopReady = desktopReady || scriptedDesktopReady,
                                checklist = checklistApplies,
                                preview = previewOnly,
                                onTryAgain = beginLocalInstall,
                                onExportDiagnostics = if (previewOnly) null else ComposeOverlay::exportDiagnostics,
                                exportStatus = diagnosticsExport?.message,
                                exportRunning = diagnosticsExport?.running == true,
                            )
                        }
                    }
                }
            }
        }
    }
}

/**
 * The setup card on the ambient background: sized for the screen (see
 * [setupMetrics]), centred in the safe area, and reporting its bounds so the
 * ambient frost can gather around it.
 */
@Composable
internal fun SetupCard(
    palette: PortalPalette,
    cardBounds: androidx.compose.runtime.MutableState<Rect>,
    content: @Composable (SetupMetrics) -> Unit,
) {
    BoxWithConstraints(
        modifier = Modifier
            .fillMaxSize()
            .windowInsetsPadding(WindowInsets.systemBars.union(WindowInsets.displayCutout)),
        contentAlignment = Alignment.Center,
    ) {
        val metrics = setupMetrics(maxWidth, maxHeight)
        Box(
            modifier = Modifier
                .size(metrics.cardWidth, metrics.cardHeight)
                .onGloballyPositioned {
                    // Unclipped bounds in the ambient root coordinate space.
                    cardBounds.value = Rect(
                        it.positionInRoot(),
                        androidx.compose.ui.geometry.Size(it.size.width.toFloat(), it.size.height.toFloat()),
                    )
                }
                .setupCardSurface(palette, metrics)
                .padding(horizontal = metrics.paddingH, vertical = metrics.paddingV),
        ) {
            content(metrics)
        }
    }
}

/** The setup card's surface: rounded, softly graded, hairline border. */
internal fun Modifier.setupCardSurface(palette: PortalPalette, metrics: SetupMetrics): Modifier {
    val shape = RoundedCornerShape(if (metrics.compact) 28.dp else PortalDimens.SurfaceCorner)
    return this
        .clip(shape)
        .background(brush = Brush.verticalGradient(colors = listOf(palette.surfaceTop, palette.surfaceBottom)))
        .border(1.dp, palette.surfaceBorder, shape)
}

/** Read Android's actual theme and display metrics at acceptance time. */
private fun androidSystemIsDark(context: Context): Boolean =
    (context.resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) ==
        Configuration.UI_MODE_NIGHT_YES

private fun currentInstallDisplayMetrics(context: Context): InstallDisplayMetrics {
    val resources = context.resources
    val configuration = resources.configuration
    val display = resources.displayMetrics
    // Match Portal's established baseline from Resources.getDisplayMetrics().
    val densityDpi = display.densityDpi.takeIf { it > 0 } ?: configuration.densityDpi
    require(densityDpi > 0) { "Android did not report a usable densityDpi" }
    val density = densityDpi.toDouble() / 160.0
    fun logicalExtent(pixelExtent: Int, dpExtent: Int, axis: String): Int =
        pixelExtent.takeIf { it > 0 }
            ?: (dpExtent * density).roundToInt().takeIf { it > 0 }
            ?: error("Android did not report a usable $axis display extent")
    return InstallDisplayMetrics(
        densityDpi = densityDpi,
        logicalWidthPx = logicalExtent(display.widthPixels, configuration.screenWidthDp, "width"),
        logicalHeightPx = logicalExtent(display.heightPixels, configuration.screenHeightDp, "height"),
    )
}

// Shared Portal ambient background primitive: exact installer background
// implementation (opaque charcoal + seven blurred drifting Portal fragments,
// 720 ms scatter dispersion on readyPrelude, 1.0 -> 0.83 final alpha). Used
// by BOTH the first-install screen and the Return to Plasma screen, so there
// is exactly ONE implementation. Callers without a card pass Rect.Zero, which
// keeps the identical soft surroundings and skips only the card frost mask.
@Composable
internal fun PortalAmbientBackground(
    palette: PortalPalette,
    cardBounds: () -> Rect,
    readyPrelude: Boolean,
) {
    val finalLayerAlpha = remember { Animatable(1f) }
    LaunchedEffect(readyPrelude) {
        val target = if (readyPrelude) READY_BACKGROUND_ALPHA else 1f
        if (ValueAnimator.areAnimatorsEnabled()) {
            finalLayerAlpha.animateTo(target, tween(720))
        } else {
            finalLayerAlpha.snapTo(target)
        }
        if (readyPrelude) {
            Log.i(PREVIEW_TAG, "ambient veil settled at final compositing alpha=$READY_BACKGROUND_ALPHA")
        }
    }
    // Live desktop frost: once the veil may be lifted, the desktop's own
    // colours glow through a blurred pane instead of a flat translucent dim.
    val veil = LocalPortalVeil.current
    val snapshot = rememberPlasmaSnapshot(active = readyPrelude && veil.eligible)
    val hasFrost by remember { derivedStateOf { snapshot.value != null } }
    val frost = remember { Animatable(0f) }
    LaunchedEffect(hasFrost) {
        val target = if (hasFrost) 1f else 0f
        if (ValueAnimator.areAnimatorsEnabled()) {
            frost.animateTo(target, tween(if (hasFrost) 1100 else 240, easing = PortalEmphasized))
        } else {
            frost.snapTo(target)
        }
    }
    val tint = PortalFrostStyle.tint(palette.isDark)
    BoxWithConstraints(
        modifier = Modifier
            .fillMaxSize()
            .graphicsLayer { alpha = lerp(finalLayerAlpha.value, 1f, frost.value) },
    ) {
        // Full-canvas pixel size for the ambient loop tables (arc-length
        // traversal needs the real aspect, not fraction space). All ambient
        // interest now comes from the drifting Portal fragments; there is no
        // separate blob or glow here.
        val density = LocalDensity.current
        val scenePx = remember(density, maxWidth, maxHeight) {
            with(density) { IntSize(maxWidth.toPx().toInt(), maxHeight.toPx().toInt()) }
        }
        PlasmaFrostBackdrop(
            snapshot = { snapshot.value },
            opacity = { frost.value },
        )
        PortalAmbientFragments(
            background = { palette.background.copy(alpha = lerp(1f, tint, frost.value)) },
            ink = { palette.fragmentInk },
            gains = { Offset(palette.fragmentGain, palette.thresholdGain) },
            cardBounds = cardBounds,
            scenePx = scenePx,
            scatter = readyPrelude,
        )
    }
}

/**
 * The Portal mark, [height] tall at its own proportions. The launch
 * transition lands on whichever one is on screen first.
 */
@Composable
internal fun PortalMark(palette: PortalPalette, height: Dp, modifier: Modifier = Modifier) {
    Image(
        painter = portalMarkPainter(main = palette.logoMain, threshold = palette.logoThreshold),
        contentDescription = "Portal logo",
        contentScale = ContentScale.Fit,
        modifier = modifier.size(width = height * (345f / 514f), height = height),
    )
}

/**
 * Debug preview: the native install's messages at roughly real proportions,
 * about a minute end to end. [fail] stops during setup like a dropped
 * connection would; [resume] carries on from there.
 */
private suspend fun playScriptedInstall(
    fail: Boolean,
    resume: Boolean,
    update: (ComposeOverlay.InstallUiState) -> Unit,
) {
    fun state(progress: Int, message: String, error: String? = null, complete: Boolean = false) =
        update(ComposeOverlay.InstallUiState(if (error != null) "Failed" else "Installing", progress, message, error, complete))
    if (!resume) {
        state(5, "Downloading Debian runtime…")
        delay(900)
        for (mib in 0..920 step 23) {
            state(5 + mib * 40 / 920, "Downloading Debian runtime: $mib / 920 MiB")
            delay(320)
        }
        state(45, "Debian runtime download complete; checking it…")
        delay(1_200)
        for (entries in 101_888..146_417 step 1_731) {
            state(55, "Extracting Debian runtime: $entries entries")
            delay(450)
        }
        state(70, "Debian runtime committed; configuring Portal…")
        delay(800)
        state(75, "Configuring Portal (mesa-kgsl-layer)")
        delay(1_400)
        state(84, "Configuring Portal (desktop-login)")
        delay(2_600)
        if (fail) {
            state(84, "Configuring Portal (desktop-login)", error = "Portal lost its internet connection. Check it and tap Try again.")
            return
        }
    } else {
        state(84, "Configuring Portal (desktop-login)")
        delay(2_000)
    }
    state(85, "Installing optional apps: refreshing package lists")
    delay(1_600)
    val apps = listOf("GIMP", "VLC", "Steam")
    apps.forEachIndexed { index, name ->
        for (step in 0..5) {
            state(85 + index * 3 + step / 3, "Installing optional app ${index + 1} of ${apps.size}: $name (downloading)")
            delay(400)
        }
        for (step in 0..3) {
            state(87 + index * 3, "Installing optional app ${index + 1} of ${apps.size}: $name (installing)")
            delay(450)
        }
    }
    for (mb in 0..310 step 31) {
        state(94 + mb / 160, "Downloading Steam client: $mb / 310 MB")
        delay(300)
    }
    state(96, "Configuring Portal (xkb-symlink)")
    delay(1_000)
    state(99, "Applying your appearance and display size…")
    delay(2_200)
    state(100, "Portal is ready", complete = true)
}

/** What the user picked, echoed under the install progress. */
private fun planSummary(plan: InstallPlan): String {
    val appearance = when (plan.appearance) {
        AppearanceMode.System -> "Match Android"
        AppearanceMode.Dark -> "Dark"
        AppearanceMode.Light -> "Light"
    }
    val apps = plan.selectedAppIds.size
    val extras = when (apps) {
        0 -> "no extra apps"
        1 -> "1 extra app"
        else -> "$apps extra apps"
    }
    return "$appearance · ${plan.interfaceSize.name} size · $extras"
}

// A getter: the optional part comes from the native catalog.
internal val APP_ICON_IDS: List<String> get() = INCLUDED_APPS.map { it.id } + OPTIONAL_APPS.map { it.id }

@OptIn(ExperimentalSharedTransitionApi::class)
private val NoOverlayClip = object : SharedTransitionScope.OverlayClip {
    override fun getClipPath(
        sharedContentState: SharedTransitionScope.SharedContentState,
        bounds: Rect,
        layoutDirection: LayoutDirection,
        density: Density,
    ): Path? = null
}

/** Lets a shared element's glow reach past its bounds while it travels. */
@OptIn(ExperimentalSharedTransitionApi::class)
internal val UnclippedOverlay: SharedTransitionScope.OverlayClip get() = NoOverlayClip

/** Portal's primary glowing button, shared by setup and the recovery screen. */
@Composable
internal fun BeginInstallButton(
    palette: PortalPalette,
    centered: Boolean,
    onBeginInstall: () -> Unit,
    enabled: Boolean = true,
    label: String = "Begin Install",
    // Applied to the box that holds the button AND its glow, so a shared
    // element built on it carries (and clips) the whole glow, not just the pill.
    glowBoundsModifier: Modifier = Modifier,
    modifier: Modifier = Modifier,
) {
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val pressScale by animateFloatAsState(
        targetValue = if (enabled && pressed) 0.985f else 1f,
        animationSpec = tween(110),
        label = "press",
    )
    // Light dips slightly while pressed; quick and restrained, no bounce.
    val dip by animateFloatAsState(
        targetValue = if (enabled && pressed) 0.8f else 1f,
        animationSpec = tween(110),
        label = "pressDip",
    )
    val outer = (if (centered) Modifier.fillMaxWidth() else Modifier).then(modifier)
    BoxWithConstraints(
        modifier = outer,
        contentAlignment = Alignment.Center,
    ) {
        val buttonWidth = (if (centered) maxWidth * 0.8f else maxWidth)
            .coerceAtMost(PortalDimens.BeginMaxWidth)
        val buttonHeight = PortalDimens.BeginHeight
        val glowMargin = 56.dp
        // Explicit visual stack, bottom to top: faint static bloom, animated
        // upstream shader, opaque button. The static layer never paints over
        // the animation.
        // The glow always gets its full canvas, even where the layout only
        // leaves room for the pill: a clamped canvas cuts the halo off at the
        // pill's round ends. It is centred on the pill and spills past it.
        Box(
            modifier = Modifier
                .requiredSize(buttonWidth + glowMargin * 2, buttonHeight + glowMargin * 2)
                .then(glowBoundsModifier),
            contentAlignment = Alignment.Center,
        ) {
            Box(
                modifier = Modifier
                    .size(buttonWidth, buttonHeight)
                    .portalBloom(
                        glow = palette.glow,
                        cornerRadius = 26.dp,
                        intensity = dip * (if (palette.isDark) 1f else 0.7f),
                        tightAlpha = 0.22f,
                        broadAlpha = 0.06f,
                    ),
            )
            PortalAgslGlow(
                modifier = Modifier.size(
                    buttonWidth + glowMargin * 2,
                    buttonHeight + glowMargin * 2,
                ),
                buttonWidth = buttonWidth,
                buttonHeight = buttonHeight,
                margin = glowMargin,
                glowAlpha = dip * (if (palette.isDark) 1f else 0.72f),
            )
            Box(
                modifier = Modifier
                    .size(buttonWidth, buttonHeight)
                    .graphicsLayer {
                        scaleX = pressScale
                        scaleY = pressScale
                    }
                    .clip(RoundedCornerShape(26.dp))
                    .background(palette.buttonInterior)
                    .border(1.dp, palette.buttonOutline, RoundedCornerShape(26.dp))
                    .then(
                        if (enabled) {
                            Modifier.clickable(
                                interactionSource = tap,
                                indication = null,
                                role = Role.Button,
                                onClick = {
                                    Log.d(PREVIEW_TAG, "compose-setup-preview: $label pressed")
                                    onBeginInstall()
                                },
                            )
                        } else {
                            Modifier
                        },
                    ),
                contentAlignment = Alignment.Center,
            ) {
                // A new label crossfades in place rather than popping.
                AnimatedContent(
                    targetState = label,
                    transitionSpec = {
                        fadeIn(tween(260, delayMillis = 80, easing = PortalEmphasizedDecelerate))
                            .togetherWith(fadeOut(tween(140, easing = PortalEmphasizedAccelerate)))
                    },
                    label = "primary label",
                ) { text ->
                    Text(
                        text = text,
                        fontSize = 16.sp,
                        fontWeight = FontWeight.SemiBold,
                        color = PortalColors.Ivory,
                    )
                }
            }
        }
    }
}
