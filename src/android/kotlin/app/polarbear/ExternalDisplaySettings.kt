package app.polarbear

import android.os.Build
import android.view.Display
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.selection.selectable
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import app.polarbear.setup.PortalPalette
import app.polarbear.setup.components.SlidingSegmentedControl
import app.polarbear.setup.components.openAccessibilitySettings

@Composable
internal fun ExternalDisplaySettings(
    displays: PortalDisplays,
    palette: PortalPalette,
    settingsOpen: Boolean,
    onMove: () -> Unit,
) {
    var targets by remember(displays) { mutableStateOf(displays.targets()) }
    var touchKept by remember { mutableStateOf(touchKeptWhileControlling()) }
    DisposableEffect(displays, settingsOpen) {
        // The capsule keeps its settings content composed after the page closes.
        val stopObserving = if (settingsOpen) displays.observe { targets = displays.targets() } else null
        if (settingsOpen) {
            targets = displays.targets()
            touchKept = touchKeptWhileControlling()
        }
        onDispose { stopObserving?.invoke() }
    }
    // Android's Accessibility page is a round trip away from this one.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) touchKept = touchKeptWhileControlling()
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    val context = LocalContext.current
    // Nothing to choose until Android offers a second display for apps.
    if (targets.none { it.id != Display.DEFAULT_DISPLAY }) return

    Spacer(Modifier.height(18.dp))
    Text("External display", fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = palette.textPrimary)
    Spacer(Modifier.height(8.dp))
    // The controls below also apply to a phone presenting the desktop, once
    // Portal is behind other apps.
    val device = deviceNoun(context)
    SettingsGroup(palette) {
        targets.forEach { target ->
            DisplayRow(target, palette) { if (displays.moveTo(target.id)) onMove() }
            SettingsDivider(palette)
        }
        Row(Modifier.fillMaxWidth().padding(vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
            SettingsRowText("${device.replaceFirstChar(Char::uppercaseChar)} position",
                "Puts Control desktop on the side facing the monitor",
                palette, Modifier.weight(1f).padding(end = 12.dp))
            SlidingSegmentedControl(
                options = TabletEdge.entries,
                selected = displays.tabletEdge,
                onSelect = displays::setTabletPosition,
                label = { if (it == TabletEdge.Left) "Left" else "Right" },
                palette = palette,
                modifier = Modifier.width(132.dp),
                height = 36.dp,
                enabled = LocalUtilityContentActive.current,
            )
        }
        SettingsDivider(palette)
        SettingsToggle(
            title = "Floating controls",
            subtitle = "Keep Control desktop on the $device over other apps",
            checked = displays.floatingControls,
            palette = palette,
            onChange = displays::setFloatingControls,
        )
        if (!touchKept) {
            SettingsDivider(palette)
            Row(Modifier.fillMaxWidth().padding(vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                SettingsRowText(
                    "Touch while controlling",
                    "Turn on Portal in Accessibility to keep using Android by touch while the desktop has " +
                        "the mouse and keyboard",
                    palette, Modifier.weight(1f).padding(end = 12.dp),
                )
                QuietButton("Turn on", palette) { openAccessibilitySettings(context) }
            }
        }
    }
}

/**
 * Whether Control desktop leaves the touchscreen to Android: true once Portal's
 * accessibility service runs, and on Android before 14, where it can't.
 */
private fun touchKeptWhileControlling(): Boolean =
    Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE || KeyboardAccessibilityService.canRouteMotion()

/** One display: tap to move the desktop there; the current one carries a check. */
@Composable
private fun DisplayRow(target: PortalDisplays.Target, palette: PortalPalette, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth()
            .selectable(
                selected = target.current,
                enabled = LocalUtilityContentActive.current && !target.current,
                role = Role.RadioButton,
                onClick = onClick,
            )
            .padding(vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val detail = if (target.presented) {
            "${target.resolution} · this ${deviceNoun(LocalContext.current)} becomes the touchpad"
        } else target.resolution
        SettingsRowText(target.name, detail, palette, Modifier.weight(1f).padding(end = 12.dp))
        if (target.current) {
            Canvas(Modifier.size(18.dp)) {
                val stroke = 2.dp.toPx()
                drawLine(palette.accent, Offset(size.width * 0.16f, size.height * 0.54f),
                    Offset(size.width * 0.40f, size.height * 0.78f), stroke, StrokeCap.Round)
                drawLine(palette.accent, Offset(size.width * 0.40f, size.height * 0.78f),
                    Offset(size.width * 0.86f, size.height * 0.24f), stroke, StrokeCap.Round)
            }
        }
    }
}
