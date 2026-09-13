package app.polarbear.setup

// SPIKE-ONLY (branch compose-setup-spike): Portal first-run CONFIGURE
// screen. Compose owns only the presentation and local selections. The
// process-lifetime Rust coordinator owns installation and publishes the
// durable provisioning snapshot consumed below.

import android.animation.ValueAnimator
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
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.components.AddAppsPicker
import app.polarbear.setup.components.PortalAmbientFragments
import app.polarbear.setup.components.StorageCapacityBar
import app.polarbear.setup.components.rememberStorageCapacity
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.wrapContentSize
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.layout.positionInParent
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.IntSize
import app.polarbear.setup.components.PortalAgslGlow
import app.polarbear.setup.components.SlidingSegmentedControl
import app.polarbear.setup.components.portalBloom
import app.polarbear.ComposeOverlay

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

@Composable
fun PortalSetupScreen(
    desktopReady: Boolean = false,
    onSetupReady: () -> Unit = {},
    launchMarkModifier: Modifier = Modifier,
) {
    var appearance by remember { mutableStateOf(AppearanceMode.System) }
    var interfaceSize by remember { mutableStateOf(InterfaceSize.Balanced) }
    val ambientCardBounds = remember { mutableStateOf(Rect.Zero) }
    var pickerBounds by remember { mutableStateOf(Rect.Zero) }
    var rootOrigin by remember { mutableStateOf(Offset.Zero) }
    var pickerVisible by remember { mutableStateOf(false) }
    var essentials by remember { mutableStateOf(DEFAULT_ESSENTIALS) }
    var settingsHeightPx by remember { mutableStateOf(0) }
    var appearanceControlTopPx by remember { mutableStateOf(0f) }
    var interfaceControlBottomPx by remember { mutableStateOf(0f) }
    val nativeInstallState by ComposeOverlay.installState()
    val phase = when {
        nativeInstallState.complete -> SetupPhase.Ready
        nativeInstallState.failed -> SetupPhase.Failed
        nativeInstallState.running -> SetupPhase.Installing
        else -> SetupPhase.Configure
    }
    var beginAccepted by remember { mutableStateOf(false) }
    var frozenEssentials by remember { mutableStateOf(DEFAULT_ESSENTIALS) }
    val installProgressState: State<Float> = androidx.compose.animation.core.animateFloatAsState(
        targetValue = nativeInstallState.progress / 100f,
        animationSpec = tween(360),
        label = "native installation progress",
    )

    val currentSetupReady by rememberUpdatedState(onSetupReady)
    LaunchedEffect(phase) {
        if (phase == SetupPhase.Ready) {
            Log.i(PREVIEW_TAG, "setup READY; starting ambient scatter and translucent veil prelude")
            currentSetupReady()
        }
    }

    val beginLocalInstall: () -> Unit = {
        if (phase == SetupPhase.Configure && !beginAccepted) {
            if (ComposeOverlay.beginInstall()) {
                beginAccepted = true
                frozenEssentials = essentials.toSet()
                pickerVisible = false
            }
        } else if (phase == SetupPhase.Failed) {
            // Retry re-attaches to the process-lifetime native coordinator;
            // it never creates a second installer or clears safe disk state.
            ComposeOverlay.beginInstall()
        }
    }
    val palette = resolvePalette(appearance)
    val capacity = rememberStorageCapacity()
    val density = LocalDensity.current
    val settingsHeight = with(density) {
        if (settingsHeightPx > 0) settingsHeightPx.toDp() else 161.dp
    }
    val addAppsTopInset = with(density) {
        if (appearanceControlTopPx > 0f) appearanceControlTopPx.toDp() else 24.dp
    }
    val addAppsCollapsedHeight = with(density) {
        val measured = interfaceControlBottomPx - appearanceControlTopPx
        if (measured >= 48f) measured.toDp() else settingsHeight - addAppsTopInset
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
    val configurationVisual = if (configurationInactive || configurationTransition.isRunning) {
        Modifier.graphicsLayer {
            alpha = configurationAlpha
            scaleX = configurationScale
            scaleY = configurationScale
            translationY = configurationLift.toPx()
            compositingStrategy = CompositingStrategy.ModulateAlpha
        }
    } else {
        Modifier
    }
    val configurationRegion = configurationVisual.blockInput(configurationInactive)

    Box(modifier = Modifier.fillMaxSize()
        .onGloballyPositioned { rootOrigin = it.positionInRoot() }
        .pointerInput(Unit) {
            awaitEachGesture {
                val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
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
            readyPrelude = phase == SetupPhase.Ready,
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
                    .widthIn(max = PortalDimens.SurfaceMaxWidth)
                    .fillMaxWidth(0.94f)
                    .clip(RoundedCornerShape(PortalDimens.SurfaceCorner))
                    .background(
                        brush = Brush.verticalGradient(
                            colors = listOf(palette.surfaceTop, palette.surfaceBottom),
                        ),
                    )
                    .border(1.dp, palette.surfaceBorder, RoundedCornerShape(PortalDimens.SurfaceCorner))
                    .padding(
                        horizontal = PortalDimens.SurfacePaddingH,
                        vertical = PortalDimens.SurfacePaddingV,
                    ),
            ) {
                SetupHeader(
                    palette = palette,
                    launchMarkModifier = launchMarkModifier,
                    phase = phase,
                    inactiveConfigurationModifier = configurationVisual,
                )
                Spacer(modifier = Modifier.height(16.dp))
                BoxWithConstraints(modifier = Modifier.fillMaxWidth()) {
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
                                Column(
                                    modifier = Modifier
                                        .weight(1f)
                                        .onSizeChanged { settingsHeightPx = it.height },
                                ) {
                                    AppearanceSection(
                                        appearance = appearance,
                                        onSelect = { appearance = it },
                                        palette = palette,
                                        enabled = phase == SetupPhase.Configure,
                                        controlModifier = Modifier.onGloballyPositioned {
                                            appearanceControlTopPx = it.positionInParent().y
                                        },
                                    )
                                    Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                                    InterfaceSizeSection(
                                        size = interfaceSize,
                                        onSelect = { interfaceSize = it },
                                        palette = palette,
                                        enabled = phase == SetupPhase.Configure,
                                        controlModifier = Modifier.onGloballyPositioned {
                                            interfaceControlBottomPx =
                                                it.positionInParent().y + it.size.height
                                        },
                                    )
                                }
                                Box(
                                    modifier = Modifier
                                        .weight(1f)
                                        .heightIn(min = settingsHeight)
                                        .padding(top = addAppsTopInset),
                                    contentAlignment = Alignment.TopCenter,
                                ) {
                                    AddAppsPicker(
                                        expanded = pickerVisible,
                                        selectedIds = essentials,
                                        onExpandedChange = { pickerVisible = it },
                                        onToggle = { id -> essentials = if (id in essentials) essentials - id else essentials + id },
                                        onBounds = { pickerBounds = it },
                                        palette = palette,
                                        collapsedHeight = addAppsCollapsedHeight,
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
                                    selectedIds = if (phase == SetupPhase.Configure) essentials else frozenEssentials,
                                    palette = palette,
                                    modifier = Modifier.weight(1f),
                                    phase = phase,
                                    installProgress = installProgressState,
                                    installMessage = nativeInstallState.message,
                                    hasSelectedApps = frozenEssentials.isNotEmpty(),
                                )
                                // Reserve the button, let its unchanged 56dp glow
                                // overlap the footer breathing room instead of
                                // making a separate 164dp-tall layout island.
                                InstallActionArea(
                                    phase = phase,
                                    desktopReady = desktopReady,
                                    palette = palette,
                                    inactiveConfigurationModifier = configurationVisual,
                                    onBeginInstall = beginLocalInstall,
                                )
                            }
                        }
                    } else {
                        Column(
                            modifier = Modifier
                                .fillMaxWidth()
                                .then(configurationRegion),
                        ) {
                            AppearanceSection(
                                appearance = appearance,
                                onSelect = { appearance = it },
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
                            MinimalInstallRow(palette = palette)
                            Spacer(modifier = Modifier.height(11.dp))
                            AddAppsPicker(
                                expanded = pickerVisible,
                                selectedIds = essentials,
                                onExpandedChange = { pickerVisible = it },
                                onToggle = { id -> essentials = if (id in essentials) essentials - id else essentials + id },
                                onBounds = { pickerBounds = it },
                                palette = palette,
                                enabled = phase == SetupPhase.Configure,
                            )
                        }
                        Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                        StorageCapacityBar(
                            capacity = capacity,
                            selectedIds = if (phase == SetupPhase.Configure) essentials else frozenEssentials,
                            palette = palette,
                            phase = phase,
                            installProgress = installProgressState,
                            installMessage = nativeInstallState.message,
                            hasSelectedApps = frozenEssentials.isNotEmpty(),
                        )
                        Spacer(Modifier.height(28.dp))
                        Box(Modifier.fillMaxWidth().padding(bottom = 26.dp), contentAlignment = Alignment.Center) {
                            InstallActionArea(
                                phase = phase,
                                desktopReady = desktopReady,
                                palette = palette,
                                inactiveConfigurationModifier = configurationVisual,
                                onBeginInstall = beginLocalInstall,
                            )
                        }
                    }
                }
            }
        }

    }
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
    BoxWithConstraints(
        modifier = Modifier
            .fillMaxSize()
            .graphicsLayer { alpha = finalLayerAlpha.value },
    ) {
        // Full-canvas pixel size for the ambient loop tables (arc-length
        // traversal needs the real aspect, not fraction space). All ambient
        // interest now comes from the drifting Portal fragments; there is no
        // separate blob or glow here.
        val density = LocalDensity.current
        val scenePx = remember(density, maxWidth, maxHeight) {
            with(density) { IntSize(maxWidth.toPx().toInt(), maxHeight.toPx().toInt()) }
        }
        PortalAmbientFragments(
            background = palette.background,
            cardBounds = cardBounds,
            scenePx = scenePx,
            scatter = readyPrelude,
        )
    }
}

@Composable
private fun SetupHeader(
    palette: PortalPalette,
    launchMarkModifier: Modifier,
    phase: SetupPhase,
    inactiveConfigurationModifier: Modifier,
) {
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        if (maxWidth >= PortalDimens.TwoColumnBreakpoint) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(PortalDimens.ColumnGutter),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                SetupHeaderIdentity(
                    palette = palette,
                    launchMarkModifier = launchMarkModifier,
                    title = phase.title,
                    modifier = Modifier.weight(1f),
                )
                MinimalInstallRow(
                    palette = palette,
                    modifier = Modifier
                        .weight(1f)
                        .then(inactiveConfigurationModifier),
                )
            }
        } else {
            SetupHeaderIdentity(
                palette = palette,
                launchMarkModifier = launchMarkModifier,
                title = phase.title,
            )
        }
    }
}

@Composable
private fun SetupHeaderIdentity(
    palette: PortalPalette,
    launchMarkModifier: Modifier,
    title: String,
    modifier: Modifier = Modifier,
) {
    Row(modifier = modifier, verticalAlignment = Alignment.CenterVertically) {
        Image(
            painter = portalMarkPainter(main = palette.logoMain, threshold = palette.logoThreshold),
            contentDescription = "Portal logo",
            modifier = Modifier.size(PortalDimens.LogoSize).then(launchMarkModifier),
        )
        Column(modifier = Modifier.padding(start = 20.dp)) {
            AnimatedContent(
                targetState = title,
                transitionSpec = {
                    (fadeIn(tween(260, delayMillis = 45)) +
                        slideInVertically(tween(300)) { it / 4 } +
                        scaleIn(tween(300), initialScale = 0.985f))
                        .togetherWith(
                            fadeOut(tween(180)) +
                                slideOutVertically(tween(220)) { -it / 5 } +
                                scaleOut(tween(220), targetScale = 0.99f),
                        )
                },
                contentAlignment = Alignment.CenterStart,
                label = "setup title",
            ) { animatedTitle ->
                Text(
                    text = animatedTitle,
                    fontSize = PortalDimens.TitleSize,
                    fontWeight = FontWeight.SemiBold,
                    color = palette.textPrimary,
                )
            }
            Text(
                text = "Powered by Debian 13 · KDE Plasma",
                fontSize = 13.sp,
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

@Composable
private fun MinimalInstallRow(
    palette: PortalPalette,
    modifier: Modifier = Modifier,
) {
    val shape = RoundedCornerShape(24.dp)
    Row(
        modifier = modifier
            .fillMaxWidth()
            .height(64.dp)
            .clip(shape)
            .background(palette.trackFill)
            .border(1.dp, palette.surfaceBorder, shape),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(
            modifier = Modifier
                .weight(1f)
                .padding(horizontal = 18.dp),
        ) {
            Text(
                "Minimal install",
                color = palette.textPrimary,
                fontSize = 14.sp,
                fontWeight = FontWeight.Medium,
            )
            Text(
                "Firefox · Dolphin · Konsole · codecs · Portal tools",
                color = palette.textSecondary,
                fontSize = 11.sp,
                lineHeight = 15.sp,
                maxLines = 1,
            )
        }
    }
}
@Composable
private fun InstallActionArea(
    phase: SetupPhase,
    desktopReady: Boolean,
    palette: PortalPalette,
    inactiveConfigurationModifier: Modifier,
    onBeginInstall: () -> Unit,
) {
    Box(
        modifier = Modifier
            .size(PortalDimens.BeginMaxWidth, PortalDimens.BeginHeight)
            .wrapContentSize(unbounded = true),
        contentAlignment = Alignment.Center,
    ) {
        AnimatedVisibility(
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
                modifier = inactiveConfigurationModifier,
            )
        }
        AnimatedVisibility(
            visible = phase == SetupPhase.Ready && !desktopReady,
            enter = fadeIn(tween(durationMillis = 300, delayMillis = 90)) +
                slideInVertically(tween(340, delayMillis = 50)) { it / 3 } +
                scaleIn(tween(340, delayMillis = 50), initialScale = 0.98f),
            exit = fadeOut(tween(180)) +
                slideOutVertically(tween(210)) { -it / 4 } +
                scaleOut(tween(210), targetScale = 0.99f),
        ) {
            Text(
                text = "Finishing Portal…",
                fontSize = 14.sp,
                fontWeight = FontWeight.Medium,
                color = palette.textMuted,
            )
        }
    }
}

@Composable
private fun BeginInstallButton(
    palette: PortalPalette,
    centered: Boolean,
    onBeginInstall: () -> Unit,
    enabled: Boolean = true,
    label: String = "Begin Install",
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
        Box(contentAlignment = Alignment.Center) {
            Box(
                modifier = Modifier
                    .size(buttonWidth, buttonHeight)
                    .portalBloom(
                        glow = palette.glow,
                        cornerRadius = 26.dp,
                        intensity = dip,
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
                glowAlpha = dip,
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
                                    Log.d(PREVIEW_TAG, "compose-setup-preview: Begin Install pressed")
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
