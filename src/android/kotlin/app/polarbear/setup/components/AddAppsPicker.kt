package app.polarbear.setup.components

// Optional apps: a 48dp pill, the same size as the segmented controls beside
// it, that unfolds in place into a list of real app icons.
//
// Every row is composed and laid out from the start and only clipped by the
// animated height, so the first expansion does no composition, text layout
// or image decoding mid-animation. Height, row stagger and the header's
// icon cluster all read one spring in layout/draw; nothing recomposes per
// frame.

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.ColorMatrix
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathMeasure
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.EssentialApp
import app.polarbear.setup.OPTIONAL_APPS
import app.polarbear.setup.PortalColors
import app.polarbear.setup.PortalPalette
import app.polarbear.setup.groupedOptionalApps
import app.polarbear.setup.selectedApps
import kotlin.math.roundToInt

private val PillShape = RoundedCornerShape(24.dp)

@Composable
fun AddAppsPicker(
    expanded: Boolean,
    selectedIds: Set<String>,
    onExpandedChange: (Boolean) -> Unit,
    onToggle: (String) -> Unit,
    onBounds: (Rect) -> Unit,
    palette: PortalPalette,
    icons: Map<String, ImageBitmap>,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
) {
    val open = remember { Animatable(if (expanded) 1f else 0f) }
    LaunchedEffect(expanded) {
        open.animateTo(
            if (expanded) 1f else 0f,
            // Critically damped; closing is a little quicker than opening.
            spring(dampingRatio = 1f, stiffness = if (expanded) 340f else 520f),
        )
    }
    val headerTap = remember { MutableInteractionSource() }
    Column(
        modifier
            .fillMaxWidth()
            .onGloballyPositioned { onBounds(it.boundsInRoot()) }
            .clip(PillShape)
            .background(palette.trackFill)
            .border(1.dp, palette.surfaceBorder, PillShape),
    ) {
        Row(
            Modifier
                .fillMaxWidth()
                .height(48.dp)
                .clickable(
                    enabled = enabled,
                    interactionSource = headerTap,
                    indication = null,
                    role = Role.Button,
                    onClick = { onExpandedChange(!expanded) },
                )
                .semantics { stateDescription = if (expanded) "Expanded" else "Collapsed" }
                .padding(start = 18.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            val chosen = selectedApps(selectedIds)
            Text(
                text = when {
                    chosen.isEmpty() -> "Add apps"
                    chosen.size == 1 -> chosen.first().name
                    else -> "${chosen.first().name} +${chosen.size - 1}"
                },
                modifier = Modifier.padding(end = 10.dp),
                fontSize = 14.sp,
                fontWeight = FontWeight.Medium,
                color = palette.textPrimary,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            IconCluster(
                selectedIds = selectedIds,
                icons = icons,
                palette = palette,
                modifier = Modifier.weight(1f).graphicsLayer {
                    // The cluster hands over to the full rows as they unfold.
                    val hide = (open.value * 1.8f).coerceIn(0f, 1f)
                    alpha = 1f - hide
                    translationX = hide * 10.dp.toPx()
                },
            )
            Box(Modifier.width(44.dp).fillMaxHeight(), contentAlignment = Alignment.Center) {
                Canvas(Modifier.size(20.dp)) {
                    // The chevron morphs rather than spins: its tip travels
                    // through a flat line and folds the other way.
                    val t = open.value.coerceIn(0f, 1f)
                    val arms = size.height * (0.36f + 0.26f * t)
                    val tip = size.height * (0.62f - 0.26f * t)
                    val path = Path().apply {
                        moveTo(size.width * 0.22f, arms)
                        lineTo(size.width * 0.5f, tip)
                        lineTo(size.width * 0.78f, arms)
                    }
                    drawPath(
                        path,
                        palette.textSecondary,
                        style = Stroke(2.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round),
                    )
                }
            }
        }
        val density = LocalDensity.current
        val risePx = with(density) { 10.dp.toPx() }
        val blurPx = with(density) { 6.dp.toPx() }
        Column(
            Modifier
                .clipToBounds()
                .layout { measurable, constraints ->
                    val placeable = measurable.measure(
                        constraints.copy(minHeight = 0, maxHeight = Constraints.Infinity),
                    )
                    val height = (placeable.height * open.value.coerceIn(0f, 1f)).roundToInt()
                    layout(placeable.width, height) {
                        if (height > 0) placeable.place(0, 0)
                    }
                }
                .padding(start = 6.dp, end = 6.dp, bottom = 6.dp),
        ) {
            // Rows condense into place one after another, group labels
            // included; on the way back the last row leaves first.
            val groups = groupedOptionalApps()
            val stagger = 0.42f / (groups.size + OPTIONAL_APPS.size).coerceAtLeast(6)
            val condense: (Int) -> Modifier = { index ->
                Modifier.graphicsLayer {
                    val start = 0.1f + index * stagger
                    val shown = ((open.value - start) / 0.42f).coerceIn(0f, 1f)
                    val eased = PortalEmphasizedDecelerate.transform(shown)
                    alpha = eased
                    translationY = (1f - eased) * risePx
                    portalBlur((1f - eased) * blurPx)
                }
            }
            var index = 0
            groups.forEach { (label, apps) ->
                CategoryLabel(label, palette, condense(index++))
                apps.forEach { app ->
                    AppSelectionRow(
                        app = app,
                        icon = icons[app.id],
                        checked = app.id in selectedIds,
                        onToggle = { onToggle(app.id) },
                        palette = palette,
                        enabled = enabled && expanded && app.available,
                        modifier = condense(index++),
                    )
                }
            }
        }
    }
}

/** Small-caps group heading inside the unfolded list. */
@Composable
internal fun CategoryLabel(label: String, palette: PortalPalette, modifier: Modifier = Modifier) {
    Text(
        text = label.uppercase(),
        modifier = modifier.padding(start = 14.dp, top = 10.dp, bottom = 2.dp),
        color = palette.textSecondary.copy(alpha = 0.75f),
        fontSize = 10.sp,
        fontWeight = FontWeight.SemiBold,
        letterSpacing = 1.2.sp,
        maxLines = 1,
    )
}

/** A quiet row of every optional app, with the chosen ones lit. */
@Composable
private fun IconCluster(
    selectedIds: Set<String>,
    icons: Map<String, ImageBitmap>,
    palette: PortalPalette,
    modifier: Modifier = Modifier,
) {
    if (icons.isEmpty()) {
        Spacer(modifier)
        return
    }
    // Right-aligned against the chevron. On a narrow row the label keeps its
    // room and the icons that no longer fit are left out.
    Layout(
        modifier = modifier.semantics { contentDescription = "Optional apps" },
        content = { ClusterIcons(selectedIds, icons, palette) },
    ) { measurables, constraints ->
        val placeables = measurables.map { it.measure(Constraints()) }
        val gap = 5.dp.roundToPx()
        var used = 0
        val shown = placeables.takeWhile { placeable ->
            val next = used + (if (used == 0) 0 else gap) + placeable.width
            (next <= constraints.maxWidth).also { fits -> if (fits) used = next }
        }
        val height = placeables.maxOfOrNull { it.height } ?: 0
        layout(constraints.maxWidth, height) {
            var x = constraints.maxWidth - used
            shown.forEach { placeable ->
                placeable.place(x, 0)
                x += placeable.width + gap
            }
        }
    }
}

@Composable
private fun ClusterIcons(
    selectedIds: Set<String>,
    icons: Map<String, ImageBitmap>,
    palette: PortalPalette,
) {
    OPTIONAL_APPS.forEach { app ->
        val icon = icons[app.id] ?: return@forEach
        val lit by animateFloatAsState(
            if (selectedIds.isEmpty() || app.id in selectedIds) 1f else 0f,
            tween(320, easing = PortalEmphasized),
            label = "cluster icon",
        )
        Image(
            bitmap = icon,
            contentDescription = null,
            // Unchosen apps rest nearly monochrome and colour up when picked.
            colorFilter = if (lit >= 0.999f) {
                null
            } else {
                ColorFilter.colorMatrix(ColorMatrix().apply { setToSaturation(0.15f + 0.85f * lit) })
            },
            modifier = Modifier
                .size(18.dp)
                .graphicsLayer {
                    alpha = if (palette.isDark) 0.38f + 0.62f * lit else 0.45f + 0.55f * lit
                    val s = 0.9f + 0.1f * lit
                    scaleX = s
                    scaleY = s
                },
        )
    }
}

@Composable
private fun AppSelectionRow(
    app: EssentialApp,
    icon: ImageBitmap?,
    checked: Boolean,
    onToggle: () -> Unit,
    palette: PortalPalette,
    enabled: Boolean,
    modifier: Modifier = Modifier,
) {
    val fill by animateColorAsState(
        if (checked) palette.accent.copy(alpha = if (palette.isDark) 0.11f else 0.09f) else palette.accent.copy(alpha = 0f),
        tween(240),
        label = "selected row",
    )
    val check by animateFloatAsState(
        if (checked) 1f else 0f,
        if (checked) tween(360, easing = PortalEmphasized) else tween(200, easing = PortalEmphasizedAccelerate),
        label = "check",
    )
    val tap = remember { MutableInteractionSource() }
    Row(
        modifier
            .fillMaxWidth()
            .graphicsLayer { if (!app.available) alpha = 0.55f }
            .clip(RoundedCornerShape(18.dp))
            .background(fill)
            .clickable(
                enabled = enabled,
                interactionSource = tap,
                indication = null,
                role = Role.Checkbox,
                onClick = onToggle,
            )
            .semantics {
                stateDescription = when {
                    !app.available -> app.unavailableReason ?: "Unavailable"
                    checked -> "Selected"
                    else -> "Not selected"
                }
            }
            .padding(start = 12.dp, end = 12.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(30.dp), contentAlignment = Alignment.Center) {
            if (icon != null) {
                Image(bitmap = icon, contentDescription = null, modifier = Modifier.size(30.dp))
            }
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(app.name, color = palette.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
            Text(
                app.unavailableReason ?: "${app.blurb} · ${formatInstalledSize(app.installedMb)}",
                color = palette.textSecondary,
                fontSize = 11.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Spacer(Modifier.width(8.dp))
        Canvas(Modifier.size(20.dp)) {
            val corner = CornerRadius(6.dp.toPx())
            drawRoundRect(palette.textSecondary.copy(alpha = 0.45f), cornerRadius = corner, style = Stroke(1.dp.toPx()))
            // The box fills first, then the tick draws itself stroke by stroke.
            val boxFill = (check * 1.8f).coerceIn(0f, 1f)
            drawRoundRect(PortalColors.Orange, cornerRadius = corner, alpha = boxFill)
            val tick = ((check - 0.2f) / 0.8f).coerceIn(0f, 1f)
            if (tick > 0f) {
                val path = Path().apply {
                    moveTo(size.width * 0.25f, size.height * 0.52f)
                    lineTo(size.width * 0.44f, size.height * 0.70f)
                    lineTo(size.width * 0.76f, size.height * 0.31f)
                }
                val measure = PathMeasure().apply { setPath(path, false) }
                val drawn = Path()
                measure.getSegment(0f, measure.length * tick, drawn, true)
                drawPath(drawn, PortalColors.Charcoal, style = Stroke(2.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round))
            }
        }
    }
}

internal fun formatInstalledSize(mb: Int): String =
    if (mb >= 1000) "%.1f GB".format(mb / 1000f) else "$mb MB"
