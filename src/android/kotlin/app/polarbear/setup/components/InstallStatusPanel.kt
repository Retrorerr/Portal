package app.polarbear.setup.components

// What the setup card shows once Begin Install is tapped: the four steps, one
// honest progress bar, a plain sentence about what is happening, and, only
// when setup truly needs the user, a clear Try again. The configuration
// controls are gone by now rather than left greyed out behind it.
//
// Motion is reserved for meaning: the phrase changes only when Portal moves
// on to something else, and while it keeps doing the same thing only the
// digits that changed roll, like an odometer.

import android.os.SystemClock
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandHorizontally
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkHorizontally
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.wrapContentSize
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.BeginInstallButton
import app.polarbear.setup.INSTALL_STEP_TITLES
import app.polarbear.setup.PortalColors
import app.polarbear.setup.PortalDimens
import app.polarbear.setup.PortalPalette
import app.polarbear.setup.SetupPhase
import app.polarbear.setup.installStep
import app.polarbear.setup.installedIn
import app.polarbear.setup.pausedLabel
import kotlin.math.roundToInt

/** Wide enough for the failure message and Try again to sit side by side. */
private val SideBySideFooter = 560.dp

@Composable
internal fun InstallStatusPanel(
    phase: SetupPhase,
    /** Animated 0..1 overall progress. */
    progressState: State<Float>,
    nativeProgress: Int,
    message: String,
    errorMessage: String?,
    /** The choices being installed, when this process still knows them. */
    summary: String?,
    desktopReady: Boolean,
    palette: PortalPalette,
    onTryAgain: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val failed = phase == SetupPhase.Failed
    val ready = phase == SetupPhase.Ready
    val step = installStep(if (ready) 100 else nativeProgress, message)
    val activeIndex = if (ready) INSTALL_STEP_TITLES.size else step.index

    // How long the install took, only when this screen saw all of it: a
    // relaunch mid-install or a pause in between would make it a lie.
    var startedAt by rememberSaveable { mutableLongStateOf(0L) }
    var finishedAt by rememberSaveable { mutableLongStateOf(0L) }
    var interrupted by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(phase) {
        when (phase) {
            SetupPhase.Installing -> if (startedAt == 0L && nativeProgress <= 5) {
                startedAt = SystemClock.elapsedRealtime()
            }
            SetupPhase.Failed -> interrupted = true
            SetupPhase.Ready -> if (finishedAt == 0L) finishedAt = SystemClock.elapsedRealtime()
            else -> Unit
        }
    }
    val took = if (startedAt > 0L && finishedAt > startedAt && !interrupted) finishedAt - startedAt else null

    // The title already says "Portal is ready" or "Setup paused"; this line
    // says what the title does not.
    val label: String?
    val figure: String?
    when {
        ready && !desktopReady -> { label = "Opening your desktop"; figure = null }
        ready -> { label = took?.let(::installedIn); figure = null }
        failed -> { label = pausedLabel(step.index); figure = null }
        else -> { label = step.label; figure = step.figure }
    }
    // A phrase on its way out keeps the last figure it showed.
    val figures = remember { mutableStateMapOf<String, String>() }
    if (label != null && figure != null) figures[label] = figure
    val working = !ready && !failed
    val labelColor = when {
        working && step.waiting -> palette.accent
        else -> palette.textSecondary
    }

    Column(modifier = modifier.fillMaxWidth()) {
        StepTrack(activeIndex = activeIndex, failed = failed, palette = palette)
        Spacer(Modifier.height(26.dp))
        Row(
            verticalAlignment = Alignment.Bottom,
            modifier = Modifier.semantics(mergeDescendants = true) {
                liveRegion = LiveRegionMode.Polite
            },
        ) {
            val percent = (progressState.value.coerceIn(0f, 1f) * 100f).roundToInt()
            RollingText(
                text = "$percent%",
                style = TextStyle(
                    fontSize = 34.sp,
                    lineHeight = 36.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = palette.textPrimary,
                    fontFeatureSettings = "tnum",
                ),
                modifier = Modifier.clearAndSetSemantics { contentDescription = "$percent percent" },
            )
            Spacer(Modifier.width(14.dp))
            // Keyed on the phrase alone: a new figure never replays this.
            AnimatedContent(
                targetState = label,
                transitionSpec = {
                    (fadeIn(tween(280, delayMillis = 80, easing = PortalEmphasizedDecelerate)) +
                        slideInVertically(tween(360, easing = PortalEmphasized)) { it / 2 })
                        .togetherWith(
                            fadeOut(tween(160, easing = PortalEmphasizedAccelerate)) +
                                slideOutVertically(tween(220, easing = PortalEmphasizedAccelerate)) { -it / 2 },
                        )
                        .using(SizeTransform(clip = false))
                },
                contentAlignment = Alignment.BottomStart,
                label = "install phrase",
                modifier = Modifier.padding(bottom = 5.dp),
            ) { phrase ->
                if (phrase != null) {
                    DetailLine(
                        label = phrase,
                        figure = if (phrase == label) figure else figures[phrase],
                        // A phrase that is still going on trails off.
                        ellipsis = working || (ready && !desktopReady),
                        color = labelColor,
                    )
                }
            }
        }
        Spacer(Modifier.height(12.dp))
        ProgressTrack(
            progressState = progressState,
            moving = working && !step.waiting,
            failed = failed,
            palette = palette,
        )
        // The footer brings its own top gap, so a finished card with nothing
        // under the bar keeps even margins.
        AnimatedContent(
            targetState = failed,
            transitionSpec = {
                (fadeIn(tween(320, delayMillis = 140)) + expandVertically(tween(420, easing = PortalEmphasized), clip = false))
                    .togetherWith(fadeOut(tween(160)) + shrinkVertically(tween(320, easing = PortalEmphasized), clip = false))
                    .using(SizeTransform(clip = false))
            },
            label = "install footer",
        ) { showFailure ->
            if (showFailure) {
                Box(Modifier.padding(top = 16.dp)) {
                    FailureFooter(errorMessage = errorMessage, palette = palette, onTryAgain = onTryAgain)
                }
            } else {
                Column(Modifier.fillMaxWidth()) {
                    if (summary != null) {
                        FooterText(summary, palette.textMuted, Modifier.padding(top = 16.dp))
                    }
                    AnimatedVisibility(
                        visible = !ready,
                        enter = fadeIn() + expandVertically(),
                        exit = fadeOut(tween(200)) + shrinkVertically(tween(320, easing = PortalEmphasized)),
                    ) {
                        FooterText(
                            "You can use other apps meanwhile. Setup keeps going in the background.",
                            palette.textMuted,
                            Modifier.padding(top = if (summary != null) 2.dp else 16.dp),
                        )
                    }
                }
            }
        }
    }
}

/** "Downloading Debian · 58 of 964 MB": the phrase holds still, the figure rolls. */
@Composable
private fun DetailLine(label: String, figure: String?, ellipsis: Boolean, color: Color) {
    val style = TextStyle(
        fontSize = 14.sp,
        lineHeight = 18.sp,
        fontWeight = FontWeight.Medium,
        color = color,
        fontFeatureSettings = "tnum",
    )
    Row(verticalAlignment = Alignment.Bottom) {
        Text(text = label, style = style, maxLines = 1)
        // The figure slides out of the phrase rather than popping in, and
        // the trailing dots step aside for it.
        AnimatedContent(
            targetState = figure != null,
            transitionSpec = {
                (fadeIn(tween(240, delayMillis = 60)) + expandHorizontally(tween(320, easing = PortalEmphasized), clip = false))
                    .togetherWith(fadeOut(tween(140)) + shrinkHorizontally(tween(260, easing = PortalEmphasized), clip = false))
                    .using(SizeTransform(clip = false))
            },
            contentAlignment = Alignment.BottomStart,
            label = "install figure",
        ) { hasFigure ->
            if (hasFigure && figure != null) {
                Row(verticalAlignment = Alignment.Bottom) {
                    Text(text = " · ", style = style, maxLines = 1)
                    RollingText(text = figure, style = style)
                }
            } else if (ellipsis) {
                Text(text = "…", style = style, maxLines = 1)
            }
        }
    }
}

/**
 * Text whose digits roll individually when they change; everything else
 * holds still. Slots are keyed from the right, so units stay units as a
 * number grows a digit.
 */
@Composable
private fun RollingText(text: String, style: TextStyle, modifier: Modifier = Modifier) {
    val runs = remember(text) { splitRuns(text) }
    Row(modifier = modifier, verticalAlignment = Alignment.Bottom) {
        var fromRight = text.length
        runs.forEach { run ->
            if (run.numeric) {
                run.text.forEach { char ->
                    key("c$fromRight") {
                        RollingChar(char, style)
                    }
                    fromRight--
                }
            } else {
                key("t$fromRight") {
                    Text(text = run.text, style = style, maxLines = 1)
                }
                fromRight -= run.text.length
            }
        }
    }
}

@Composable
private fun RollingChar(char: Char, style: TextStyle) {
    AnimatedContent(
        targetState = char,
        transitionSpec = {
            if (initialState.isDigit() && targetState.isDigit()) {
                // Counting up: the new digit rises into place.
                val rising = targetState > initialState || (initialState == '9' && targetState == '0')
                val direction = if (rising) 1 else -1
                (slideInVertically(tween(300, easing = PortalEmphasized)) { direction * it } +
                    fadeIn(tween(200, delayMillis = 40)))
                    .togetherWith(
                        slideOutVertically(tween(300, easing = PortalEmphasized)) { -direction * it } +
                            fadeOut(tween(160)),
                    )
                    .using(SizeTransform(clip = true))
            } else {
                fadeIn(tween(200)).togetherWith(fadeOut(tween(120))).using(SizeTransform(clip = false))
            }
        },
        contentAlignment = Alignment.BottomCenter,
        label = "rolling digit",
    ) { shown ->
        Text(text = shown.toString(), style = style, maxLines = 1)
    }
}

private data class TextRun(val text: String, val numeric: Boolean)

/** Digit groups (with their thousands separators) apart from the words around them. */
private fun splitRuns(text: String): List<TextRun> {
    val runs = mutableListOf<TextRun>()
    val current = StringBuilder()
    var numeric = false
    text.forEachIndexed { index, char ->
        val partOfNumber = char.isDigit() ||
            (char == ',' && numeric && text.getOrNull(index + 1)?.isDigit() == true)
        if (current.isNotEmpty() && partOfNumber != numeric) {
            runs += TextRun(current.toString(), numeric)
            current.clear()
        }
        numeric = partOfNumber
        current.append(char)
    }
    if (current.isNotEmpty()) runs += TextRun(current.toString(), numeric)
    return runs
}

@Composable
private fun FooterText(text: String, color: Color, modifier: Modifier = Modifier) {
    Text(
        text = text,
        modifier = modifier,
        fontSize = 12.sp,
        lineHeight = 16.sp,
        color = color,
    )
}

/**
 * Why setup stopped and the one way on, side by side like Begin Install
 * beside the storage bar, stacked when the card is narrow. The button keeps
 * only its own 52dp in the layout; its glow spills into the card padding
 * instead of reserving a tall empty block around it.
 */
@Composable
private fun FailureFooter(errorMessage: String?, palette: PortalPalette, onTryAgain: () -> Unit) {
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val sideBySide = maxWidth >= SideBySideFooter
        val explanation: @Composable (Modifier) -> Unit = { textModifier ->
            Column(textModifier) {
                Text(
                    text = errorMessage ?: "Portal couldn’t finish setting up. Tap Try again.",
                    fontSize = 14.sp,
                    lineHeight = 19.sp,
                    fontWeight = FontWeight.Medium,
                    color = palette.textPrimary,
                )
                Spacer(Modifier.height(3.dp))
                FooterText("Your choices and everything already downloaded are kept.", palette.textMuted)
            }
        }
        val button: @Composable () -> Unit = {
            Box(
                Modifier
                    .size(PortalDimens.BeginMaxWidth, PortalDimens.BeginHeight)
                    .wrapContentSize(unbounded = true),
                contentAlignment = Alignment.Center,
            ) {
                BeginInstallButton(
                    palette = palette,
                    centered = false,
                    onBeginInstall = onTryAgain,
                    label = "Try again",
                )
            }
        }
        if (sideBySide) {
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(PortalDimens.SurfacePaddingH + 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                explanation(Modifier.weight(1f))
                button()
            }
        } else {
            Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
                explanation(Modifier.fillMaxWidth())
                Spacer(Modifier.height(22.dp))
                button()
            }
        }
    }
}

/** Download › Unpack › Set up › Finish, with the current step lit. */
@Composable
private fun StepTrack(activeIndex: Int, failed: Boolean, palette: PortalPalette) {
    val pulse by rememberInfiniteTransition(label = "step pulse").animateFloat(
        initialValue = 0.35f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(900, easing = PortalEmphasized), RepeatMode.Reverse),
        label = "step pulse alpha",
    )
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .semantics(mergeDescendants = true) {
                contentDescription = "Step ${minOf(activeIndex + 1, INSTALL_STEP_TITLES.size)} of " +
                    "${INSTALL_STEP_TITLES.size}: ${INSTALL_STEP_TITLES[minOf(activeIndex, INSTALL_STEP_TITLES.lastIndex)]}"
            },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        INSTALL_STEP_TITLES.forEachIndexed { index, title ->
            val done = index < activeIndex
            val current = index == activeIndex
            val strength by animateFloatAsState(
                targetValue = if (done || current) 1f else 0.42f,
                animationSpec = tween(420, easing = PortalEmphasized),
                label = "step strength",
            )
            Row(verticalAlignment = Alignment.CenterVertically) {
                Canvas(Modifier.size(18.dp)) {
                    val radius = size.minDimension / 2f
                    when {
                        done -> {
                            drawCircle(PortalColors.Orange, radius)
                            val tick = androidx.compose.ui.graphics.Path().apply {
                                moveTo(size.width * 0.29f, size.height * 0.52f)
                                lineTo(size.width * 0.44f, size.height * 0.67f)
                                lineTo(size.width * 0.72f, size.height * 0.36f)
                            }
                            drawPath(tick, PortalColors.Ivory, style = Stroke(2.dp.toPx(), cap = StrokeCap.Round))
                        }
                        current && failed -> drawCircle(palette.accent, radius - 1.dp.toPx(), style = Stroke(2.dp.toPx()))
                        current -> {
                            drawCircle(PortalColors.Orange.copy(alpha = 0.22f * pulse), radius)
                            drawCircle(PortalColors.Orange, radius * 0.42f)
                        }
                        else -> drawCircle(
                            palette.textPrimary.copy(alpha = 0.16f),
                            radius - 0.75.dp.toPx(),
                            style = Stroke(1.5.dp.toPx()),
                        )
                    }
                }
                Spacer(Modifier.width(8.dp))
                Text(
                    text = title,
                    modifier = Modifier.graphicsLayer { alpha = strength },
                    fontSize = 13.sp,
                    fontWeight = if (current) FontWeight.SemiBold else FontWeight.Medium,
                    color = palette.textPrimary,
                )
            }
            if (index < INSTALL_STEP_TITLES.lastIndex) {
                // The connector fills as its step completes.
                val filled by animateFloatAsState(
                    targetValue = if (done) 1f else 0f,
                    animationSpec = tween(620, easing = PortalEmphasized),
                    label = "step connector",
                )
                Canvas(
                    Modifier
                        .weight(1f)
                        .padding(horizontal = 10.dp)
                        .height(2.dp),
                ) {
                    val y = size.height / 2f
                    drawLine(
                        color = palette.textPrimary.copy(alpha = 0.1f),
                        start = Offset(0f, y),
                        end = Offset(size.width, y),
                        strokeWidth = size.height,
                        cap = StrokeCap.Round,
                    )
                    if (filled > 0f) {
                        drawLine(
                            color = PortalColors.Orange.copy(alpha = 0.7f),
                            start = Offset(0f, y),
                            end = Offset(size.width * filled, y),
                            strokeWidth = size.height,
                            cap = StrokeCap.Round,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun ProgressTrack(
    progressState: State<Float>,
    moving: Boolean,
    failed: Boolean,
    palette: PortalPalette,
) {
    // A slow glint travels the filled track: the bar reads as alive even
    // while a long step holds the percentage still.
    val glint by rememberInfiniteTransition(label = "progress glint").animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(1_700, delayMillis = 900, easing = PortalEmphasized), RepeatMode.Restart),
        label = "glint position",
    )
    val fill by animateFloatAsState(if (failed) 0.45f else 1f, tween(420), label = "progress fill strength")
    Canvas(
        Modifier
            .fillMaxWidth()
            .height(10.dp),
    ) {
        val progress = progressState.value.coerceIn(0f, 1f)
        val radius = CornerRadius(size.height / 2f)
        drawRoundRect(color = palette.textPrimary.copy(alpha = 0.075f), cornerRadius = radius)
        val width = size.width * progress
        if (width <= 0f) return@Canvas
        val fillRadius = CornerRadius(minOf(size.height / 2f, width / 2f))
        drawRoundRect(
            color = PortalColors.Orange.copy(alpha = fill),
            size = Size(width, size.height),
            cornerRadius = fillRadius,
        )
        if (moving && progress < 1f) {
            val band = 120.dp.toPx()
            val centre = -band + (size.width + band * 2f) * glint
            drawRoundRect(
                brush = Brush.horizontalGradient(
                    0f to Color.Transparent,
                    0.5f to PortalColors.Ivory.copy(alpha = 0.34f),
                    1f to Color.Transparent,
                    startX = centre - band / 2f,
                    endX = centre + band / 2f,
                ),
                size = Size(width, size.height),
                cornerRadius = fillRadius,
            )
        }
    }
}
