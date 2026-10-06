package app.polarbear.setup

// Debian updates on the Return-to-Plasma screen. One surface carries the
// whole flow: a quiet capsule under "Restoring your desktop" says updates
// are waiting; tapping it grows the same surface into a card with the
// package list; Update keeps that card and turns it into live progress; the
// result folds back into a capsule. Nothing appears when there is nothing to
// install. Native state (ComposeOverlay.systemUpdateState) is the only
// source of truth; the only local state is whether the card is open.

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.MutableTransitionState
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.ComposeOverlay
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import app.polarbear.setup.components.dissolveBlur
import kotlinx.coroutines.delay

/** Let the launch mark land before anything else asks for attention. */
private const val UPDATE_ENTRANCE_DELAY_MS = 650L
private const val PREVIEW_ROWS = 5
private val CardWidth = 320.dp
private val CapsuleCorner = 18.dp
private val CardCorner = 24.dp

private enum class UpdatePanelPhase { Hidden, Offer, Details, Running, Complete, Failed }

@Composable
internal fun ReturnUpdatePanel(
    palette: PortalPalette,
    desktopReady: Boolean,
    suppressed: Boolean,
    onBlockedChanged: (Boolean) -> Unit,
) {
    val state by ComposeOverlay.systemUpdateState()
    var expanded by rememberSaveable { mutableStateOf(false) }
    var dismissed by rememberSaveable { mutableStateOf(false) }
    // Bridges the tap to the first native Running snapshot.
    var requested by remember { mutableStateOf(false) }
    var entered by remember { mutableStateOf(false) }
    val currentBlocked by rememberUpdatedState(onBlockedChanged)

    LaunchedEffect(state.status) {
        if (state.status != ComposeOverlay.SYSTEM_UPDATE_AVAILABLE) requested = false
        if (state.running) dismissed = false
    }
    val phase = when {
        suppressed -> UpdatePanelPhase.Hidden
        state.running || requested -> UpdatePanelPhase.Running
        state.complete -> UpdatePanelPhase.Complete
        state.failed && !dismissed -> UpdatePanelPhase.Failed
        state.available && !dismissed ->
            if (expanded) UpdatePanelPhase.Details else UpdatePanelPhase.Offer
        else -> UpdatePanelPhase.Hidden
    }
    LaunchedEffect(phase) { currentBlocked(phase == UpdatePanelPhase.Running) }
    LaunchedEffect(Unit) {
        delay(UPDATE_ENTRANCE_DELAY_MS)
        entered = true
    }
    val begin = {
        if (ComposeOverlay.beginSystemUpdate()) {
            requested = true
            expanded = false
        }
    }

    val isCard = phase == UpdatePanelPhase.Details ||
        phase == UpdatePanelPhase.Running ||
        phase == UpdatePanelPhase.Failed
    val corner by animateDpAsState(
        targetValue = if (isCard) CardCorner else CapsuleCorner,
        animationSpec = tween(420, easing = PortalEmphasized),
        label = "update surface corner",
    )
    val shape = RoundedCornerShape(corner)

    AnimatedVisibility(
        visible = phase != UpdatePanelPhase.Hidden && (entered || phase == UpdatePanelPhase.Running),
        enter = expandVertically(tween(420, easing = FastOutSlowInEasing), expandFrom = Alignment.Top) +
            fadeIn(tween(420, delayMillis = 60, easing = PortalEmphasizedDecelerate)) +
            slideInVertically(tween(560, easing = PortalEmphasized)) { it / 3 } +
            scaleIn(tween(560, easing = PortalEmphasized), initialScale = 0.96f),
        exit = fadeOut(tween(220, easing = PortalEmphasizedAccelerate)) +
            slideOutVertically(tween(260, easing = PortalEmphasizedAccelerate)) { -it / 5 } +
            scaleOut(tween(260), targetScale = 0.98f) +
            shrinkVertically(tween(420, easing = FastOutSlowInEasing), shrinkTowards = Alignment.Top),
    ) {
        Box(
            modifier = Modifier
                .dissolveBlur(this, radius = 10.dp)
                .padding(top = 22.dp),
        ) {
            AnimatedContent(
                targetState = phase,
                modifier = Modifier
                    .clip(shape)
                    .background(Brush.verticalGradient(listOf(palette.surfaceTop, palette.surfaceBottom)))
                    .border(1.dp, palette.surfaceBorder, shape),
                transitionSpec = {
                    (fadeIn(tween(300, delayMillis = 110, easing = PortalEmphasizedDecelerate)) +
                        scaleIn(tween(420, easing = PortalEmphasized), initialScale = 0.985f))
                        .togetherWith(fadeOut(tween(140, easing = PortalEmphasizedAccelerate)))
                        .using(
                            SizeTransform(clip = true) { _, _ ->
                                spring(
                                    dampingRatio = Spring.DampingRatioNoBouncy,
                                    stiffness = 380f,
                                )
                            },
                        )
                },
                contentAlignment = Alignment.TopCenter,
                label = "update surface",
            ) { target ->
                when (target) {
                    UpdatePanelPhase.Offer -> UpdateCapsule(
                        state = state,
                        palette = palette,
                        onClick = { expanded = true },
                    )
                    UpdatePanelPhase.Details -> UpdateDetails(
                        state = state,
                        palette = palette,
                        onLater = { expanded = false },
                        onUpdate = begin,
                    )
                    UpdatePanelPhase.Running -> UpdateRunning(state = state, palette = palette)
                    UpdatePanelPhase.Complete -> UpdateComplete(
                        desktopReady = desktopReady,
                        palette = palette,
                    )
                    UpdatePanelPhase.Failed -> UpdateFailed(
                        state = state,
                        palette = palette,
                        onDismiss = { dismissed = true },
                        onRetry = begin,
                    )
                    UpdatePanelPhase.Hidden -> Spacer(Modifier.size(0.dp))
                }
            }
        }
    }
}

private fun updateCountText(state: ComposeOverlay.SystemUpdateUiState): String = when {
    state.total == 0 && state.interrupted -> "Finish the last update"
    state.total == 1 -> "1 update available"
    else -> "${state.total} updates available"
}

@Composable
private fun UpdateCapsule(
    state: ComposeOverlay.SystemUpdateUiState,
    palette: PortalPalette,
    onClick: () -> Unit,
) {
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val pressScale by animateFloatAsState(
        targetValue = if (pressed) 0.975f else 1f,
        animationSpec = tween(110),
        label = "update capsule press",
    )
    Row(
        modifier = Modifier
            .graphicsLayer {
                scaleX = pressScale
                scaleY = pressScale
            }
            .clickable(
                interactionSource = tap,
                indication = null,
                role = Role.Button,
                onClickLabel = "Show updates",
                onClick = onClick,
            )
            .padding(start = 14.dp, end = 16.dp, top = 9.dp, bottom = 9.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        UpdateArrow(palette = palette, lifting = state.security > 0 || state.interrupted)
        Spacer(Modifier.width(10.dp))
        Text(
            text = updateCountText(state),
            fontSize = 13.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textPrimary.copy(alpha = 0.92f),
        )
        if (state.security > 0) {
            Text(
                text = "  ·  ${state.security} security",
                fontSize = 13.sp,
                fontWeight = FontWeight.Medium,
                color = palette.accent,
            )
        }
        Spacer(Modifier.width(8.dp))
        Text(
            text = "›",
            fontSize = 16.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textMuted,
        )
    }
}

/** Portal-orange update arrow; drifts upward when something is urgent. */
@Composable
private fun UpdateArrow(palette: PortalPalette, lifting: Boolean) {
    // A still arrow runs no transition: an idle infinite one still requests
    // a frame of the whole window every vsync.
    val lift = if (lifting) {
        rememberInfiniteTransition(label = "update arrow").animateFloat(
            initialValue = 0f,
            targetValue = 1f,
            animationSpec = infiniteRepeatable(
                animation = tween(1400, easing = FastOutSlowInEasing),
                repeatMode = RepeatMode.Reverse,
            ),
            label = "update arrow lift",
        )
    } else {
        null
    }
    Canvas(Modifier.size(14.dp)) {
        val offset = -1.5.dp.toPx() * (lift?.value ?: 0f)
        val stroke = Stroke(
            width = 1.8.dp.toPx(),
            cap = StrokeCap.Round,
            join = StrokeJoin.Round,
        )
        val x = size.width / 2f
        val top = size.height * 0.16f + offset
        val bottom = size.height * 0.86f + offset
        val wing = size.width * 0.30f
        drawLine(
            color = palette.accent,
            start = Offset(x, bottom),
            end = Offset(x, top),
            strokeWidth = stroke.width,
            cap = StrokeCap.Round,
        )
        drawPath(
            path = Path().apply {
                moveTo(x - wing, top + wing)
                lineTo(x, top)
                lineTo(x + wing, top + wing)
            },
            color = palette.accent,
            style = stroke,
        )
    }
}

@Composable
private fun UpdateDetails(
    state: ComposeOverlay.SystemUpdateUiState,
    palette: PortalPalette,
    onLater: () -> Unit,
    onUpdate: () -> Unit,
) {
    Column(
        modifier = Modifier
            .widthIn(max = CardWidth)
            .fillMaxWidth()
            .padding(start = 20.dp, end = 20.dp, top = 18.dp, bottom = 14.dp),
    ) {
        Text(
            text = if (state.total == 0) "Finish the last update" else "Debian updates",
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
            color = palette.textPrimary,
        )
        Spacer(Modifier.height(4.dp))
        val other = state.total - state.security
        Text(
            text = when {
                state.total == 0 -> "An earlier update stopped before it finished."
                state.security == 0 -> "${state.total} ${if (state.total == 1) "package" else "packages"} from Debian 13"
                other == 0 -> "${state.security} security ${if (state.security == 1) "fix" else "fixes"}"
                else -> "${state.security} security ${if (state.security == 1) "fix" else "fixes"} · $other other"
            },
            fontSize = 12.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textMuted,
        )
        if (state.packages.isNotEmpty()) {
            Spacer(Modifier.height(14.dp))
            state.packages.take(PREVIEW_ROWS).forEachIndexed { index, pending ->
                StaggeredRow(index) { PackageRow(pending, palette) }
            }
            val more = state.total - PREVIEW_ROWS
            if (more > 0) {
                StaggeredRow(PREVIEW_ROWS) {
                    Text(
                        text = "+ $more more",
                        modifier = Modifier.padding(top = 4.dp),
                        fontSize = 12.sp,
                        color = palette.textMuted,
                    )
                }
            }
        }
        Spacer(Modifier.height(14.dp))
        Text(
            text = "Plasma closes while these install, then opens again.",
            fontSize = 11.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textMuted.copy(alpha = 0.8f),
        )
        Spacer(Modifier.height(14.dp))
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.End,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            PanelButton(text = "Not now", primary = false, palette = palette, onClick = onLater)
            Spacer(Modifier.width(6.dp))
            PanelButton(
                text = if (state.total == 0) "Finish" else "Update",
                primary = true,
                palette = palette,
                onClick = onUpdate,
            )
        }
    }
}

/** Rows settle in one after another as the card opens. */
@Composable
private fun StaggeredRow(index: Int, content: @Composable () -> Unit) {
    val visible = remember { MutableTransitionState(false).apply { targetState = true } }
    AnimatedVisibility(
        visibleState = visible,
        enter = fadeIn(tween(260, delayMillis = 120 + index * 40, easing = PortalEmphasizedDecelerate)) +
            slideInVertically(tween(380, delayMillis = 120 + index * 40, easing = PortalEmphasized)) { it / 2 },
    ) {
        content()
    }
}

@Composable
private fun PackageRow(pending: ComposeOverlay.PendingPackage, palette: PortalPalette) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 3.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = pending.name,
            modifier = Modifier.weight(1f),
            fontSize = 12.sp,
            fontFamily = FontFamily.Monospace,
            color = palette.textSecondary,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        if (pending.security) {
            Text(
                text = "security",
                modifier = Modifier
                    .padding(start = 8.dp)
                    .clip(RoundedCornerShape(50))
                    .background(palette.accent.copy(alpha = 0.14f))
                    .padding(horizontal = 7.dp, vertical = 2.dp),
                fontSize = 10.sp,
                fontWeight = FontWeight.Medium,
                color = palette.accent,
            )
        }
    }
}

@Composable
internal fun PanelButton(
    text: String,
    primary: Boolean,
    palette: PortalPalette,
    onClick: () -> Unit,
) {
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val pressScale by animateFloatAsState(
        targetValue = if (pressed) 0.96f else 1f,
        animationSpec = tween(110),
        label = "update button press",
    )
    val shape = RoundedCornerShape(50)
    Box(
        modifier = Modifier
            .graphicsLayer {
                scaleX = pressScale
                scaleY = pressScale
            }
            .clip(shape)
            .then(if (primary) Modifier.background(palette.accent) else Modifier)
            .clickable(
                interactionSource = tap,
                indication = null,
                role = Role.Button,
                onClick = onClick,
            )
            .padding(horizontal = 18.dp, vertical = 9.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = text,
            fontSize = 13.sp,
            fontWeight = FontWeight.SemiBold,
            color = if (primary) PortalColors.Charcoal else palette.textMuted,
        )
    }
}

@Composable
private fun UpdateRunning(state: ComposeOverlay.SystemUpdateUiState, palette: PortalPalette) {
    Column(
        modifier = Modifier
            .widthIn(max = CardWidth)
            .fillMaxWidth()
            .padding(horizontal = 20.dp, vertical = 18.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                text = "Installing updates",
                modifier = Modifier.weight(1f),
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
                color = palette.textPrimary,
            )
            Text(
                text = "${state.progress.coerceIn(0, 100)}%",
                fontSize = 12.sp,
                fontWeight = FontWeight.Medium,
                fontFamily = FontFamily.Monospace,
                color = palette.textMuted,
            )
        }
        Spacer(Modifier.height(14.dp))
        UpdateProgressBar(progress = state.progress, palette = palette)
        Spacer(Modifier.height(10.dp))
        Text(
            text = state.message.ifBlank { "Preparing…" },
            fontSize = 11.sp,
            fontWeight = FontWeight.Medium,
            fontFamily = FontFamily.Monospace,
            color = palette.textMuted,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Spacer(Modifier.height(10.dp))
        Text(
            text = "Keep Portal open. Plasma returns when this finishes.",
            fontSize = 11.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textMuted.copy(alpha = 0.8f),
        )
    }
}

@Composable
private fun UpdateProgressBar(progress: Int, palette: PortalPalette) {
    val animated by animateFloatAsState(
        targetValue = progress.coerceIn(0, 100) / 100f,
        animationSpec = tween(360, easing = FastOutSlowInEasing),
        label = "update progress",
    )
    val sheen = rememberInfiniteTransition(label = "update progress sheen")
    val glow by sheen.animateFloat(
        initialValue = 0.22f,
        targetValue = 0.48f,
        animationSpec = infiniteRepeatable(
            animation = tween(1100, easing = FastOutSlowInEasing),
            repeatMode = RepeatMode.Reverse,
        ),
        label = "update progress glow",
    )
    Canvas(
        modifier = Modifier
            .fillMaxWidth()
            .height(4.dp),
    ) {
        val radius = CornerRadius(size.height / 2f, size.height / 2f)
        drawRoundRect(color = palette.textPrimary.copy(alpha = 0.10f), cornerRadius = radius)
        val width = size.width * animated
        if (width > 0f) {
            drawRoundRect(
                color = palette.accent.copy(alpha = glow),
                topLeft = Offset(0f, -2.dp.toPx()),
                size = Size(width, size.height + 4.dp.toPx()),
                cornerRadius = CornerRadius(size.height, size.height),
            )
            drawRoundRect(
                color = palette.accent,
                size = Size(width, size.height),
                cornerRadius = radius,
            )
        }
    }
}

@Composable
private fun UpdateComplete(desktopReady: Boolean, palette: PortalPalette) {
    Row(
        modifier = Modifier.padding(start = 14.dp, end = 18.dp, top = 9.dp, bottom = 9.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Canvas(Modifier.size(12.dp)) {
            val stroke = 1.8.dp.toPx()
            val path = Path().apply {
                moveTo(size.width * 0.14f, size.height * 0.54f)
                lineTo(size.width * 0.42f, size.height * 0.80f)
                lineTo(size.width * 0.88f, size.height * 0.24f)
            }
            drawPath(
                path = path,
                color = palette.accent,
                style = Stroke(
                    width = stroke,
                    cap = StrokeCap.Round,
                    join = StrokeJoin.Round,
                ),
            )
        }
        Spacer(Modifier.width(10.dp))
        AnimatedContent(
            targetState = desktopReady,
            transitionSpec = {
                fadeIn(tween(320, delayMillis = 80)).togetherWith(fadeOut(tween(160)))
                    .using(SizeTransform(clip = false))
            },
            label = "update complete text",
        ) { ready ->
            Text(
                text = if (ready) "Updates installed" else "Updates installed · Restarting Plasma…",
                fontSize = 13.sp,
                fontWeight = FontWeight.Medium,
                color = palette.textPrimary.copy(alpha = 0.92f),
            )
        }
    }
}

@Composable
private fun UpdateFailed(
    state: ComposeOverlay.SystemUpdateUiState,
    palette: PortalPalette,
    onDismiss: () -> Unit,
    onRetry: () -> Unit,
) {
    Column(
        modifier = Modifier
            .widthIn(max = CardWidth)
            .fillMaxWidth()
            .padding(start = 20.dp, end = 20.dp, top = 18.dp, bottom = 14.dp),
    ) {
        Text(
            text = "Updates didn't finish",
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
            color = palette.textPrimary,
        )
        Spacer(Modifier.height(6.dp))
        Text(
            text = "Plasma is back with the updates that did install. Portal's graphics stack was not touched.",
            fontSize = 12.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textMuted,
        )
        if (state.message.isNotBlank()) {
            Spacer(Modifier.height(10.dp))
            Text(
                text = state.message,
                fontSize = 11.sp,
                fontFamily = FontFamily.Monospace,
                color = palette.textMuted.copy(alpha = 0.85f),
                maxLines = 3,
                overflow = TextOverflow.Ellipsis,
                textAlign = TextAlign.Start,
            )
        }
        Spacer(Modifier.height(14.dp))
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.End,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            PanelButton(text = "Dismiss", primary = false, palette = palette, onClick = onDismiss)
            Spacer(Modifier.width(6.dp))
            PanelButton(text = "Try again", primary = true, palette = palette, onClick = onRetry)
        }
    }
}
