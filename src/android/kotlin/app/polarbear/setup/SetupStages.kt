package app.polarbear.setup

// The stages of first-run setup, laid out in the card SetupFrame.kt sizes.
// Every stage has the same parts: what it is about, its choices, and its
// action. A split card puts what it is about on the left and the action on
// the right, under whatever the action is about; a stacked card runs the
// stage down the card and keeps the action at the bottom, where a thumb
// finds it.
//
// Three things carry over from stage to stage instead of being replaced: the
// Portal mark, the primary button and the picture of the desktop. Each glides
// from where it was to where the next stage wants it.

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedContentTransitionScope
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.AnimatedVisibilityScope
import androidx.compose.animation.BoundsTransform
import androidx.compose.animation.ContentTransform
import androidx.compose.animation.ExperimentalSharedTransitionApi
import androidx.compose.animation.SharedTransitionScope
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Image
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.wrapContentSize
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.State
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.util.lerp
import app.polarbear.Workspaces
import app.polarbear.setup.components.AppSelectionRow
import app.polarbear.setup.components.CategoryLabel
import app.polarbear.setup.components.DesktopPreview
import app.polarbear.setup.components.INCLUDED_APPS
import app.polarbear.setup.components.InstallStatusPanel
import app.polarbear.setup.components.LocalPortalVeil
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import app.polarbear.setup.components.PortalPanelSizeSpring
import app.polarbear.setup.components.PrepareSkipWarning
import app.polarbear.setup.components.PrepareState
import app.polarbear.setup.components.PrepareSteps
import app.polarbear.setup.components.ProcessRing
import app.polarbear.setup.components.SetupChecklist
import app.polarbear.setup.components.SlidingSegmentedControl
import app.polarbear.setup.components.StorageCapacityBar
import app.polarbear.setup.components.WorkspacePair
import app.polarbear.setup.components.dissolveBlur
import app.polarbear.setup.components.prepareBody
import app.polarbear.setup.components.prepareTitle
import kotlinx.coroutines.delay

/** What a stage is drawn with: the card's metrics, and the transition it is part of. */
@OptIn(ExperimentalSharedTransitionApi::class)
@Stable
internal class StageScope(
    val shared: SharedTransitionScope,
    val visibility: AnimatedVisibilityScope,
    val metrics: SetupMetrics,
    val palette: PortalPalette,
    val launchMarkModifier: Modifier,
)

private const val MARK = "portal-mark"
private const val PRIMARY = "primary-action"
private const val DESKTOP = "desktop-preview"

/** How the mark, the button and the desktop travel between stages. */
@OptIn(ExperimentalSharedTransitionApi::class)
private val StageBounds = BoundsTransform { _, _ -> tween(680, easing = PortalEmphasized) }

/** The fade at a scroller's edge while more waits beyond it. */
private val ScrollFade = 22.dp

/** The widest the desktop picture gets. */
private val PictureMaxWidth = 440.dp

// ---------------------------------------------------------------------------
// The stages.

/**
 * Hello, and what this is. For a new workspace: its name, and what a
 * workspace is, with the way back to the one Portal came from.
 */
@Composable
internal fun StageScope.WelcomeStage(
    workspace: WorkspaceSetup?,
    previewDark: Boolean,
    interfaceSize: InterfaceSize,
    icons: Map<String, ImageBitmap>,
    preview: Boolean,
    onContinue: () -> Unit,
) {
    var leaving by remember { mutableStateOf(false) }
    val centered = !metrics.split
    StageLayout(
        centered = centered,
        fitChoices = true,
        about = {
            StageMark(hero = true)
            Spacer(Modifier.height(if (metrics.short) 14.dp else 22.dp))
            if (workspace != null) {
                WorkspaceChip(palette)
                Spacer(Modifier.height(12.dp))
            }
            StageTitle(
                title = workspace?.name ?: "Welcome to Portal",
                body = if (workspace != null) {
                    "A separate Debian with its own apps, files and settings. " +
                        "Your other workspaces stay just as you left them."
                } else {
                    "A full Linux desktop that runs right on this device: " +
                        "Debian 13 with KDE Plasma, set up in a few steps."
                },
                palette = palette,
                metrics = metrics,
                centered = centered,
                hero = true,
            )
        },
        choices = {
            val scale = interfaceSize.deviceScaleMultiplier.toFloat()
            if (workspace != null) {
                WorkspacePair(
                    newName = workspace.name,
                    currentName = workspace.backName,
                    dark = previewDark,
                    interfaceScale = scale,
                    icons = icons,
                    palette = palette,
                    modifier = Modifier.widthIn(max = PictureMaxWidth),
                    frontModifier = sharedElement(DESKTOP),
                )
            } else {
                DesktopPreview(
                    dark = previewDark,
                    interfaceScale = scale,
                    icons = icons,
                    palette = palette,
                    modifier = Modifier.widthIn(max = PictureMaxWidth).then(sharedElement(DESKTOP)),
                )
            }
        },
        actions = {
            AnimatedContent(
                targetState = leaving && workspace != null,
                transitionSpec = { actionSwap() },
                contentAlignment = Alignment.BottomCenter,
                label = "welcome action",
            ) { confirming ->
                if (confirming && workspace != null) {
                    LeaveSetup(workspace = workspace, preview = preview, onStay = { leaving = false })
                } else {
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        PrimaryAction(label = "Get started", onClick = onContinue)
                        if (workspace != null) {
                            SetupLinkButton(
                                text = "Back to ${workspace.backName}",
                                palette = palette,
                                onClick = { leaving = true },
                                modifier = Modifier.padding(top = 6.dp),
                            )
                        } else {
                            QuietNote("About a 1 GB download and 5.4 GB of space")
                        }
                    }
                }
            }
        },
    )
}

/**
 * The Android setting the desktop depends on, asked for while it is off. The
 * launch gate shows this stage too ([returning]); [installing] when an
 * install is already under way behind it.
 */
@Composable
internal fun StageScope.PrepareStage(
    state: PrepareState,
    onContinue: () -> Unit,
    returning: Boolean = false,
    installing: Boolean = false,
) {
    var warning by rememberSaveable { mutableStateOf(false) }
    val unlock by animateFloatAsState(
        targetValue = if (state.done) 1f else 0f,
        animationSpec = tween(700, easing = PortalEmphasizedDecelerate),
        label = "continue unlock",
    )
    StageLayout(
        about = {
            StageMark()
            Spacer(Modifier.height(if (metrics.short) 12.dp else 18.dp))
            AnimatedContent(
                targetState = state.done,
                transitionSpec = { copySwap() },
                contentAlignment = Alignment.TopStart,
                label = "prepare copy",
            ) { done ->
                StageTitle(
                    title = prepareTitle(done),
                    body = prepareBody(done, returning = returning, installing = installing),
                    palette = palette,
                    metrics = metrics,
                    modifier = Modifier.dissolveBlur(this, radius = 8.dp),
                )
            }
        },
        choices = {
            // The ring closes once the setting is on; a short card has no room for it.
            if (!metrics.short) {
                ProcessRing(
                    done = state.done,
                    palette = palette,
                    modifier = Modifier
                        .align(Alignment.CenterHorizontally)
                        .size(if (metrics.compact) 88.dp else 104.dp),
                )
                Spacer(Modifier.height(20.dp))
            }
            PrepareSteps(state = state, palette = palette, modifier = Modifier.fillMaxWidth())
        },
        actions = {
            AnimatedContent(
                targetState = warning && !state.done,
                transitionSpec = { actionSwap() },
                contentAlignment = Alignment.BottomCenter,
                label = "prepare action",
            ) { warn ->
                if (warn) {
                    PrepareSkipWarning(
                        palette = palette,
                        returning = returning,
                        modifier = Modifier.widthIn(max = PictureMaxWidth),
                        onBack = { warning = false },
                        onSkip = onContinue,
                    )
                } else {
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        // The button waits, dimmed, until the setting is on; then it lights.
                        PrimaryAction(
                            label = "Continue",
                            onClick = onContinue,
                            enabled = state.done,
                            light = { lerp(0.36f, 1f, unlock) },
                        )
                        AnimatedVisibility(
                            visible = !state.done,
                            enter = fadeIn(tween(300)),
                            exit = fadeOut(tween(200)) + shrinkVertically(tween(320, easing = PortalEmphasized)),
                        ) {
                            SetupLinkButton(
                                text = "Skip for now",
                                palette = palette,
                                onClick = { warning = true },
                                modifier = Modifier.padding(top = 6.dp),
                            )
                        }
                    }
                }
            }
        },
    )
}

/** How the desktop looks, shown on the desktop itself as it changes. */
@Composable
internal fun StageScope.LookStage(
    appearance: AppearanceMode,
    onAppearance: (AppearanceMode) -> Unit,
    interfaceSize: InterfaceSize,
    onInterfaceSize: (InterfaceSize) -> Unit,
    previewDark: Boolean,
    icons: Map<String, ImageBitmap>,
    onContinue: () -> Unit,
) {
    val controls: @Composable ColumnScope.() -> Unit = {
        val segment = if (metrics.short) 42.dp else 48.dp
        SetupSectionLabel(text = "Appearance", palette = palette)
        Spacer(Modifier.height(9.dp))
        SlidingSegmentedControl(
            options = AppearanceMode.entries,
            selected = appearance,
            onSelect = onAppearance,
            label = { it.name },
            palette = palette,
            height = segment,
        )
        Spacer(Modifier.height(if (metrics.short) 12.dp else 18.dp))
        SetupSectionLabel(text = "Interface size", palette = palette)
        Spacer(Modifier.height(9.dp))
        SlidingSegmentedControl(
            options = InterfaceSize.entries,
            selected = interfaceSize,
            onSelect = onInterfaceSize,
            label = { it.name },
            palette = palette,
            height = segment,
        )
    }
    val picture: @Composable ColumnScope.() -> Unit = {
        DesktopPreview(
            dark = previewDark,
            interfaceScale = interfaceSize.deviceScaleMultiplier.toFloat(),
            icons = icons,
            palette = palette,
            modifier = Modifier
                .align(Alignment.CenterHorizontally)
                .widthIn(max = PictureMaxWidth)
                .then(sharedElement(DESKTOP)),
        )
    }
    StageLayout(
        // The desktop takes the room the card has to spare, over the button
        // where the welcome had it; the choices stay in reach under the
        // description, or over the button on a stacked card.
        fitChoices = true,
        about = {
            StageMark()
            Spacer(Modifier.height(if (metrics.short) 12.dp else 18.dp))
            StageTitle(
                title = "Make it yours",
                body = if (metrics.short) null else "Choose how the desktop looks. You can change both later in System Settings.",
                palette = palette,
                metrics = metrics,
            )
        },
        choices = { picture() },
        summary = controls,
        actions = { PrimaryAction(label = "Continue", onClick = onContinue) },
    )
}

/** What comes with the desktop, what can be added, and the room it all takes. */
@Composable
internal fun StageScope.AppsStage(
    selectedIds: Set<String>,
    onToggle: (String) -> Unit,
    capacity: StorageCapacity?,
    icons: Map<String, ImageBitmap>,
    error: String?,
    workspace: WorkspaceSetup?,
    onInstall: () -> Unit,
) {
    StageLayout(
        about = {
            StageMark()
            Spacer(Modifier.height(if (metrics.short) 12.dp else 18.dp))
            StageTitle(
                title = workspace?.let { "Apps for ${it.name}" } ?: "Choose your apps",
                body = if (metrics.short) null else "Every desktop comes with Firefox, Files, a terminal and more. Pick any extras you’d like.",
                palette = palette,
                metrics = metrics,
            )
            if (metrics.split) {
                Spacer(Modifier.height(if (metrics.short) 14.dp else metrics.sectionGap))
                IncludedApps(palette, icons)
            }
        },
        choices = {
            if (!metrics.split) {
                IncludedApps(palette, icons)
                Spacer(Modifier.height(18.dp))
            }
            SetupSectionLabel(text = "Optional apps", palette = palette)
            Spacer(Modifier.height(9.dp))
            OptionalApps(selectedIds = selectedIds, onToggle = onToggle, icons = icons, palette = palette)
            Spacer(Modifier.height(10.dp))
            Text(
                text = OTHER_APPS_SETUP_NOTE,
                modifier = Modifier.padding(horizontal = 4.dp),
                fontSize = 11.sp,
                lineHeight = 15.sp,
                color = palette.textMuted,
            )
        },
        summary = {
            StorageCapacityBar(capacity = capacity, selectedIds = selectedIds, palette = palette)
        },
        actions = {
            PrimaryAction(label = "Begin Install", onClick = onInstall)
            ErrorLine(error)
        },
    )
}

/** The install: its steps and progress, and what to do if it stops. */
@Composable
internal fun StageScope.InstallStage(
    phase: SetupPhase,
    workspace: WorkspaceSetup?,
    progressState: State<Float>,
    nativeProgress: Int,
    message: String,
    errorMessage: String?,
    summary: String?,
    startedAt: Long,
    desktopReady: Boolean,
    checklist: Boolean,
    preview: Boolean,
    onTryAgain: () -> Unit,
    onExportDiagnostics: (() -> Unit)?,
    exportStatus: String?,
    exportRunning: Boolean,
) {
    var leaving by remember { mutableStateOf(false) }
    val name = workspace?.name
    val title = when (phase) {
        SetupPhase.Failed -> "Setup paused"
        SetupPhase.Ready -> if (name != null) "$name is ready" else "Portal is ready"
        else -> if (name != null) "Setting up $name" else "Installing Portal"
    }
    val body = if (phase == SetupPhase.Installing) {
        "This takes a few minutes. You can use other apps meanwhile; setup keeps going in the background."
    } else {
        null
    }
    StageLayout(
        about = {
            StageMark()
            Spacer(Modifier.height(if (metrics.short) 12.dp else 18.dp))
            AnimatedContent(
                targetState = title to body,
                transitionSpec = { copySwap() },
                contentAlignment = Alignment.TopStart,
                label = "install copy",
            ) { (heading, sentence) ->
                StageTitle(
                    title = heading,
                    body = sentence,
                    palette = palette,
                    metrics = metrics,
                    modifier = Modifier.dissolveBlur(this, radius = 8.dp),
                )
            }
        },
        choices = {
            InstallStatusPanel(
                phase = phase,
                progressState = progressState,
                nativeProgress = nativeProgress,
                message = message,
                errorMessage = errorMessage,
                summary = summary,
                startedAt = startedAt,
                desktopReady = desktopReady,
                palette = palette,
                onExportDiagnostics = onExportDiagnostics,
                exportStatus = exportStatus,
                exportRunning = exportRunning,
            )
            // The install takes minutes: offer the Android setting Portal
            // cannot change itself while the user waits.
            AnimatedVisibility(
                visible = checklist && (phase == SetupPhase.Installing || phase == SetupPhase.Failed),
                enter = fadeIn(tween(420, delayMillis = 360, easing = PortalEmphasizedDecelerate)) +
                    expandVertically(tween(560, delayMillis = 300, easing = PortalEmphasized)),
                exit = fadeOut(tween(200, easing = PortalEmphasizedAccelerate)) +
                    shrinkVertically(tween(420, easing = PortalEmphasized)),
            ) {
                SetupChecklist(palette = palette, modifier = Modifier.padding(top = 22.dp).fillMaxWidth())
            }
        },
        actions = {
            val failed = phase == SetupPhase.Failed
            AnimatedContent(
                targetState = when {
                    !failed -> InstallAction.None
                    leaving && workspace != null -> InstallAction.Leave
                    else -> InstallAction.TryAgain
                },
                transitionSpec = { actionSwap() },
                contentAlignment = Alignment.BottomCenter,
                label = "install action",
            ) { action ->
                when (action) {
                    InstallAction.None -> Spacer(Modifier.fillMaxWidth())
                    InstallAction.TryAgain -> Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        PrimaryAction(label = "Try again", onClick = onTryAgain)
                        if (workspace != null) {
                            SetupLinkButton(
                                text = "Back to ${workspace.backName}",
                                palette = palette,
                                onClick = { leaving = true },
                                modifier = Modifier.padding(top = 6.dp),
                            )
                        }
                    }
                    InstallAction.Leave -> if (workspace != null) {
                        LeaveSetup(workspace = workspace, preview = preview, onStay = { leaving = false })
                    }
                }
            }
        },
    )
}

private enum class InstallAction { None, TryAgain, Leave }

// ---------------------------------------------------------------------------
// The layout every stage shares.

/**
 * [about], [choices] and [actions] arranged for the card. [summary] stays
 * in view beside the action on a stacked card and goes under the description
 * on a split one. [fitChoices]: the choices size themselves to the room they
 * get (a picture) instead of scrolling, on a split card.
 */
@Composable
private fun StageScope.StageLayout(
    about: @Composable ColumnScope.() -> Unit,
    actions: @Composable ColumnScope.() -> Unit,
    choices: (@Composable ColumnScope.() -> Unit)? = null,
    summary: (@Composable ColumnScope.() -> Unit)? = null,
    centered: Boolean = false,
    fitChoices: Boolean = false,
) {
    val entering = Modifier.stageDissolve(visibility)
    if (metrics.split) {
        // Each side centres in the card's full height: the description on
        // the left, and on the right the choices with the action under them.
        // Choices too tall for the room scroll, and the action stays put.
        Row(Modifier.fillMaxSize()) {
            Column(
                modifier = Modifier.weight(SetupDimens.SplitStart).fillMaxHeight(),
                verticalArrangement = Arrangement.Center,
            ) {
                StageScroll(Modifier.weight(1f, fill = false).then(entering)) {
                    about()
                    if (summary != null) {
                        Spacer(Modifier.height(if (metrics.short) 14.dp else metrics.sectionGap))
                        summary()
                    }
                }
            }
            Spacer(Modifier.width(metrics.paneGap))
            Column(
                modifier = Modifier.weight(1f - SetupDimens.SplitStart).fillMaxHeight(),
                verticalArrangement = Arrangement.Center,
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                if (choices != null) {
                    if (fitChoices) {
                        Column(
                            modifier = Modifier.weight(1f, fill = false).then(entering),
                            verticalArrangement = Arrangement.Center,
                            horizontalAlignment = Alignment.CenterHorizontally,
                            content = choices,
                        )
                    } else {
                        StageScroll(Modifier.weight(1f, fill = false).then(entering), content = choices)
                    }
                }
                StageActions(actions)
            }
        }
    } else {
        Column(Modifier.fillMaxSize()) {
            if (fitChoices && choices != null) {
                FitScroll(
                    modifier = Modifier.weight(1f).then(entering),
                    centered = centered,
                    gap = metrics.sectionGap,
                    top = about,
                    flex = choices,
                )
            } else {
                StageScroll(Modifier.weight(1f).then(entering), center = true, centered = centered) {
                    about()
                    if (choices != null) {
                        Spacer(Modifier.height(metrics.sectionGap))
                        choices()
                    }
                }
            }
            if (summary != null) {
                // Under a scroller its fading edge already sets the summary
                // apart; under a picture it needs the full gap.
                val gap = if (fitChoices) metrics.sectionGap else 10.dp
                Column(Modifier.fillMaxWidth().padding(top = gap).then(entering)) { summary() }
            }
            StageActions(actions)
        }
    }
}

/**
 * A stage's scrolling column. With [center] its content sits in the middle
 * while it fits; [centered] centres each line too.
 */
@Composable
private fun StageScroll(
    modifier: Modifier,
    center: Boolean = false,
    centered: Boolean = false,
    content: @Composable ColumnScope.() -> Unit,
) {
    val state = rememberScrollState()
    Column(
        modifier = modifier
            .fillMaxWidth()
            .setupScrollRegion(state)
            .verticalScroll(state),
        verticalArrangement = if (center) Arrangement.Center else Arrangement.Top,
        horizontalAlignment = if (centered) Alignment.CenterHorizontally else Alignment.Start,
        content = content,
    )
}

/**
 * [top], then [flex] (a picture) given whatever height is left in view, the
 * two centred together. Only when even [MinPicture] of picture would not fit
 * does the column scroll.
 */
@Composable
private fun FitScroll(
    modifier: Modifier,
    centered: Boolean,
    gap: Dp,
    top: @Composable ColumnScope.() -> Unit,
    flex: @Composable ColumnScope.() -> Unit,
) {
    val state = rememberScrollState()
    BoxWithConstraints(modifier.fillMaxWidth()) {
        val viewport = constraints.maxHeight
        Layout(
            content = {
                Column(
                    horizontalAlignment = if (centered) Alignment.CenterHorizontally else Alignment.Start,
                    content = top,
                )
                Column(horizontalAlignment = Alignment.CenterHorizontally, content = flex)
            },
            modifier = Modifier
                .fillMaxWidth()
                .setupScrollRegion(state)
                .verticalScroll(state),
        ) { measurables, outer ->
            val width = outer.maxWidth
            val loose = Constraints(maxWidth = width)
            val head = measurables[0].measure(loose)
            val gapPx = gap.roundToPx()
            // A gap's worth of air stays around the pair, split above and below.
            val room = (viewport - head.height - gapPx * 2).coerceAtLeast(MinPicture.roundToPx())
            val picture = measurables[1].measure(loose.copy(maxHeight = room))
            val total = head.height + gapPx + picture.height
            val height = maxOf(total, viewport)
            layout(width, height) {
                val y = (height - total) / 2
                head.place(if (centered) (width - head.width) / 2 else 0, y)
                picture.place((width - picture.width) / 2, y + head.height + gapPx)
            }
        }
    }
}

/** The least a fitted picture shrinks to before its stage scrolls instead. */
private val MinPicture = 120.dp

/**
 * Edges fade where more waits, and while it can scroll the region keeps
 * upward swipes, which would otherwise lift the veil once the desktop is ready.
 */
@Composable
private fun Modifier.setupScrollRegion(state: ScrollState): Modifier {
    val veil = LocalPortalVeil.current
    val key = remember { Any() }
    // Derived: the limit moves every frame while the card resizes.
    val scrollable by remember(state) { derivedStateOf { state.maxValue in 1 until Int.MAX_VALUE } }
    DisposableEffect(veil) { onDispose { veil.gestureExclusions.remove(key) } }
    LaunchedEffect(scrollable) { if (!scrollable) veil.gestureExclusions.remove(key) }
    return this
        .onGloballyPositioned { if (scrollable) veil.gestureExclusions[key] = it.boundsInRoot() }
        .graphicsLayer {
            compositingStrategy = if (scrollable) CompositingStrategy.Offscreen else CompositingStrategy.Auto
        }
        .drawWithContent {
            drawContent()
            val edge = ScrollFade.toPx()
            if (!scrollable || size.height < edge * 3f) return@drawWithContent
            val top = (state.value / edge).coerceIn(0f, 1f)
            val bottom = ((state.maxValue - state.value) / edge).coerceIn(0f, 1f)
            drawRect(
                brush = Brush.verticalGradient(
                    0f to Color.Black.copy(alpha = 1f - top),
                    edge / size.height to Color.Black,
                    1f - edge / size.height to Color.Black,
                    1f to Color.Black.copy(alpha = 1f - bottom),
                ),
                blendMode = BlendMode.DstIn,
            )
        }
}

@Composable
private fun StageScope.StageActions(content: @Composable ColumnScope.() -> Unit) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = if (metrics.short) 10.dp else 16.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        content = content,
    )
}

// ---------------------------------------------------------------------------
// What carries across stages.

@OptIn(ExperimentalSharedTransitionApi::class)
@Composable
private fun StageScope.sharedElement(key: String): Modifier = with(shared) {
    Modifier.sharedElement(rememberSharedContentState(key), visibility, boundsTransform = StageBounds)
}

/** The Portal mark: large on the welcome, small above every other stage's title. */
@Composable
private fun StageScope.StageMark(hero: Boolean = false) {
    val height = when {
        hero && metrics.short -> 54.dp
        hero && metrics.compact -> 76.dp
        hero -> 96.dp
        metrics.short -> 28.dp
        metrics.compact -> 34.dp
        else -> 40.dp
    }
    PortalMark(palette = palette, height = height, modifier = sharedElement(MARK).then(launchMarkModifier))
}

/**
 * The glowing primary button. Its glow spills past the 52dp it keeps in the
 * layout, and from stage to stage one button turns into the next.
 */
@OptIn(ExperimentalSharedTransitionApi::class)
@Composable
private fun StageScope.PrimaryAction(
    label: String,
    onClick: () -> Unit,
    enabled: Boolean = true,
    // Below 1 while the action waits on something.
    light: () -> Float = { 1f },
) {
    val width = metrics.actionWidth.coerceAtMost(PortalDimens.BeginMaxWidth)
    val travel = with(shared) {
        Modifier.sharedBounds(
            sharedContentState = rememberSharedContentState(PRIMARY),
            animatedVisibilityScope = visibility,
            // The outgoing button stays solid under the incoming one until it
            // has mostly arrived, so the button never dips.
            enter = fadeIn(tween(420, delayMillis = 120)),
            exit = fadeOut(tween(320, delayMillis = 280)),
            boundsTransform = StageBounds,
            resizeMode = SharedTransitionScope.ResizeMode.ScaleToBounds(ContentScale.FillWidth, Alignment.Center),
            zIndexInOverlay = 1f,
            clipInOverlayDuringTransition = UnclippedOverlay,
        )
    }
    Box(
        modifier = Modifier
            .size(width, PortalDimens.BeginHeight)
            .wrapContentSize(unbounded = true),
        contentAlignment = Alignment.Center,
    ) {
        BeginInstallButton(
            palette = palette,
            centered = false,
            onBeginInstall = onClick,
            enabled = enabled,
            label = label,
            glowBoundsModifier = travel.graphicsLayer { alpha = light() },
            modifier = Modifier.width(width),
        )
    }
}

// ---------------------------------------------------------------------------
// Pieces.

/** "New workspace", over a workspace's name. */
@Composable
private fun WorkspaceChip(palette: PortalPalette) {
    val shape = RoundedCornerShape(50)
    Row(
        modifier = Modifier
            .clip(shape)
            .background(palette.accent.copy(alpha = 0.10f))
            .border(1.dp, palette.accent.copy(alpha = 0.32f), shape)
            .padding(start = 9.dp, end = 11.dp, top = 5.dp, bottom = 5.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        StackGlyph(palette = palette, busy = false)
        Spacer(Modifier.width(7.dp))
        Text(
            text = "New workspace",
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
            color = palette.textPrimary,
        )
    }
}

/**
 * Giving up a new workspace's setup for the workspace Portal came from. Its
 * files go with it, so it asks first.
 */
@Composable
private fun StageScope.LeaveSetup(workspace: WorkspaceSetup, preview: Boolean, onStay: () -> Unit) {
    val context = LocalContext.current
    var going by remember { mutableStateOf(false) }
    var refusal by remember { mutableStateOf<String?>(null) }
    val shape = RoundedCornerShape(20.dp)
    Column(
        modifier = Modifier
            .widthIn(max = PictureMaxWidth)
            .fillMaxWidth()
            .clip(shape)
            .background(palette.trackFill)
            .border(1.dp, palette.surfaceBorder, shape)
            .padding(horizontal = 18.dp, vertical = 16.dp),
    ) {
        Text(
            text = if (going) "Going back to ${workspace.backName}…" else "Go back to ${workspace.backName}?",
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
            color = palette.textPrimary,
        )
        Spacer(Modifier.height(4.dp))
        Text(
            text = refusal ?: "“${workspace.name}” isn’t set up yet, so it will be deleted.",
            fontSize = 13.sp,
            lineHeight = 18.sp,
            color = if (refusal != null) palette.accent else palette.textMuted,
        )
        if (!going) {
            Spacer(Modifier.height(14.dp))
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End),
            ) {
                PanelButton(text = "Stay", primary = false, palette = palette, onClick = onStay)
                PanelButton(text = "Delete and go back", primary = true, palette = palette) {
                    if (preview) {
                        onStay()
                    } else {
                        going = true
                        refusal = null
                        Workspaces.abandonSetup(context) {
                            refusal = it
                            going = false
                        }
                    }
                }
            }
        }
    }
}

/**
 * What every install already has, as the icons you will meet in Plasma.
 * Touching an icon names it in the label for a moment.
 */
@Composable
private fun IncludedApps(palette: PortalPalette, icons: Map<String, ImageBitmap>) {
    var named by remember { mutableStateOf<String?>(null) }
    var touches by remember { mutableIntStateOf(0) }
    LaunchedEffect(touches) {
        if (named != null) {
            delay(2200)
            named = null
        }
    }
    AnimatedContent(
        targetState = named,
        transitionSpec = {
            (fadeIn(tween(260, delayMillis = 40, easing = PortalEmphasizedDecelerate)) +
                slideInVertically(tween(360, easing = PortalEmphasized)) { it / 2 })
                .togetherWith(
                    fadeOut(tween(160, easing = PortalEmphasizedAccelerate)) +
                        slideOutVertically(tween(220, easing = PortalEmphasizedAccelerate)) { -it / 2 },
                )
                .using(SizeTransform(clip = false))
        },
        contentAlignment = Alignment.CenterStart,
        label = "included label",
    ) { name ->
        SetupSectionLabel(
            text = if (name == null) "Included" else "Included · $name",
            palette = palette,
            modifier = Modifier.dissolveBlur(this, radius = 4.dp),
        )
    }
    Spacer(Modifier.height(9.dp))
    val shape = RoundedCornerShape(24.dp)
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .height(48.dp)
            .clip(shape)
            .background(palette.trackFill)
            .border(1.dp, palette.surfaceBorder, shape)
            .padding(horizontal = 10.dp),
        horizontalArrangement = Arrangement.SpaceEvenly,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        INCLUDED_APPS.forEachIndexed { index, app ->
            val icon = icons[app.id]
            // Icons settle in one after another once decoded.
            val shown by animateFloatAsState(
                targetValue = if (icon != null) 1f else 0f,
                animationSpec = tween(480, delayMillis = 60 + index * 45, easing = PortalEmphasizedDecelerate),
                label = "included icon",
            )
            val lifted by animateFloatAsState(
                targetValue = if (named == app.name) 1f else 0f,
                animationSpec = tween(320, easing = PortalEmphasized),
                label = "included focus",
            )
            // Slots share the row so all of them fit on a phone; 34dp at most.
            Box(
                modifier = Modifier
                    .weight(1f, fill = false)
                    .widthIn(max = 34.dp)
                    .fillMaxWidth()
                    .height(34.dp)
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                        onClickLabel = app.name,
                        onClick = {
                            named = app.name
                            touches++
                        },
                    ),
                contentAlignment = Alignment.Center,
            ) {
                if (icon != null) {
                    Image(
                        bitmap = icon,
                        contentDescription = app.name,
                        modifier = Modifier
                            .size(26.dp)
                            .graphicsLayer {
                                alpha = shown
                                val s = (0.86f + 0.14f * shown) * (1f + 0.14f * lifted)
                                scaleX = s
                                scaleY = s
                                translationY = (1f - shown) * 6.dp.toPx() - lifted * 2.dp.toPx()
                            },
                    )
                }
            }
        }
    }
}

/** Every optional app by category, each with its checkbox. */
@Composable
private fun OptionalApps(
    selectedIds: Set<String>,
    onToggle: (String) -> Unit,
    icons: Map<String, ImageBitmap>,
    palette: PortalPalette,
) {
    val shape = RoundedCornerShape(22.dp)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(shape)
            .background(palette.trackFill)
            .border(1.dp, palette.surfaceBorder, shape)
            .padding(horizontal = 4.dp, vertical = 6.dp),
    ) {
        groupedOptionalApps().forEach { (category, apps) ->
            CategoryLabel(category, palette)
            apps.forEach { app ->
                AppSelectionRow(
                    app = app,
                    icon = icons[app.id],
                    checked = app.id in selectedIds,
                    onToggle = { onToggle(app.id) },
                    palette = palette,
                    enabled = app.available,
                )
            }
        }
    }
}

@Composable
private fun StageScope.QuietNote(text: String) {
    Text(
        text = text,
        modifier = Modifier.padding(top = 12.dp, start = 8.dp, end = 8.dp),
        fontSize = 12.sp,
        lineHeight = 16.sp,
        color = palette.textMuted,
        textAlign = TextAlign.Center,
    )
}

/** Why the install button didn't take, under it. */
@Composable
private fun StageScope.ErrorLine(error: String?) {
    AnimatedVisibility(
        visible = error != null,
        enter = fadeIn(tween(160)) + expandVertically(tween(220)),
        exit = fadeOut(tween(120)) + shrinkVertically(tween(220)),
    ) {
        Text(
            text = error.orEmpty(),
            modifier = Modifier.padding(top = 10.dp, start = 8.dp, end = 8.dp),
            fontSize = 12.sp,
            lineHeight = 16.sp,
            color = palette.accent,
            textAlign = TextAlign.Center,
            maxLines = 3,
        )
    }
}

/** A title and its sentence changing in place. */
private fun AnimatedContentTransitionScope<*>.copySwap(): ContentTransform =
    (
        fadeIn(tween(380, delayMillis = 90, easing = PortalEmphasizedDecelerate)) +
            slideInVertically(tween(480, easing = PortalEmphasized)) { it / 4 }
        ).togetherWith(
        fadeOut(tween(160, easing = PortalEmphasizedAccelerate)) +
            slideOutVertically(tween(220, easing = PortalEmphasizedAccelerate)) { -it / 5 },
    ).using(SizeTransform(clip = false))

/** An action giving way to a question about it, and back. */
private fun AnimatedContentTransitionScope<*>.actionSwap(): ContentTransform =
    (
        fadeIn(tween(300, delayMillis = 90, easing = PortalEmphasizedDecelerate)) +
            scaleIn(tween(420, easing = PortalEmphasized), initialScale = 0.97f)
        ).togetherWith(fadeOut(tween(140, easing = PortalEmphasizedAccelerate)))
        .using(SizeTransform(clip = false) { _, _ -> PortalPanelSizeSpring })
