package app.polarbear.setup.components

// One Android setting Portal cannot change itself, asked for before anything
// else because without it Android ends the desktop's processes (an install
// can die at 99%, a finished desktop closes at random). Portal notices the
// setting the moment the user returns from Settings. Skipping stays possible,
// but only behind a plain warning.
//
// The state and the pieces live here; the stage that arranges them is
// PrepareStage in SetupStages.kt, which the launch gate shows too.

import android.content.Context
import android.content.pm.ApplicationInfo
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathMeasure
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.lerp as lerpColor
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.util.lerp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import app.polarbear.setup.PortalPalette
import androidx.compose.runtime.Stable

/** Debug builds only: pretend the setting is off so the stage can be reviewed. */
private const val PREF_FORCE_PREPARE = "force_prepare"

private fun prepareForcedForTesting(context: Context): Boolean =
    (context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE) != 0 &&
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(PREF_FORCE_PREPARE, false)

/**
 * Whether the stage has anything to ask for. A flag Android does not report
 * counts as open until the user has said it is on: only a device reporting
 * "restrictions off" (or a confirmed one) skips the stage.
 */
fun setupPrepareNeeded(context: Context): Boolean =
    prepareForcedForTesting(context) || !prepareSatisfied(context)

private enum class StepPhase { Waiting, Active, Done }

/** Where the setting stands, re-read each time Portal comes back from Settings. */
@Stable
internal class PrepareState(
    val done: Boolean,
    val developerDone: Boolean,
    /**
     * Some ROMs never report the flag. Once the user has been to Settings and
     * Portal still cannot see it, their word has to be enough.
     */
    val offerManual: Boolean,
    val openAbout: () -> Unit,
    val openDeveloper: () -> Unit,
    val confirmManually: () -> Unit,
)

@Composable
internal fun rememberPrepareState(
    // Debug preview: behaves as if the setting were off and unreadable.
    preview: Boolean = false,
): PrepareState {
    val context = LocalContext.current
    val forced = remember { preview || prepareForcedForTesting(context) }
    var resumes by remember { mutableIntStateOf(0) }
    val lifecycleOwner = LocalLifecycleOwner.current
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) resumes++
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }
    val restrictionsOff = remember(resumes) { if (forced) null else childRestrictionsDisabled() }
    val developerOn = remember(resumes) { developerOptionsEnabled(context) }
    var visitedSettings by rememberSaveable { mutableStateOf(false) }
    var manualDone by rememberSaveable { mutableStateOf(false) }
    val done = restrictionsOff == true || manualDone
    return PrepareState(
        done = done,
        developerDone = developerOn || done,
        offerManual = (visitedSettings || preview) && !done && restrictionsOff != false,
        openAbout = {
            visitedSettings = true
            openAboutDevice(context)
        },
        openDeveloper = {
            visitedSettings = true
            openChildProcessSetting(context)
        },
        confirmManually = {
            manualDone = true
            if (!forced) rememberPrepareConfirmed(context)
        },
    )
}

internal fun prepareTitle(done: Boolean): String =
    if (done) "You’re all set" else "One switch keeps your desktop alive"

/**
 * [returning]: an installed desktop is opening rather than a first install;
 * [installing]: the install is already under way.
 */
internal fun prepareBody(done: Boolean, returning: Boolean, installing: Boolean = false): String = when {
    done && returning -> "Portal can keep the whole desktop running. Head back in whenever you’re ready."
    done && installing -> "Portal can keep the whole desktop running. The install carries on from here."
    done -> "Portal can keep the whole desktop running. Next, choose how it looks and what’s included."
    else -> "Android quietly ends apps that run lots of background processes, and a Linux " +
        "desktop runs dozens. Turn on one Developer setting and Portal stays up. " +
        "It takes about a minute, and Portal notices when you’re back."
}

private const val RING_SEGMENTS = 12

/** A segment's opacity as Android culls it: gone in a beat, back a little later. */
private fun cullLife(f: Float): Float = when {
    f < 0.30f -> 1f
    f < 0.42f -> 1f - (f - 0.30f) / 0.12f
    f < 0.78f -> 0f
    else -> (f - 0.78f) / 0.22f
}

/**
 * Twelve process segments surround the desktop while Android culls them one by one;
 * with the setting on they close ranks into a whole, glowing ring and a tick
 * draws itself in the middle.
 */
@Composable
internal fun ProcessRing(
    done: Boolean,
    palette: PortalPalette,
    modifier: Modifier = Modifier,
) {
    val settled by animateFloatAsState(
        targetValue = if (done) 1f else 0f,
        animationSpec = tween(1100, easing = PortalEmphasized),
        label = "ring settled",
    )
    val check by animateFloatAsState(
        targetValue = if (done) 1f else 0f,
        animationSpec = tween(560, delayMillis = 620, easing = PortalEmphasizedDecelerate),
        label = "ring check",
    )
    val clock = rememberInfiniteTransition(label = "ring clock")
    val spin by clock.animateFloat(
        initialValue = 0f,
        targetValue = 360f,
        animationSpec = infiniteRepeatable(tween(28_000, easing = LinearEasing)),
        label = "ring spin",
    )
    val cull by clock.animateFloat(
        initialValue = 0f,
        targetValue = RING_SEGMENTS.toFloat(),
        animationSpec = infiniteRepeatable(tween(RING_SEGMENTS * 1400, easing = LinearEasing)),
        label = "ring cull",
    )
    val description = if (done) "Child process restrictions are off" else "Android is ending background processes"
    Canvas(modifier.semantics { contentDescription = description }) {
        val c = center
        val stroke = size.minDimension * 0.07f
        val radius = size.minDimension / 2f - stroke * 2.2f
        val step = 360f / RING_SEGMENTS
        val victim = cull.toInt() % RING_SEGMENTS
        val f = cull - cull.toInt()

        if (settled > 0.01f) {
            val halo = radius * 1.55f
            drawCircle(
                brush = Brush.radialGradient(
                    colors = listOf(palette.glow.copy(alpha = 0.26f * settled), Color.Transparent),
                    center = c,
                    radius = halo,
                ),
                radius = halo,
                center = c,
            )
        }

        for (i in 0 until RING_SEGMENTS) {
            val life = if (i == victim) cullLife(f) else 1f
            val alive = life + (1f - life) * settled
            val kill = (1f - life) * (1f - settled)
            val sweep = lerp(9f, step + 0.6f, settled)
            val angle = i * step - 90f + spin * (1f - settled)
            val base = lerpColor(palette.textPrimary.copy(alpha = 0.78f), palette.accent, kill)
            val color = lerpColor(base, palette.accent, settled)
            drawArc(
                color = color.copy(alpha = color.alpha * alive),
                startAngle = angle - sweep / 2f,
                sweepAngle = sweep,
                useCenter = false,
                topLeft = Offset(c.x - radius, c.y - radius),
                size = Size(radius * 2f, radius * 2f),
                style = Stroke(stroke, cap = StrokeCap.Round),
            )
        }

        // A monitor outline represents the desktop, with no live indicator.
        val desktop = 1f-settled
        if (desktop > 0.01f) {
            drawRoundRect(palette.textPrimary.copy(alpha = 0.8f*desktop),
                Offset(c.x-radius*0.43f,c.y-radius*0.34f), Size(radius*0.86f,radius*0.56f),
                androidx.compose.ui.geometry.CornerRadius(radius*0.06f), style = Stroke(stroke*0.65f))
            drawLine(palette.textPrimary.copy(alpha = 0.8f*desktop),Offset(c.x,c.y+radius*0.22f),Offset(c.x,c.y+radius*0.40f),stroke*0.65f,StrokeCap.Round)
            drawLine(palette.textPrimary.copy(alpha = 0.8f*desktop),Offset(c.x-radius*0.20f,c.y+radius*0.40f),Offset(c.x+radius*0.20f,c.y+radius*0.40f),stroke*0.65f,StrokeCap.Round)
        }
        if (check > 0.01f) {
            val tick = Path().apply {
                moveTo(c.x - radius * 0.34f, c.y + radius * 0.02f)
                lineTo(c.x - radius * 0.10f, c.y + radius * 0.27f)
                lineTo(c.x + radius * 0.36f, c.y - radius * 0.24f)
            }
            val measure = PathMeasure().apply { setPath(tick, false) }
            val shown = Path()
            measure.getSegment(0f, measure.length * check, shown, true)
            drawPath(
                path = shown,
                color = palette.textPrimary,
                style = Stroke(stroke * 0.85f, cap = StrokeCap.Round, join = StrokeJoin.Round),
            )
        }
    }
}

/** The two steps, each opening the Settings page it needs. */
@Composable
internal fun PrepareSteps(
    state: PrepareState,
    palette: PortalPalette,
    modifier: Modifier = Modifier,
) {
    val developerDone = state.developerDone
    val done = state.done
    Column(modifier = modifier) {
        PrepareStep(
            number = 1,
            title = "Turn on Developer options",
            body = "Open About device and tap Build number 7 times.",
            phase = if (developerDone) StepPhase.Done else StepPhase.Active,
            isLast = false,
            palette = palette,
        ) {
            ChecklistButton("Open About device", emphasized = true, onClick = state.openAbout, palette = palette)
        }
        PrepareStep(
            number = 2,
            title = "Disable child process restrictions",
            body = "In Developer options, switch on “Disable child process restrictions”.",
            waitingHint = "Unlocks after step 1",
            phase = when {
                done -> StepPhase.Done
                developerDone -> StepPhase.Active
                else -> StepPhase.Waiting
            },
            isLast = true,
            palette = palette,
        ) {
            // Stacked, not a wrapping row: the stepper sizes itself by intrinsic
            // height, which a wrapping row cannot report.
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                ChecklistButton("Open Developer options", emphasized = true, onClick = state.openDeveloper, palette = palette)
                if (state.offerManual) {
                    ChecklistButton("I’ve turned it on", emphasized = false, onClick = state.confirmManually, palette = palette)
                }
            }
        }
    }
}

@Composable
private fun PrepareStep(
    number: Int,
    title: String,
    body: String,
    phase: StepPhase,
    isLast: Boolean,
    palette: PortalPalette,
    waitingHint: String? = null,
    actions: @Composable () -> Unit,
) {
    val railFill by animateFloatAsState(
        targetValue = if (phase == StepPhase.Done) 1f else 0f,
        animationSpec = tween(700, delayMillis = 300, easing = PortalEmphasized),
        label = "step rail",
    )
    Row(modifier = Modifier.height(IntrinsicSize.Min)) {
        Column(
            modifier = Modifier.width(26.dp).fillMaxHeight(),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            StepBadge(number = number, phase = phase, palette = palette)
            if (!isLast) {
                Box(
                    modifier = Modifier
                        .padding(vertical = 5.dp)
                        .width(2.dp)
                        .weight(1f)
                        .clip(CircleShape)
                        .background(palette.surfaceBorder),
                ) {
                    Box(
                        Modifier
                            .fillMaxWidth()
                            .fillMaxHeight(railFill)
                            .background(palette.accent),
                    )
                }
            }
        }
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f).padding(bottom = if (isLast) 0.dp else 14.dp)) {
            Text(
                text = title,
                modifier = Modifier.padding(top = 3.dp),
                fontSize = 14.sp,
                fontWeight = FontWeight.SemiBold,
                color = if (phase == StepPhase.Waiting) palette.textMuted else palette.textPrimary,
            )
            AnimatedVisibility(
                visible = phase == StepPhase.Active,
                enter = fadeIn(tween(320, delayMillis = 140)) +
                    expandVertically(tween(440, easing = PortalEmphasized)),
                exit = fadeOut(tween(160)) + shrinkVertically(tween(320, easing = PortalEmphasized)),
            ) {
                Column {
                    Spacer(Modifier.height(2.dp))
                    Text(text = body, fontSize = 12.sp, lineHeight = 16.sp, color = palette.textMuted)
                    Spacer(Modifier.height(10.dp))
                    actions()
                }
            }
            AnimatedVisibility(
                visible = phase == StepPhase.Waiting && waitingHint != null,
                enter = fadeIn(tween(240)),
                exit = fadeOut(tween(140)),
            ) {
                Text(text = waitingHint.orEmpty(), fontSize = 12.sp, color = palette.textMuted)
            }
            AnimatedVisibility(
                visible = phase == StepPhase.Done,
                enter = fadeIn(tween(320, delayMillis = 240)) +
                    expandVertically(tween(320, delayMillis = 240, easing = PortalEmphasized)),
                exit = fadeOut(tween(140)),
            ) {
                Text(
                    text = "Done",
                    fontSize = 12.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = palette.accent,
                )
            }
        }
    }
}

@Composable
private fun StepBadge(number: Int, phase: StepPhase, palette: PortalPalette) {
    val done = phase == StepPhase.Done
    val active = phase == StepPhase.Active
    val fill by animateFloatAsState(
        targetValue = if (done) 1f else 0f,
        animationSpec = tween(420, easing = PortalEmphasized),
        label = "badge fill",
    )
    val tick by animateFloatAsState(
        targetValue = if (done) 1f else 0f,
        animationSpec = tween(420, delayMillis = 160, easing = PortalEmphasizedDecelerate),
        label = "badge tick",
    )
    val ping by rememberInfiniteTransition(label = "badge ping").animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(1900, easing = PortalEmphasized)),
        label = "badge ping",
    )
    Box(Modifier.size(26.dp), contentAlignment = Alignment.Center) {
        if (active) {
            Canvas(Modifier.size(26.dp)) {
                drawCircle(
                    color = palette.accent.copy(alpha = (1f - ping) * 0.5f),
                    radius = size.minDimension / 2f * (1f + 0.65f * ping),
                    style = Stroke(1.5.dp.toPx()),
                )
            }
        }
        Box(
            modifier = Modifier
                .size(26.dp)
                .clip(CircleShape)
                .background(palette.accent.copy(alpha = fill))
                .border(
                    1.dp,
                    if (done || active) palette.accent else palette.surfaceBorder,
                    CircleShape,
                ),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                text = number.toString(),
                modifier = Modifier.graphicsLayer { alpha = 1f - tick },
                fontSize = 13.sp,
                fontWeight = FontWeight.SemiBold,
                color = if (active) palette.textPrimary else palette.textMuted,
            )
            Canvas(Modifier.size(12.dp)) {
                if (tick <= 0.01f) return@Canvas
                val path = Path().apply {
                    moveTo(size.width * 0.10f, size.height * 0.55f)
                    lineTo(size.width * 0.40f, size.height * 0.84f)
                    lineTo(size.width * 0.92f, size.height * 0.18f)
                }
                val measure = PathMeasure().apply { setPath(path, false) }
                val shown = Path()
                measure.getSegment(0f, measure.length * tick, shown, true)
                drawPath(
                    path = shown,
                    color = palette.buttonInterior,
                    style = Stroke(2.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round),
                )
            }
        }
    }
}

/** What skipping costs, with the way back first. */
@Composable
internal fun PrepareSkipWarning(
    palette: PortalPalette,
    returning: Boolean,
    modifier: Modifier = Modifier,
    onBack: () -> Unit,
    onSkip: () -> Unit,
) {
    val shape = RoundedCornerShape(20.dp)
    Column(
        modifier = modifier
            .fillMaxWidth()
            .clip(shape)
            .background(palette.accent.copy(alpha = 0.08f))
            .border(1.dp, palette.accent.copy(alpha = 0.40f), shape)
            .padding(horizontal = 18.dp, vertical = 16.dp),
    ) {
        Text(
            text = if (returning) "Your desktop can close at random" else "Setup can stop at 99%",
            fontSize = 14.sp,
            fontWeight = FontWeight.SemiBold,
            color = palette.textPrimary,
        )
        Spacer(Modifier.height(4.dp))
        Text(
            text = if (returning) {
                "Without this setting Android may end the desktop’s processes, closing apps or the " +
                    "whole desktop mid-use. You can turn it on later; Portal will ask again next time."
            } else {
                "Without this setting Android may end the desktop’s processes while Portal installs, " +
                    "and a finished desktop can close at random. You can turn it on later, but you may " +
                    "have to retry the install."
            },
            fontSize = 12.sp,
            lineHeight = 16.sp,
            color = palette.textMuted,
        )
        Spacer(Modifier.height(12.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            ChecklistButton("Go back", emphasized = true, onClick = onBack, palette = palette)
            ChecklistButton("Skip anyway", emphasized = false, onClick = onSkip, palette = palette)
        }
    }
}
