package app.polarbear.setup

// Updates on the Return-to-Plasma screen: Portal itself, the graphics
// driver, ChatGPT and Debian packages. One surface carries the whole flow: a
// quiet capsule under "Restoring your desktop" says updates are waiting;
// tapping it grows the same surface into a card with the list; Update keeps
// that card and turns it into live progress; the result folds back into a
// capsule. Nothing appears when there is nothing to install. Native state
// (ComposeOverlay.systemUpdateState) is the only source of truth; local
// state is only whether the card is open and the install-permission detour.

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.MutableTransitionState
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.Spring
import androidx.compose.animation.animateColorAsState
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
import androidx.compose.foundation.Image
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
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
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
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import app.polarbear.AppUpdateInstaller
import app.polarbear.ComposeOverlay
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import app.polarbear.setup.components.dissolveBlur
import app.polarbear.setup.components.rememberAppIcons
import kotlinx.coroutines.delay

/** Let the launch mark land before anything else asks for attention. */
private const val UPDATE_ENTRANCE_DELAY_MS = 650L
private const val PREVIEW_ROWS = 5
/** With other updates listed above, Debian previews fewer packages. */
private const val PREVIEW_ROWS_WITH_COMPONENTS = 3
/** Below this, unplugged, an install that dies midway is too likely. */
private const val MIN_BATTERY_PERCENT = 20
private val CardWidth = 320.dp
private val IconTile = 30.dp
private val StepIconTile = 26.dp
/** Text beside an icon starts here, so rows under it line up. */
private val IconGap = 12.dp
private val CapsuleCorner = 18.dp
private val CardCorner = 24.dp

private enum class UpdatePanelPhase { Hidden, Offer, Details, Permission, Running, Complete, Failed }

/** Whether the battery is too low, unplugged, to start a system update. */
private fun batteryTooLow(context: Context): Boolean {
    val battery = try {
        context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
    } catch (_: Exception) {
        null
    } ?: return false
    val level = battery.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
    val scale = battery.getIntExtra(BatteryManager.EXTRA_SCALE, -1)
    val plugged = battery.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) != 0
    if (level < 0 || scale <= 0) return false
    return !plugged && level * 100 / scale < MIN_BATTERY_PERCENT
}

@Composable
internal fun ReturnUpdatePanel(
    palette: PortalPalette,
    desktopReady: Boolean,
    suppressed: Boolean,
    onBlockedChanged: (Boolean) -> Unit,
) {
    val state by ComposeOverlay.systemUpdateState()
    val context = LocalContext.current
    val icons by rememberAppIcons(listOf("chatgpt"))
    var expanded by rememberSaveable { mutableStateOf(false) }
    var dismissed by rememberSaveable { mutableStateOf(false) }
    // Portal needs Android's "Install unknown apps" before it can update
    // itself; the card detours through Settings and resumes on return.
    var askPermission by rememberSaveable { mutableStateOf(false) }
    var visitedSettings by rememberSaveable { mutableStateOf(false) }
    var includedApp by rememberSaveable { mutableStateOf(false) }
    var entered by remember { mutableStateOf(false) }
    var lowBattery by remember { mutableStateOf(false) }
    val currentBlocked by rememberUpdatedState(onBlockedChanged)

    var resumes by remember { mutableIntStateOf(0) }
    val lifecycleOwner = LocalLifecycleOwner.current
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) resumes++
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }

    LaunchedEffect(state.status) {
        if (state.running) dismissed = false
    }
    val phase = when {
        suppressed -> UpdatePanelPhase.Hidden
        state.running -> UpdatePanelPhase.Running
        state.complete -> UpdatePanelPhase.Complete
        (state.failed || state.available) && !dismissed && askPermission -> UpdatePanelPhase.Permission
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
    // Only Debian and the driver risk a half-finished system; Android swaps
    // the app in one step.
    val risky = state.debian || state.components.any { it.kind != "app" }
    LaunchedEffect(phase, resumes, risky) {
        while (phase == UpdatePanelPhase.Details || phase == UpdatePanelPhase.Failed) {
            lowBattery = risky && batteryTooLow(context)
            delay(15_000)
        }
    }
    val start = { includeApp: Boolean ->
        val offersApp = state.app != null
        if (ComposeOverlay.beginSystemUpdate(includeApp)) {
            includedApp = includeApp && offersApp
            expanded = false
            askPermission = false
            visitedSettings = false
        }
    }
    val begin = {
        if (risky && batteryTooLow(context)) {
            lowBattery = true
        } else if (state.app != null && !AppUpdateInstaller.canInstall(context)) {
            askPermission = true
        } else {
            start(true)
        }
    }
    // Back from Settings with the permission granted: carry on by itself.
    LaunchedEffect(resumes) {
        if (askPermission && visitedSettings && AppUpdateInstaller.canInstall(context)) start(true)
    }

    val isCard = phase == UpdatePanelPhase.Details ||
        phase == UpdatePanelPhase.Permission ||
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
                        icons = icons,
                        lowBattery = lowBattery,
                        onLater = { expanded = false },
                        onUpdate = begin,
                    )
                    UpdatePanelPhase.Permission -> UpdatePermission(
                        state = state,
                        palette = palette,
                        visitedSettings = visitedSettings,
                        onSkip = {
                            if (state.count > 1 || state.debian) {
                                start(false)
                            } else {
                                askPermission = false
                                visitedSettings = false
                                expanded = false
                            }
                        },
                        onAllow = {
                            if (AppUpdateInstaller.canInstall(context)) {
                                start(true)
                            } else if (AppUpdateInstaller.openInstallPermissionSettings(context)) {
                                visitedSettings = true
                            }
                        },
                    )
                    UpdatePanelPhase.Running -> UpdateRunning(
                        state = state,
                        palette = palette,
                        icons = icons,
                        includedApp = includedApp,
                    )
                    UpdatePanelPhase.Complete -> UpdateComplete(
                        desktopReady = desktopReady,
                        palette = palette,
                    )
                    UpdatePanelPhase.Failed -> UpdateFailed(
                        state = state,
                        palette = palette,
                        icons = icons,
                        lowBattery = lowBattery,
                        onDismiss = { dismissed = true },
                        onRetry = begin,
                    )
                    UpdatePanelPhase.Hidden -> Spacer(Modifier.size(0.dp))
                }
            }
        }
    }
}

private fun updateCountText(state: ComposeOverlay.SystemUpdateUiState): String {
    val app = state.app
    return when {
        state.count == 0 && state.interrupted -> "Finish the last update"
        state.count == 1 && app != null -> "${app.title} available"
        state.count == 1 -> "1 update available"
        else -> "${state.count} updates available"
    }
}

private fun debianSummary(state: ComposeOverlay.SystemUpdateUiState): String {
    val other = state.total - state.security
    return when {
        state.total == 0 -> "An earlier update stopped before it finished."
        state.security == 0 -> "${state.total} ${if (state.total == 1) "package" else "packages"} from Debian 13"
        other == 0 -> "${state.security} security ${if (state.security == 1) "fix" else "fixes"}"
        else -> "${state.security} security ${if (state.security == 1) "fix" else "fixes"} · $other other"
    }
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
    icons: Map<String, ImageBitmap>,
    lowBattery: Boolean,
    onLater: () -> Unit,
    onUpdate: () -> Unit,
) {
    Column(
        modifier = Modifier
            .widthIn(max = CardWidth)
            .fillMaxWidth()
            .padding(start = 20.dp, end = 20.dp, top = 18.dp, bottom = 14.dp),
    ) {
        val onlyDebian = state.components.isEmpty()
        Text(
            text = when {
                state.count == 0 -> "Finish the last update"
                onlyDebian -> "Debian updates"
                state.count == 1 -> "Update available"
                else -> "Updates"
            },
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
            color = palette.textPrimary,
        )
        var row = 0
        if (onlyDebian) {
            Spacer(Modifier.height(4.dp))
            Text(
                text = debianSummary(state),
                fontSize = 12.sp,
                fontWeight = FontWeight.Medium,
                color = palette.textMuted,
            )
            if (state.packages.isNotEmpty()) Spacer(Modifier.height(14.dp))
            PackagePreview(state, palette, rows = PREVIEW_ROWS, firstIndex = row)
        } else {
            Spacer(Modifier.height(12.dp))
            state.components.forEach { component ->
                StaggeredRow(row++) {
                    SourceRow(component.kind, component.title, component.detail, palette, icons)
                }
            }
            if (state.debian) {
                StaggeredRow(row++) {
                    SourceRow("debian", "Debian packages", debianSummary(state), palette, icons)
                }
                Column(Modifier.padding(start = IconTile + IconGap)) {
                    PackagePreview(state, palette, rows = PREVIEW_ROWS_WITH_COMPONENTS, firstIndex = row)
                }
            }
        }
        Spacer(Modifier.height(14.dp))
        Text(
            text = detailsFootnote(state),
            fontSize = 11.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textMuted.copy(alpha = 0.8f),
        )
        if (lowBattery) {
            Spacer(Modifier.height(8.dp))
            LowBatteryNote(palette)
        }
        Spacer(Modifier.height(14.dp))
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.End,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            PanelButton(text = "Not now", primary = false, palette = palette, onClick = onLater)
            Spacer(Modifier.width(6.dp))
            PanelButton(
                text = if (state.count == 0) "Finish" else "Update",
                primary = true,
                enabled = !lowBattery,
                palette = palette,
                onClick = onUpdate,
            )
        }
    }
}

/** What happens to the desktop (and to Portal) while the updates install. */
private fun detailsFootnote(state: ComposeOverlay.SystemUpdateUiState): String = when {
    state.app != null && state.count == 1 ->
        "Plasma closes, then Android asks to install the update and closes Portal to finish. Open Portal again afterwards."
    state.app != null ->
        "Plasma closes while these install, then opens again. Portal updates last; Android closes it to finish."
    state.count <= 1 -> "Plasma closes while this installs, then opens again."
    else -> "Plasma closes while these install, then opens again."
}

/** The head of the pending Debian list, security fixes first. */
@Composable
private fun PackagePreview(
    state: ComposeOverlay.SystemUpdateUiState,
    palette: PortalPalette,
    rows: Int,
    firstIndex: Int,
) {
    state.packages.take(rows).forEachIndexed { index, pending ->
        StaggeredRow(firstIndex + index) { PackageRow(pending, palette) }
    }
    val more = state.total - rows
    if (more > 0) {
        StaggeredRow(firstIndex + rows) {
            Text(
                text = "+ $more more",
                modifier = Modifier.padding(top = 4.dp),
                fontSize = 12.sp,
                color = palette.textMuted,
            )
        }
    }
}

@Composable
private fun LowBatteryNote(palette: PortalPalette) {
    Text(
        text = "Plug in or charge to $MIN_BATTERY_PERCENT% first. An install cut off midway can leave Plasma unable to start.",
        fontSize = 11.sp,
        fontWeight = FontWeight.Medium,
        color = palette.accent,
    )
}

/** One thing that updates: its icon, its name and a line about it. */
@Composable
private fun SourceRow(
    kind: String,
    title: String,
    detail: String,
    palette: PortalPalette,
    icons: Map<String, ImageBitmap>,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 5.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        UpdateIcon(kind, palette, icons, IconTile)
        Spacer(Modifier.width(IconGap))
        Column(Modifier.weight(1f)) {
            Text(
                text = title,
                fontSize = 13.sp,
                fontWeight = FontWeight.Medium,
                color = palette.textPrimary.copy(alpha = 0.92f),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (detail.isNotBlank()) {
                Text(
                    text = detail,
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Medium,
                    color = palette.textMuted,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

/** A small tile per source: Portal's mark, ChatGPT's icon, drawn glyphs otherwise. */
@Composable
private fun UpdateIcon(
    kind: String,
    palette: PortalPalette,
    icons: Map<String, ImageBitmap>,
    tile: Dp,
) {
    val shape = RoundedCornerShape(tile * 0.3f)
    Box(
        modifier = Modifier
            .size(tile)
            .clip(shape)
            .background(palette.textPrimary.copy(alpha = 0.06f))
            .border(1.dp, palette.surfaceBorder, shape),
        contentAlignment = Alignment.Center,
    ) {
        val chatgpt = icons["chatgpt"]
        when {
            kind == "app" -> Image(
                painter = portalMarkPainter(main = palette.logoMain, threshold = palette.logoThreshold),
                contentDescription = null,
                modifier = Modifier.size(width = tile * 0.4f, height = tile * 0.6f),
            )
            kind == "chatgpt" && chatgpt != null -> Image(
                bitmap = chatgpt,
                contentDescription = null,
                modifier = Modifier.size(tile * 0.66f),
            )
            kind == "driver" -> ChipGlyph(palette, tile * 0.6f)
            else -> PackageGlyph(palette, tile * 0.6f)
        }
    }
}

/** A chip with pins: the graphics driver. */
@Composable
private fun ChipGlyph(palette: PortalPalette, size: Dp) {
    Canvas(Modifier.size(size)) {
        val stroke = 1.4.dp.toPx()
        val w = this.size.width
        val body = w * 0.25f
        drawRoundRect(
            color = palette.textSecondary,
            topLeft = Offset(body, body),
            size = Size(w - 2 * body, w - 2 * body),
            cornerRadius = CornerRadius(w * 0.08f, w * 0.08f),
            style = Stroke(width = stroke),
        )
        drawRoundRect(
            color = palette.accent,
            topLeft = Offset(w * 0.4f, w * 0.4f),
            size = Size(w * 0.2f, w * 0.2f),
            cornerRadius = CornerRadius(w * 0.04f, w * 0.04f),
        )
        for (at in listOf(0.38f, 0.62f)) {
            val p = w * at
            drawLine(palette.textSecondary, Offset(p, w * 0.08f), Offset(p, body), stroke, StrokeCap.Round)
            drawLine(palette.textSecondary, Offset(p, w - body), Offset(p, w * 0.92f), stroke, StrokeCap.Round)
            drawLine(palette.textSecondary, Offset(w * 0.08f, p), Offset(body, p), stroke, StrokeCap.Round)
            drawLine(palette.textSecondary, Offset(w - body, p), Offset(w * 0.92f, p), stroke, StrokeCap.Round)
        }
    }
}

/** A parcel: Debian's packages. */
@Composable
private fun PackageGlyph(palette: PortalPalette, size: Dp) {
    Canvas(Modifier.size(size)) {
        val stroke = Stroke(width = 1.4.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round)
        val w = this.size.width
        fun at(x: Float, y: Float) = Offset(w * x, w * y)
        val outline = Path().apply {
            moveTo(w * 0.5f, w * 0.1f)
            lineTo(w * 0.88f, w * 0.3f)
            lineTo(w * 0.88f, w * 0.7f)
            lineTo(w * 0.5f, w * 0.9f)
            lineTo(w * 0.12f, w * 0.7f)
            lineTo(w * 0.12f, w * 0.3f)
            close()
        }
        drawPath(outline, palette.textSecondary, style = stroke)
        drawPath(
            Path().apply {
                moveTo(w * 0.12f, w * 0.3f)
                lineTo(w * 0.5f, w * 0.5f)
                lineTo(w * 0.88f, w * 0.3f)
                moveTo(w * 0.5f, w * 0.5f)
                lineTo(w * 0.5f, w * 0.9f)
            },
            palette.textSecondary,
            style = stroke,
        )
        drawLine(palette.accent, at(0.31f, 0.2f), at(0.69f, 0.4f), stroke.width, StrokeCap.Round)
    }
}

/** Android's install permission, asked for only when Portal would update itself. */
@Composable
private fun UpdatePermission(
    state: ComposeOverlay.SystemUpdateUiState,
    palette: PortalPalette,
    visitedSettings: Boolean,
    onSkip: () -> Unit,
    onAllow: () -> Unit,
) {
    val others = state.count > 1 || state.debian
    Column(
        modifier = Modifier
            .widthIn(max = CardWidth)
            .fillMaxWidth()
            .padding(start = 20.dp, end = 20.dp, top = 18.dp, bottom = 14.dp),
    ) {
        Text(
            text = "Let Portal update itself",
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
            color = palette.textPrimary,
        )
        Spacer(Modifier.height(6.dp))
        Text(
            text = "Android asks before an app installs updates. Turn on \u201cAllow from this source\u201d " +
                "for Portal, then come back and the update carries on.",
            fontSize = 12.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textMuted,
        )
        if (visitedSettings) {
            Spacer(Modifier.height(8.dp))
            Text(
                text = "It's still off, so Portal can't install its update yet.",
                fontSize = 11.sp,
                fontWeight = FontWeight.Medium,
                color = palette.accent,
            )
        }
        Spacer(Modifier.height(14.dp))
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.End,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            PanelButton(
                text = if (others) "Update the rest" else "Not now",
                primary = false,
                palette = palette,
                onClick = onSkip,
            )
            Spacer(Modifier.width(6.dp))
            PanelButton(text = "Open settings", primary = true, palette = palette, onClick = onAllow)
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
    enabled: Boolean = true,
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
                alpha = if (enabled) 1f else 0.4f
            }
            .clip(shape)
            .then(if (primary) Modifier.background(palette.accent) else Modifier)
            .clickable(
                interactionSource = tap,
                indication = null,
                enabled = enabled,
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
private fun UpdateRunning(
    state: ComposeOverlay.SystemUpdateUiState,
    palette: PortalPalette,
    icons: Map<String, ImageBitmap>,
    includedApp: Boolean,
) {
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
        val message = state.message.ifBlank { "Preparing…" }
        // One thing updating needs no checklist: the line under the bar says it.
        if (state.steps.size >= 2) {
            Spacer(Modifier.height(10.dp))
            state.steps.forEach { step ->
                StepRow(
                    step = step,
                    palette = palette,
                    icons = icons,
                    detail = when (step.state) {
                        ComposeOverlay.STEP_ACTIVE -> message
                        ComposeOverlay.STEP_FAILED, ComposeOverlay.STEP_SKIPPED -> step.note
                        else -> ""
                    },
                )
            }
            if (state.steps.none { it.state == ComposeOverlay.STEP_ACTIVE }) {
                Spacer(Modifier.height(6.dp))
                RunningMessage(message, palette)
            }
        } else {
            Spacer(Modifier.height(10.dp))
            RunningMessage(message, palette)
        }
        Spacer(Modifier.height(12.dp))
        Text(
            text = when {
                !includedApp -> "Keep Portal open. Plasma returns when this finishes."
                state.steps.size <= 1 -> "Android asks to install the update, then closes Portal to finish it."
                else -> "Keep Portal open. Android asks to update Portal at the end, then closes it to finish."
            },
            fontSize = 11.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textMuted.copy(alpha = 0.8f),
        )
    }
}

@Composable
private fun RunningMessage(message: String, palette: PortalPalette) {
    Text(
        text = message,
        fontSize = 11.sp,
        fontWeight = FontWeight.Medium,
        fontFamily = FontFamily.Monospace,
        color = palette.textMuted,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
    )
}

/** One part of a run: what it is, where it stands, and the line about it. */
@Composable
private fun StepRow(
    step: ComposeOverlay.RunStep,
    palette: PortalPalette,
    icons: Map<String, ImageBitmap>,
    detail: String,
) {
    val active = step.state == ComposeOverlay.STEP_ACTIVE
    val titleColor by animateColorAsState(
        targetValue = when (step.state) {
            ComposeOverlay.STEP_PENDING -> palette.textMuted
            ComposeOverlay.STEP_ACTIVE -> palette.textPrimary
            else -> palette.textSecondary
        },
        animationSpec = tween(320),
        label = "step title",
    )
    val tileAlpha by animateFloatAsState(
        targetValue = if (step.state == ComposeOverlay.STEP_PENDING) 0.55f else 1f,
        animationSpec = tween(320),
        label = "step icon",
    )
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.graphicsLayer { alpha = tileAlpha }) {
            UpdateIcon(step.kind, palette, icons, StepIconTile)
        }
        Spacer(Modifier.width(IconGap))
        Column(Modifier.weight(1f)) {
            Text(
                text = step.title,
                fontSize = 13.sp,
                fontWeight = FontWeight.Medium,
                color = titleColor,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            AnimatedVisibility(
                visible = detail.isNotBlank(),
                enter = fadeIn(tween(220)) + expandVertically(tween(260, easing = PortalEmphasized)),
                exit = fadeOut(tween(160)) + shrinkVertically(tween(220, easing = PortalEmphasized)),
            ) {
                Text(
                    text = detail,
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Medium,
                    fontFamily = if (active) FontFamily.Monospace else FontFamily.Default,
                    color = palette.textMuted,
                    maxLines = if (active) 1 else 3,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        Spacer(Modifier.width(10.dp))
        StepIndicator(step.state, palette)
    }
}

/** Pending ring, working arc, done check, failed cross, skipped dash. */
@Composable
private fun StepIndicator(state: Int, palette: PortalPalette) {
    val spin = if (state == ComposeOverlay.STEP_ACTIVE) {
        rememberInfiniteTransition(label = "step spinner").animateFloat(
            initialValue = 0f,
            targetValue = 360f,
            animationSpec = infiniteRepeatable(tween(900, easing = LinearEasing)),
            label = "step spinner turn",
        )
    } else {
        null
    }
    AnimatedContent(
        targetState = state,
        transitionSpec = {
            (fadeIn(tween(220, delayMillis = 60)) + scaleIn(tween(320, easing = PortalEmphasized), initialScale = 0.6f))
                .togetherWith(fadeOut(tween(120)))
        },
        label = "step indicator",
    ) { shown ->
        Canvas(Modifier.size(14.dp)) {
            val stroke = Stroke(width = 1.6.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round)
            val w = size.width
            when (shown) {
                ComposeOverlay.STEP_PENDING -> drawCircle(
                    color = palette.textMuted.copy(alpha = 0.45f),
                    radius = w * 0.38f,
                    style = Stroke(width = 1.2.dp.toPx()),
                )
                ComposeOverlay.STEP_ACTIVE -> rotate(spin?.value ?: 0f) {
                    drawCircle(
                        color = palette.accent.copy(alpha = 0.18f),
                        radius = w * 0.4f,
                        style = Stroke(width = stroke.width),
                    )
                    drawArc(
                        color = palette.accent,
                        startAngle = -90f,
                        sweepAngle = 100f,
                        useCenter = false,
                        topLeft = Offset(w * 0.1f, w * 0.1f),
                        size = Size(w * 0.8f, w * 0.8f),
                        style = stroke,
                    )
                }
                ComposeOverlay.STEP_DONE -> drawPath(
                    path = Path().apply {
                        moveTo(w * 0.14f, w * 0.54f)
                        lineTo(w * 0.42f, w * 0.80f)
                        lineTo(w * 0.88f, w * 0.24f)
                    },
                    color = palette.accent,
                    style = stroke,
                )
                ComposeOverlay.STEP_FAILED -> {
                    drawLine(palette.accent, Offset(w * 0.22f, w * 0.22f), Offset(w * 0.78f, w * 0.78f), stroke.width, StrokeCap.Round)
                    drawLine(palette.accent, Offset(w * 0.78f, w * 0.22f), Offset(w * 0.22f, w * 0.78f), stroke.width, StrokeCap.Round)
                }
                else -> drawLine(
                    palette.textMuted,
                    Offset(w * 0.24f, w * 0.5f),
                    Offset(w * 0.76f, w * 0.5f),
                    stroke.width,
                    StrokeCap.Round,
                )
            }
        }
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
    icons: Map<String, ImageBitmap>,
    lowBattery: Boolean,
    onDismiss: () -> Unit,
    onRetry: () -> Unit,
) {
    // With the run's steps known, each says how it went; otherwise (Plasma
    // would not stop) the error stands on its own.
    val steps = state.steps.takeIf { list -> list.any { it.state != ComposeOverlay.STEP_PENDING } }
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
            text = "Plasma is back. Whatever finished stays installed; the rest was left as it was.",
            fontSize = 12.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textMuted,
        )
        if (steps != null) {
            Spacer(Modifier.height(10.dp))
            steps.forEach { step ->
                StepRow(
                    step = step,
                    palette = palette,
                    icons = icons,
                    detail = when (step.state) {
                        ComposeOverlay.STEP_PENDING -> "Not started"
                        ComposeOverlay.STEP_DONE -> "Installed"
                        else -> step.note
                    },
                )
            }
        } else if (state.message.isNotBlank()) {
            Spacer(Modifier.height(10.dp))
            Text(
                text = state.message,
                fontSize = 11.sp,
                fontFamily = FontFamily.Monospace,
                color = palette.textMuted.copy(alpha = 0.85f),
                maxLines = 4,
                overflow = TextOverflow.Ellipsis,
                textAlign = TextAlign.Start,
            )
        }
        if (lowBattery) {
            Spacer(Modifier.height(8.dp))
            LowBatteryNote(palette)
        }
        Spacer(Modifier.height(14.dp))
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.End,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            PanelButton(text = "Dismiss", primary = false, palette = palette, onClick = onDismiss)
            Spacer(Modifier.width(6.dp))
            PanelButton(
                text = "Try again",
                primary = true,
                enabled = !lowBattery,
                palette = palette,
                onClick = onRetry,
            )
        }
    }
}
