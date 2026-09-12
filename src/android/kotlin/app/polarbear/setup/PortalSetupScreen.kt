package app.polarbear.setup

// SPIKE-ONLY (branch compose-setup-spike): Portal first-run CONFIGURE
// screen. Visual/interaction prototype with local fake state only — no
// provisioning, no JNI/Rust references; the Begin Install action is a plain
// callback supplied by the host (ComposeOverlay owns the native signal).

import android.util.Log
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.Canvas
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
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
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
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.graphics.vector.rememberVectorPainter
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.components.EssentialsPicker
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
private const val APERTURE_PATH =
    "M188,473 C149,487 112,473 85,438 C27,364 38,221 87,126 " +
        "C124,54 186,27 236,57 C302,97 318,208 287,308"
private const val THRESHOLD_PATH = "M281,353 C267,390 249,419 225,438"

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
    var minimalExpanded by remember { mutableStateOf(false) }
    var pickerVisible by remember { mutableStateOf(false) }
    var essentials by remember { mutableStateOf(DEFAULT_ESSENTIALS) }

    val palette = resolvePalette(appearance)
    val (downloadGb, installedGb, freeGb) = formatStorageLine(essentials)

    Box(modifier = Modifier.fillMaxSize().background(palette.background)) {
        SetupBackground(palette = palette)
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
                    .widthIn(max = PortalDimens.SurfaceMaxWidth)
                    .fillMaxWidth(0.94f)
                    .shadow(
                        44.dp,
                        RoundedCornerShape(PortalDimens.SurfaceCorner),
                        ambientColor = palette.surfaceShadow,
                        spotColor = palette.surfaceShadow,
                    )
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
                                Column(modifier = Modifier.weight(1f)) {
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
                                }
                                Column(modifier = Modifier.weight(1f)) {
                                    MinimalInstallRow(
                                        expanded = minimalExpanded,
                                        onToggle = { minimalExpanded = !minimalExpanded },
                                        palette = palette,
                                    )
                                    Spacer(modifier = Modifier.height(11.dp))
                                    EssentialsRow(
                                        selectedIds = essentials,
                                        onOpen = { pickerVisible = true },
                                        palette = palette,
                                    )
                                }
                            }
                            Spacer(modifier = Modifier.height(18.dp))
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Text(
                                    text = "$downloadGb GB download · $installedGb GB installed · $freeGb GB free",
                                    fontSize = 13.sp,
                                    color = palette.textMuted,
                                    modifier = Modifier.weight(1f),
                                )
                                BeginInstallButton(palette = palette, centered = false, onBeginInstall = onBeginInstall)
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
                        MinimalInstallRow(
                            expanded = minimalExpanded,
                            onToggle = { minimalExpanded = !minimalExpanded },
                            palette = palette,
                        )
                        Spacer(modifier = Modifier.height(11.dp))
                        EssentialsRow(
                            selectedIds = essentials,
                            onOpen = { pickerVisible = true },
                            palette = palette,
                        )
                        Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                        Box(
                            modifier = Modifier.fillMaxWidth(),
                            contentAlignment = Alignment.Center,
                        ) {
                            Text(
                                text = "$downloadGb GB download · $installedGb GB installed · $freeGb GB free",
                                fontSize = 13.sp,
                                color = palette.textMuted,
                            )
                        }
                        Spacer(modifier = Modifier.height(18.dp))
                        BeginInstallButton(palette = palette, centered = true, onBeginInstall = onBeginInstall)
                    }
                }
            }
        }
        EssentialsPicker(
            visible = pickerVisible,
            selectedIds = essentials,
            onToggle = { id ->
                essentials = if (id in essentials) essentials - id else essentials + id
            },
            onDismiss = { pickerVisible = false },
            palette = palette,
        )
    }
}

@Composable
private fun SetupBackground(palette: PortalPalette) {
    BoxWithConstraints(modifier = Modifier.fillMaxSize()) {
        val w = maxWidth
        val h = maxHeight
        // Dedicated static backdrop layer: a few large aperture fragments,
        // heavily softened once. Never animated, never blurred per-frame.
        Canvas(modifier = Modifier.fillMaxSize().softBackdropBlur()) {
            val widthPx = w.toPx()
            val heightPx = h.toPx()
            drawArc(
                color = palette.arcIvory,
                startAngle = 130f,
                sweepAngle = 200f,
                useCenter = false,
                topLeft = Offset(-widthPx * 0.72f, -heightPx * 0.10f),
                size = androidx.compose.ui.geometry.Size(widthPx * 1.35f, widthPx * 1.35f),
                style = Stroke(width = 26.dp.toPx(), cap = StrokeCap.Round),
            )
            drawArc(
                color = palette.arcIvory,
                startAngle = 300f,
                sweepAngle = 150f,
                useCenter = false,
                topLeft = Offset(widthPx * 0.55f, heightPx * 0.45f),
                size = androidx.compose.ui.geometry.Size(widthPx * 1.05f, widthPx * 1.05f),
                style = Stroke(width = 22.dp.toPx(), cap = StrokeCap.Round),
            )
        }
        // Warm emitted light: soft radial falloff only, no painted arc.
        Canvas(modifier = Modifier.fillMaxSize()) {
            val widthPx = w.toPx()
            val heightPx = h.toPx()
            drawCircle(
                brush = Brush.radialGradient(
                    colors = listOf(
                        palette.arcOrange.copy(alpha = 0.5f),
                        palette.arcOrange.copy(alpha = 0.0f),
                    ),
                    center = Offset(widthPx * 0.94f, -heightPx * 0.10f),
                    radius = widthPx * 0.30f,
                ),
                radius = widthPx * 0.30f,
                center = Offset(widthPx * 0.94f, -heightPx * 0.10f),
            )
            drawCircle(
                brush = Brush.radialGradient(
                    colors = listOf(
                        palette.arcOrange.copy(alpha = 0.16f),
                        palette.arcOrange.copy(alpha = 0.0f),
                    ),
                    center = Offset(widthPx * 0.86f, -heightPx * 0.02f),
                    radius = widthPx * 0.55f,
                ),
                radius = widthPx * 0.55f,
                center = Offset(widthPx * 0.86f, -heightPx * 0.02f),
            )
        }
    }
}

private fun Modifier.softBackdropBlur(): Modifier =
    if (android.os.Build.VERSION.SDK_INT >= 31) {
        this.then(Modifier.blur(64.dp))
    } else {
        this
    }

@Composable
private fun SetupHeader(palette: PortalPalette, launchMarkModifier: Modifier) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Image(
            painter = portalMarkPainter(main = palette.logoMain, threshold = palette.logoThreshold),
            contentDescription = "Portal logo",
            modifier = Modifier.size(PortalDimens.LogoSize).then(launchMarkModifier),
        )
        Column(modifier = Modifier.padding(start = 20.dp)) {
            Text(
                text = "Install Portal",
                fontSize = PortalDimens.TitleSize,
                fontWeight = FontWeight.SemiBold,
                color = palette.textPrimary,
            )
            Text(
                text = "Debian 13 · KDE Plasma · Firefox · media codecs",
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
) {
    SectionLabel(text = "Appearance", palette = palette)
    Spacer(modifier = Modifier.height(9.dp))
    SlidingSegmentedControl(
        options = AppearanceMode.entries,
        selected = appearance,
        onSelect = onSelect,
        label = { it.name.lowercase().replaceFirstChar(Char::uppercase) },
        palette = palette,
    )
}

@Composable
private fun InterfaceSizeSection(
    size: InterfaceSize,
    onSelect: (InterfaceSize) -> Unit,
    palette: PortalPalette,
) {
    SectionLabel(text = "Interface size", palette = palette)
    Spacer(modifier = Modifier.height(9.dp))
    SlidingSegmentedControl(
        options = InterfaceSize.entries,
        selected = size,
        onSelect = onSelect,
        label = { it.name },
        palette = palette,
    )
}

@Composable
private fun MinimalInstallRow(expanded: Boolean, onToggle: () -> Unit, palette: PortalPalette) {
    OptionButton(
        onClick = onToggle,
        title = "Minimal install",
        secondary = "Everything needed for a complete Portal desktop",
        palette = palette,
        trailing = { DisclosureChevron(expanded = expanded, palette = palette) },
        expanded = expanded,
    ) {
        Text(
            text = "Debian 13 · KDE Plasma + KWin · Portal integration · " +
                "Firefox · media codecs · Dolphin · Konsole · core system tools",
            fontSize = 13.sp,
            lineHeight = 20.sp,
            color = palette.textSecondary,
            modifier = Modifier.padding(top = 8.dp, bottom = 4.dp, end = 8.dp),
        )
    }
}

@Composable
private fun EssentialsRow(selectedIds: Set<String>, onOpen: () -> Unit, palette: PortalPalette) {
    OptionButton(
        onClick = onOpen,
        title = "Essentials",
        secondary = "${selectedIds.size} selected · ${formatExtrasDelta(selectedIds)}",
        palette = palette,
        trailing = { ForwardChevron(palette = palette) },
    )
}

/**
 * Shared secondary-option button family: identical width, collapsed height,
 * radius, padding, glass fill, border and press behaviour. The collapsed
 * header stays visually intact when [expandedContent] opens beneath it.
 */
@Composable
private fun OptionButton(
    onClick: () -> Unit,
    title: String,
    secondary: String,
    palette: PortalPalette,
    trailing: @Composable RowScope.() -> Unit,
    expanded: Boolean = false,
    expandedContent: (@Composable ColumnScope.() -> Unit)? = null,
) {
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val shape = RoundedCornerShape(17.dp)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(shape)
            .background(
                if (pressed) palette.trackFill.copy(alpha = 0.10f) else palette.trackFill,
                shape,
            )
            .border(1.dp, palette.surfaceBorder, shape)
            .clickable(
                interactionSource = tap,
                indication = null,
                role = Role.Button,
                onClick = onClick,
            )
            .padding(horizontal = 18.dp, vertical = 10.dp),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier.heightIn(min = 42.dp),
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = title,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.Medium,
                    color = palette.textPrimary,
                )
                Text(
                    text = secondary,
                    fontSize = 13.sp,
                    color = palette.textMuted,
                )
            }
            trailing()
        }
        if (expandedContent != null) {
            AnimatedVisibility(
                visible = expanded,
                enter = expandVertically(animationSpec = tween(260)) + fadeIn(tween(260)),
                exit = shrinkVertically(animationSpec = tween(240)) + fadeOut(tween(240)),
            ) {
                Column { expandedContent() }
            }
        }
    }
}

@Composable
private fun DisclosureChevron(expanded: Boolean, palette: PortalPalette) {
    val rotation by animateFloatAsState(
        targetValue = if (expanded) 180f else 0f,
        animationSpec = tween(240),
        label = "chevron",
    )
    Canvas(
        modifier = Modifier
            .size(20.dp)
            .graphicsLayer { rotationZ = rotation },
    ) {
        val stroke = 2.dp.toPx()
        val path = Path().apply {
            moveTo(size.width * 0.22f, size.height * 0.36f)
            lineTo(size.width * 0.5f, size.height * 0.62f)
            lineTo(size.width * 0.78f, size.height * 0.36f)
        }
        drawPath(path, palette.textMuted, style = Stroke(width = stroke, cap = StrokeCap.Round))
    }
}

@Composable
private fun ForwardChevron(palette: PortalPalette) {
    Canvas(modifier = Modifier.size(20.dp)) {
        val stroke = 2.dp.toPx()
        val path = Path().apply {
            moveTo(size.width * 0.38f, size.height * 0.22f)
            lineTo(size.width * 0.62f, size.height * 0.5f)
            lineTo(size.width * 0.38f, size.height * 0.78f)
        }
        drawPath(path, palette.textMuted, style = Stroke(width = stroke, cap = StrokeCap.Round))
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
