package app.polarbear.setup

// A new workspace, from the Workspaces card. The card turns to this page: what
// a workspace is, a name for it (typed, or one of a few ideas), and what
// making it takes. Create makes it and Portal restarts into its setup, the
// same steps as the first one; the page stays to say so.

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import app.polarbear.setup.components.PortalPanelSizeSpring
import app.polarbear.setup.components.rememberStorageCapacity

/** Names a tap away; any already in use are left out. */
private val NAME_IDEAS = listOf("Gaming", "Work", "School", "Sandbox")

/** The longest name the native side takes (workspaces.rs, MAX_NAME_CHARS). */
private const val MAX_NAME_CHARS = 40

/** What setting up a workspace downloads and keeps, in GB. */
private const val SETUP_GB = MINIMAL_INSTALLED_MB / 1000f

/**
 * The new-workspace page. [names] are the workspaces there are; [created]
 * is the name of the one just made, while Portal restarts into it.
 * [onCreate] makes the workspace: null once Portal is restarting, else why not.
 */
@Composable
internal fun NewWorkspaceSheet(
    names: List<String>,
    created: String?,
    palette: PortalPalette,
    onBack: () -> Unit,
    onCreate: (String) -> String?,
) {
    var value by remember { mutableStateOf(TextFieldValue()) }
    var refusal by remember { mutableStateOf<String?>(null) }
    // As the native side will store it: trimmed, inner spaces folded.
    val name = value.text.split(Regex("\\s+")).filter { it.isNotEmpty() }.joinToString(" ")
    val problem = when {
        name.length > MAX_NAME_CHARS -> "Keep the name to $MAX_NAME_CHARS characters."
        names.any { it.equals(name, ignoreCase = true) } -> "There’s already a workspace called $name."
        else -> null
    }
    val ready = name.isNotEmpty() && problem == null && created == null
    val create = { if (ready) refusal = onCreate(name) }
    val capacity = rememberStorageCapacity()
    val freeGb = capacity?.availableBytes?.let { it / 1_000_000_000f }

    Column(Modifier.fillMaxWidth()) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            SheetBack(palette = palette, enabled = created == null, onClick = onBack)
            Text(
                text = "New workspace",
                modifier = Modifier.padding(start = 4.dp),
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
                color = palette.textPrimary,
            )
        }
        Text(
            text = "Its own Debian, with its own apps, files and settings, kept apart from the rest.",
            modifier = Modifier.padding(start = 4.dp, top = 4.dp),
            fontSize = 12.sp,
            lineHeight = 17.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textMuted,
        )
        Spacer(Modifier.height(16.dp))
        // Settled once the workspace is made.
        val settled by animateFloatAsState(if (created == null) 1f else 0.5f, tween(320), label = "name settled")
        Column(Modifier.padding(horizontal = 4.dp).graphicsLayer { alpha = settled }) {
            NameInput(
                value = value,
                onValueChange = {
                    if (it.text != value.text) refusal = null
                    value = it
                },
                enabled = created == null,
                palette = palette,
                onDone = create,
            )
            val ideas = NAME_IDEAS.filter { idea -> names.none { it.equals(idea, ignoreCase = true) } }
            if (ideas.isNotEmpty()) {
                Row(
                    modifier = Modifier
                        .padding(top = 10.dp)
                        .horizontalScroll(rememberScrollState()),
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    ideas.forEach { idea ->
                        IdeaChip(
                            label = idea,
                            selected = name.equals(idea, ignoreCase = true),
                            enabled = created == null,
                            palette = palette,
                        ) {
                            value = TextFieldValue(idea, selection = TextRange(idea.length))
                            refusal = null
                        }
                    }
                }
            }
        }
        Spacer(Modifier.height(16.dp))
        AnimatedContent(
            targetState = created,
            transitionSpec = {
                fadeIn(tween(320, delayMillis = 120, easing = PortalEmphasizedDecelerate))
                    .togetherWith(fadeOut(tween(160, easing = PortalEmphasizedAccelerate)))
                    .using(SizeTransform(clip = false) { _, _ -> PortalPanelSizeSpring })
            },
            label = "new workspace footer",
        ) { made ->
            if (made == null) {
                Column(Modifier.fillMaxWidth()) {
                    // What it takes, and whether there is room for it.
                    val short = freeGb != null && freeGb < SETUP_GB
                    Text(
                        text = when {
                            short -> "It needs %.1f GB of space, and %.1f GB is free.".format(SETUP_GB, freeGb)
                            freeGb != null -> "Needs %.1f GB of space (%.0f GB free) and about a 1 GB download.".format(SETUP_GB, freeGb)
                            else -> "Needs %.1f GB of space and about a 1 GB download.".format(SETUP_GB)
                        },
                        modifier = Modifier.padding(start = 4.dp),
                        fontSize = 11.sp,
                        lineHeight = 15.sp,
                        fontWeight = FontWeight.Medium,
                        color = if (short) palette.accent else palette.textSecondary,
                    )
                    Text(
                        text = "Portal restarts to set it up. Your other workspaces stay just as they are.",
                        modifier = Modifier.padding(start = 4.dp, top = 2.dp),
                        fontSize = 11.sp,
                        lineHeight = 15.sp,
                        color = palette.textMuted,
                    )
                    val message = problem ?: refusal
                    AnimatedVisibility(
                        visible = message != null,
                        enter = fadeIn(tween(240, delayMillis = 60)) + expandVertically(tween(320, easing = PortalEmphasized)),
                        exit = fadeOut(tween(140)) + shrinkVertically(tween(280, easing = PortalEmphasized)),
                    ) {
                        // Keeps its last words while it folds away.
                        var shown by remember { mutableStateOf("") }
                        if (message != null) shown = message
                        Text(
                            text = shown,
                            modifier = Modifier.padding(start = 4.dp, top = 10.dp),
                            fontSize = 12.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.accent,
                        )
                    }
                    Row(
                        modifier = Modifier.fillMaxWidth().padding(top = 16.dp, end = 4.dp),
                        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        PanelButton(text = "Cancel", primary = false, palette = palette, onClick = onBack)
                        PanelButton(text = "Create workspace", primary = true, palette = palette, enabled = ready, onClick = create)
                    }
                }
            } else {
                Row(
                    modifier = Modifier.fillMaxWidth().padding(start = 4.dp, top = 2.dp, bottom = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    StackGlyph(palette, busy = true)
                    Column(Modifier.padding(start = 12.dp)) {
                        Text(
                            text = "Restarting Portal…",
                            fontSize = 13.sp,
                            fontWeight = FontWeight.SemiBold,
                            color = palette.textPrimary,
                        )
                        Text(
                            text = "“$made” opens in setup next: the same few steps as the first time.",
                            fontSize = 11.sp,
                            lineHeight = 15.sp,
                            color = palette.textMuted,
                        )
                    }
                }
            }
        }
    }
}

/**
 * The name: one line, words capitalised, Enter makes the workspace. It asks
 * to stay in view while the keyboard opens.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun NameInput(
    value: TextFieldValue,
    onValueChange: (TextFieldValue) -> Unit,
    enabled: Boolean,
    palette: PortalPalette,
    onDone: () -> Unit,
) {
    var focused by remember { mutableStateOf(false) }
    val seen = remember { BringIntoViewRequester() }
    val keyboard = WindowInsets.ime.getBottom(LocalDensity.current)
    LaunchedEffect(keyboard, focused) { if (keyboard > 0 && focused) seen.bringIntoView() }
    val ring by animateColorAsState(
        if (focused) palette.accent.copy(alpha = 0.75f) else palette.surfaceBorder,
        tween(200),
        label = "name ring",
    )
    val shape = RoundedCornerShape(12.dp)
    val style = TextStyle(color = palette.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
    BasicTextField(
        value = value,
        onValueChange = onValueChange,
        enabled = enabled,
        modifier = Modifier
            .fillMaxWidth()
            .bringIntoViewRequester(seen)
            .onFocusChanged { focused = it.isFocused },
        singleLine = true,
        textStyle = style,
        cursorBrush = SolidColor(palette.accent),
        keyboardOptions = KeyboardOptions(
            capitalization = KeyboardCapitalization.Words,
            autoCorrectEnabled = false,
            imeAction = ImeAction.Done,
        ),
        keyboardActions = KeyboardActions(onDone = { onDone() }),
        decorationBox = { field ->
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .height(46.dp)
                    .clip(shape)
                    .background(palette.textPrimary.copy(alpha = 0.05f))
                    .border(1.dp, ring, shape)
                    .padding(horizontal = 14.dp),
                contentAlignment = Alignment.CenterStart,
            ) {
                if (value.text.isEmpty()) {
                    Text(
                        text = "Name it",
                        style = style.copy(color = palette.textMuted.copy(alpha = 0.7f), fontWeight = FontWeight.Medium),
                        maxLines = 1,
                    )
                }
                field()
            }
        },
    )
}

/** A name to use as it is, or as a start. */
@Composable
private fun IdeaChip(
    label: String,
    selected: Boolean,
    enabled: Boolean,
    palette: PortalPalette,
    onClick: () -> Unit,
) {
    val fill by animateColorAsState(
        if (selected) palette.accent else palette.accent.copy(alpha = 0f),
        tween(200),
        label = "idea fill",
    )
    val shape = RoundedCornerShape(50)
    Box(
        Modifier
            .clip(shape)
            .background(fill)
            .border(1.dp, if (selected) fill else palette.surfaceBorder, shape)
            .clickable(enabled = enabled, role = Role.Button, onClickLabel = "Name it $label", onClick = onClick)
            .semantics { this.selected = selected }
            .padding(horizontal = 12.dp, vertical = 6.dp),
    ) {
        Text(
            text = label,
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
            color = if (selected) PortalColors.Charcoal else palette.textMuted,
            maxLines = 1,
        )
    }
}

/** The way back to the list, a chevron the size of a fingertip. */
@Composable
private fun SheetBack(palette: PortalPalette, enabled: Boolean, onClick: () -> Unit) {
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val press by animateFloatAsState(if (pressed) 1f else 0f, tween(110), label = "sheet back press")
    val shown by animateFloatAsState(if (enabled) 1f else 0.35f, tween(220), label = "sheet back enabled")
    Box(
        modifier = Modifier
            .size(36.dp)
            .clip(RoundedCornerShape(50))
            .background(palette.textPrimary.copy(alpha = 0.07f * press))
            .clickable(
                interactionSource = tap,
                indication = null,
                enabled = enabled,
                role = Role.Button,
                onClickLabel = "Back to workspaces",
                onClick = onClick,
            )
            .graphicsLayer { alpha = shown },
        contentAlignment = Alignment.Center,
    ) {
        Canvas(Modifier.size(18.dp)) {
            val stroke = 1.8.dp.toPx()
            val ink = palette.textSecondary
            drawLine(ink, Offset(size.width * 0.62f, size.height * 0.2f), Offset(size.width * 0.32f, size.height * 0.5f), stroke, StrokeCap.Round)
            drawLine(ink, Offset(size.width * 0.32f, size.height * 0.5f), Offset(size.width * 0.62f, size.height * 0.8f), stroke, StrokeCap.Round)
        }
    }
}

/** "+ New" in the Workspaces card's header, beside Done. */
@Composable
internal fun NewWorkspaceButton(palette: PortalPalette, enabled: Boolean, onClick: () -> Unit) {
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val pressScale by animateFloatAsState(if (pressed) 0.96f else 1f, tween(110), label = "new workspace press")
    val opacity by animateFloatAsState(if (enabled) 1f else 0.4f, tween(220, easing = PortalEmphasized), label = "new workspace enabled")
    val shape = RoundedCornerShape(50)
    Row(
        modifier = Modifier
            .graphicsLayer {
                scaleX = pressScale
                scaleY = pressScale
                alpha = opacity
            }
            .clip(shape)
            .border(1.dp, palette.surfaceBorder, shape)
            .clickable(
                interactionSource = tap,
                indication = null,
                enabled = enabled,
                role = Role.Button,
                onClickLabel = "Make a new workspace",
                onClick = onClick,
            )
            .padding(start = 11.dp, end = 14.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Canvas(Modifier.size(12.dp)) {
            val stroke = 1.8.dp.toPx()
            val inset = stroke / 2
            drawLine(palette.accent, Offset(size.width / 2, inset), Offset(size.width / 2, size.height - inset), stroke, StrokeCap.Round)
            drawLine(palette.accent, Offset(inset, size.height / 2), Offset(size.width - inset, size.height / 2), stroke, StrokeCap.Round)
        }
        Spacer(Modifier.width(7.dp))
        Text(
            text = "New",
            fontSize = 13.sp,
            fontWeight = FontWeight.SemiBold,
            color = palette.textPrimary,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}
