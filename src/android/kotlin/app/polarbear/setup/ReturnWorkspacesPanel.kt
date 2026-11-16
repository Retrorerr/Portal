package app.polarbear.setup

// Workspaces on the Return and session veils. A capsule naming the running
// workspace grows, like Apps and Files, into a card of every workspace shown
// as it was last left: its own desktop, frozen at that moment. Tapping one
// opens it. Portal stays open; only the guest restarts behind the veil,
// which shows the chosen workspace's last frame until its desktop is live and
// then lifts by itself. Names are edited in place, and the card says which
// workspace Portal opens at launch. Native state (Workspaces.state) is the
// only source of truth.

import android.text.format.DateUtils
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
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
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.coerceIn
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.ComposeOverlay
import app.polarbear.Workspaces
import app.polarbear.setup.components.LocalPortalVeil
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import app.polarbear.setup.components.findDesktopSurface
import kotlinx.coroutines.delay

/** Long enough to see the chosen card answer the tap before the card folds. */
private const val SWITCH_FOLD_DELAY_MS = 160L
/** Also the widest the veil's pill row lets any of its cards grow. */
internal val WorkspacesCardWidth = 560.dp
/** Tiles share the card's width, within these bounds; more than fit scroll. */
private val TileMinWidth = 156.dp
private val TileMaxWidth = 248.dp
private val TileGap = 12.dp
/** Name and status, or the rename field and its note: the same height, so renaming never moves the card. */
private val TileFooterHeight = 46.dp
/** Over the frozen old desktop, so the chosen one is what is seen. */
private const val BACKDROP_ALPHA = 0.88f
/** Over the preview, enough for white text on a bright wallpaper. */
private const val BACKDROP_SHADE = 0.38f
/** Long names end early so the pill stays beside Apps and Files. */
private val CapsuleNameMaxWidth = 132.dp
private val TileCorner = 14.dp

/**
 * The Workspaces pill in the veil's row beside Apps and Files, opening into
 * every workspace as it was left; one tap opens another.
 */
@Composable
internal fun WorkspacesSurface(palette: PortalPalette) {
    val state by Workspaces.state
    val panel = ComposeOverlay.activePanel
    SessionActionSurface(panel == "workspaces", palette, "workspaces surface") { expanded ->
        if (expanded) {
            WorkspacesCard(state, palette) { ComposeOverlay.activePanel = null }
        } else {
            WorkspacesCapsule(state, palette) {
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
private fun WorkspacesCapsule(state: Workspaces.State, palette: PortalPalette, onClick: () -> Unit) {
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
                modifier = Modifier.widthIn(max = CapsuleNameMaxWidth),
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
private fun StackGlyph(palette: PortalPalette, busy: Boolean) {
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
    val view = LocalView.current
    val veil = LocalPortalVeil.current
    val exclusionKey = remember { Any() }
    var renaming by remember { mutableStateOf<String?>(null) }
    var chosen by remember { mutableStateOf<String?>(null) }
    DisposableEffect(veil) {
        onDispose {
            veil.gestureExclusions.remove(exclusionKey)
            veil.removePanel(exclusionKey)
        }
    }
    // The running workspace's card shows it as it is right now.
    LaunchedEffect(Unit) {
        Workspaces.refresh()
        Workspaces.capture(state.active, findDesktopSurface(view.rootView))
    }
    LaunchedEffect(chosen) {
        val id = chosen ?: return@LaunchedEffect
        delay(SWITCH_FOLD_DELAY_MS)
        Workspaces.switchTo(id, findDesktopSurface(view.rootView))
        onClose()
    }
    Column(
        modifier = Modifier
            .widthIn(max = WorkspacesCardWidth)
            .fillMaxWidth()
            .onGloballyPositioned {
                val bounds = it.boundsInRoot()
                veil.gestureExclusions[exclusionKey] = bounds
                if (ComposeOverlay.activePanel == "workspaces") veil.placePanel(exclusionKey, "workspaces", bounds)
            }
            .padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 14.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f).padding(start = 4.dp)) {
                Text("Workspaces", fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = palette.textPrimary)
                Text(
                    "Each one keeps its own desktop. Tap one to open it; tap a name to rename it.",
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Medium,
                    color = palette.textMuted,
                )
            }
            PanelButton(text = "Done", primary = false, palette = palette, onClick = onClose)
        }
        Spacer(Modifier.height(14.dp))
        BoxWithConstraints(Modifier.fillMaxWidth().padding(horizontal = 4.dp)) {
            val count = state.workspaces.size.coerceAtLeast(1)
            val tileWidth = ((maxWidth - TileGap * (count - 1)) / count).coerceIn(TileMinWidth, TileMaxWidth)
            Row(
                // At least the card's width, so a few tiles sit centred and
                // more than fit scroll.
                modifier = Modifier.horizontalScroll(rememberScrollState()).widthIn(min = maxWidth),
                horizontalArrangement = Arrangement.spacedBy(TileGap, Alignment.CenterHorizontally),
            ) {
            state.workspaces.forEach { workspace ->
                WorkspaceTile(
                    workspace = workspace,
                    width = tileWidth,
                    preview = Workspaces.previews[workspace.id],
                    running = workspace.id == state.active,
                    chosen = chosen == workspace.id,
                    renaming = renaming == workspace.id,
                    renamingAny = renaming != null,
                    palette = palette,
                    onOpen = {
                        when {
                            // A tap away from the field ends the rename, unsaved.
                            renaming != null -> renaming = null
                            chosen == null && workspace.id != state.active -> chosen = workspace.id
                        }
                    },
                    onRename = { renaming = workspace.id },
                    onRenameDone = { renaming = null },
                )
            }
            }
        }
        AnimatedVisibility(
            visible = state.error != null,
            enter = expandVertically(tween(300)) + fadeIn(tween(300, delayMillis = 60)),
            exit = shrinkVertically(tween(260)) + fadeOut(tween(140)),
        ) {
            Text(
                text = state.error ?: "",
                modifier = Modifier.padding(start = 4.dp, top = 10.dp),
                fontSize = 12.sp,
                fontWeight = FontWeight.Medium,
                color = palette.accent,
            )
        }
        Spacer(Modifier.height(14.dp))
        OpenAtLaunchRow(state, palette)
    }
}

@Composable
private fun WorkspaceTile(
    workspace: Workspaces.Workspace,
    width: androidx.compose.ui.unit.Dp,
    preview: ImageBitmap?,
    running: Boolean,
    chosen: Boolean,
    renaming: Boolean,
    /** Any tile is being renamed: a tap on this one, even the running one, ends it. */
    renamingAny: Boolean,
    palette: PortalPalette,
    onOpen: () -> Unit,
    onRename: () -> Unit,
    onRenameDone: () -> Unit,
) {
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val scale by animateFloatAsState(
        targetValue = when {
            chosen -> 1.03f
            pressed && !running -> 0.97f
            else -> 1f
        },
        animationSpec = tween(if (chosen) 220 else 110, easing = PortalEmphasized),
        label = "workspace tile press",
    )
    val ring by animateColorAsState(
        if (running || chosen) palette.accent else palette.surfaceBorder,
        tween(220),
        label = "workspace tile ring",
    )
    val shape = RoundedCornerShape(TileCorner)
    Column(
        Modifier
            .width(width)
            .semantics {
                contentDescription = if (running) "${workspace.name}, open now" else "Open ${workspace.name}"
                selected = running
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
                .border(if (running || chosen) 2.dp else 1.dp, ring, shape)
                .clickable(
                    interactionSource = tap,
                    indication = null,
                    role = Role.Button,
                    enabled = !running || renamingAny,
                    onClickLabel = "Open ${workspace.name}",
                    onClick = onOpen,
                ),
            contentAlignment = Alignment.Center,
        ) {
            if (preview != null) {
                Image(
                    bitmap = preview,
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
                    Box(Modifier.size(6.dp).clip(RoundedCornerShape(50)).background(palette.accent))
                    Spacer(Modifier.width(5.dp))
                    Text("Open", fontSize = 10.sp, fontWeight = FontWeight.SemiBold, color = Color.White)
                }
            }
        }
        Spacer(Modifier.height(8.dp))
        Column(Modifier.height(TileFooterHeight)) {
        if (renaming) {
            RenameField(workspace, palette, onRenameDone)
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
                    chosen -> "Opening…"
                    running -> "Open now"
                    workspace.lastUsedUnix <= 0 -> "Not opened yet"
                    else -> "Used " + DateUtils.getRelativeTimeSpanString(
                        workspace.lastUsedUnix * 1000,
                        System.currentTimeMillis(),
                        DateUtils.MINUTE_IN_MILLIS,
                    ).toString().replaceFirstChar { it.lowercase() }
                },
                modifier = Modifier.padding(start = 2.dp),
                fontSize = 11.sp,
                color = if (chosen) palette.accent else palette.textSecondary,
                maxLines = 1,
            )
        }
        }
    }
}

@Composable
private fun RenameField(workspace: Workspaces.Workspace, palette: PortalPalette, onDone: () -> Unit) {
    // The whole name selected, so typing replaces it.
    var value by remember {
        mutableStateOf(TextFieldValue(workspace.name, selection = TextRange(0, workspace.name.length)))
    }
    var refusal by remember { mutableStateOf<String?>(null) }
    var focused by remember { mutableStateOf(false) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    val save = {
        if (value.text.trim() == workspace.name) {
            onDone()
        } else {
            refusal = Workspaces.rename(workspace.id, value.text)
            if (refusal == null) onDone()
        }
    }
    Column {
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
            text = refusal ?: "Enter saves",
            modifier = Modifier.padding(start = 2.dp, top = 3.dp),
            fontSize = 11.sp,
            color = if (refusal != null) palette.accent else palette.textMuted,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** Which workspace Portal opens at launch: the last one used, or a fixed one. */
@Composable
private fun OpenAtLaunchRow(state: Workspaces.State, palette: PortalPalette) {
    Column(Modifier.padding(horizontal = 4.dp)) {
        Text("When Portal starts", fontSize = 11.sp, fontWeight = FontWeight.Medium, color = palette.textMuted)
        Spacer(Modifier.height(6.dp))
        Row(
            Modifier.horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            LaunchChoice("Open the last one used", state.openAtLaunch == null, palette) {
                Workspaces.setOpenAtLaunch(null)
            }
            state.workspaces.forEach { workspace ->
                LaunchChoice(workspace.name, state.openAtLaunch == workspace.id, palette) {
                    Workspaces.setOpenAtLaunch(workspace.id)
                }
            }
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
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
            color = if (selected) PortalColors.Charcoal else palette.textMuted,
            maxLines = 1,
        )
    }
}
