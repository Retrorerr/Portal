package app.polarbear.setup

// Optional apps on the Return-to-Plasma screen. A quiet "Apps" capsule sits
// under the update slot; tapping it grows the same surface into the full
// catalog, grouped like the setup picker, where each app can be added or
// removed at any time. Installs run beside the live desktop, so nothing here
// blocks the veil: lifting it leaves the work running, and the next Return
// screen shows where it got to. Native state (ComposeOverlay.optionalAppsState)
// is the only source of truth; the only local state is which card is open
// and which removal is awaiting a second tap.

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandIn
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.shrinkOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInVertically
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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import app.polarbear.setup.components.LocalPortalVeil
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.constrainWidth
import androidx.compose.ui.unit.constrainHeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.ComposeOverlay
import app.polarbear.setup.components.CategoryLabel
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import app.polarbear.setup.components.PortalPanelSizeSpring
import app.polarbear.setup.components.PortalPanelStiffness
import app.polarbear.setup.components.dissolveBlur
import app.polarbear.setup.components.formatInstalledSize
import app.polarbear.setup.components.rememberAppIcons
import kotlinx.coroutines.delay
import kotlin.math.roundToInt

/** After the update capsule, so the two never arrive in the same frame. */
private const val APPS_ENTRANCE_DELAY_MS = 800L
/** A pending removal reverts to "Remove" if not confirmed in time. */
private const val CONFIRM_WINDOW_MS = 3500L
private val AppsCardWidth = 360.dp
private val AppsCapsuleCorner = 18.dp
private val AppsCardCorner = 24.dp

@Composable
internal fun ReturnAppsPanel(
    palette: PortalPalette,
    suppressed: Boolean,
    immediate: Boolean = false,
) {
    val catalog = remember { OptionalAppCatalog.refresh(); OPTIONAL_APPS }
    val states by ComposeOverlay.optionalAppsState()
    val icons by rememberAppIcons(catalog.map { it.id })
    var entered by remember { mutableStateOf(immediate) }
    LaunchedEffect(Unit) { if (!immediate) delay(APPS_ENTRANCE_DELAY_MS); entered = true }
    LaunchedEffect(app.polarbear.FileTransfers.filesRequested) {
        if (app.polarbear.FileTransfers.filesRequested) {
            ComposeOverlay.activePanel = "files"
            app.polarbear.FileTransfers.filesRequested = false
        }
    }
    val panel = ComposeOverlay.activePanel
    LaunchedEffect(panel) { if (panel == "files") app.polarbear.FileTransfers.open() }
    // Workspaces sits in this row once there is a second one to switch to.
    LaunchedEffect(Unit) { app.polarbear.Workspaces.refresh() }
    val workspaces by app.polarbear.Workspaces.state
    val openingId by app.polarbear.Workspaces.opening
    // While a workspace opens it owns the runtime: only its pill stays.
    val switching = openingId != null || workspaces.switching != null
    AnimatedVisibility(
        visible = !suppressed && entered,
        enter = expandVertically(tween(420, easing = FastOutSlowInEasing), expandFrom = Alignment.Top) +
            fadeIn(tween(420, delayMillis = 60, easing = PortalEmphasizedDecelerate)) +
            slideInVertically(tween(560, easing = PortalEmphasized)) { it / 3 } +
            scaleIn(tween(560, easing = PortalEmphasized), initialScale = 0.96f),
        exit = fadeOut(tween(220, easing = PortalEmphasizedAccelerate)) +
            shrinkVertically(tween(420, easing = FastOutSlowInEasing), shrinkTowards = Alignment.Top),
    ) {
        Box(Modifier.dissolveBlur(this, radius = 10.dp).widthIn(max = WorkspacesCardWidth).padding(top = 14.dp)) {
            // The gap between pills closes while one of them is open as a card.
            val gap by animateDpAsState(if (panel == null) 10.dp else 0.dp,
                spring(dampingRatio = Spring.DampingRatioNoBouncy, stiffness = PortalPanelStiffness), label = "controls capsule spacing")
            // The report card takes the capsules' place, and the stack's height
            // has to travel with it or the logo snaps when either one settles.
            val capsuleRow by animateFloatAsState(if (panel == "report") 0f else 1f,
                spring(dampingRatio = Spring.DampingRatioNoBouncy, stiffness = PortalPanelStiffness), label = "controls capsule row")
            androidx.compose.ui.layout.Layout(content = {
                SessionActionSlot(visible = (panel == null || panel == "apps") && !switching && catalog.isNotEmpty()) {
                    SessionActionSurface(panel == "apps", palette, "apps surface") { expanded ->
                        if (expanded) AppsCard(catalog, states, icons, palette) { ComposeOverlay.activePanel = null }
                        else AppsCapsule(palette) {
                            if (ComposeOverlay.activePanel == null) ComposeOverlay.activePanel = "apps"
                        }
                    }
                }
                SessionActionSlot(visible = (panel == null || panel == "files") && !switching) {
                    SessionActionSurface(panel == "files", palette, "files surface") { expanded ->
                        if (expanded) ReturnFilesPanel(palette) { ComposeOverlay.activePanel = null }
                        else FilesCapsule(palette) { if (ComposeOverlay.activePanel == null) ComposeOverlay.activePanel = "files" }
                    }
                }
                SessionActionSlot(visible = (panel == null || panel == "workspaces") && workspaces.workspaces.size > 1) {
                    WorkspacesSurface(palette)
                }
                // Opened only from the pill's menu, so it never competes with the pills.
                SessionActionSlot(visible = panel == "report", fromEdge = false) {
                    SessionActionSurface(true, palette, "report surface") { _ ->
                        ReturnReportPanel(palette) { ComposeOverlay.activePanel = null }
                    }
                }
            }) { children, constraints ->
                // Every pill and card sees the same, final available width
                // throughout a morph. Re-constraining against a retreating
                // peer restarts its size spring every frame and feels sticky.
                val relaxed = constraints.copy(minWidth = 0, minHeight = 0)
                val row = children.subList(0, 3).map { it.measure(relaxed) }
                val report = children[3].measure(relaxed)
                val shown = row.filter { it.width > 0 }
                val spacing = gap.roundToPx()
                val rowWidth = shown.sumOf { it.width } + spacing * (shown.size - 1).coerceAtLeast(0)
                val rowHeight = ((row.maxOfOrNull { it.height } ?: 0) * capsuleRow).roundToInt()
                val width = constraints.constrainWidth(maxOf(rowWidth, report.width))
                layout(width, constraints.constrainHeight(rowHeight + report.height)) {
                    var x = (width - rowWidth) / 2
                    for (pill in row) {
                        pill.placeRelative(x, 0)
                        if (pill.width > 0) x += pill.width + spacing
                    }
                    report.placeRelative((width - report.width) / 2, rowHeight)
                }
            }
        }
    }
}

@Composable
private fun SessionActionSlot(visible: Boolean, fromEdge: Boolean = true, content: @Composable () -> Unit) {
    // Keep one measurable per slot after AnimatedVisibility removes its content.
    Box {
        AnimatedVisibility(visible,
            // A capsule only needs its width animated, its row's height is shared.
            // A slot with no capsule to morph from also owns its height.
            enter = (if (fromEdge) androidx.compose.animation.expandHorizontally(PortalPanelSizeSpring)
                else expandIn(PortalPanelSizeSpring, expandFrom = Alignment.TopCenter)) +
                fadeIn(tween(300, delayMillis = 110, easing = PortalEmphasizedDecelerate)),
            exit = (if (fromEdge) androidx.compose.animation.shrinkHorizontally(PortalPanelSizeSpring)
                else shrinkOut(PortalPanelSizeSpring, shrinkTowards = Alignment.TopCenter)) +
                fadeOut(tween(140, easing = PortalEmphasizedAccelerate))) {
            Box(Modifier.dissolveBlur(this, radius = 8.dp)) { content() }
        }
    }
}

/** The original Apps capsule-to-card transition, shared by Files and Workspaces. */
@Composable
internal fun SessionActionSurface(expanded: Boolean, palette: PortalPalette, label: String,
    content: @Composable androidx.compose.animation.AnimatedContentScope.(Boolean) -> Unit) {
    val corner by animateDpAsState(if (expanded) AppsCardCorner else AppsCapsuleCorner,
        tween(420, easing = PortalEmphasized), label = "$label corner")
    val shape = RoundedCornerShape(corner)
    AnimatedContent(targetState = expanded,
        modifier = Modifier.clip(shape)
            .background(Brush.verticalGradient(listOf(palette.surfaceTop, palette.surfaceBottom)))
            .border(1.dp, palette.surfaceBorder, shape),
        transitionSpec = {
            (fadeIn(tween(300, delayMillis = 110, easing = PortalEmphasizedDecelerate)) +
                scaleIn(tween(420, easing = PortalEmphasized), initialScale = 0.985f))
                .togetherWith(fadeOut(tween(140, easing = PortalEmphasizedAccelerate)))
                .using(SizeTransform(clip = true) { _, _ -> PortalPanelSizeSpring })
        }, contentAlignment = Alignment.TopCenter, label = label, content = content)
}

@Composable
private fun AppsCapsule(palette: PortalPalette, onClick: () -> Unit) {
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val pressScale by animateFloatAsState(if (pressed) 0.975f else 1f, tween(110), label = "apps capsule press")
    // The same shape as Files and Workspaces: a glyph, the name, a chevron.
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
                onClickLabel = "Manage apps",
                onClick = onClick,
            )
            .padding(horizontal = 14.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        app.polarbear.setup.components.PortalIcon(
            app.polarbear.setup.components.PortalGlyph.Apps,
            palette.textPrimary,
            size = 16.dp,
        )
        Text(
            text = "Apps",
            fontSize = 13.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textPrimary,
        )
        Text(text = "›", fontSize = 16.sp, color = palette.textMuted)
    }
}

@Composable
private fun AppsCard(
    catalog: List<EssentialApp>,
    states: Map<String, ComposeOverlay.OptionalAppUiState>,
    icons: Map<String, ImageBitmap>,
    palette: PortalPalette,
    onClose: () -> Unit,
) {
    var confirming by remember { mutableStateOf<String?>(null) }
    // The card scrolls, so swipes that start on it must not lift the veil.
    val veil = LocalPortalVeil.current
    val exclusionKey = remember { Any() }
    DisposableEffect(veil) { onDispose {
        veil.gestureExclusions.remove(exclusionKey)
        veil.removePanel(exclusionKey)
    } }
    LaunchedEffect(confirming) {
        if (confirming != null) {
            delay(CONFIRM_WINDOW_MS)
            confirming = null
        }
    }
    Column(
        modifier = Modifier
            .widthIn(max = AppsCardWidth)
            .fillMaxWidth()
            .onGloballyPositioned {
                val bounds = it.boundsInRoot()
                veil.gestureExclusions[exclusionKey] = bounds
                if (ComposeOverlay.activePanel == "apps") veil.placePanel(exclusionKey,"apps",bounds)
            }
            .padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f).padding(start = 4.dp)) {
                Text("Apps", fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = palette.textPrimary)
                Text(
                    "Add or remove apps any time; the desktop keeps running.",
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Medium,
                    color = palette.textMuted,
                )
            }
            PanelButton(text = "Done", primary = false, palette = palette, onClick = onClose)
        }
        Spacer(Modifier.height(4.dp))
        Column(
            Modifier
                .heightIn(max = (androidx.compose.ui.platform.LocalConfiguration.current.screenHeightDp - 300).coerceIn(100, 420).dp)
                .verticalScroll(rememberScrollState()),
        ) {
            groupedOptionalApps(catalog).forEach { (label, apps) ->
                CategoryLabel(label, palette)
                apps.forEach { app ->
                    ManagedAppRow(
                        app = app,
                        state = states[app.id],
                        icon = icons[app.id],
                        confirming = confirming == app.id,
                        palette = palette,
                        onInstall = {
                            confirming = null
                            ComposeOverlay.requestOptionalApp(app.id, install = true)
                        },
                        onRemove = {
                            if (confirming == app.id) {
                                confirming = null
                                ComposeOverlay.requestOptionalApp(app.id, install = false)
                            } else {
                                confirming = app.id
                            }
                        },
                    )
                }
            }
            Text(
                text = OTHER_APPS_DESKTOP_NOTE,
                modifier = Modifier.padding(start = 14.dp, end = 14.dp, top = 12.dp, bottom = 4.dp),
                fontSize = 11.sp,
                lineHeight = 15.sp,
                color = palette.textMuted,
            )
        }
    }
}

@Composable
private fun ManagedAppRow(
    app: EssentialApp,
    state: ComposeOverlay.OptionalAppUiState?,
    icon: ImageBitmap?,
    confirming: Boolean,
    palette: PortalPalette,
    onInstall: () -> Unit,
    onRemove: () -> Unit,
) {
    val installed = state?.installed == true
    val busy = state?.busy == true
    val failed = state?.failed == true
    val subtitle = when {
        state == null -> app.blurb
        busy && state.state == ComposeOverlay.APP_QUEUED -> "Waiting…"
        busy -> state.message.ifBlank { "Working…" }
        failed -> state.message.ifBlank { "Something went wrong" }
        confirming && app.id == "steam" -> "Removes Steam; your installed games are kept"
        confirming -> "Tap again to remove"
        installed -> "Installed · ${app.blurb}"
        !app.available -> app.unavailableReason ?: "Unavailable"
        else -> "${app.blurb} · ${formatInstalledSize(app.installedMb)}"
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .graphicsLayer { if (!app.available && !installed) alpha = 0.55f }
            .semantics { stateDescription = subtitle }
            .padding(horizontal = 6.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(30.dp), contentAlignment = Alignment.Center) {
            if (icon != null) Image(bitmap = icon, contentDescription = null, modifier = Modifier.size(30.dp))
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(app.name, color = palette.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
            Text(
                subtitle,
                color = if (failed) palette.accent else palette.textSecondary,
                fontSize = 11.sp,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Spacer(Modifier.width(8.dp))
        when {
            busy -> ProgressRing(progress = state!!.progress, palette = palette)
            failed -> AppActionPill("Retry", primary = true, palette = palette, onClick = if (installed) onRemove else onInstall)
            installed -> AppActionPill(if (confirming) "Remove?" else "Remove", primary = confirming, palette = palette, onClick = onRemove)
            app.available -> AppActionPill("Get", primary = true, palette = palette, onClick = onInstall)
            else -> Spacer(Modifier.width(1.dp))
        }
    }
}

@Composable
private fun AppActionPill(text: String, primary: Boolean, palette: PortalPalette, onClick: () -> Unit) {
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val scale by animateFloatAsState(if (pressed) 0.95f else 1f, tween(110), label = "app action press")
    val fill by animateColorAsState(
        if (primary) palette.accent else palette.accent.copy(alpha = 0f),
        tween(200),
        label = "app action fill",
    )
    val shape = RoundedCornerShape(50)
    Box(
        modifier = Modifier
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
            }
            .clip(shape)
            .background(fill)
            .border(1.dp, if (primary) fill else palette.surfaceBorder, shape)
            .clickable(interactionSource = tap, indication = null, role = Role.Button, onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 6.dp),
        contentAlignment = Alignment.Center,
    ) {
        AnimatedContent(text, transitionSpec = {
            fadeIn(tween(260, delayMillis = 70, easing = PortalEmphasizedDecelerate))
                .togetherWith(fadeOut(tween(120, easing = PortalEmphasizedAccelerate)))
                .using(SizeTransform(clip = false) { _, _ -> spring(dampingRatio = 1f, stiffness = 380f) })
        }, label = "app action label") { label ->
            Text(label, fontSize = 12.sp, fontWeight = FontWeight.SemiBold,
                color = if (primary) PortalColors.Charcoal else palette.textMuted)
        }
    }
}

/** Determinate ring for the app's native progress. */
@Composable
private fun ProgressRing(progress: Int, palette: PortalPalette) {
    val shown by animateFloatAsState(progress / 100f, tween(300, easing = PortalEmphasized), label = "app progress")
    Box(Modifier.size(30.dp), contentAlignment = Alignment.Center) {
        Canvas(Modifier.size(24.dp)) {
            val stroke = 2.5.dp.toPx()
            val inset = stroke / 2f
            val arcSize = androidx.compose.ui.geometry.Size(size.width - stroke, size.height - stroke)
            drawArc(
                color = palette.surfaceBorder,
                startAngle = 0f,
                sweepAngle = 360f,
                useCenter = false,
                topLeft = Offset(inset, inset),
                size = arcSize,
                style = Stroke(stroke),
            )
            drawArc(
                color = palette.accent,
                startAngle = -90f,
                sweepAngle = 360f * shown.coerceIn(0.02f, 1f),
                useCenter = false,
                topLeft = Offset(inset, inset),
                size = arcSize,
                style = Stroke(stroke, cap = StrokeCap.Round),
            )
        }
        Text("${progress}", fontSize = 8.sp, fontWeight = FontWeight.SemiBold, color = palette.textMuted)
    }
}
