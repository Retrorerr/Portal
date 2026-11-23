package app.polarbear.setup

// Workspaces on the Return and session veils. A capsule naming the running
// workspace grows, like Apps and Files, into a card of every workspace shown
// as it was last left: its own desktop, frozen at that moment. Tapping one
// opens it. Portal stays open; only the guest restarts behind the veil,
// which shows the chosen workspace's last frame until its desktop is live and
// then lifts by itself. Names are edited in place, and the card says which
// workspace Portal opens at launch. Native state (Workspaces.state) is the
// only source of truth.
//
// Holding a tile selects it for deletion: a strip under the tiles names it
// and says what goes, and Delete there removes it, Main and the open one
// included. A deleted tile folds away and the rest close up; deleting the
// open one stops it and restarts Portal into its setup.
//
// New, beside Done, turns the card to a page for a new workspace: a separate
// Debian with its own apps, files and settings (NewWorkspaceSheet.kt). Portal
// restarts into the first-run setup for it, as it does for a workspace whose
// install never finished.

import android.text.format.DateUtils
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.MutableTransitionState
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.LinearOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleOut
import androidx.compose.animation.shrinkHorizontally
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathMeasure
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalViewConfiguration
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.coerceAtLeast
import androidx.compose.ui.unit.coerceIn
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.ComposeOverlay
import app.polarbear.PortalActivity
import app.polarbear.Workspaces
import app.polarbear.Workspaces.findActivity
import app.polarbear.setup.components.LocalPortalVeil
import app.polarbear.setup.components.PortalPanelSizeSpring
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import kotlinx.coroutines.delay
import kotlin.math.roundToInt

/** Long enough to see the chosen card answer the tap before the card folds. */
private const val SWITCH_FOLD_DELAY_MS = 160L
/** Also the widest the veil's pill row lets any of its cards grow. */
internal val WorkspacesCardWidth = 560.dp
/** Tiles share the card's width, within these bounds; more than fit scroll. */
private val TileMinWidth = 128.dp
private val TileMaxWidth = 248.dp
/** Where the card is short (a phone on its side), tiles shrink to fit its height, down to this. */
private val TileMinHeight = 64.dp
private val TileGap = 12.dp
/** Below this much height for its contents, the list tightens its spacing and puts the launch choice on one line. */
private val CompactListHeight = 340.dp
/** Name and status, or the rename field and its note: the same height, so renaming never moves the card. */
private val TileFooterHeight = 46.dp
/** Over the frozen old desktop, so the chosen one is what is seen. */
private const val BACKDROP_ALPHA = 0.88f
/** Over the preview, enough for white text on a bright wallpaper. */
private const val BACKDROP_SHADE = 0.38f
/** Long names end early so the pill stays beside Apps and Files; narrower where the row is short. */
internal val WorkspacesCapsuleNameMaxWidth = 132.dp
private val TileCorner = 14.dp
/** A deleted tile folding away, and the others closing up behind it. */
private const val TILE_FOLD_MS = 420
/** After the card's own opening morph, so composing the delete strip never shares its frames. */
private const val STRIP_COMPOSE_DELAY_MS = 700L

/**
 * The Workspaces pill in the veil's row beside Apps and Files, opening into
 * every workspace as it was left; one tap opens another.
 */
@Composable
internal fun WorkspacesSurface(palette: PortalPalette, nameMaxWidth: Dp = WorkspacesCapsuleNameMaxWidth) {
    val state by Workspaces.state
    val panel = ComposeOverlay.activePanel
    SessionActionSurface(panel == "workspaces", palette, "workspaces surface") { expanded ->
        if (expanded) {
            WorkspacesCard(state, palette) { ComposeOverlay.activePanel = null }
        } else {
            WorkspacesCapsule(state, palette, nameMaxWidth) {
                if (ComposeOverlay.activePanel == null && state.switching == null &&
                    Workspaces.opening.value == null
                ) {
                    ComposeOverlay.activePanel = "workspaces"
                }
            }
        }
    }
}

/**
 * Behind the veil's content while a switch runs: the chosen workspace's last
 * frame, softly blurred, so the desktop arriving looks like that one coming
 * into focus rather than a blank wait.
 */
@Composable
internal fun WorkspaceSwitchBackdrop() {
    val state by Workspaces.state
    val opening by Workspaces.opening
    val target = opening ?: state.switching
    // Keep the last target drawn while it fades out.
    var shown by remember { mutableStateOf<ImageBitmap?>(null) }
    val preview = target?.let { Workspaces.previews[it] }
    if (preview != null) shown = preview
    val alpha by animateFloatAsState(
        targetValue = if (preview != null) BACKDROP_ALPHA else 0f,
        animationSpec = tween(if (preview != null) 520 else 360, easing = PortalEmphasized),
        label = "workspace backdrop",
    )
    val bitmap = shown
    if (bitmap != null && alpha > 0.001f) {
        Image(
            bitmap = bitmap,
            contentDescription = null,
            contentScale = ContentScale.Crop,
            modifier = Modifier
                .fillMaxSize()
                .graphicsLayer {
                    this.alpha = alpha
                    // Settles from slightly enlarged, as if stepping back into it.
                    val scale = 1.04f - 0.04f * (alpha / BACKDROP_ALPHA)
                    scaleX = scale
                    scaleY = scale
                }
                .blur(18.dp),
        )
        // A soft shade over a bright wallpaper, so the veil's words stay readable.
        Box(
            Modifier
                .fillMaxSize()
                .graphicsLayer { this.alpha = alpha / BACKDROP_ALPHA }
                .background(Color.Black.copy(alpha = BACKDROP_SHADE)),
        )
    }
}

/**
 * The capsule: two stacked panes and the running workspace's name. While one
 * opens, the veil's subtitle says so and the capsule names the one arriving.
 */
@Composable
private fun WorkspacesCapsule(state: Workspaces.State, palette: PortalPalette, nameMaxWidth: Dp, onClick: () -> Unit) {
    val openingId by Workspaces.opening
    val opening = openingId ?: state.switching
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val pressScale by animateFloatAsState(if (pressed) 0.975f else 1f, tween(110), label = "workspaces capsule press")
    val label = opening?.let { state.name(it) } ?: state.name(state.active) ?: "Workspaces"
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
                onClickLabel = "Switch workspace",
                onClick = onClick,
            )
            .padding(horizontal = 14.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        StackGlyph(palette, busy = opening != null)
        AnimatedContent(
            targetState = label,
            transitionSpec = {
                (fadeIn(tween(300, delayMillis = 60, easing = PortalEmphasizedDecelerate)) +
                    slideInVertically(tween(420, easing = PortalEmphasized)) { it / 2 })
                    .togetherWith(fadeOut(tween(140, easing = PortalEmphasizedAccelerate)))
                    .using(SizeTransform(clip = false))
            },
            label = "workspaces capsule label",
        ) { text ->
            Text(
                text = text,
                modifier = Modifier.widthIn(max = nameMaxWidth),
                fontSize = 13.sp,
                fontWeight = FontWeight.Medium,
                color = palette.textPrimary,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (opening == null) Text(text = "›", fontSize = 16.sp, color = palette.textMuted)
    }
}

/** Two offset panes; they breathe while a workspace opens. */
@Composable
internal fun StackGlyph(palette: PortalPalette, busy: Boolean) {
    val breathe = rememberInfiniteTransition(label = "workspace glyph")
    val phase by breathe.animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(900, easing = LinearEasing), RepeatMode.Reverse),
        label = "workspace glyph phase",
    )
    val back = palette.textMuted.copy(alpha = 0.55f)
    val front = if (busy) palette.accent else palette.textPrimary.copy(alpha = 0.85f)
    Canvas(Modifier.size(16.dp)) {
        val stroke = 1.4.dp.toPx()
        val pane = Size(size.width * 0.72f, size.height * 0.58f)
        val shift = if (busy) size.width * 0.06f * phase else 0f
        val radius = CornerRadius(2.5.dp.toPx())
        drawRoundRect(
            color = back,
            topLeft = Offset(size.width - pane.width - stroke / 2 - shift, stroke / 2),
            size = pane,
            cornerRadius = radius,
            style = Stroke(stroke),
        )
        drawRoundRect(
            color = front,
            topLeft = Offset(stroke / 2 + shift, size.height - pane.height - stroke / 2),
            size = pane,
            cornerRadius = radius,
        )
    }
}

@Composable
private fun WorkspacesCard(state: Workspaces.State, palette: PortalPalette, onClose: () -> Unit) {
    val context = LocalContext.current
    val host = context.findActivity() as? PortalActivity
    val veil = LocalPortalVeil.current
    val exclusionKey = remember { Any() }
    var renaming by remember { mutableStateOf<String?>(null) }
    var chosen by remember { mutableStateOf<String?>(null) }
    /** The card shows the new-workspace page. */
    var naming by remember { mutableStateOf(false) }
    /** A new workspace was made; Portal is restarting into its installer. */
    var created by remember { mutableStateOf<String?>(null) }
    /** Held to be deleted: the strip under the tiles asks to confirm. */
    var selected by remember { mutableStateOf<String?>(null) }
    /** Confirmed; its files are going. */
    var deleting by remember { mutableStateOf<String?>(null) }
    var deleteRefusal by remember { mutableStateOf<String?>(null) }
    /** The open one was deleted and Portal is switching away: the card folds. */
    var leaving by remember { mutableStateOf(false) }
    val restarting = chosen != null || created != null || deleting != null
    // Android's Back leaves the new-workspace page for the list, like the page's
    // own Back; once Portal is restarting into the new workspace it waits.
    SetupBackHandler(enabled = naming) { if (created == null) naming = false }
    // Back lets go of a held tile before it closes the card.
    SetupBackHandler(enabled = !naming && selected != null && deleting == null) { selected = null }
    // A selection whose workspace has gone (deleted, or given up) ends; a
    // delete stays in hand until its tile has folded away, so neither the
    // tile nor the strip shows how it was before while it goes.
    LaunchedEffect(state.workspaces) {
        if (selected != null && state.workspaces.none { it.id == selected }) selected = null
        if (deleting != null && state.workspaces.none { it.id == deleting }) {
            delay(TILE_FOLD_MS.toLong())
            deleting = null
        }
    }
    LaunchedEffect(selected) { deleteRefusal = null }
    LaunchedEffect(leaving) {
        if (!leaving) return@LaunchedEffect
        delay(SWITCH_FOLD_DELAY_MS)
        onClose()
    }
    DisposableEffect(veil) {
        onDispose {
            veil.gestureExclusions.remove(exclusionKey)
            veil.removePanel(exclusionKey)
        }
    }
    // The running workspace's card shows it as it is right now.
    LaunchedEffect(Unit) {
        Workspaces.refresh()
        Workspaces.capture(Workspaces.state.value.active, host?.desktopSurface())
    }
    LaunchedEffect(chosen) {
        val id = chosen ?: return@LaunchedEffect
        val surface = host?.desktopSurface()
        if (state.workspaces.firstOrNull { it.id == id }?.needsSetup == true) {
            // Its installer runs after a restart; the card stays to say so,
            // or to show why it can't.
            if (Workspaces.setUp(context, id, surface) != null) chosen = null
            return@LaunchedEffect
        }
        // The switch starts at once; only the card's fold waits for the
        // chosen tile to answer the tap.
        if (!Workspaces.switchTo(id, surface)) {
            chosen = null
            return@LaunchedEffect
        }
        delay(SWITCH_FOLD_DELAY_MS)
        onClose()
    }
    Column(
        modifier = Modifier
            .widthIn(max = WorkspacesCardWidth)
            .fillMaxWidth()
            .veilPanelHeight()
            .onGloballyPositioned {
                val bounds = it.boundsInRoot()
                veil.gestureExclusions[exclusionKey] = bounds
                if (ComposeOverlay.activePanel == "workspaces") veil.placePanel(exclusionKey, "workspaces", bounds)
            }
            .padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 14.dp),
    ) {
        // The list, or the page for a new workspace; the card resizes between them.
        AnimatedContent(
            targetState = naming,
            transitionSpec = {
                stageTransition(forward = targetState).using(SizeTransform(clip = false) { _, _ -> PortalPanelSizeSpring })
            },
            contentAlignment = Alignment.TopStart,
            label = "workspaces card page",
        ) { newPage ->
            if (newPage) {
                NewWorkspaceSheet(
                    names = state.workspaces.map { it.name },
                    created = created,
                    palette = palette,
                    onBack = { naming = false },
                    onCreate = { name ->
                        val refusal = Workspaces.create(context, name, host?.desktopSurface())
                        if (refusal == null) created = name.trim()
                        refusal
                    },
                )
            } else {
                WorkspacesList(
                    state = state,
                    palette = palette,
                    chosen = chosen,
                    renaming = renaming,
                    restarting = restarting,
                    onNew = {
                        renaming = null
                        naming = true
                    },
                    onClose = onClose,
                    onChoose = { chosen = it },
                    onRenaming = { renaming = it },
                    selection = WorkspaceSelection(
                        selected = selected,
                        deleting = deleting,
                        refusal = deleteRefusal,
                        onSelect = {
                            if (deleting == null) {
                                if (it != null) renaming = null
                                selected = it
                            }
                        },
                        onDelete = { id ->
                            deleting = id
                            deleteRefusal = null
                            // The open one only answers if refused: Portal restarts into its setup.
                            Workspaces.delete(context, id) { refusal ->
                                if (refusal == null) {
                                    selected = null
                                    if (Workspaces.opening.value != null) leaving = true
                                } else {
                                    deleting = null
                                    deleteRefusal = refusal
                                }
                            }
                        },
                    ),
                )
            }
        }
    }
}

/** Every workspace as it was left, New and Done, and which one opens at launch. */
@Composable
private fun WorkspacesList(
    state: Workspaces.State,
    palette: PortalPalette,
    chosen: String?,
    renaming: String?,
    restarting: Boolean,
    onNew: () -> Unit,
    onClose: () -> Unit,
    onChoose: (String) -> Unit,
    onRenaming: (String?) -> Unit,
    selection: WorkspaceSelection,
) {
    Column(Modifier.fillMaxWidth()) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                "Workspaces",
                modifier = Modifier.weight(1f).padding(start = 4.dp),
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
                color = palette.textPrimary,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            NewWorkspaceButton(palette = palette, enabled = !restarting, onClick = onNew)
            Spacer(Modifier.width(4.dp))
            PanelButton(text = "Done", primary = false, palette = palette, onClick = onClose)
        }
        // Under the header, the rest fits the height the veil gives the card:
        // the tiles shrink first, then it scrolls.
        BoxWithConstraints(Modifier.weight(1f, fill = false).fillMaxWidth()) {
            val budget = maxHeight
            val compact = budget < CompactListHeight
            val scroll = rememberScrollState()
            Column(Modifier.fillMaxWidth().veilScrollRegion(scroll).verticalScroll(scroll)) {
                WorkspacesBody(state, palette, chosen, renaming, restarting, onChoose, onRenaming, selection, budget, compact)
            }
        }
    }
}

/**
 * The description, the tiles, any error and the launch choice. The tiles get
 * whatever height [budget] leaves once the rest is measured.
 */
@Composable
private fun WorkspacesBody(
    state: Workspaces.State,
    palette: PortalPalette,
    chosen: String?,
    renaming: String?,
    restarting: Boolean,
    onChoose: (String) -> Unit,
    onRenaming: (String?) -> Unit,
    selection: WorkspaceSelection,
    budget: Dp,
    compact: Boolean,
) {
    val gap = if (compact) 10.dp else 14.dp
    val (entries, onGone) = rememberTileEntries(state.workspaces)
    // Tiles widen or narrow as one goes, in step with it folding away.
    val tileCount by animateFloatAsState(
        state.workspaces.size.coerceAtLeast(1).toFloat(),
        tween(TILE_FOLD_MS, easing = PortalEmphasized),
        label = "workspace tile count",
    )
    val haptics = LocalHapticFeedback.current
    Layout(content = {
        Text(
            "Each has its own apps, files and settings. Tap one to open it, its name to rename it, or hold it to delete it.",
            modifier = Modifier.padding(start = 4.dp, end = 4.dp, top = 2.dp),
            fontSize = 11.sp,
            lineHeight = 15.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textMuted,
        )
        BoxWithConstraints(Modifier.fillMaxWidth().padding(horizontal = 4.dp)) {
            // Each tile carries half a gap on either side, so one folding
            // away takes its gap with it.
            val byWidth = ((maxWidth - TileGap * tileCount) / tileCount).coerceIn(TileMinWidth, TileMaxWidth)
            // A tile is 16:10 over its name; where height is short, that sets its width.
            val byHeight = if (constraints.hasBoundedHeight) {
                ((maxHeight - 8.dp - TileFooterHeight) * 1.6f).coerceAtLeast(TileMinHeight * 1.6f)
            } else {
                TileMaxWidth
            }
            val tileWidth = minOf(byWidth, byHeight)
            Row(
                // At least the card's width, so a few tiles sit centred and
                // more than fit scroll.
                modifier = Modifier.horizontalScroll(rememberScrollState()).widthIn(min = maxWidth),
                horizontalArrangement = Arrangement.Center,
            ) {
                entries.forEach { entry ->
                    val workspace = entry.workspace
                    key(workspace.id) {
                        val shown = remember { MutableTransitionState(true) }
                        shown.targetState = entry.present
                        if (!entry.present && shown.isIdle && !shown.currentState) {
                            LaunchedEffect(Unit) { onGone(workspace.id) }
                        }
                        AnimatedVisibility(
                            visibleState = shown,
                            exit = shrinkHorizontally(tween(TILE_FOLD_MS, easing = PortalEmphasized)) +
                                scaleOut(tween(TILE_FOLD_MS, easing = PortalEmphasized), targetScale = 0.86f) +
                                fadeOut(tween(TILE_FOLD_MS / 2, easing = PortalEmphasizedAccelerate)),
                        ) {
                            WorkspaceTile(
                                workspace = workspace,
                                width = tileWidth,
                                preview = Workspaces.previews[workspace.id],
                                present = entry.present,
                                running = workspace.id == state.active,
                                chosen = chosen == workspace.id,
                                renaming = renaming == workspace.id,
                                renamingAny = renaming != null,
                                selected = selection.selected == workspace.id,
                                selecting = selection.selected != null,
                                deleting = selection.deleting == workspace.id,
                                palette = palette,
                                modifier = Modifier.padding(horizontal = TileGap / 2),
                                onOpen = {
                                    when {
                                        // A tap away from a field ends it, unsaved.
                                        renaming != null -> onRenaming(null)
                                        // While one is held, a tap moves the hold or lets go.
                                        selection.selected != null -> selection.onSelect(
                                            workspace.id.takeIf { it != selection.selected },
                                        )
                                        !restarting && workspace.id != state.active -> onChoose(workspace.id)
                                    }
                                },
                                onHold = {
                                    if (!restarting && selection.selected != workspace.id) {
                                        haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                        selection.onSelect(workspace.id)
                                    }
                                },
                                onRename = {
                                    if (!restarting) {
                                        selection.onSelect(null)
                                        onRenaming(workspace.id)
                                    }
                                },
                                onRenameDone = { onRenaming(null) },
                            )
                        }
                    }
                }
            }
        }
        // A Column, so the layout keeps a child here while both are gone.
        Column {
            DeleteStrip(state, selection, palette)
            AnimatedVisibility(
                visible = state.error != null,
                enter = expandVertically(tween(300)) + fadeIn(tween(300, delayMillis = 60)),
                exit = shrinkVertically(tween(260)) + fadeOut(tween(140)),
            ) {
                // Keeps its last words while it folds away.
                var shown by remember { mutableStateOf("") }
                state.error?.let { shown = it }
                Text(
                    text = shown,
                    modifier = Modifier.padding(start = 4.dp, end = 4.dp, top = 10.dp),
                    fontSize = 12.sp,
                    fontWeight = FontWeight.Medium,
                    color = palette.accent,
                )
            }
        }
        OpenAtLaunchRow(state, palette, inline = compact)
    }) { measurables, constraints ->
        val loose = Constraints(maxWidth = constraints.maxWidth)
        val description = measurables[0].measure(loose)
        val error = measurables[2].measure(loose)
        val launch = measurables[3].measure(loose)
        val gapPx = gap.roundToPx()
        val others = description.height + error.height + launch.height + gapPx * 2
        val leastTiles = (TileMinHeight + 8.dp + TileFooterHeight).roundToPx()
        val tilesMax = if (budget == Dp.Infinity) Constraints.Infinity
            else (budget.roundToPx() - others).coerceAtLeast(leastTiles)
        val tiles = measurables[1].measure(loose.copy(maxHeight = tilesMax))
        val width = maxOf(description.width, tiles.width, error.width, launch.width).coerceAtMost(constraints.maxWidth)
        layout(width, others + tiles.height) {
            var y = 0
            description.placeRelative(0, y); y += description.height + gapPx
            tiles.placeRelative(0, y); y += tiles.height
            error.placeRelative(0, y); y += error.height + gapPx
            launch.placeRelative(0, y)
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun WorkspaceTile(
    workspace: Workspaces.Workspace,
    width: androidx.compose.ui.unit.Dp,
    preview: ImageBitmap?,
    /** False while a deleted tile folds away: it keeps its looks and takes no taps. */
    present: Boolean,
    running: Boolean,
    chosen: Boolean,
    renaming: Boolean,
    /** Any tile is being renamed: a tap on this one, even the running one, ends it. */
    renamingAny: Boolean,
    /** Held for deletion. */
    selected: Boolean,
    /** Some tile is held; the others step back. */
    selecting: Boolean,
    deleting: Boolean,
    palette: PortalPalette,
    modifier: Modifier = Modifier,
    onOpen: () -> Unit,
    onHold: () -> Unit,
    onRename: () -> Unit,
    onRenameDone: () -> Unit,
) {
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    // A held tile sinks over the long-press time to exactly where a selected
    // one rests, so selection lands without a jump; a tap lets go early.
    val holdMillis = LocalViewConfiguration.current.longPressTimeoutMillis.toInt()
    val scale by animateFloatAsState(
        targetValue = when {
            chosen -> 1.03f
            selected || pressed -> 0.95f
            else -> 1f
        },
        animationSpec = when {
            chosen -> tween(260, easing = PortalEmphasized)
            pressed && !selected -> tween(holdMillis, easing = LinearOutSlowInEasing)
            else -> tween(220, easing = PortalEmphasized)
        },
        label = "workspace tile press",
    )
    // Ring, badge and emphasis are read only while drawing, so their
    // animations redraw the tile without composing it again each frame.
    val ring = animateColorAsState(
        if (running || chosen || selected) palette.accent else palette.surfaceBorder,
        tween(220),
        label = "workspace tile ring",
    )
    val ringWidth = animateFloatAsState(
        if (running || chosen || selected) 2f else 1f,
        tween(220, easing = PortalEmphasized),
        label = "workspace tile ring width",
    )
    val emphasis by animateFloatAsState(
        if (selecting && !selected) 0.5f else 1f,
        tween(260, easing = PortalEmphasized),
        label = "workspace tile emphasis",
    )
    // The badge's check draws in as the tile is held and out as it is let go.
    val held = animateFloatAsState(
        if (selected) 1f else 0f,
        if (selected) tween(320, easing = PortalEmphasizedDecelerate) else tween(200, easing = PortalEmphasizedAccelerate),
        label = "workspace tile held",
    )
    // A tile folding away keeps the frame it had, never its initial.
    var lastPreview by remember { mutableStateOf<ImageBitmap?>(null) }
    if (preview != null) lastPreview = preview
    val shownPreview = preview ?: lastPreview
    val shape = RoundedCornerShape(TileCorner)
    Column(
        modifier
            .width(width)
            .graphicsLayer { alpha = emphasis }
            .semantics {
                contentDescription = when {
                    selected -> "${workspace.name}, selected to delete"
                    running -> "${workspace.name}, open now"
                    else -> "Open ${workspace.name}"
                }
                this.selected = running || selected
            },
    ) {
        Box(
            Modifier
                .fillMaxWidth()
                .aspectRatio(16f / 10f)
                .graphicsLayer {
                    scaleX = scale
                    scaleY = scale
                }
                .clip(shape)
                .background(Brush.verticalGradient(listOf(palette.surfaceTop, palette.surfaceBottom)))
                .drawWithContent {
                    drawContent()
                    val stroke = ringWidth.value.dp.toPx()
                    val corner = TileCorner.toPx() - stroke / 2
                    drawRoundRect(
                        color = ring.value,
                        topLeft = Offset(stroke / 2, stroke / 2),
                        size = Size(size.width - stroke, size.height - stroke),
                        cornerRadius = CornerRadius(corner, corner),
                        style = Stroke(stroke),
                    )
                }
                .combinedClickable(
                    interactionSource = tap,
                    indication = null,
                    role = Role.Button,
                    enabled = present,
                    onClickLabel = if (selecting) "Select ${workspace.name}" else "Open ${workspace.name}",
                    onClick = onOpen,
                    onLongClickLabel = "Select ${workspace.name} to delete",
                    onLongClick = onHold,
                ),
            contentAlignment = Alignment.Center,
        ) {
            if (shownPreview != null) {
                Image(
                    bitmap = shownPreview,
                    contentDescription = null,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.fillMaxSize(),
                )
            } else {
                // Never shown yet: its initial, quietly.
                Text(
                    text = workspace.name.take(1).uppercase(),
                    fontSize = 34.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = palette.textMuted.copy(alpha = 0.6f),
                )
            }
            if (running) {
                Row(
                    Modifier
                        .align(Alignment.TopStart)
                        .padding(8.dp)
                        .clip(RoundedCornerShape(50))
                        .background(Color.Black.copy(alpha = 0.45f))
                        .padding(horizontal = 8.dp, vertical = 3.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("Open", fontSize = 10.sp, fontWeight = FontWeight.SemiBold, color = Color.White)
                }
            }
            HeldBadge({ held.value }, palette, Modifier.align(Alignment.TopEnd).padding(8.dp))
        }
        Spacer(Modifier.height(8.dp))
        Column(Modifier.height(TileFooterHeight)) {
        if (renaming) {
            NameField(
                initial = workspace.name,
                note = "Enter saves",
                palette = palette,
                submit = { name ->
                    if (name.trim() == workspace.name) null else Workspaces.rename(workspace.id, name)
                },
                onDone = onRenameDone,
            )
        } else {
            Text(
                text = workspace.name,
                modifier = Modifier
                    .padding(start = 2.dp)
                    .clickable(role = Role.Button, onClickLabel = "Rename ${workspace.name}", onClick = onRename),
                fontSize = 13.sp,
                fontWeight = FontWeight.SemiBold,
                color = palette.textPrimary,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                text = when {
                    deleting && running -> "Closing…"
                    deleting -> "Deleting…"
                    chosen && workspace.needsSetup -> "Restarting Portal…"
                    chosen -> "Opening…"
                    running -> "Open now"
                    workspace.needsSetup -> "Not set up yet · tap to set up"
                    workspace.lastUsedUnix <= 0 -> "Not opened yet"
                    System.currentTimeMillis() - workspace.lastUsedUnix * 1000 <
                        DateUtils.MINUTE_IN_MILLIS -> "Used just now"
                    else -> "Used " + DateUtils.getRelativeTimeSpanString(
                        workspace.lastUsedUnix * 1000,
                        System.currentTimeMillis(),
                        DateUtils.MINUTE_IN_MILLIS,
                    ).toString().replaceFirstChar { it.lowercase() }
                },
                modifier = Modifier.padding(start = 2.dp),
                fontSize = 11.sp,
                color = if (chosen || deleting) palette.accent else palette.textSecondary,
                maxLines = 1,
            )
        }
        }
    }
}

/**
 * A one-line name field: Enter submits, leaving it cancels. [submit] returns
 * null when it was accepted, else the reason shown under the field.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun NameField(
    initial: String,
    note: String,
    palette: PortalPalette,
    submit: (String) -> String?,
    onDone: () -> Unit,
) {
    // The whole name selected, so typing replaces it.
    var value by remember {
        mutableStateOf(TextFieldValue(initial, selection = TextRange(0, initial.length)))
    }
    var refusal by remember { mutableStateOf<String?>(null) }
    var focused by remember { mutableStateOf(false) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    // The field asks to be seen as the keyboard opens, not only on focus,
    // which comes before the veil has made room for the keyboard.
    val seen = remember { BringIntoViewRequester() }
    val keyboard = WindowInsets.ime.getBottom(LocalDensity.current)
    LaunchedEffect(keyboard) { if (keyboard > 0) seen.bringIntoView() }
    val save = {
        refusal = submit(value.text)
        if (refusal == null) onDone()
    }
    Column(Modifier.bringIntoViewRequester(seen)) {
        BasicTextField(
            value = value,
            onValueChange = {
                if (it.text != value.text) refusal = null
                value = it
            },
            modifier = Modifier
                .fillMaxWidth()
                .focusRequester(focus)
                .onFocusChanged {
                    // Leaving the field cancels; only Enter saves.
                    if (focused && !it.isFocused) onDone()
                    focused = it.isFocused
                }
                .clip(RoundedCornerShape(8.dp))
                .border(1.dp, palette.accent.copy(alpha = 0.7f), RoundedCornerShape(8.dp))
                .padding(horizontal = 8.dp, vertical = 4.dp),
            singleLine = true,
            textStyle = TextStyle(color = palette.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold),
            cursorBrush = SolidColor(palette.accent),
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { save() }),
        )
        Text(
            text = refusal ?: note,
            modifier = Modifier.padding(start = 2.dp, top = 3.dp),
            fontSize = 11.sp,
            color = if (refusal != null) palette.accent else palette.textMuted,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/**
 * Which workspace Portal opens at launch: the last one used, or a fixed one.
 * [inline] puts the label beside the choices, for a short card.
 */
@Composable
private fun OpenAtLaunchRow(state: Workspaces.State, palette: PortalPalette, inline: Boolean = false) {
    val label = @Composable {
        Text(
            "When Portal starts",
            modifier = if (inline) Modifier.padding(end = 10.dp) else Modifier,
            fontSize = 11.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textMuted,
            maxLines = 1,
        )
    }
    val choices = @Composable {
        Row(
            Modifier.horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            LaunchChoice("Open the last one used", state.openAtLaunch == null, palette) {
                Workspaces.setOpenAtLaunch(null)
            }
            // A workspace still being set up isn't offered: Portal would
            // start into its installer every time.
            state.workspaces.filter { !it.needsSetup || state.openAtLaunch == it.id }.forEach { workspace ->
                LaunchChoice(workspace.name, state.openAtLaunch == workspace.id, palette) {
                    Workspaces.setOpenAtLaunch(workspace.id)
                }
            }
        }
    }
    if (inline) {
        Row(Modifier.padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            label()
            choices()
        }
    } else {
        Column(Modifier.padding(horizontal = 4.dp)) {
            label()
            Spacer(Modifier.height(6.dp))
            choices()
        }
    }
}

@Composable
private fun LaunchChoice(label: String, selected: Boolean, palette: PortalPalette, onClick: () -> Unit) {
    val fill by animateColorAsState(
        if (selected) palette.accent else palette.accent.copy(alpha = 0f),
        tween(200),
        label = "launch choice fill",
    )
    val shape = RoundedCornerShape(50)
    Box(
        Modifier
            .clip(shape)
            .background(fill)
            .border(1.dp, if (selected) fill else palette.surfaceBorder, shape)
            .clickable(role = Role.RadioButton, onClick = onClick)
            .semantics { this.selected = selected }
            .padding(horizontal = 12.dp, vertical = 6.dp),
    ) {
        Text(
            label,
            modifier = Modifier.widthIn(max = 180.dp),
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
            color = if (selected) PortalColors.Charcoal else palette.textMuted,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** The held tile and what happens to it. */
private class WorkspaceSelection(
    val selected: String?,
    val deleting: String?,
    /** Why the last delete was refused, shown in the strip. */
    val refusal: String?,
    val onSelect: (String?) -> Unit,
    val onDelete: (String) -> Unit,
)

private class TileEntry(val workspace: Workspaces.Workspace, val present: Boolean)

/**
 * The tiles to draw: every workspace, plus any that just went, still in its
 * place, until its fold reports back through the returned callback.
 */
@Composable
private fun rememberTileEntries(workspaces: List<Workspaces.Workspace>): Pair<List<TileEntry>, (String) -> Unit> {
    // The tiles last drawn, in order. Plain, so composition keeps it current.
    val drawn = remember { ArrayList<Workspaces.Workspace>() }
    var folded by remember { mutableIntStateOf(0) }
    val entries = remember(workspaces, folded) {
        val ids = workspaces.mapTo(HashSet()) { it.id }
        val merged = workspaces.mapTo(ArrayList()) { TileEntry(it, present = true) }
        drawn.forEachIndexed { index, old ->
            if (old.id in ids) return@forEachIndexed
            // Back where it was: after the tile that came before it.
            val before = drawn.subList(0, index).lastOrNull { prev -> merged.any { it.workspace.id == prev.id } }
            val at = before?.let { prev -> merged.indexOfFirst { it.workspace.id == prev.id } + 1 } ?: 0
            merged.add(at, TileEntry(old, present = false))
        }
        drawn.clear()
        merged.mapTo(drawn) { it.workspace }
        merged
    }
    val gone: (String) -> Unit = remember {
        { id ->
            if (drawn.removeAll { it.id == id }) folded++
        }
    }
    return entries to gone
}

/** A round check on a held tile, drawn in by [progress]. */
@Composable
private fun HeldBadge(progress: () -> Float, palette: PortalPalette, modifier: Modifier) {
    Canvas(
        modifier
            .size(22.dp)
            .graphicsLayer {
                val shown = progress()
                val scale = 0.6f + 0.4f * shown
                scaleX = scale
                scaleY = scale
                alpha = shown.coerceIn(0f, 1f)
            },
    ) {
        val progress = progress()
        if (progress <= 0.001f) return@Canvas
        drawCircle(palette.accent)
        drawCircle(PortalColors.Charcoal.copy(alpha = 0.35f), style = Stroke(1.dp.toPx()))
        // The tick draws along its length as the badge arrives.
        val tick = Path().apply {
            moveTo(size.width * 0.29f, size.height * 0.52f)
            lineTo(size.width * 0.44f, size.height * 0.66f)
            lineTo(size.width * 0.72f, size.height * 0.37f)
        }
        val measure = PathMeasure().apply { setPath(tick, false) }
        val drawnTick = Path()
        measure.getSegment(0f, measure.length * progress.coerceIn(0f, 1f), drawnTick, true)
        drawPath(
            drawnTick,
            PortalColors.Charcoal,
            style = Stroke(width = 2.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round),
        )
    }
}

/**
 * Under the tiles while one is held: it names the workspace, says what goes
 * and asks to delete. It keeps the last one it named while folding away.
 */
@Composable
private fun DeleteStrip(state: Workspaces.State, selection: WorkspaceSelection, palette: PortalPalette) {
    val target = selection.selected?.let { id -> state.workspaces.firstOrNull { it.id == id } }
    var shown by remember { mutableStateOf<Workspaces.Workspace?>(null) }
    if (target != null) shown = target
    // Composed once the card has opened and kept composed, so holding a tile
    // only animates the strip in: composing it at that moment cost a visible
    // hitch.
    var ready by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        delay(STRIP_COMPOSE_DELAY_MS)
        ready = true
    }
    val reveal = animateFloatAsState(
        if (target != null) 1f else 0f,
        tween(if (target != null) 360 else 300, easing = PortalEmphasized),
        label = "delete strip reveal",
    )
    val fade = animateFloatAsState(
        if (target != null) 1f else 0f,
        if (target != null) {
            tween(260, delayMillis = 80, easing = PortalEmphasizedDecelerate)
        } else {
            tween(140, easing = PortalEmphasizedAccelerate)
        },
        label = "delete strip fade",
    )
    if (!ready && target == null) return
    // Hidden, the name and detail change without animating, so the strip
    // never arrives mid-transition.
    val hidden = { fade.value < 0.05f }
    // Unfolds from its top edge, clipped, so it never draws over the tiles
    // above; the height is read while laying out, never composing.
    Box(
        Modifier
            .clipToBounds()
            .layout { measurable, constraints ->
                val placeable = measurable.measure(constraints)
                layout(placeable.width, (placeable.height * reveal.value).roundToInt()) {
                    placeable.placeRelative(0, 0)
                }
            }
            .graphicsLayer { alpha = fade.value }
            .then(if (target == null) Modifier.clearAndSetSemantics {} else Modifier),
    ) {
        val workspace = shown ?: state.workspaces.firstOrNull() ?: return@Box
        val open = workspace.id == state.active
        val deleting = selection.deleting == workspace.id
        val next = state.afterDelete?.name
        val detail = when {
            selection.refusal != null -> selection.refusal
            deleting && open && next != null -> "Switching to “$next”…"
            deleting && open -> "Closing it, then Portal starts setup again…"
            deleting -> "Removing its apps, files and settings…"
            open && next != null ->
                "It’s open now: Portal switches to “$next”. Its apps, files and settings are removed for good."
            open -> "It’s your only workspace. Its apps, files and settings are removed for good, and Portal starts setup again."
            else -> "Its apps, files and settings are removed for good."
        }
        val shape = RoundedCornerShape(TileCorner)
        BoxWithConstraints(Modifier.fillMaxWidth().padding(start = 4.dp, end = 4.dp, top = 12.dp)) {
            val stacked = maxWidth < 420.dp
            val words = @Composable { mod: Modifier ->
                Column(mod) {
                    AnimatedContent(
                        targetState = workspace.name,
                        transitionSpec = {
                            if (hidden()) {
                                EnterTransition.None togetherWith ExitTransition.None
                            } else {
                                (fadeIn(tween(260, delayMillis = 60, easing = PortalEmphasizedDecelerate)) +
                                    slideInVertically(tween(360, easing = PortalEmphasized)) { it / 2 })
                                    .togetherWith(fadeOut(tween(120, easing = PortalEmphasizedAccelerate)))
                                    .using(SizeTransform(clip = false))
                            }
                        },
                        label = "delete strip name",
                    ) { name ->
                        Text(
                            "Delete “$name”?",
                            fontSize = 13.sp,
                            fontWeight = FontWeight.SemiBold,
                            color = palette.textPrimary,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                    AnimatedContent(
                        targetState = detail,
                        transitionSpec = {
                            if (hidden()) {
                                EnterTransition.None togetherWith ExitTransition.None
                            } else {
                                fadeIn(tween(220, delayMillis = 60, easing = PortalEmphasizedDecelerate))
                                    .togetherWith(fadeOut(tween(120, easing = PortalEmphasizedAccelerate)))
                                    .using(SizeTransform(clip = false))
                            }
                        },
                        label = "delete strip detail",
                    ) { text ->
                        Text(
                            text,
                            modifier = Modifier.padding(top = 2.dp),
                            fontSize = 11.sp,
                            lineHeight = 15.sp,
                            fontWeight = FontWeight.Medium,
                            color = if (selection.refusal != null) palette.accent else palette.textMuted,
                        )
                    }
                }
            }
            val buttons = @Composable {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    PanelButton(text = "Cancel", primary = false, palette = palette, enabled = !deleting) {
                        selection.onSelect(null)
                    }
                    Spacer(Modifier.width(4.dp))
                    PanelButton(
                        text = if (deleting) "Deleting" else "Delete",
                        primary = true,
                        palette = palette,
                        enabled = !deleting,
                    ) { selection.onDelete(workspace.id) }
                }
            }
            Box(
                Modifier
                    .fillMaxWidth()
                    .clip(shape)
                    .background(Brush.verticalGradient(listOf(palette.surfaceTop, palette.surfaceBottom)))
                    .border(1.dp, palette.accent.copy(alpha = 0.45f), shape)
                    .padding(start = 14.dp, end = 8.dp, top = 10.dp, bottom = 10.dp),
            ) {
                if (stacked) {
                    Column {
                        words(Modifier.padding(end = 6.dp))
                        Spacer(Modifier.height(8.dp))
                        Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) { buttons() }
                    }
                } else {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        words(Modifier.weight(1f).padding(end = 10.dp))
                        buttons()
                    }
                }
            }
        }
    }
}
