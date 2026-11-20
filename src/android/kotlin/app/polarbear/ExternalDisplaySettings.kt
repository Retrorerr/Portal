package app.polarbear

import android.view.Display
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
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
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import app.polarbear.setup.PortalPalette
import app.polarbear.setup.components.PortalGlyph
import app.polarbear.setup.components.SlidingSegmentedControl

@Composable
internal fun ExternalDisplaySettings(
    displays: PortalDisplays,
    palette: PortalPalette,
    settingsOpen: Boolean,
    onMove: () -> Unit,
) {
    var targets by remember(displays) { mutableStateOf(displays.targets()) }
    var touchOff by remember { mutableStateOf(touchNeedsSetup(displays)) }
    DisposableEffect(displays, settingsOpen) {
        // The capsule keeps its settings content composed after the page closes.
        val stopObserving = if (settingsOpen) displays.observe { targets = displays.targets() } else null
        if (settingsOpen) {
            targets = displays.targets()
            touchOff = touchNeedsSetup(displays)
        }
        onDispose { stopObserving?.invoke() }
    }
    // Android's Accessibility page is a round trip away from this one.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) touchOff = touchNeedsSetup(displays)
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    val context = LocalContext.current
    // Nothing to choose until Android offers a second display for apps.
    if (targets.none { it.id != Display.DEFAULT_DISPLAY }) return

    // The setup grows out of the row it is for, edge to edge of the group.
    val inset = with(LocalDensity.current) { 14.dp.toPx() }
    fun Modifier.rowBounds(onBounds: (Rect) -> Unit) = onGloballyPositioned { coordinates ->
        val bounds = coordinates.boundsInWindow()
        onBounds(Rect(bounds.left - inset, bounds.top, bounds.right + inset, bounds.bottom))
    }

    Spacer(Modifier.height(18.dp))
    Text("External display", fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = palette.textPrimary)
    Spacer(Modifier.height(8.dp))
    // The controls below also apply to a phone presenting the desktop, once
    // Portal is behind other apps.
    val device = deviceNoun(context)
    SettingsGroup(palette) {
        targets.forEach { target ->
            key(target.id) {
                var row by remember { mutableStateOf<Rect?>(null) }
                DisplayRow(target, device, palette, Modifier.rowBounds { row = it }) { displays.choose(target, row, onMove) }
            }
            SettingsDivider(palette)
        }
        var floatingRow by remember { mutableStateOf<Rect?>(null) }
        Box(Modifier.rowBounds { floatingRow = it }) {
            SettingsToggle(
                title = "Floating controls",
                subtitle = "Over other apps on this $device",
                checked = displays.floatingControls,
                palette = palette,
                icon = PortalGlyph.Layers,
                onChange = { displays.setFloatingControls(it, floatingRow) },
            )
        }
        // Which edge they keep to only matters while they float.
        AnimatedVisibility(
            displays.floatingControls,
            enter = fadeIn(tween(200, delayMillis = 60)) + expandVertically(spring(dampingRatio = 1f, stiffness = 380f)),
            exit = fadeOut(tween(120)) + shrinkVertically(spring(dampingRatio = 1f, stiffness = 380f)),
            label = "floating controls side",
        ) {
            Row(Modifier.fillMaxWidth().padding(start = SettingsGlyphInset, bottom = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                SettingsRowText("Side facing the monitor", null, palette, Modifier.weight(1f).padding(end = 12.dp))
                // TabletEdge is where this device sits: to the monitor's right, its left side faces it.
                SlidingSegmentedControl(
                    options = listOf(TabletEdge.Right, TabletEdge.Left),
                    selected = displays.tabletEdge,
                    onSelect = displays::setTabletPosition,
                    label = { if (it == TabletEdge.Right) "Left" else "Right" },
                    palette = palette,
                    modifier = Modifier.width(132.dp),
                    height = 36.dp,
                    enabled = LocalUtilityContentActive.current,
                )
            }
        }
        // Put off in the setup, or never asked: the same step, from here.
        AnimatedVisibility(
            touchOff,
            enter = fadeIn(tween(200, delayMillis = 60)) + expandVertically(spring(dampingRatio = 1f, stiffness = 380f)),
            exit = fadeOut(tween(120)) + shrinkVertically(spring(dampingRatio = 1f, stiffness = 380f)),
            label = "touch setup",
        ) {
            Column {
                SettingsDivider(palette)
                var row by remember { mutableStateOf<Rect?>(null) }
                Row(
                    Modifier.fillMaxWidth().rowBounds { row = it }.padding(vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    SettingsGlyph(PortalGlyph.Accessibility, palette)
                    SettingsRowText(
                        DisplayPermission.Accessibility.offTitle, DisplayPermission.Accessibility.offReason(device),
                        palette, Modifier.weight(1f).padding(end = 12.dp),
                    )
                    QuietButton("Turn on", palette) { displays.setUp(DisplayPermission.Accessibility, row) }
                }
            }
        }
    }
}

/**
 * Whether Control desktop would take the whole touchscreen: Portal's
 * accessibility service is off, on Android 14 and later, where it can leave
 * touch to Android.
 */
private fun touchNeedsSetup(displays: PortalDisplays): Boolean =
    DisplayPermission.Accessibility.applies && !displays.granted(DisplayPermission.Accessibility)

/** One display: tap to move the desktop there; the current one carries a check. */
@Composable
private fun DisplayRow(target: PortalDisplays.Target, device: String, palette: PortalPalette, modifier: Modifier, onClick: () -> Unit) {
    Row(
        modifier.fillMaxWidth()
            .selectable(
                selected = target.current,
                enabled = LocalUtilityContentActive.current && !target.current,
                role = Role.RadioButton,
                onClick = onClick,
            )
            .padding(vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val glyph = when {
            target.id != Display.DEFAULT_DISPLAY -> PortalGlyph.Monitor
            device == "tablet" -> PortalGlyph.Tablet
            else -> PortalGlyph.Phone
        }
        SettingsGlyph(glyph, palette)
        SettingsRowText(target.name, target.resolution, palette, Modifier.weight(1f).padding(end = 12.dp))
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
