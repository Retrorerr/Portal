package app.polarbear.setup

// SPIKE-ONLY (branch compose-setup-spike): Portal first-run CONFIGURE
// screen. Visual/interaction prototype with local fake state only — no
// provisioning, no Rust calls except the Begin Install debug log.

import android.util.Log
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
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
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
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
import app.polarbear.setup.components.SlidingSegmentedControl

private const val PREVIEW_TAG = "PortalComposeSetup"

// Official Portal aperture geometry (assets/portal-icon-foreground.svg).
private const val APERTURE_PATH =
    "M524,718 C485,732 448,718 421,683 C363,609 374,466 423,371 " +
        "C460,299 522,272 572,302 C638,342 654,453 623,553"
private const val THRESHOLD_PATH = "M617,598 C603,635 585,664 561,683"

@Composable
fun portalMarkPainter(main: Color, threshold: Color) = rememberVectorPainter(
    image = remember(main, threshold) {
        ImageVector.Builder(
            name = "portal",
            defaultWidth = 108.dp,
            defaultHeight = 108.dp,
            viewportWidth = 1024f,
            viewportHeight = 1024f,
        )
            .addPath(
                pathData = addPathNodes(APERTURE_PATH),
                stroke = SolidColor(main),
                strokeLineWidth = 54f,
                strokeLineCap = StrokeCap.Round,
            )
            .addPath(
                pathData = addPathNodes(THRESHOLD_PATH),
                stroke = SolidColor(threshold),
                strokeLineWidth = 54f,
                strokeLineCap = StrokeCap.Round,
            )
            .build()
    },
)

@Composable
fun PortalSetupScreen() {
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
                        28.dp,
                        RoundedCornerShape(PortalDimens.SurfaceCorner),
                        ambientColor = palette.surfaceShadow,
                        spotColor = palette.surfaceShadow,
                    )
                    .clip(RoundedCornerShape(PortalDimens.SurfaceCorner))
                    .background(palette.surfaceFill)
                    .border(1.dp, palette.surfaceBorder, RoundedCornerShape(PortalDimens.SurfaceCorner))
                    .padding(
                        horizontal = PortalDimens.SurfacePaddingH,
                        vertical = PortalDimens.SurfacePaddingV,
                    ),
            ) {
                SetupHeader(palette = palette)
                Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                SectionLabel(text = "Appearance", palette = palette)
                Spacer(modifier = Modifier.height(10.dp))
                SlidingSegmentedControl(
                    options = AppearanceMode.entries,
                    selected = appearance,
                    onSelect = { appearance = it },
                    label = { it.name.lowercase().replaceFirstChar(Char::uppercase) },
                    palette = palette,
                )
                Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                SectionLabel(text = "Interface size", palette = palette)
                Spacer(modifier = Modifier.height(10.dp))
                SlidingSegmentedControl(
                    options = InterfaceSize.entries,
                    selected = interfaceSize,
                    onSelect = { interfaceSize = it },
                    label = { it.name },
                    palette = palette,
                )
                Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                MinimalInstallRow(
                    expanded = minimalExpanded,
                    onToggle = { minimalExpanded = !minimalExpanded },
                    palette = palette,
                )
                Spacer(modifier = Modifier.height(18.dp))
                EssentialsRow(
                    selectedIds = essentials,
                    onOpen = { pickerVisible = true },
                    palette = palette,
                )
                Spacer(modifier = Modifier.height(PortalDimens.SectionSpacing))
                Box(
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(1.dp)
                        .background(palette.surfaceBorder),
                )
                Spacer(modifier = Modifier.height(18.dp))
                Text(
                    text = "$downloadGb GB download · $installedGb GB installed · $freeGb GB free",
                    fontSize = 13.sp,
                    color = palette.textMuted,
                    modifier = Modifier.align(Alignment.CenterHorizontally),
                )
                Spacer(modifier = Modifier.height(24.dp))
                BeginInstallButton(palette = palette)
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
        Canvas(modifier = Modifier.fillMaxSize()) {
            val widthPx = w.toPx()
            val heightPx = h.toPx()
            // Soft orange threshold glow, upper right, mostly off-canvas.
            drawCircle(
                brush = Brush.radialGradient(
                    colors = listOf(
                        palette.arcOrange.copy(alpha = 0.30f),
                        palette.arcOrange.copy(alpha = 0.0f),
                    ),
                    center = Offset(widthPx * 0.96f, -heightPx * 0.12f),
                    radius = widthPx * 0.34f,
                ),
                radius = widthPx * 0.34f,
                center = Offset(widthPx * 0.96f, -heightPx * 0.12f),
            )
            // Oversized partial ivory aperture, bleeding in from the left.
            drawArc(
                color = palette.arcIvory,
                startAngle = 130f,
                sweepAngle = 200f,
                useCenter = false,
                topLeft = Offset(-widthPx * 0.62f, heightPx * 0.02f),
                size = androidx.compose.ui.geometry.Size(widthPx * 1.1f, widthPx * 1.1f),
                style = Stroke(width = 10.dp.toPx(), cap = StrokeCap.Round),
            )
            // Second aperture echo, lower right.
            drawArc(
                color = palette.arcIvory,
                startAngle = 300f,
                sweepAngle = 150f,
                useCenter = false,
                topLeft = Offset(widthPx * 0.62f, heightPx * 0.52f),
                size = androidx.compose.ui.geometry.Size(widthPx * 0.85f, widthPx * 0.85f),
                style = Stroke(width = 8.dp.toPx(), cap = StrokeCap.Round),
            )
            // Restrained orange threshold arc near the glow.
            drawArc(
                color = palette.arcOrange,
                startAngle = 205f,
                sweepAngle = 62f,
                useCenter = false,
                topLeft = Offset(widthPx * 0.52f, -heightPx * 0.30f),
                size = androidx.compose.ui.geometry.Size(widthPx * 0.62f, widthPx * 0.62f),
                style = Stroke(width = 9.dp.toPx(), cap = StrokeCap.Round),
            )
        }
    }
}

@Composable
private fun SetupHeader(palette: PortalPalette) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Image(
            painter = portalMarkPainter(main = palette.logoMain, threshold = palette.logoThreshold),
            contentDescription = "Portal logo",
            modifier = Modifier.size(PortalDimens.LogoSize),
        )
        Column(modifier = Modifier.padding(start = 16.dp)) {
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
        fontSize = 13.sp,
        fontWeight = FontWeight.SemiBold,
        color = palette.textSecondary,
    )
}

@Composable
private fun MinimalInstallRow(expanded: Boolean, onToggle: () -> Unit, palette: PortalPalette) {
    val tap = remember { MutableInteractionSource() }
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .clickable(
                interactionSource = tap,
                indication = null,
                role = Role.Button,
                onClick = onToggle,
            )
            .padding(vertical = 6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = "Minimal install",
                    fontSize = 16.sp,
                    fontWeight = FontWeight.Medium,
                    color = palette.textPrimary,
                )
                Text(
                    text = "Everything needed for a complete Portal desktop",
                    fontSize = 13.sp,
                    color = palette.textMuted,
                )
            }
            DisclosureChevron(expanded = expanded, palette = palette)
        }
        AnimatedVisibility(
            visible = expanded,
            enter = expandVertically(animationSpec = tween(260)) + fadeIn(tween(260)),
            exit = shrinkVertically(animationSpec = tween(240)) + fadeOut(tween(240)),
        ) {
            Text(
                text = "Debian 13 · KDE Plasma + KWin · Portal integration · " +
                    "Firefox · media codecs · Dolphin · Konsole · core system tools",
                fontSize = 13.sp,
                lineHeight = 20.sp,
                color = palette.textSecondary,
                modifier = Modifier.padding(top = 10.dp, end = 28.dp),
            )
        }
    }
}

@Composable
private fun EssentialsRow(selectedIds: Set<String>, onOpen: () -> Unit, palette: PortalPalette) {
    val tap = remember { MutableInteractionSource() }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .clickable(
                interactionSource = tap,
                indication = null,
                role = Role.Button,
                onClick = onOpen,
            )
            .padding(vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = "Essentials",
            fontSize = 16.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textPrimary,
            modifier = Modifier.weight(1f),
        )
        Text(
            text = "${selectedIds.size} selected · ${formatExtrasDelta(selectedIds)}",
            fontSize = 13.sp,
            color = palette.textSecondary,
        )
        Spacer(modifier = Modifier.size(8.dp))
        ForwardChevron(palette = palette)
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
private fun BeginInstallButton(palette: PortalPalette) {
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val pressScale by animateFloatAsState(
        targetValue = if (pressed) 0.97f else 1f,
        animationSpec = tween(120),
        label = "press",
    )
    // Stationary glow: fixed soft shape, opacity breathes gently. This is the
    // only deliberately attention-seeking element on the screen.
    val glowBreath = rememberInfiniteTransition(label = "glow")
    val glowAlpha by glowBreath.animateFloat(
        initialValue = 0.45f,
        targetValue = 0.75f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 2800, easing = FastOutSlowInEasing),
            repeatMode = RepeatMode.Reverse,
        ),
        label = "glowAlpha",
    )
    Box(
        modifier = Modifier.fillMaxWidth(),
        contentAlignment = Alignment.Center,
    ) {
        Box(
            modifier = Modifier
                .widthIn(max = PortalDimens.BeginMaxWidth + 72.dp)
                .fillMaxWidth(0.9f)
                .height(PortalDimens.BeginHeight + 36.dp)
                .graphicsLayer { alpha = glowAlpha }
                .background(
                    brush = Brush.radialGradient(
                        colors = listOf(
                            palette.glow.copy(alpha = 0.30f),
                            palette.glow.copy(alpha = 0.0f),
                        ),
                    ),
                ),
        )
        Box(
            modifier = Modifier
                .widthIn(max = PortalDimens.BeginMaxWidth)
                .fillMaxWidth(0.8f)
                .height(PortalDimens.BeginHeight)
                .graphicsLayer {
                    scaleX = pressScale
                    scaleY = pressScale
                    alpha = if (pressed) 0.92f else 1f
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
                    },
                ),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                text = "Begin Install",
                fontSize = 16.sp,
                fontWeight = FontWeight.SemiBold,
                color = if (palette.isDark) palette.textPrimary else Color.White,
            )
        }
    }
}
