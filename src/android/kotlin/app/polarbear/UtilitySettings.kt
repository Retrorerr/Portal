package app.polarbear

import android.os.Build
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.setup.PortalColors
import app.polarbear.setup.PortalPalette
import app.polarbear.setup.components.PortalSwitch

internal val UtilitySettingsWidth = 440.dp

@Composable
internal fun PortalSettingsContent(
    host: PortalActivity,
    palette: PortalPalette,
    width: Dp,
    maxHeight: Dp,
    onBack: () -> Unit,
) {
    UtilitySettingsCard(palette, width, maxHeight, onBack, header = {
        Text("Portal settings", fontSize = 19.sp, fontWeight = FontWeight.SemiBold, color = palette.textPrimary)
    }) {
        Spacer(Modifier.height(14.dp))
        SettingsGroup(palette) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                SettingsToggle(
                    title = "Hide cursor",
                    subtitle = "Show only the Plasma cursor on the desktop",
                    checked = host.androidCursorHidden,
                    palette = palette,
                    onChange = host::setHideAndroidCursor,
                )
                SettingsDivider(palette)
            }
            SettingsToggle(
                title = "Use Android screen timeout",
                subtitle = "Apps can still keep the screen awake",
                checked = host.useAndroidScreenTimeout,
                palette = palette,
                accessory = { SettingsCoffeeCup(visible = !host.useAndroidScreenTimeout, palette = palette) },
                onChange = host::setAndroidScreenTimeoutEnabled,
            )
            SettingsDivider(palette)
            SettingsToggle(
                title = "Use Portal keyboard",
                subtitle = "Replaces Android's keyboard for text fields",
                checked = host.usePortalKeyboard,
                palette = palette,
                onChange = host::setPortalKeyboardEnabled,
            )
        }
    }
}

@Composable
internal fun UtilitySettingsCard(
    palette: PortalPalette,
    width: Dp,
    maxHeight: Dp,
    onBack: () -> Unit,
    header: @Composable ColumnScope.() -> Unit,
    content: @Composable ColumnScope.() -> Unit,
) {
    Column(
        Modifier
            .widthIn(max = UtilitySettingsWidth)
            .width(width)
            .heightIn(max = maxHeight)
            // Taps on the card's own blank space must not reach the scrim.
            .pointerInput(Unit) { detectTapGestures { } }
            .padding(top = 18.dp),
    ) {
        Row(Modifier.padding(horizontal = 20.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f), content = header)
            Spacer(Modifier.width(12.dp))
            QuietButton("Done", palette, onClick = onBack)
        }

        Column(
            Modifier
                .weight(1f, fill = false)
                .verticalScroll(rememberScrollState(), enabled = LocalUtilityContentActive.current)
                .padding(start = 20.dp, end = 20.dp, bottom = 18.dp),
            content = content,
        )
    }
}

/** Grouped rows: one soft inset surface per section, like a settings list. */
@Composable
internal fun SettingsGroup(palette: PortalPalette, content: @Composable () -> Unit) {
    val shape = RoundedCornerShape(16.dp)
    Column(
        Modifier
            .fillMaxWidth()
            .animateContentSize(spring(dampingRatio = 1f, stiffness = 380f))
            .clip(shape)
            .background(palette.textPrimary.copy(alpha = 0.035f))
            .border(1.dp, palette.surfaceBorder, shape)
            .padding(horizontal = 14.dp, vertical = 4.dp),
    ) { content() }
}

@Composable
internal fun SettingsDivider(palette: PortalPalette) {
    Box(Modifier.fillMaxWidth().padding(vertical = 2.dp).height(1.dp).background(palette.surfaceBorder))
}

@Composable
internal fun SettingsRowText(title: String, subtitle: String?, palette: PortalPalette, modifier: Modifier) {
    Column(modifier.animateContentSize(spring(dampingRatio = 1f, stiffness = 380f))) {
        Text(title, fontSize = 14.sp, fontWeight = FontWeight.Medium, color = palette.textPrimary)
        AnimatedContent(subtitle, transitionSpec = { fadeIn(tween(200)).togetherWith(fadeOut(tween(100))) }, label = "setting detail") { text ->
            if (text != null) Text(text, fontSize = 11.5.sp, fontWeight = FontWeight.Normal, color = palette.textMuted, lineHeight = 15.sp)
        }
    }
}

/** A small, quiet pill for secondary actions (Done, Reset, Hide). */
@Composable
internal fun QuietButton(text: String, palette: PortalPalette, accent: Boolean = false, onClick: () -> Unit) {
    val shape = RoundedCornerShape(50)
    val fill by animateColorAsState(if (accent) palette.accent else palette.surfaceTop, tween(180), label = "button fill")
    Box(
        Modifier
            .clip(shape)
            .background(fill)
            .heightIn(min = 44.dp)
            .clickable(enabled = LocalUtilityContentActive.current, role = Role.Button, onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 8.dp),
        contentAlignment = Alignment.Center,
    ) {
        AnimatedContent(text, transitionSpec = { fadeIn(tween(180)).togetherWith(fadeOut(tween(100))) }, label = "button label") { label -> Text(
            label,
            fontSize = 13.sp,
            fontWeight = FontWeight.SemiBold,
            color = if (accent) PortalColors.Charcoal else palette.textPrimary,
        ) }
    }
}

@Composable
internal fun SettingsToggle(
    title: String,
    subtitle: String?,
    checked: Boolean,
    palette: PortalPalette,
    accessory: @Composable () -> Unit = {},
    onChange: (Boolean) -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .toggleable(
                value = checked,
                enabled = LocalUtilityContentActive.current,
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
                role = Role.Switch,
                onValueChange = onChange,
            )
            .padding(vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        SettingsRowText(title, subtitle, palette, Modifier.weight(1f).padding(end = 12.dp))
        accessory()
        PortalSwitch(checked, palette)
    }
}
