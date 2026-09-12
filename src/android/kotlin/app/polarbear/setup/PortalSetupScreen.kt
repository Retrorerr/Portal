package app.polarbear.setup

// SPIKE-ONLY (branch compose-setup-spike): Portal first-run CONFIGURE
// screen. Local setup selections and storage projection only — no
// provisioning, no JNI/Rust references; the Begin Install action is a plain
// callback supplied by the host (ComposeOverlay owns the native signal).

import android.util.Log
import androidx.compose.animation.core.animateFloatAsState
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
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

private const val PREVIEW_TAG = "PortalComposeSetup"

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
    onBeginInstall: () -> Unit = {},
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

    Box(modifier = Modifier.fillMaxSize().background(palette.background)
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
        SetupBackground(palette = palette, cardBounds = { ambientCardBounds.value.translate(-rootOrigin) })
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
                SetupHeader(palette = palette, launchMarkModifier = launchMarkModifier)
                Spacer(modifier = Modifier.height(16.dp))
                BoxWithConstraints(modifier = Modifier.fillMaxWidth()) {
                    if (maxWidth >= PortalDimens.TwoColumnBreakpoint) {
                        Column {
                            Row(
                                modifier = Modifier.fillMaxWidth(),
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
                                        controlModifier = Modifier.onGloballyPositioned {
                                            appearanceControlTopPx = it.positionInParent().y
                                        },
                                    )
                                    Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                                    InterfaceSizeSection(
                                        size = interfaceSize,
                                        onSelect = { interfaceSize = it },
                                        palette = palette,
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
                                StorageCapacityBar(capacity, essentials, palette, Modifier.weight(1f))
                                // Reserve the button, let its unchanged 56dp glow
                                // overlap the footer breathing room instead of
                                // making a separate 164dp-tall layout island.
                                Box(Modifier.size(PortalDimens.BeginMaxWidth, PortalDimens.BeginHeight)
                                    .wrapContentSize(unbounded = true)) {
                                    BeginInstallButton(palette, centered = false, onBeginInstall)
                                }
                            }
                        }
                    } else {
                        AppearanceSection(
                            appearance = appearance,
                            onSelect = { appearance = it },
                            palette = palette,
                        )
                        Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                        InterfaceSizeSection(
                            size = interfaceSize,
                            onSelect = { interfaceSize = it },
                            palette = palette,
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
                                    )
                        Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                        StorageCapacityBar(capacity, essentials, palette)
                        Spacer(Modifier.height(28.dp))
                        Box(Modifier.fillMaxWidth().padding(bottom = 26.dp), contentAlignment = Alignment.Center) {
                            Box(Modifier.size(PortalDimens.BeginMaxWidth, PortalDimens.BeginHeight).wrapContentSize(unbounded = true)) {
                                BeginInstallButton(palette, centered = false, onBeginInstall)
                            }
                        }
                    }
                }
            }
        }

    }
}

@Composable
private fun SetupBackground(palette: PortalPalette, cardBounds: () -> Rect) {
    BoxWithConstraints(modifier = Modifier.fillMaxSize()) {
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
        )
    }
}

@Composable
private fun SetupHeader(palette: PortalPalette, launchMarkModifier: Modifier) {
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        if (maxWidth >= PortalDimens.TwoColumnBreakpoint) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(PortalDimens.ColumnGutter),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                SetupHeaderIdentity(palette, launchMarkModifier, Modifier.weight(1f))
                MinimalInstallRow(palette, Modifier.weight(1f))
            }
        } else {
            SetupHeaderIdentity(palette, launchMarkModifier)
        }
    }
}

@Composable
private fun SetupHeaderIdentity(
    palette: PortalPalette,
    launchMarkModifier: Modifier,
    modifier: Modifier = Modifier,
) {
    Row(modifier = modifier, verticalAlignment = Alignment.CenterVertically) {
        Image(
            painter = portalMarkPainter(main = palette.logoMain, threshold = palette.logoThreshold),
            contentDescription = "Portal logo",
            modifier = Modifier.size(PortalDimens.LogoSize).then(launchMarkModifier),
        )
        Column(modifier = Modifier.padding(start = 20.dp)) {
            Text(
                text = "Install Portal Desktop",
                fontSize = PortalDimens.TitleSize,
                fontWeight = FontWeight.SemiBold,
                color = palette.textPrimary,
            )
            Text(
                text = "Powered by Debian 13 · KDE Plasma",
                fontSize = 13.sp,
                color = palette.textMuted,
            )
        }
    }
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
    )
}

@Composable
private fun InterfaceSizeSection(
    size: InterfaceSize,
    onSelect: (InterfaceSize) -> Unit,
    palette: PortalPalette,
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
private fun BeginInstallButton(palette: PortalPalette, centered: Boolean, onBeginInstall: () -> Unit) {
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val pressScale by animateFloatAsState(
        targetValue = if (pressed) 0.985f else 1f,
        animationSpec = tween(110),
        label = "press",
    )
    // Light dips slightly while pressed; quick and restrained, no bounce.
    val dip by animateFloatAsState(
        targetValue = if (pressed) 0.8f else 1f,
        animationSpec = tween(110),
        label = "pressDip",
    )
    val outer = if (centered) Modifier.fillMaxWidth() else Modifier
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
                    .clickable(
                        interactionSource = tap,
                        indication = null,
                        role = Role.Button,
                        onClick = {
                            Log.d(PREVIEW_TAG, "compose-setup-preview: Begin Install pressed")
                            onBeginInstall()
                        },
                    ),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    text = "Begin Install",
                    fontSize = 16.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = PortalColors.Ivory,
                )
            }
        }
    }
}
