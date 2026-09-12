package app.polarbear.setup.components

import androidx.compose.animation.*
import androidx.compose.animation.core.*
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.*

/** Inline container transformation: AnimatedContent reserves spring-driven space
 * in the real layout; sharedBounds carries the container and sharedElement carries
 * the header. No floating dialog, screen navigation or animateContentSize shortcut.
 */
@OptIn(ExperimentalSharedTransitionApi::class)
@Composable
fun AddAppsPicker(expanded: Boolean, selectedIds: Set<String>, onExpandedChange: (Boolean) -> Unit,
    onToggle: (String) -> Unit, onBounds: (Rect) -> Unit, palette: PortalPalette,
    modifier: Modifier = Modifier, collapsedHeight: Dp = 48.dp) {
    val transition = updateTransition(expanded, label = "Add apps container")
    val corner by transition.animateDp({ spring(dampingRatio = 1f, stiffness = 260f) }, label = "glass corner") { 24.dp }
    val inset by transition.animateDp({ spring(dampingRatio = 1f, stiffness = 260f) }, label = "glass inset") { if (it) 16.dp else 18.dp }
    val rotation = transition.animateFloat({ spring(dampingRatio = 1f, stiffness = 260f) }, label = "chevron") { if (it) 180f else 0f }
    val collapsedTap = remember { MutableInteractionSource() }
    val headerTap = remember { MutableInteractionSource() }
    SharedTransitionLayout(modifier.fillMaxWidth().onGloballyPositioned { onBounds(it.boundsInRoot()) }) {
        transition.AnimatedContent(
            contentAlignment = Alignment.TopStart,
            transitionSpec = {
                (EnterTransition.None togetherWith ExitTransition.None).using(
                    SizeTransform(clip = true) { _, _ -> spring(dampingRatio = 1f, stiffness = 260f) })
            },
        ) { open ->
            val shape = RoundedCornerShape(corner)
            Column(Modifier.fillMaxWidth()
                .sharedBounds(rememberSharedContentState("add-apps-container"), this,
                    boundsTransform = { _, _ -> spring(dampingRatio = 1f, stiffness = 260f) },
                    enter = fadeIn(tween(220, delayMillis = if (open) 100 else 40)),
                    exit = fadeOut(tween(100)),
                    resizeMode = SharedTransitionScope.ResizeMode.RemeasureToBounds,
                    renderInOverlayDuringTransition = false)
                .then(
                    if (!open) {
                        Modifier.clickable(
                            interactionSource = collapsedTap,
                            indication = null,
                            role = Role.Button,
                            onClick = { onExpandedChange(true) },
                        )
                    } else Modifier
                )
                .then(if (!open) Modifier.height(collapsedHeight) else Modifier)
                .clip(shape)
                .background(palette.trackFill)
                .border(1.dp, palette.surfaceBorder, shape)
                .padding(
                    start = inset,
                    end = if (open) inset else 0.dp,
                    top = if (open) 8.dp else 0.dp,
                    bottom = if (open) 8.dp else 0.dp,
                )) {
                Row(Modifier.fillMaxWidth()
                    .sharedElement(rememberSharedContentState("add-apps-header"), this@AnimatedContent,
                        boundsTransform = { _, _ -> spring(dampingRatio = 1f, stiffness = 260f) },
                        renderInOverlayDuringTransition = false)
                    .clip(RoundedCornerShape(10.dp))
                    .then(
                        if (open) {
                            Modifier.clickable(
                                interactionSource = headerTap,
                                indication = null,
                                role = Role.Button,
                                onClick = { onExpandedChange(false) },
                            )
                        } else Modifier
                    )
                    .semantics { stateDescription = if (expanded) "Expanded" else "Collapsed" }
                    .height(48.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(
                            "Add Essential Apps",
                            fontSize = 14.sp,
                            lineHeight = 17.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textPrimary,
                        )
                    }
                    Box(
                        modifier = Modifier
                            .width(48.dp)
                            .fillMaxHeight(),
                        contentAlignment = Alignment.Center,
                    ) {
                        Canvas(Modifier.size(20.dp).graphicsLayer { rotationZ = rotation.value }) {
                            val path = Path().apply {
                                moveTo(size.width * 0.22f, size.height * 0.36f)
                                lineTo(size.width * 0.5f, size.height * 0.62f)
                                lineTo(size.width * 0.78f, size.height * 0.36f)
                            }
                            drawPath(path, palette.textSecondary, style = Stroke(2.dp.toPx(), cap = StrokeCap.Round))
                        }
                    }
                }
                if (!open) {
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(end = 48.dp, bottom = 10.dp),
                        verticalArrangement = Arrangement.spacedBy(2.dp),
                    ) {
                        Text(
                            if (selectedIds.isEmpty()) {
                                "Optional desktop applications"
                            } else {
                                "Selected · ${selectedAppsSummary(selectedIds)}"
                            },
                            fontSize = 11.sp,
                            lineHeight = 14.sp,
                            color = palette.textSecondary,
                            maxLines = 1,
                        )
                        Text(
                            "LibreOffice · VLC · GIMP",
                            fontSize = 11.sp,
                            lineHeight = 14.sp,
                            color = palette.textSecondary.copy(alpha = 0.76f),
                        )
                        Text(
                            "Krita · Inkscape · Thunderbird",
                            fontSize = 11.sp,
                            lineHeight = 14.sp,
                            color = palette.textSecondary.copy(alpha = 0.76f),
                        )
                        Text(
                            "Okular · Kate",
                            fontSize = 11.sp,
                            lineHeight = 14.sp,
                            color = palette.textSecondary.copy(alpha = 0.76f),
                        )
                    }
                }
                if (open) {
                    // Eight compact custom rows; the growing container clips and
                    // progressively reveals them while the subtitle dissolves.
                    ESSENTIAL_APPS.forEach { app ->
                        AppSelectionRow(app, app.id in selectedIds, { onToggle(app.id) }, palette)
                    }
                }
            }
        }
    }
}

@Composable
private fun AppSelectionRow(app: EssentialApp, checked: Boolean, onToggle: () -> Unit, palette: PortalPalette) {
    val fill by animateColorAsState(if (checked) palette.accent.copy(alpha = 0.11f) else palette.accent.copy(alpha = 0f), tween(180), label = "selected row")
    val check by animateFloatAsState(if (checked) 1f else 0f, tween(180), label = "check")
    val tap = remember { MutableInteractionSource() }
    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(fill)
        .clickable(
            interactionSource = tap,
            indication = null,
            role = Role.Checkbox,
            onClick = onToggle,
        )
        .semantics { stateDescription = if (checked) "Selected" else "Not selected" }
        .padding(horizontal = 10.dp, vertical = 9.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(app.name, color = palette.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
            Text(app.blurb, color = palette.textSecondary, fontSize = 11.sp)
        }
        Canvas(Modifier.size(20.dp)) {
            drawRoundRect(palette.textSecondary.copy(alpha = 0.5f), cornerRadius = androidx.compose.ui.geometry.CornerRadius(5.dp.toPx()), style = Stroke(1.dp.toPx()))
            drawRoundRect(PortalColors.Orange, cornerRadius = androidx.compose.ui.geometry.CornerRadius(5.dp.toPx()), alpha = check)
            val path = Path().apply {
                moveTo(size.width * 0.23f, size.height * 0.51f)
                lineTo(size.width * 0.44f, size.height * 0.72f)
                lineTo(size.width * 0.78f, size.height * 0.28f)
            }
            drawPath(path, PortalColors.Charcoal, alpha = check, style = Stroke(2.dp.toPx(), cap = StrokeCap.Round))
        }
    }
}
