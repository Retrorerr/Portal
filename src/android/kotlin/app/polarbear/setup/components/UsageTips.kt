package app.polarbear.setup.components

// Optional Android settings that make the desktop nicer, shown on the return
// screen only while a setting is off. Each tip ticks itself off when Android
// reports the setting on, and "Not now" hides it for good.

import android.app.ActivityManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.Settings
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.EnterTransition
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import app.polarbear.setup.PortalPalette

private const val PREF_KEYS_DISMISSED = "tip_keyboard_service_dismissed"
private const val PREF_CONTROLS_DISMISSED = "tip_portal_roundtable_dismissed"
private const val PREF_FILES_DISMISSED = "tip_all_files_dismissed"

private enum class KeyServiceState { Off, NotRunning, On }

private fun keyServiceComponent(context: Context) =
    ComponentName(context.packageName, "app.polarbear.KeyboardAccessibilityService")

/**
 * The settings list says what the user switched on; only Portal's own running
 * services say whether Android actually bound it (it stops binding a service
 * that keeps crashing). AccessibilityManager's enabled list can't tell the
 * two apart.
 */
private fun keyServiceState(context: Context): KeyServiceState {
    val component = keyServiceComponent(context)
    @Suppress("DEPRECATION") // Still reports the caller's own services.
    val running = context.getSystemService(ActivityManager::class.java)
        ?.getRunningServices(Int.MAX_VALUE)
        ?.any { it.service == component && it.clientCount > 0 }
        ?: false
    if (running) return KeyServiceState.On
    val enabled = Settings.Secure.getString(
        context.contentResolver,
        Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,
    )?.split(':')?.any { ComponentName.unflattenFromString(it) == component } ?: false
    return if (enabled) KeyServiceState.NotRunning else KeyServiceState.Off
}

/** All files access exists from Android 11; older devices have no such tip. */
private fun allFilesGranted(): Boolean =
    Build.VERSION.SDK_INT < Build.VERSION_CODES.R || Environment.isExternalStorageManager()

/** Whether any tip still has something to offer, for the return screen. */
fun usageTipsPending(context: Context): Boolean {
    val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    return !prefs.getBoolean(PREF_CONTROLS_DISMISSED, false) || (!prefs.getBoolean(PREF_KEYS_DISMISSED, false) && keyServiceState(context) != KeyServiceState.On) ||
        (!prefs.getBoolean(PREF_FILES_DISMISSED, false) && !allFilesGranted())
}

private fun openAccessibilitySettings(context: Context) {
    // The details page (hidden in the SDK, but Settings lets an app open it
    // for its own service) shows Portal's switch directly; otherwise fall
    // back to the accessibility list.
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        val details = Intent("android.settings.ACCESSIBILITY_DETAILS_SETTINGS")
            .putExtra(Intent.EXTRA_COMPONENT_NAME, keyServiceComponent(context).flattenToString())
        if (launch(context, details)) return
    }
    launch(context, Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS))
}

private fun openAllFilesSetting(context: Context) {
    val own = Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION)
        .setData(Uri.parse("package:${context.packageName}"))
    if (!launch(context, own)) {
        launch(context, Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION))
    }
}

@Composable
fun UsageTips(
    palette: PortalPalette,
    modifier: Modifier = Modifier,
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
    val keys = remember(resumes) { keyServiceState(context) }
    val filesGranted = remember(resumes) { allFilesGranted() }
    var keysDismissed by remember { mutableStateOf(prefs.getBoolean(PREF_KEYS_DISMISSED, false)) }
    var controlsDismissed by remember { mutableStateOf(prefs.getBoolean(PREF_CONTROLS_DISMISSED, false)) }
    var filesDismissed by remember { mutableStateOf(prefs.getBoolean(PREF_FILES_DISMISSED, false)) }
    // Decided once: a tip completed while the card is up stays, showing done.
    val keysTip = remember { keyServiceState(context) != KeyServiceState.On }
    val filesTip = remember { !allFilesGranted() }
    val controlsShown = !controlsDismissed
    val keysShown = keysTip && !keysDismissed
    val filesShown = filesTip && !filesDismissed
    // While the card itself folds, its last tip stays inside it rather than
    // collapsing twice.
    val cardLeaving = !controlsShown && !keysShown && !filesShown
    val lastStep = rememberLastStep(controlsShown, keysShown, filesShown)

    val shape = RoundedCornerShape(24.dp)
    AnimatedVisibility(
        visible = !cardLeaving,
        enter = EnterTransition.None,
        exit = ChecklistCardExit,
        label = "tips card",
    ) {
    Column(
        modifier = modifier
            .clip(shape)
            .background(palette.trackFill)
            .border(1.dp, palette.surfaceBorder, shape)
            .padding(horizontal = 24.dp, vertical = 20.dp),
    ) {
        Text(
            text = "Tips",
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
            color = palette.textPrimary,
        )
        Spacer(Modifier.height(2.dp))
        Text(
            text = "Optional settings that make Portal nicer to use.",
            fontSize = 12.sp,
            lineHeight = 16.sp,
            color = palette.textMuted,
        )
        CollapsingStep(visible = controlsShown || (cardLeaving && lastStep == 0)) {
            ChecklistStep(
                number = 1,
                title = "Bring Portal back",
                body = "Tap the icon at the screen edge to open Portal’s labelled controls. " +
                    "Drag it to either edge; it remembers the position. Ctrl+Alt+P opens the veil directly.",
                state = StepState.Todo,
                actionLabel = "Got it",
                onAction = {
                    controlsDismissed = true
                    prefs.edit().putBoolean(PREF_CONTROLS_DISMISSED, true).apply()
                },
                palette = palette,
            )
        }
        CollapsingStep(visible = keysShown || (cardLeaving && lastStep == 1)) {
            ChecklistStep(
                number = if (controlsShown) 2 else 1,
                title = "Shortcuts and background",
                body = if (keys == KeyServiceState.NotRunning) {
                    "Portal’s accessibility switch is on, but Android isn’t running it. " +
                        "Turn it off and on again."
                } else {
                    "Turn on Portal in Accessibility so Alt+Tab, Ctrl+C and other shortcuts reach Linux " +
                        "and Android keeps the desktop running in the background. It only forwards key " +
                        "presses. If the switch is greyed out, allow restricted settings in Portal’s App info › ⋮ first."
                },
                state = if (keys == KeyServiceState.On) StepState.Done else StepState.Todo,
                actionLabel = "Open Accessibility",
                onAction = { openAccessibilitySettings(context) },
                secondaryLabel = "Not now",
                onSecondary = {
                    keysDismissed = true
                    prefs.edit().putBoolean(PREF_KEYS_DISMISSED, true).apply()
                },
                palette = palette,
            )
        }
        CollapsingStep(visible = filesShown || (cardLeaving && lastStep == 2)) {
            ChecklistStep(
                number = 1 + (if (controlsShown) 1 else 0) + (if (keysShown) 1 else 0),
                title = "Open your Android files",
                body = "Allow all files access to reach Downloads, Pictures and other shared folders from Linux.",
                state = if (filesGranted) StepState.Done else StepState.Todo,
                actionLabel = "Allow access",
                onAction = { openAllFilesSetting(context) },
                secondaryLabel = "Not now",
                onSecondary = {
                    filesDismissed = true
                    prefs.edit().putBoolean(PREF_FILES_DISMISSED, true).apply()
                },
                palette = palette,
            )
        }
    }
    }
}
