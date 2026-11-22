package app.polarbear.setup

// The frame every first-run stage sits in: one card on the ambient
// background, sized for the screen it is on and kept the same size from stage
// to stage, so moving through setup changes what is in the card, never the
// card itself.
//
// Phones in either orientation get a card that fills the screen. Tablets get
// a centred card of fixed proportions. Wherever the screen is wider than it is
// tall (and wide enough), the card splits in two: what the stage is about on
// the left, its choices and its action on the right. Rotating swaps the two
// arrangements in place, under Android's own rotation animation.

import androidx.compose.animation.AnimatedContentTransitionScope
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.ContentTransform
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.LineBreak
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import app.polarbear.setup.components.dissolveBlur

/** The stages of first-run setup, in the order they are met. */
internal enum class SetupStage { Welcome, Prepare, Look, Apps, Install }

/** How the setup card sits on a screen of a given size. */
@Immutable
internal data class SetupMetrics(
    /** About the stage on the left, its choices on the right. */
    val split: Boolean,
    /** Phone-sized: the card fills the screen and the type steps down. */
    val compact: Boolean,
    val cardWidth: Dp,
    val cardHeight: Dp,
) {
    /** A phone on its side: every dp of height counts. */
    val short: Boolean get() = compact && split
    val paddingH: Dp get() = if (compact) 20.dp else 36.dp
    val paddingV: Dp get() = when {
        short -> 12.dp
        compact -> 18.dp
        else -> 30.dp
    }
    val topBarHeight: Dp get() = if (short) 30.dp else 36.dp
    /** Between the two halves of a split card. */
    val paneGap: Dp get() = if (compact) 28.dp else 44.dp
    /** Between a stage's description and its choices, stacked. */
    val sectionGap: Dp get() = if (compact) 22.dp else 28.dp
    val titleSize get() = if (compact) 24.sp else 28.sp
    val heroTitleSize get() = if (compact) 28.sp else 34.sp
    val bodySize get() = if (compact) 14.sp else 15.sp
    val bodyLineHeight get() = if (compact) 20.sp else 22.sp

    /** The room a stage's action has: the card's width stacked, its right side split. */
    val actionWidth: Dp
        get() = if (split) {
            (cardWidth - paddingH * 2 - paneGap) * (1f - SetupDimens.SplitStart) * ActionShare
        } else {
            (cardWidth - paddingH * 2) * ActionShare
        }
}

/** The primary button's share of its room, leaving its glow space to fade out inside the card. */
private const val ActionShare = 0.8f

/** The card for a screen whose safe area is [width] x [height]. */
internal fun setupMetrics(width: Dp, height: Dp): SetupMetrics {
    val compact = minOf(width, height) < SetupDimens.TabletMinSide
    val split = (width >= SetupDimens.SplitMinWidth && width > height) ||
        (height < SetupDimens.ShortHeight && width >= SetupDimens.ShortSplitMinWidth)
    if (compact) {
        return SetupMetrics(
            split = split,
            compact = true,
            cardWidth = width - SetupDimens.PhoneGutter * 2,
            cardHeight = height - SetupDimens.PhoneGutter * 2,
        )
    }
    return SetupMetrics(
        split = split,
        compact = false,
        cardWidth = if (split) minOf(width * 0.88f, SetupDimens.SplitCardMaxWidth) else minOf(width * 0.84f, SetupDimens.StackedCardMaxWidth),
        cardHeight = if (split) minOf(height * 0.80f, SetupDimens.SplitCardMaxHeight) else minOf(height * 0.74f, SetupDimens.StackedCardMaxHeight),
    )
}

internal object SetupDimens {
    /** Below this shorter side the screen is a phone's. */
    val TabletMinSide: Dp = 600.dp
    val SplitMinWidth: Dp = 720.dp
    /** A phone on its side: too short to stack, so split even when narrower. */
    val ShortHeight: Dp = 480.dp
    val ShortSplitMinWidth: Dp = 560.dp
    val PhoneGutter: Dp = 10.dp
    val SplitCardMaxWidth: Dp = 960.dp
    val SplitCardMaxHeight: Dp = 640.dp
    val StackedCardMaxWidth: Dp = 600.dp
    val StackedCardMaxHeight: Dp = 880.dp
    /** Share of a split card's width given to the stage's description. */
    const val SplitStart = 0.42f
}

/**
 * Moving between stages: forward slides in from the right, back from the
 * left, both settling out of a soft blur while the card holds still.
 */
internal fun AnimatedContentTransitionScope<*>.stageTransition(forward: Boolean): ContentTransform =
    (
        fadeIn(tween(520, delayMillis = 140, easing = PortalEmphasizedDecelerate)) +
            slideInHorizontally(tween(720, easing = PortalEmphasized)) { width ->
                (if (forward) 1 else -1) * width / 10
            }
        ).togetherWith(
        fadeOut(tween(260, easing = PortalEmphasizedAccelerate)) +
            slideOutHorizontally(tween(420, easing = PortalEmphasizedAccelerate)) { width ->
                (if (forward) -1 else 1) * width / 14
            },
    ).using(SizeTransform(clip = false))

/**
 * The card's top row: Back on the left, where this stage is among the rest
 * on the right. Both stay put while the stage changes beneath them; on a stage
 * with neither, the row keeps its height so nothing below it moves.
 */
@Composable
internal fun SetupTopBar(
    palette: PortalPalette,
    showBack: Boolean,
    onBack: () -> Unit,
    stepCount: Int,
    stepIndex: Int,
    showSteps: Boolean,
    height: Dp,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier = modifier.height(height),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        AnimatedVisibility(
            visible = showBack,
            enter = fadeIn(tween(320, delayMillis = 120, easing = PortalEmphasizedDecelerate)),
            exit = fadeOut(tween(160, easing = PortalEmphasizedAccelerate)),
        ) {
            SetupBackButton(palette = palette, onClick = onBack)
        }
        Spacer(Modifier.weight(1f))
        AnimatedVisibility(
            visible = showSteps,
            enter = fadeIn(tween(420, delayMillis = 160, easing = PortalEmphasizedDecelerate)),
            exit = fadeOut(tween(200, easing = PortalEmphasizedAccelerate)),
        ) {
            StepDots(count = stepCount, index = stepIndex, palette = palette)
        }
    }
}

/** "‹ Back", quiet until pressed. */
@Composable
internal fun SetupBackButton(palette: PortalPalette, onClick: () -> Unit) {
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val press by animateFloatAsState(if (pressed) 1f else 0f, tween(110), label = "back press")
    Row(
        modifier = Modifier
            .clip(RoundedCornerShape(50))
            .background(palette.textPrimary.copy(alpha = 0.07f * press))
            .clickable(interactionSource = tap, indication = null, role = Role.Button, onClickLabel = "Back", onClick = onClick)
            .padding(start = 4.dp, end = 12.dp, top = 6.dp, bottom = 6.dp)
            .graphicsLayer { alpha = 1f - 0.25f * press },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Canvas(Modifier.size(18.dp)) {
            val stroke = 1.8.dp.toPx()
            drawLine(palette.textSecondary, Offset(size.width * 0.62f, size.height * 0.2f), Offset(size.width * 0.32f, size.height * 0.5f), stroke, StrokeCap.Round)
            drawLine(palette.textSecondary, Offset(size.width * 0.32f, size.height * 0.5f), Offset(size.width * 0.62f, size.height * 0.8f), stroke, StrokeCap.Round)
        }
        Text(text = "Back", fontSize = 14.sp, fontWeight = FontWeight.Medium, color = palette.textSecondary)
    }
}

/**
 * Where setup is: a dot per stage, the current one drawn out into a short
 * bar. The bar travels from dot to dot as the stage changes.
 */
@Composable
internal fun StepDots(count: Int, index: Int, palette: PortalPalette, modifier: Modifier = Modifier) {
    val at by animateFloatAsState(
        targetValue = index.toFloat(),
        animationSpec = spring(dampingRatio = Spring.DampingRatioNoBouncy, stiffness = 260f),
        label = "step dots",
    )
    val dot = 6.dp
    val bar = 20.dp
    val gap = 6.dp
    val width = dot * (count - 1) + bar + gap * (count - 1)
    Canvas(
        modifier
            .size(width, dot)
            .semantics { contentDescription = "Step ${index + 1} of $count" },
    ) {
        val d = dot.toPx()
        val b = bar.toPx()
        val g = gap.toPx()
        var x = 0f
        for (i in 0 until count) {
            // 1 on the current dot, easing to 0 one dot away.
            val near = (1f - kotlin.math.abs(at - i)).coerceIn(0f, 1f)
            val w = d + (b - d) * near
            val done = i < at
            val color = when {
                near > 0f -> lerpColor(palette.textMuted.copy(alpha = 0.45f), palette.accent, near)
                done -> palette.textMuted.copy(alpha = 0.8f)
                else -> palette.textMuted.copy(alpha = 0.32f)
            }
            drawRoundRect(color, topLeft = Offset(x, 0f), size = Size(w, d), cornerRadius = CornerRadius(d / 2f))
            x += w + g
        }
    }
}

private fun lerpColor(a: androidx.compose.ui.graphics.Color, b: androidx.compose.ui.graphics.Color, t: Float) =
    androidx.compose.ui.graphics.lerp(a, b, t)

/**
 * The quiet action under the primary button ("Skip for now", "Back to Main"):
 * text only, with a soft pill when pressed.
 */
@Composable
internal fun SetupLinkButton(
    text: String,
    palette: PortalPalette,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
) {
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val press by animateFloatAsState(if (pressed) 1f else 0f, tween(110), label = "link press")
    Box(
        modifier = modifier
            .clip(RoundedCornerShape(50))
            .background(palette.textPrimary.copy(alpha = 0.07f * press))
            .clickable(interactionSource = tap, indication = null, enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 10.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = text,
            fontSize = 14.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textMuted.copy(alpha = if (enabled) 1f else 0.5f),
        )
    }
}

/** A stage's title and the sentence or two under it. */
@Composable
internal fun StageTitle(
    title: String,
    body: String?,
    palette: PortalPalette,
    metrics: SetupMetrics,
    modifier: Modifier = Modifier,
    centered: Boolean = false,
    hero: Boolean = false,
) {
    androidx.compose.foundation.layout.Column(
        modifier = modifier,
        horizontalAlignment = if (centered) Alignment.CenterHorizontally else Alignment.Start,
    ) {
        Text(
            text = title,
            fontSize = if (hero) metrics.heroTitleSize else metrics.titleSize,
            lineHeight = (if (hero) metrics.heroTitleSize else metrics.titleSize) * 1.18f,
            fontWeight = FontWeight.SemiBold,
            color = palette.textPrimary,
            textAlign = if (centered) androidx.compose.ui.text.style.TextAlign.Center else null,
        )
        if (body != null) {
            Spacer(Modifier.height(if (metrics.compact) 8.dp else 10.dp))
            Text(
                text = body,
                fontSize = metrics.bodySize,
                lineHeight = metrics.bodyLineHeight,
                color = palette.textMuted,
                textAlign = if (centered) androidx.compose.ui.text.style.TextAlign.Center else null,
                // Even lines, so a short last word is never left on its own.
                style = LocalTextStyle.current.copy(lineBreak = LineBreak.Heading),
            )
        }
    }
}

/** Small-caps label over a group of choices. */
@Composable
internal fun SetupSectionLabel(text: String, palette: PortalPalette, modifier: Modifier = Modifier) {
    Text(
        text = text,
        modifier = modifier,
        fontSize = 12.sp,
        fontWeight = FontWeight.SemiBold,
        color = palette.textMuted,
    )
}

/** Holds the space between two blocks of a stage. */
@Composable
internal fun Gap(height: Dp) = Spacer(Modifier.height(height))

/** A dissolve for whatever enters with a stage, bound to that stage's transition. */
@Composable
internal fun Modifier.stageDissolve(scope: androidx.compose.animation.AnimatedVisibilityScope): Modifier =
    dissolveBlur(scope, radius = 10.dp, enterMillis = 620, exitMillis = 300)
