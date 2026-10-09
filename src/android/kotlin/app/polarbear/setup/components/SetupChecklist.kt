package app.polarbear.setup.components

// Android settings Portal cannot change itself, offered while the install
// runs (the user is waiting anyway). Each step opens the right Settings
// screen; the child-process step ticks itself off once Android reports the
// restriction disabled, the Games step (OPPO-family brands) on the user's word.
// A ticked-off step folds away after a beat, and the card with its last step.

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.os.Build
import android.provider.Settings
import android.util.Log
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import app.polarbear.setup.PortalPalette
import kotlinx.coroutines.delay

private const val TAG = "PortalSetupChecklist"
internal const val PREFS = "portal_setup_checklist"
private const val PREF_GAMES_DONE = "games_added"

/** The user said the setting is on, on a device that never reports it. */
private const val PREF_PREPARE_CONFIRMED = "prepare_confirmed"

/** The Games app behind game handling on OxygenOS, ColorOS and realme UI. */
private const val OPLUS_GAMES_PACKAGE = "com.oplus.games"

/**
 * Android 14 added "Disable child process restrictions" to Developer options.
 * It persists as this feature-flag override; `false` means Android no longer
 * kills an app's child processes past its phantom-process limit (32 across
 * all apps), which otherwise takes down the desktop's processes at random.
 */
private const val PHANTOM_MONITOR_PROPERTY =
    "persist.sys.fflag.override.settings_enable_monitor_phantom_procs"

/** Settings' preference key for the toggle; Settings highlights it when passed. */
private const val PHANTOM_MONITOR_PREFERENCE_KEY = "disable_phantom_process_monitor"

internal enum class StepState { Todo, Done }

/**
 * Whether the checklist has anything to offer on this device. The child
 * process step is the onboarding gate's job: it only reappears here for a
 * user who chose to skip the gate. A step already done is not offered again.
 */
fun setupChecklistApplies(context: Context): Boolean = setupChecklistPending(context)

/** Whether a step is still open, for surfaces shown on every launch. */
fun setupChecklistPending(context: Context): Boolean =
    (childProcessStepApplies() && !prepareSatisfied(context)) ||
        (gamesStepApplies() &&
            !context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(PREF_GAMES_DONE, false))

/**
 * Whether the Android setting the desktop depends on is known to be on. Android
 * reports it on most devices; where it does not (the flag stays unset), the
 * user's own confirmation from the onboarding stands in. A device reporting
 * the restriction as active is never satisfied, whatever was confirmed before.
 */
fun prepareSatisfied(context: Context): Boolean {
    if (!childProcessStepApplies()) return true
    return when (childRestrictionsDisabled()) {
        true -> true
        false -> false
        null -> context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getBoolean(PREF_PREPARE_CONFIRMED, false)
    }
}

internal fun rememberPrepareConfirmed(context: Context) {
    context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        .edit().putBoolean(PREF_PREPARE_CONFIRMED, true).apply()
}

internal fun childProcessStepApplies(): Boolean =
    Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE

/**
 * OPPO-family brands share one game stack (com.oplus.games) across OxygenOS,
 * ColorOS and realme UI. Nothing tells the ROMs apart reliably (they report
 * the same ro.build.version.oplusrom, and OnePlus ships ColorOS in China), so
 * the step names the brand, never the ROM.
 */
private val OPLUS_BRANDS = mapOf("oneplus" to "OnePlus", "oppo" to "OPPO", "realme" to "realme")

private fun oplusBrand(): String? = OPLUS_BRANDS[Build.MANUFACTURER.lowercase()]

private fun gamesStepApplies(): Boolean = oplusBrand() != null

/**
 * `true` once the restriction is off, `false` while it applies, `null` when
 * Android does not expose the flag to apps (the step then stays a plain
 * instruction).
 */
internal fun childRestrictionsDisabled(): Boolean? {
    val value = runCatching {
        Class.forName("android.os.SystemProperties")
            .getMethod("get", String::class.java)
            .invoke(null, PHANTOM_MONITOR_PROPERTY) as String
    }.getOrNull()
    return when (value) {
        "false" -> true
        "true" -> false
        else -> null
    }
}

internal fun developerOptionsEnabled(context: Context): Boolean =
    Settings.Global.getInt(
        context.contentResolver,
        Settings.Global.DEVELOPMENT_SETTINGS_ENABLED,
        0,
    ) != 0

internal fun launch(context: Context, intent: Intent): Boolean = try {
    context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    true
} catch (error: ActivityNotFoundException) {
    Log.w(TAG, "no activity for $intent", error)
    false
} catch (error: SecurityException) {
    Log.w(TAG, "not allowed to open $intent", error)
    false
}

internal fun openChildProcessSetting(context: Context) {
    if (developerOptionsEnabled(context)) {
        val developer = Intent(Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS)
            .putExtra(":settings:fragment_args_key", PHANTOM_MONITOR_PREFERENCE_KEY)
        if (launch(context, developer)) return
    }
    if (!launch(context, Intent(Settings.ACTION_DEVICE_INFO_SETTINGS))) {
        launch(context, Intent(Settings.ACTION_SETTINGS))
    }
}

internal fun openAboutDevice(context: Context) {
    if (!launch(context, Intent(Settings.ACTION_DEVICE_INFO_SETTINGS))) {
        launch(context, Intent(Settings.ACTION_SETTINGS))
    }
}

private fun openGames(context: Context) {
    val games = Intent(Intent.ACTION_MAIN)
        .addCategory(Intent.CATEGORY_LAUNCHER)
        .setPackage(OPLUS_GAMES_PACKAGE)
    if (!launch(context, games)) {
        launch(context, Intent(Settings.ACTION_SETTINGS))
    }
}

@Composable
fun SetupChecklist(
    palette: PortalPalette,
    modifier: Modifier = Modifier,
    title: String = "While Portal installs",
) {
    val context = LocalContext.current
    val prefs = remember { context.getSharedPreferences(PREFS, Context.MODE_PRIVATE) }
    // Re-read Android's state whenever Portal comes back from Settings.
    var resumes by remember { mutableIntStateOf(0) }
    val lifecycleOwner = LocalLifecycleOwner.current
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) resumes++
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }
    val restrictionsOff = remember(resumes) { childRestrictionsDisabled() }
    val developerOn = remember(resumes) { developerOptionsEnabled(context) }
    var gamesDone by remember { mutableStateOf(prefs.getBoolean(PREF_GAMES_DONE, false)) }
    // Decided once: only steps still open when the card appears are offered.
    val childStep = remember { childProcessStepApplies() && !prepareSatisfied(context) }
    val gamesStep = remember { gamesStepApplies() && !gamesDone }
    val childDone = restrictionsOff == true
    // A ticked-off step shows its tick for a beat, then folds away; the card
    // folds with the last one.
    val childGone = rememberSettled(childDone)
    val gamesGone = rememberSettled(gamesDone)
    val childShown = childStep && !childGone
    val gamesShown = gamesStep && !gamesGone

    val shape = RoundedCornerShape(24.dp)
    AnimatedVisibility(
        visible = childShown || gamesShown,
        enter = EnterTransition.None,
        exit = fadeOut(tween(260, easing = PortalEmphasizedAccelerate)) +
            shrinkVertically(tween(480, easing = PortalEmphasized), shrinkTowards = Alignment.Top),
        label = "checklist card",
    ) {
        Column(
            modifier = modifier
                .clip(shape)
                .background(palette.trackFill)
                .border(1.dp, palette.surfaceBorder, shape)
                .padding(horizontal = 24.dp, vertical = 20.dp),
        ) {
            Text(
                text = title,
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
                color = palette.textPrimary,
            )
            Spacer(Modifier.height(2.dp))
            Text(
                text = "Android settings that keep the desktop running smoothly. Portal can’t change them for you.",
                fontSize = 12.sp,
                lineHeight = 16.sp,
                color = palette.textMuted,
            )
            // While the card itself folds, its last step stays inside it
            // rather than collapsing twice.
            val cardLeaving = !childShown && !gamesShown
            CollapsingStep(visible = childStep && (!childGone || cardLeaving)) {
                ChecklistStep(
                    number = 1,
                    title = "Keep desktop apps running",
                    body = if (developerOn || childDone) {
                        "Android stops apps that start many processes. In Developer options, turn on " +
                            "“Disable child process restrictions”."
                    } else {
                        "Android stops apps that start many processes. Turn on Developer options first " +
                            "(About device › tap Build number 7 times), then turn on " +
                            "“Disable child process restrictions”."
                    },
                    state = if (childDone) StepState.Done else StepState.Todo,
                    actionLabel = if (developerOn) "Open Developer options" else "Open About device",
                    onAction = { openChildProcessSetting(context) },
                    palette = palette,
                )
            }
            CollapsingStep(visible = gamesStep && (!gamesGone || cardLeaving)) {
                ChecklistStep(
                    number = if (childShown) 2 else 1,
                    title = "Add Portal to Games",
                    body = "${oplusBrand()} devices lower the refresh rate for apps they don’t treat as games. " +
                        "In the Games app, add Portal to your games.",
                    state = if (gamesDone) StepState.Done else StepState.Todo,
                    actionLabel = "Open Games",
                    onAction = { openGames(context) },
                    secondaryLabel = "I’ve added it",
                    onSecondary = {
                        gamesDone = true
                        prefs.edit().putBoolean(PREF_GAMES_DONE, true).apply()
                    },
                    palette = palette,
                )
            }
        }
    }
}

/** How long a ticked-off step shows "Done" before it folds away. */
private const val DONE_HOLD_MS = 1_100L

/** True once [done] has held for [DONE_HOLD_MS]; never goes back. */
@Composable
private fun rememberSettled(done: Boolean): Boolean {
    var settled by remember { mutableStateOf(false) }
    LaunchedEffect(done) {
        if (done) {
            delay(DONE_HOLD_MS)
            settled = true
        }
    }
    return settled
}

/** A step with its top gap, folding up and fading when it leaves. */
@Composable
private fun CollapsingStep(visible: Boolean, content: @Composable () -> Unit) {
    AnimatedVisibility(
        visible = visible,
        enter = EnterTransition.None,
        exit = fadeOut(tween(220, easing = PortalEmphasizedAccelerate)) +
            shrinkVertically(tween(440, easing = PortalEmphasized), shrinkTowards = Alignment.Top),
        label = "checklist step",
    ) {
        Column {
            Spacer(Modifier.height(16.dp))
            content()
        }
    }
}

@Composable
internal fun ChecklistStep(
    number: Int,
    title: String,
    body: String,
    state: StepState,
    actionLabel: String,
    onAction: () -> Unit,
    palette: PortalPalette,
    secondaryLabel: String? = null,
    onSecondary: () -> Unit = {},
) {
    Row(verticalAlignment = Alignment.Top) {
        Box(
            modifier = Modifier
                .size(26.dp)
                .clip(CircleShape)
                .background(if (state == StepState.Done) palette.accent else palette.trackFill)
                .border(
                    1.dp,
                    if (state == StepState.Done) palette.accent else palette.surfaceBorder,
                    CircleShape,
                ),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                text = if (state == StepState.Done) "✓" else number.toString(),
                fontSize = 13.sp,
                fontWeight = FontWeight.SemiBold,
                color = if (state == StepState.Done) palette.buttonInterior else palette.textSecondary,
            )
        }
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Text(
                text = title,
                fontSize = 14.sp,
                fontWeight = FontWeight.SemiBold,
                color = palette.textPrimary,
            )
            Spacer(Modifier.height(2.dp))
            Text(
                text = body,
                fontSize = 12.sp,
                lineHeight = 16.sp,
                color = palette.textMuted,
            )
            Spacer(Modifier.height(10.dp))
            AnimatedContent(
                targetState = state,
                transitionSpec = { fadeIn(tween(240)) togetherWith fadeOut(tween(160)) },
                label = "checklist step state",
            ) { shown ->
                if (shown == StepState.Done) {
                    Box(Modifier.heightIn(min = 34.dp), contentAlignment = Alignment.CenterStart) {
                        Text(
                            text = "Done",
                            fontSize = 13.sp,
                            fontWeight = FontWeight.SemiBold,
                            color = palette.accent,
                        )
                    }
                } else {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        ChecklistButton(actionLabel, emphasized = true, onClick = onAction, palette = palette)
                        if (secondaryLabel != null) {
                            ChecklistButton(secondaryLabel, emphasized = false, onClick = onSecondary, palette = palette)
                        }
                    }
                }
            }
        }
    }
}

@Composable
internal fun ChecklistButton(
    label: String,
    emphasized: Boolean,
    onClick: () -> Unit,
    palette: PortalPalette,
) {
    val shape = RoundedCornerShape(17.dp)
    Box(
        modifier = Modifier
            .heightIn(min = 34.dp)
            .clip(shape)
            .background(if (emphasized) palette.buttonInterior else palette.trackFill)
            .border(1.dp, if (emphasized) palette.buttonOutline else palette.surfaceBorder, shape)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 8.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = label,
            fontSize = 13.sp,
            fontWeight = FontWeight.Medium,
            color = if (emphasized) app.polarbear.setup.PortalColors.Ivory else palette.textSecondary,
        )
    }
}
