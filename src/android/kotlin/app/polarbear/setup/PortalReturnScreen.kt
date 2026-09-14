package app.polarbear.setup

// Return-to-Plasma screen for already-installed launches (cold start after
// force-close included). This is intentionally minimal: the EXACT shared
// PortalAmbientBackground (same charcoal, same seven blurred fragments, same
// drift, same 720 ms scatter, same 1.0 -> 0.83 alpha), the official Portal
// mark, and two lines of text. A quiet inline graphics affordance is added
// only when native state says an explicit Anland repair is available; it is
// part of this layout rather than a separate control.
//
// READY uses the SAME path as the installer: a simple 3 second timer,
// started once this screen is presented, sets the shared readyPrelude, which
// drives the identical fragment scatter + translucency + PortalRevealVeil +
// swipe affordance through PortalLaunchTransition. No second animation
// implementation anywhere.

import android.util.Log
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.animateDp
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.core.updateTransition
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.ComposeOverlay
import kotlinx.coroutines.delay

private const val TAG = "PortalReturn"
private const val RETURN_READY_DELAY_MS = 3_000L

private enum class ReturnRepairPhase {
    Hidden,
    Available,
    Running,
    Failed,
    RecoveryRequested,
}

@Composable
private fun ReturnAnlandAffordance(
    phase: ReturnRepairPhase,
    progress: Int,
    palette: PortalPalette,
    onEnable: () -> Unit,
    onRecovery: () -> Unit,
) {
    val visible = phase != ReturnRepairPhase.Hidden
    val phaseTransition = updateTransition(
        targetState = phase,
        label = "return graphics affordance",
    )
    val contentAlpha by phaseTransition.animateFloat(
        transitionSpec = { tween(260) },
        label = "return graphics opacity",
    ) { target ->
        if (target == ReturnRepairPhase.RecoveryRequested) 0.78f else 1f
    }
    val contentLift by phaseTransition.animateDp(
        transitionSpec = {
            spring(
                dampingRatio = Spring.DampingRatioNoBouncy,
                stiffness = 520f,
            )
        },
        label = "return graphics movement",
    ) { target ->
        if (target == ReturnRepairPhase.Available) 0.dp else (-1).dp
    }
    val glowAlpha by phaseTransition.animateFloat(
        transitionSpec = { tween(380, easing = FastOutSlowInEasing) },
        label = "return graphics glow",
    ) { target ->
        when (target) {
            ReturnRepairPhase.Available -> 0.14f
            ReturnRepairPhase.Running -> 0.42f
            ReturnRepairPhase.Failed -> 0.08f
            else -> 0f
        }
    }
    val tap = remember { MutableInteractionSource() }
    val pressed by tap.collectIsPressedAsState()
    val interactive = phase == ReturnRepairPhase.Available || phase == ReturnRepairPhase.Failed
    val pressScale by animateFloatAsState(
        targetValue = if (interactive && pressed) 0.985f else 1f,
        animationSpec = tween(110),
        label = "return graphics press",
    )

    AnimatedVisibility(
        visible = visible,
        enter = fadeIn(tween(320, delayMillis = 35)) +
            slideInVertically(tween(380, delayMillis = 20)) { it / 5 } +
            scaleIn(tween(380, delayMillis = 20), initialScale = 0.985f),
        exit = fadeOut(tween(220)) +
            slideOutVertically(tween(260)) { -it / 6 } +
            scaleOut(tween(260), targetScale = 0.99f),
    ) {
        AnimatedContent(
            targetState = phase,
            transitionSpec = {
                (fadeIn(tween(260, delayMillis = 35)) +
                    slideInVertically(tween(320)) { it / 5 } +
                    scaleIn(tween(320), initialScale = 0.99f))
                    .togetherWith(
                        fadeOut(tween(170)) +
                            slideOutVertically(tween(220)) { -it / 6 } +
                            scaleOut(tween(220), targetScale = 0.995f),
                    )
                    .using(
                        SizeTransform(clip = false) { _, _ ->
                            tween(360, easing = FastOutSlowInEasing)
                        },
                    )
            },
            contentAlignment = Alignment.Center,
            label = "return graphics state",
        ) { target ->
            val click = when (target) {
                ReturnRepairPhase.Available -> onEnable
                ReturnRepairPhase.Failed -> onRecovery
                else -> ({ })
            }
            Column(
                modifier = Modifier
                    .animateContentSize(
                        animationSpec = tween(360, easing = FastOutSlowInEasing),
                    )
                    .graphicsLayer {
                        alpha = contentAlpha
                        translationY = contentLift.toPx()
                        scaleX = pressScale
                        scaleY = pressScale
                    }
                    .padding(top = 18.dp, start = 10.dp, end = 10.dp, bottom = 2.dp)
                    .then(
                        if (interactive) {
                            Modifier.clickable(
                                interactionSource = tap,
                                indication = null,
                                role = Role.Button,
                                onClick = click,
                            )
                        } else {
                            Modifier
                        },
                    ),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                when (target) {
                    ReturnRepairPhase.Available -> Text(
                        text = "Accelerated graphics available →",
                        fontSize = 12.sp,
                        fontWeight = FontWeight.Medium,
                        color = palette.textMuted.copy(alpha = 0.88f),
                        textAlign = TextAlign.Center,
                    )

                    ReturnRepairPhase.Running -> Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        Text(
                            text = "Enabling accelerated graphics…",
                            fontSize = 14.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textPrimary,
                            textAlign = TextAlign.Center,
                        )
                        Spacer(modifier = Modifier.height(5.dp))
                        Text(
                            text = "Preparing Anland and GPU acceleration",
                            fontSize = 11.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textMuted,
                            textAlign = TextAlign.Center,
                        )
                        ReturnRepairProgress(
                            progress = progress,
                            palette = palette,
                            glowAlpha = glowAlpha,
                        )
                    }

                    ReturnRepairPhase.Failed -> Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        Text(
                            text = "Acceleration could not be enabled",
                            fontSize = 14.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textPrimary,
                            textAlign = TextAlign.Center,
                        )
                        Spacer(modifier = Modifier.height(5.dp))
                        Text(
                            text = "Your desktop is unchanged",
                            fontSize = 11.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textMuted,
                            textAlign = TextAlign.Center,
                        )
                        Spacer(modifier = Modifier.height(10.dp))
                        Text(
                            text = "Retry Plasma →",
                            fontSize = 12.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textMuted.copy(alpha = 0.9f),
                            textAlign = TextAlign.Center,
                        )
                    }

                    ReturnRepairPhase.RecoveryRequested -> Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        Text(
                            text = "Returning to Plasma…",
                            fontSize = 14.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textPrimary,
                            textAlign = TextAlign.Center,
                        )
                        Spacer(modifier = Modifier.height(5.dp))
                        Text(
                            text = "Keeping your desktop safe",
                            fontSize = 11.sp,
                            fontWeight = FontWeight.Medium,
                            color = palette.textMuted,
                            textAlign = TextAlign.Center,
                        )
                    }

                    ReturnRepairPhase.Hidden -> Spacer(modifier = Modifier.size(0.dp))
                }
            }
        }
    }
}

@Composable
private fun ReturnRepairProgress(
    progress: Int,
    palette: PortalPalette,
    glowAlpha: Float,
) {
    val target = (progress.coerceIn(0, 99) / 100f).coerceIn(0f, 0.99f)
    val animatedProgress by animateFloatAsState(
        targetValue = target,
        animationSpec = tween(360, easing = FastOutSlowInEasing),
        label = "native graphics progress",
    )
    Canvas(
        modifier = Modifier
            .padding(top = 14.dp)
            .size(width = 112.dp, height = 4.dp),
    ) {
        val radius = size.height / 2f
        drawRoundRect(
            color = palette.textPrimary.copy(alpha = 0.12f),
            cornerRadius = androidx.compose.ui.geometry.CornerRadius(radius, radius),
        )
        val progressWidth = size.width * animatedProgress
        if (progressWidth > 0f) {
            drawRoundRect(
                color = palette.accent.copy(alpha = glowAlpha),
                topLeft = androidx.compose.ui.geometry.Offset.Zero,
                size = androidx.compose.ui.geometry.Size(progressWidth, size.height),
                cornerRadius = androidx.compose.ui.geometry.CornerRadius(radius, radius),
                style = Stroke(width = 3.dp.toPx()),
            )
            drawRoundRect(
                color = palette.accent.copy(alpha = 0.84f),
                topLeft = androidx.compose.ui.geometry.Offset.Zero,
                size = androidx.compose.ui.geometry.Size(progressWidth, size.height),
                cornerRadius = androidx.compose.ui.geometry.CornerRadius(radius, radius),
            )
        }
    }
}

@Composable
internal fun PortalReturnScreen(
    launchMarkModifier: Modifier,
    onReturnReady: () -> Unit = {},
    onRepairBlockedChanged: (Boolean) -> Unit = {},
) {
    val palette = resolvePalette(AppearanceMode.System)
    var returnReady by remember { mutableStateOf(false) }
    var repairRequested by remember { mutableStateOf(false) }
    var recoveryRequested by remember { mutableStateOf(false) }
    val repairState by ComposeOverlay.anlandRepairState()
    val currentReturnReady by rememberUpdatedState(onReturnReady)
    val currentRepairBlocked by rememberUpdatedState(onRepairBlockedChanged)
    val repairPhase = when {
        recoveryRequested -> ReturnRepairPhase.RecoveryRequested
        repairState.failed -> ReturnRepairPhase.Failed
        repairState.complete -> ReturnRepairPhase.Hidden
        repairState.running -> ReturnRepairPhase.Running
        repairRequested -> ReturnRepairPhase.Running
        repairState.available -> ReturnRepairPhase.Available
        else -> ReturnRepairPhase.Hidden
    }
    LaunchedEffect(repairPhase) {
        currentRepairBlocked(
            repairPhase == ReturnRepairPhase.Running ||
                repairPhase == ReturnRepairPhase.Failed ||
                repairPhase == ReturnRepairPhase.RecoveryRequested,
        )
    }
    LaunchedEffect(Unit) {
        delay(RETURN_READY_DELAY_MS)
        returnReady = true
        Log.i(TAG, "return READY; starting ambient scatter and translucent veil prelude")
        currentReturnReady()
    }
    Box(modifier = Modifier.fillMaxSize()) {
        PortalAmbientBackground(
            palette = palette,
            cardBounds = { Rect.Zero },
            readyPrelude = returnReady,
        )
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(horizontal = 32.dp),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Column(
                modifier = Modifier
                    .animateContentSize(
                        animationSpec = tween(420, easing = FastOutSlowInEasing),
                    ),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Image(
                    painter = portalMarkPainter(
                        main = palette.logoMain,
                        threshold = palette.logoThreshold,
                    ),
                    contentDescription = "Portal logo",
                    modifier = Modifier.size(PortalDimens.LogoSize).then(launchMarkModifier),
                )
                Spacer(modifier = Modifier.height(20.dp))
                Text(
                    text = "Returning to Plasma",
                    fontSize = PortalDimens.TitleSize,
                    fontWeight = FontWeight.SemiBold,
                    color = palette.textPrimary,
                    textAlign = TextAlign.Center,
                )
                Spacer(modifier = Modifier.height(8.dp))
                Text(
                    text = "Restoring your desktop",
                    fontSize = 14.sp,
                    fontWeight = FontWeight.Medium,
                    color = palette.textMuted,
                    textAlign = TextAlign.Center,
                )
                ReturnAnlandAffordance(
                    phase = repairPhase,
                    progress = repairState.progress,
                    palette = palette,
                    onEnable = {
                        if (ComposeOverlay.repairEnableAnland()) {
                            repairRequested = true
                        }
                    },
                    onRecovery = {
                        if (ComposeOverlay.requestAnlandRepairRecovery()) {
                            recoveryRequested = true
                        }
                    },
                )
            }
        }
    }
}
