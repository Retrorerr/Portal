package app.polarbear.setup

// SPIKE-ONLY (branch compose-setup-spike): Portal first-run CONFIGURE
// screen. Compose owns only the presentation and local selections. The
// process-lifetime Rust coordinator owns installation and publishes the
// durable provisioning snapshot consumed below.

import android.animation.ValueAnimator
import android.content.Context
import android.content.res.Configuration
import android.util.Log
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.animateDp
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.updateTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
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
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.graphics.vector.rememberVectorPainter
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.components.AddAppsPicker
import app.polarbear.setup.components.PortalAmbientFragments
import app.polarbear.setup.components.PortalPrepareStage
import app.polarbear.setup.components.SetupChecklist
import app.polarbear.setup.components.setupChecklistApplies
import app.polarbear.setup.components.setupPrepareNeeded
import app.polarbear.setup.components.StorageCapacityBar
import app.polarbear.setup.components.rememberStorageCapacity
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.wrapContentSize
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.layout.positionInParent
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.IntSize
import app.polarbear.setup.components.PortalAgslGlow
import app.polarbear.setup.components.SlidingSegmentedControl
import app.polarbear.setup.components.portalBloom
import app.polarbear.ComposeOverlay
import androidx.compose.animation.ExperimentalSharedTransitionApi
import androidx.compose.animation.SharedTransitionLayout
import androidx.compose.animation.SharedTransitionScope
import androidx.compose.animation.SizeTransform
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.Density
import androidx.compose.ui.graphics.Path
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.ui.util.lerp
import app.polarbear.setup.components.LocalPortalVeil
import app.polarbear.setup.components.PlasmaFrostBackdrop
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import app.polarbear.setup.components.dissolveBlur
import app.polarbear.setup.components.portalBlur
import app.polarbear.setup.components.rememberPlasmaSnapshot
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.graphics.ImageBitmap
import app.polarbear.setup.components.INCLUDED_APPS
import app.polarbear.setup.components.rememberAppIcons
import app.polarbear.setup.components.rememberThemeReveal
import app.polarbear.setup.components.themeReveal
import kotlinx.coroutines.delay
import kotlin.math.roundToInt

private const val PREVIEW_TAG = "PortalComposeSetup"
private const val READY_BACKGROUND_ALPHA = 0.83f
// Veil tint over the live desktop frost: enough ground for ivory (dark) or
// charcoal (light) text on any wallpaper, light enough to read as glass.
private const val FROST_TINT_DARK = 0.64f
private const val FROST_TINT_LIGHT = 0.70f

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

@OptIn(ExperimentalSharedTransitionApi::class)
private val NoOverlayClip = object : SharedTransitionScope.OverlayClip {
    override fun getClipPath(
        sharedContentState: SharedTransitionScope.SharedContentState,
        bounds: Rect,
        layoutDirection: LayoutDirection,
        density: Density,
    ): Path? = null
}

@OptIn(ExperimentalSharedTransitionApi::class)
@Composable
fun PortalSetupScreen(
    desktopReady: Boolean = false,
    onSetupReady: () -> Unit = {},
    launchMarkModifier: Modifier = Modifier,
    // Debug preview: shows the first-run stages over a finished install and
    // never reads or starts a real installation.
    previewOnly: Boolean = false,
) {
    var appearance by remember { mutableStateOf(AppearanceMode.System) }
    // What is painted. It trails [appearance] by the capture of one frame so
    // the theme reveal can freeze the old theme before the repaint.
    var paintedAppearance by remember { mutableStateOf(AppearanceMode.System) }
    var interfaceSize by remember { mutableStateOf(InterfaceSize.Balanced) }
    val ambientCardBounds = remember { mutableStateOf(Rect.Zero) }
    var pickerBounds by remember { mutableStateOf(Rect.Zero) }
    var rootOrigin by remember { mutableStateOf(Offset.Zero) }
    var pickerVisible by remember { mutableStateOf(false) }
    var optionalAppIds by remember { mutableStateOf(DEFAULT_OPTIONAL_APP_IDS) }
    var lastPress by remember { mutableStateOf(Offset.Unspecified) }
    val nativeInstallState by ComposeOverlay.installState()
    val phase = when {
        previewOnly -> SetupPhase.Configure
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
    var installSubmissionError by remember { mutableStateOf<String?>(null) }
    val installProgressState: State<Float> = androidx.compose.animation.core.animateFloatAsState(
        targetValue = nativeInstallState.progress / 100f,
        animationSpec = tween(360),
        label = "native installation progress",
    )

    val context = LocalContext.current
    // The stage before Configure: shown from the moment the app opens until
    // the Android setting that keeps the desktop's processes alive is on, or
    // the user has chosen to go on. Nothing else lifts until it clears.
    val prepareNeeded = remember { previewOnly || setupPrepareNeeded(context) }
    var prepareDone by rememberSaveable { mutableStateOf(false) }
    val showPrepare = prepareNeeded && !prepareDone

    val currentSetupReady by rememberUpdatedState(onSetupReady)
    LaunchedEffect(phase, showPrepare) {
        if (phase == SetupPhase.Ready && !showPrepare) {
            Log.i(PREVIEW_TAG, "setup READY; starting ambient scatter and translucent veil prelude")
            currentSetupReady()
        }
    }

    val beginLocalInstall: () -> Unit = {
        if (previewOnly) {
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
                beginAccepted = true
                acceptedPlan = plan
                pickerVisible = false
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
    val density = LocalDensity.current
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
    val configurationInactive = phase != SetupPhase.Configure
    val configurationTransition = updateTransition(
        targetState = configurationInactive,
        label = "configuration layer",
    )
    val configurationAlpha by configurationTransition.animateFloat(
        transitionSpec = { tween(320) },
        label = "configuration opacity",
    ) { inactive -> if (inactive) 0.32f else 1f }
    val configurationScale by configurationTransition.animateFloat(
        transitionSpec = { tween(360) },
        label = "configuration compression",
    ) { inactive -> if (inactive) 0.978f else 1f }
    val configurationLift by configurationTransition.animateDp(
        transitionSpec = { tween(360) },
        label = "configuration lift",
    ) { inactive -> if (inactive) (-3).dp else 0.dp }
    // Depth of field: settings drift out of focus as installation takes the
    // stage, and sharpen again if it ever hands control back.
    val configurationFocus by configurationTransition.animateFloat(
        transitionSpec = { tween(560, easing = PortalEmphasized) },
        label = "configuration focus",
    ) { inactive -> if (inactive) 1f else 0f }
    val recedeBlurPx = with(density) { 4.dp.toPx() }
    val configurationVisual = if (configurationInactive || configurationTransition.isRunning) {
        Modifier.graphicsLayer {
            alpha = configurationAlpha
            scaleX = configurationScale
            scaleY = configurationScale
            translationY = configurationLift.toPx()
            portalBlur(configurationFocus * recedeBlurPx)
            compositingStrategy = if (renderEffect == null) {
                CompositingStrategy.ModulateAlpha
            } else {
                CompositingStrategy.Auto
            }
        }
    } else {
        Modifier
    }
    val configurationRegion = configurationVisual.blockInput(configurationInactive)
    val screenWidth = LocalConfiguration.current.screenWidthDp.dp
    val compact = screenWidth < PortalDimens.CompactBreakpoint
    // The install card at rest: 94% of the screen, up to its maximum width.
    val settledCardWidth = minOf(PortalDimens.SurfaceMaxWidth, screenWidth) * 0.94f
    val cardWidth by animateDpAsState(
        targetValue = if (showPrepare) minOf(PortalDimens.PrepareCardWidth, settledCardWidth) else settledCardWidth,
        animationSpec = tween(780, easing = PortalEmphasized),
        label = "setup card width",
    )
    val surfaceWidth = if (compact) {
        Modifier
            .padding(horizontal = PortalDimens.CompactScreenGutter)
            .fillMaxWidth()
    } else {
        Modifier
            .widthIn(max = PortalDimens.SurfaceMaxWidth)
            .fillMaxWidth(0.94f)
    }

    Box(modifier = Modifier.fillMaxSize()
        .onGloballyPositioned { rootOrigin = it.positionInRoot() }
        .themeReveal(reveal)
        .pointerInput(Unit) {
            awaitEachGesture {
                val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
                // Where a theme change blooms from.
                lastPress = down.position
                if (pickerVisible && !down.isConsumed && !pickerBounds.contains(down.position + rootOrigin)) {
                    // Local Compose dismissal: consume this whole outside gesture
                    // so closing the picker cannot also activate Begin Install.
                    pickerVisible = false
                    down.consume()
                    do {
                        val event = awaitPointerEvent(PointerEventPass.Initial)
                        event.changes.forEach { it.consume() }
                    } while (event.changes.any { it.pressed })
                }
            }
        }) {
        PortalAmbientBackground(
            palette = palette,
            cardBounds = { ambientCardBounds.value.translate(-rootOrigin) },
            readyPrelude = phase == SetupPhase.Ready && !showPrepare,
        )
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(vertical = 32.dp),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Column(
                modifier = Modifier
                    .onGloballyPositioned {
                        // Unclipped bounds in the ambient root coordinate space;
                        // scrolling/expansion must not invent new rounded corners.
                        ambientCardBounds.value = Rect(it.positionInRoot(),
                            androidx.compose.ui.geometry.Size(it.size.width.toFloat(), it.size.height.toFloat()))
                    }
                    .then(if (compact) surfaceWidth else Modifier.width(cardWidth))
                    .portalSurface(palette, compact),
            ) {
                SetupHeaderIdentity(
                    palette = palette,
                    launchMarkModifier = launchMarkModifier,
                    title = if (showPrepare) "Before you begin" else phase.title,
                    compact = compact,
                )
                Spacer(modifier = Modifier.height(22.dp))
                SharedTransitionLayout {
                    AnimatedContent(
                        targetState = showPrepare,
                        // Slow and soft: the old stage lets go first, the new one
                        // settles in behind it while the card glides to its new
                        // height, and the action button travels between the two.
                        transitionSpec = {
                            (fadeIn(tween(720, delayMillis = 300, easing = PortalEmphasizedDecelerate)) +
                                slideInVertically(tween(900, delayMillis = 200, easing = PortalEmphasized)) { it / 16 })
                                .togetherWith(
                                    fadeOut(tween(380, easing = PortalEmphasizedAccelerate)) +
                                        slideOutVertically(tween(520, easing = PortalEmphasizedAccelerate)) { -it / 24 },
                                )
                                .using(SizeTransform(clip = false) { _, _ -> tween(950, easing = PortalEmphasized) })
                        },
                        contentAlignment = Alignment.TopCenter,
                        label = "setup stage",
                    ) { preparing ->
                        val stage = this
                        val primaryAction = Modifier.sharedBounds(
                            sharedContentState = rememberSharedContentState(key = "primary-action"),
                            animatedVisibilityScope = stage,
                            boundsTransform = { _, _ -> tween(1000, easing = PortalEmphasized) },
                            // Continue turns into Begin Install mid-flight: the outgoing
                            // button stays solid under the incoming one until it has
                            // mostly arrived, so the button never dips.
                            enter = fadeIn(tween(480, delayMillis = 140)),
                            exit = fadeOut(tween(360, delayMillis = 300)),
                            // The glow and bloom reach past the button's bounds.
                            clipInOverlayDuringTransition = NoOverlayClip,
                            zIndexInOverlay = 1f,
                            resizeMode = SharedTransitionScope.ResizeMode.ScaleToBounds(ContentScale.FillWidth, Alignment.Center),
                        )
                        Box(Modifier.dissolveBlur(stage, radius = 12.dp, enterMillis = 760, exitMillis = 420)) {
                            if (preparing) {
                                PortalPrepareStage(
                                    palette = palette,
                                    actionModifier = primaryAction,
                                    preview = previewOnly,
                                    onContinue = { prepareDone = true },
                                )
                            } else {
                                BoxWithConstraints(
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .holdWidth(if (compact) 0.dp else settledCardWidth - PortalDimens.SurfacePaddingH * 2),
                                ) {
                                    if (maxWidth >= PortalDimens.TwoColumnBreakpoint) {
                                        Column {
                                            Row(
                                                modifier = Modifier
                                                    .fillMaxWidth()
                                                    .then(configurationRegion),
                                                horizontalArrangement = Arrangement.spacedBy(
                                                    PortalDimens.ColumnGutter,
                                                ),
                                            ) {
                                                Column(modifier = Modifier.weight(1f)) {
                                                    AppearanceSection(
                                                        appearance = appearance,
                                                        onSelect = selectAppearance,
                                                        palette = palette,
                                                        enabled = phase == SetupPhase.Configure,
                                                    )
                                                    Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                                                    InterfaceSizeSection(
                                                        size = interfaceSize,
                                                        onSelect = { interfaceSize = it },
                                                        palette = palette,
                                                        enabled = phase == SetupPhase.Configure,
                                                    )
                                                }
                                                Column(modifier = Modifier.weight(1f)) {
                                                    IncludedAppsSection(palette = palette, icons = icons)
                                                    Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                                                    OptionalAppsSection(
                                                        expanded = pickerVisible,
                                                        selectedIds = optionalAppIds,
                                                        onExpandedChange = { pickerVisible = it },
                                                        onToggle = { id ->
                                                            optionalAppIds = if (id in optionalAppIds) {
                                                                optionalAppIds - id
                                                            } else {
                                                                optionalAppIds + id
                                                            }
                                                        },
                                                        onBounds = { pickerBounds = it },
                                                        palette = palette,
                                                        icons = icons,
                                                        enabled = phase == SetupPhase.Configure,
                                                    )
                                                }
                                            }
                                            Spacer(Modifier.height(28.dp))
                                            Row(
                                                Modifier
                                                    .fillMaxWidth()
                                                    .padding(horizontal = 12.dp)
                                                    .padding(bottom = 26.dp),
                                                // Card padding is 34dp; adding this 12dp inset
                                                // makes both outer gaps and the centre gap 46dp.
                                                horizontalArrangement = Arrangement.spacedBy(
                                                    PortalDimens.SurfacePaddingH + 12.dp,
                                                ),
                                                verticalAlignment = Alignment.Top,
                                            ) {
                                                StorageCapacityBar(
                                                    capacity = capacity,
                                                    selectedIds = if (phase == SetupPhase.Configure) {
                                                        optionalAppIds
                                                    } else {
                                                        acceptedPlan?.selectedAppIds?.toSet()
                                                    },
                                                    palette = palette,
                                                    modifier = Modifier.weight(1f),
                                                    phase = phase,
                                                    installProgress = installProgressState,
                                                    installMessage = nativeInstallState.message,
                                                    hasSelectedApps = acceptedPlan?.selectedAppIds?.isNotEmpty(),
                                                )
                                                // Reserve the button, let its unchanged 56dp glow
                                                // overlap the footer breathing room instead of
                                                // making a separate 164dp-tall layout island.
                                                InstallActionArea(
                                                    phase = phase,
                                                    desktopReady = desktopReady,
                                                    palette = palette,
                                                    actionModifier = primaryAction,
                                                    inactiveConfigurationModifier = configurationVisual,
                                                    errorMessage = installSubmissionError,
                                                    onBeginInstall = beginLocalInstall,
                                                )
                                            }
                                        }
                                    } else {
                                        // One column: BoxWithConstraints stacks its children,
                                        // so the storage bar and button must not be siblings
                                        // of the configuration column here.
                                        Column {
                                            Column(
                                                modifier = Modifier
                                                    .fillMaxWidth()
                                                    .then(configurationRegion),
                                            ) {
                                                AppearanceSection(
                                                    appearance = appearance,
                                                    onSelect = selectAppearance,
                                                    palette = palette,
                                                    enabled = phase == SetupPhase.Configure,
                                                )
                                                Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                                                InterfaceSizeSection(
                                                    size = interfaceSize,
                                                    onSelect = { interfaceSize = it },
                                                    palette = palette,
                                                    enabled = phase == SetupPhase.Configure,
                                                )
                                                Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                                                IncludedAppsSection(palette = palette, icons = icons)
                                                Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                                                OptionalAppsSection(
                                                    expanded = pickerVisible,
                                                    selectedIds = optionalAppIds,
                                                    onExpandedChange = { pickerVisible = it },
                                                    onToggle = { id ->
                                                        optionalAppIds = if (id in optionalAppIds) {
                                                            optionalAppIds - id
                                                        } else {
                                                            optionalAppIds + id
                                                        }
                                                    },
                                                    onBounds = { pickerBounds = it },
                                                    palette = palette,
                                                    icons = icons,
                                                    enabled = phase == SetupPhase.Configure,
                                                )
                                            }
                                            Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                                            StorageCapacityBar(
                                                capacity = capacity,
                                                selectedIds = if (phase == SetupPhase.Configure) {
                                                    optionalAppIds
                                                } else {
                                                    acceptedPlan?.selectedAppIds?.toSet()
                                                },
                                                palette = palette,
                                                phase = phase,
                                                installProgress = installProgressState,
                                                installMessage = nativeInstallState.message,
                                                hasSelectedApps = acceptedPlan?.selectedAppIds?.isNotEmpty(),
                                            )
                                            Spacer(Modifier.height(28.dp))
                                            Box(Modifier.fillMaxWidth().padding(bottom = 26.dp), contentAlignment = Alignment.Center) {
                                                InstallActionArea(
                                                    phase = phase,
                                                    desktopReady = desktopReady,
                                                    palette = palette,
                                                    actionModifier = primaryAction,
                                                    inactiveConfigurationModifier = configurationVisual,
                                                    errorMessage = installSubmissionError,
                                                    onBeginInstall = beginLocalInstall,
                                                )
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // The install takes minutes: offer the Android settings Portal
            // cannot change itself while the user waits.
            AnimatedVisibility(
                visible = checklistApplies && !showPrepare &&
                    (phase == SetupPhase.Installing || phase == SetupPhase.Failed),
                enter = fadeIn(tween(420, delayMillis = 360, easing = PortalEmphasizedDecelerate)) +
                    slideInVertically(tween(560, delayMillis = 300, easing = PortalEmphasized)) { it / 6 },
                exit = fadeOut(tween(200, easing = PortalEmphasizedAccelerate)),
            ) {
                SetupChecklist(
                    palette = palette,
                    modifier = Modifier
                        .padding(top = 16.dp)
                        .then(surfaceWidth),
                )
            }
        }

    }
}

/** The setup card's surface: rounded, softly graded, hairline border, inset. */
internal fun Modifier.portalSurface(palette: PortalPalette, compact: Boolean): Modifier = this
    .clip(RoundedCornerShape(PortalDimens.SurfaceCorner))
    .background(brush = Brush.verticalGradient(colors = listOf(palette.surfaceTop, palette.surfaceBottom)))
    .border(1.dp, palette.surfaceBorder, RoundedCornerShape(PortalDimens.SurfaceCorner))
    .padding(
        horizontal = if (compact) PortalDimens.CompactSurfacePaddingH else PortalDimens.SurfacePaddingH,
        vertical = if (compact) PortalDimens.CompactSurfacePaddingV else PortalDimens.SurfacePaddingV,
    )

/**
 * Lays the content out at [width] even while its parent is still narrower
 * (the card opening up), centred, so nothing reflows as the card grows: the
 * card's edges simply reveal it.
 */
private fun Modifier.holdWidth(width: Dp): Modifier = layout { measurable, constraints ->
    val held = maxOf(constraints.maxWidth, width.roundToPx())
    val placeable = measurable.measure(constraints.copy(minWidth = held, maxWidth = held))
    layout(constraints.maxWidth, placeable.height) {
        placeable.placeRelative((constraints.maxWidth - held) / 2, 0)
    }
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
    val tint = if (palette.isDark) FROST_TINT_DARK else FROST_TINT_LIGHT
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

@Composable
internal fun SetupHeaderIdentity(
    palette: PortalPalette,
    launchMarkModifier: Modifier,
    title: String,
    compact: Boolean = false,
    modifier: Modifier = Modifier,
) {
    Row(modifier = modifier, verticalAlignment = Alignment.CenterVertically) {
        Image(
            painter = portalMarkPainter(main = palette.logoMain, threshold = palette.logoThreshold),
            contentDescription = "Portal logo",
            modifier = Modifier
                .size(if (compact) PortalDimens.CompactLogoSize else PortalDimens.LogoSize)
                .then(launchMarkModifier),
        )
        Column(modifier = Modifier.padding(start = if (compact) 14.dp else 20.dp)) {
            AnimatedContent(
                targetState = title,
                transitionSpec = {
                    (fadeIn(tween(380, delayMillis = 70, easing = PortalEmphasizedDecelerate)) +
                        slideInVertically(tween(560, easing = PortalEmphasized)) { it / 3 } +
                        scaleIn(tween(560, easing = PortalEmphasized), initialScale = 0.97f))
                        .togetherWith(
                            fadeOut(tween(200, easing = PortalEmphasizedAccelerate)) +
                                slideOutVertically(tween(260, easing = PortalEmphasizedAccelerate)) { -it / 4 } +
                                scaleOut(tween(260), targetScale = 0.98f),
                        )
                        .using(SizeTransform(clip = false))
                },
                contentAlignment = Alignment.CenterStart,
                label = "setup title",
            ) { animatedTitle ->
                Text(
                    text = animatedTitle,
                    modifier = Modifier.dissolveBlur(this, radius = 12.dp),
                    fontSize = if (compact) PortalDimens.CompactTitleSize else PortalDimens.TitleSize,
                    lineHeight = if (compact) 27.sp else TextUnit.Unspecified,
                    fontWeight = FontWeight.SemiBold,
                    color = palette.textPrimary,
                )
            }
            Text(
                text = "Powered by Debian 13 · KDE Plasma",
                fontSize = if (compact) 12.sp else 13.sp,
                color = palette.textMuted,
            )
        }
    }
}

private val SetupPhase.title: String
    get() = when (this) {
        SetupPhase.Configure -> "Install Portal Desktop"
        SetupPhase.Installing -> "Installing Portal Desktop"
        SetupPhase.Failed -> "Portal setup needs attention"
        SetupPhase.Ready -> "Portal is ready"
    }

@Composable
private fun SectionLabel(text: String, palette: PortalPalette) {
    Text(
        text = text,
        fontSize = 12.sp,
        fontWeight = FontWeight.SemiBold,
        color = palette.textMuted,
    )
}

@Composable
private fun AppearanceSection(
    appearance: AppearanceMode,
    onSelect: (AppearanceMode) -> Unit,
    palette: PortalPalette,
    enabled: Boolean = true,
    controlModifier: Modifier = Modifier,
) {
    SectionLabel(text = "Appearance", palette = palette)
    Spacer(modifier = Modifier.height(9.dp))
    SlidingSegmentedControl(
        options = AppearanceMode.entries,
        selected = appearance,
        onSelect = onSelect,
        label = { it.name.lowercase().replaceFirstChar(Char::uppercase) },
        palette = palette,
        modifier = controlModifier,
        enabled = enabled,
    )
}

@Composable
private fun InterfaceSizeSection(
    size: InterfaceSize,
    onSelect: (InterfaceSize) -> Unit,
    palette: PortalPalette,
    enabled: Boolean = true,
    controlModifier: Modifier = Modifier,
) {
    SectionLabel(text = "Interface size", palette = palette)
    Spacer(modifier = Modifier.height(9.dp))
    SlidingSegmentedControl(
        options = InterfaceSize.entries,
        selected = size,
        onSelect = onSelect,
        label = { it.name },
        palette = palette,
        modifier = controlModifier,
        enabled = enabled,
    )
}

private val APP_ICON_IDS = INCLUDED_APPS.map { it.id } + OPTIONAL_APPS.map { it.id }

/**
 * What every install already has, as the icons you will meet in Plasma.
 * Touching an icon names it in the section label for a moment.
 */
@Composable
private fun IncludedAppsSection(
    palette: PortalPalette,
    icons: Map<String, ImageBitmap>,
) {
    var named by remember { mutableStateOf<String?>(null) }
    var touchCount by remember { mutableStateOf(0) }
    LaunchedEffect(touchCount) {
        if (named != null) {
            delay(2200)
            named = null
        }
    }
    AnimatedContent(
        targetState = named,
        transitionSpec = {
            (fadeIn(tween(260, delayMillis = 40, easing = PortalEmphasizedDecelerate)) +
                slideInVertically(tween(360, easing = PortalEmphasized)) { it / 2 })
                .togetherWith(
                    fadeOut(tween(160, easing = PortalEmphasizedAccelerate)) +
                        slideOutVertically(tween(220, easing = PortalEmphasizedAccelerate)) { -it / 2 },
                )
                .using(SizeTransform(clip = false))
        },
        contentAlignment = Alignment.CenterStart,
        label = "included label",
    ) { name ->
        Text(
            text = if (name == null) "Included" else "Included · $name",
            modifier = Modifier.dissolveBlur(this, radius = 4.dp),
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
            color = palette.textMuted,
        )
    }
    Spacer(modifier = Modifier.height(9.dp))
    val shape = RoundedCornerShape(24.dp)
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .height(48.dp)
            .clip(shape)
            .background(palette.trackFill)
            .border(1.dp, palette.surfaceBorder, shape)
            .padding(horizontal = 10.dp),
        horizontalArrangement = Arrangement.SpaceEvenly,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        INCLUDED_APPS.forEachIndexed { index, app ->
            val icon = icons[app.id]
            // Icons settle in one after another once decoded.
            val shown by animateFloatAsState(
                targetValue = if (icon != null) 1f else 0f,
                animationSpec = tween(480, delayMillis = 60 + index * 45, easing = PortalEmphasizedDecelerate),
                label = "included icon",
            )
            val lifted by animateFloatAsState(
                targetValue = if (named == app.name) 1f else 0f,
                animationSpec = tween(320, easing = PortalEmphasized),
                label = "included focus",
            )
            val tap = remember { MutableInteractionSource() }
            // Slots share the row so all eight fit on a phone; 34dp at most.
            Box(
                modifier = Modifier
                    .weight(1f, fill = false)
                    .widthIn(max = 34.dp)
                    .fillMaxWidth()
                    .height(34.dp)
                    .clickable(
                        interactionSource = tap,
                        indication = null,
                        onClickLabel = app.name,
                        onClick = {
                            named = app.name
                            touchCount++
                        },
                    ),
                contentAlignment = Alignment.Center,
            ) {
                if (icon != null) {
                    Image(
                        bitmap = icon,
                        contentDescription = app.name,
                        modifier = Modifier
                            .size(26.dp)
                            .graphicsLayer {
                                alpha = shown
                                val s = (0.86f + 0.14f * shown) * (1f + 0.14f * lifted)
                                scaleX = s
                                scaleY = s
                                translationY = (1f - shown) * 6.dp.toPx() - lifted * 2.dp.toPx()
                            },
                    )
                }
            }
        }
    }
}

@Composable
private fun OptionalAppsSection(
    expanded: Boolean,
    selectedIds: Set<String>,
    onExpandedChange: (Boolean) -> Unit,
    onToggle: (String) -> Unit,
    onBounds: (Rect) -> Unit,
    palette: PortalPalette,
    icons: Map<String, ImageBitmap>,
    enabled: Boolean,
) {
    SectionLabel(text = "Optional apps", palette = palette)
    Spacer(modifier = Modifier.height(9.dp))
    AddAppsPicker(
        expanded = expanded,
        selectedIds = selectedIds,
        onExpandedChange = onExpandedChange,
        onToggle = onToggle,
        onBounds = onBounds,
        palette = palette,
        icons = icons,
        enabled = enabled,
    )
}

@Composable
private fun InstallActionArea(
    phase: SetupPhase,
    desktopReady: Boolean,
    palette: PortalPalette,
    // Shared with the onboarding's Continue button so one becomes the other.
    actionModifier: Modifier,
    inactiveConfigurationModifier: Modifier,
    errorMessage: String?,
    onBeginInstall: () -> Unit,
) {
    Column(
        modifier = Modifier.width(PortalDimens.BeginMaxWidth),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Box(
            modifier = Modifier
                .size(PortalDimens.BeginMaxWidth, PortalDimens.BeginHeight)
                .wrapContentSize(unbounded = true),
            contentAlignment = Alignment.Center,
        ) {
            this@Column.AnimatedVisibility(
                visible = phase != SetupPhase.Ready,
                exit = fadeOut(tween(200)) +
                    slideOutVertically(tween(220)) { -it / 8 } +
                    scaleOut(tween(220), targetScale = 0.985f),
            ) {
                BeginInstallButton(
                    palette = palette,
                    centered = false,
                    onBeginInstall = onBeginInstall,
                    enabled = phase == SetupPhase.Configure || phase == SetupPhase.Failed,
                    label = if (phase == SetupPhase.Failed) "Retry" else "Begin Install",
                    glowBoundsModifier = actionModifier,
                    modifier = inactiveConfigurationModifier.dissolveBlur(this, radius = 16.dp),
                )
            }
            this@Column.AnimatedVisibility(
                visible = phase == SetupPhase.Ready && !desktopReady,
                enter = fadeIn(tween(durationMillis = 420, delayMillis = 120, easing = PortalEmphasizedDecelerate)) +
                    slideInVertically(tween(560, delayMillis = 60, easing = PortalEmphasized)) { it / 2 },
                exit = fadeOut(tween(200, easing = PortalEmphasizedAccelerate)) +
                    slideOutVertically(tween(240, easing = PortalEmphasizedAccelerate)) { -it / 3 },
            ) {
                // A slow breath while the desktop finishes coming up.
                val breath by rememberInfiniteTransition(label = "finishing breath").animateFloat(
                    initialValue = 1f,
                    targetValue = 0.55f,
                    animationSpec = infiniteRepeatable(
                        tween(1400, easing = PortalEmphasized),
                        RepeatMode.Reverse,
                    ),
                    label = "finishing alpha",
                )
                Text(
                    text = "Finishing Portal…",
                    modifier = Modifier
                        .dissolveBlur(this, radius = 12.dp)
                        .graphicsLayer { alpha = breath },
                    fontSize = 14.sp,
                    fontWeight = FontWeight.Medium,
                    color = palette.textMuted,
                )
            }
        }
        AnimatedVisibility(
            visible = errorMessage != null && phase != SetupPhase.Ready,
            enter = fadeIn(tween(160)) + slideInVertically(tween(180)) { -it / 4 },
            exit = fadeOut(tween(120)),
        ) {
            Text(
                text = errorMessage.orEmpty(),
                modifier = Modifier
                    .dissolveBlur(this, radius = 6.dp)
                    .fillMaxWidth()
                    .padding(horizontal = 8.dp),
                fontSize = 11.sp,
                lineHeight = 14.sp,
                color = palette.accent,
                textAlign = TextAlign.Center,
                maxLines = 3,
            )
        }
    }
}

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
        Box(modifier = glowBoundsModifier, contentAlignment = Alignment.Center) {
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
                Text(
                    text = label,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = PortalColors.Ivory,
                )
            }
        }
    }
}

private fun Modifier.blockInput(blocked: Boolean): Modifier = if (!blocked) {
    this
} else {
    pointerInput(Unit) {
        awaitPointerEventScope {
            while (true) {
                awaitPointerEvent(PointerEventPass.Initial).changes.forEach { it.consume() }
            }
        }
    }
}
