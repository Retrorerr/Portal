package app.polarbear.gaming

// The quick game panel and the controller edit bar: small Compose cards
// over a running game, in the setup screens' visual language (charcoal
// glass, ivory type, one orange accent). Controller state lives in
// TouchControls; game settings live in the files GameSettings writes.

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.MutableTransitionState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.polarbear.TouchControls
import app.polarbear.setup.PanelButton
import app.polarbear.setup.PortalColors
import app.polarbear.setup.PortalPalette
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalEmphasizedAccelerate
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import app.polarbear.setup.components.SlidingSegmentedControl
import kotlinx.coroutines.delay
import kotlin.math.roundToInt

private val PanelWidth = 440.dp
private val EditBarWidth = 540.dp
private val CardCorner = 24.dp

/** Over a game, always the dark reading of Portal's palette. */
private val GamePalette: PortalPalette = PortalColors.dark()

/** Charcoal glass that stays legible over any game frame. */
private fun Modifier.gameCard(palette: PortalPalette, shape: RoundedCornerShape): Modifier = this
    .shadow(18.dp, shape, ambientColor = Color(0x99000000), spotColor = Color(0x99000000))
    .clip(shape)
    .background(Color(0xF0191B1C))
    .background(Brush.verticalGradient(listOf(palette.surfaceTop, palette.surfaceBottom)))
    .border(1.dp, palette.surfaceBorder, shape)

@Composable
fun GamePanel(onClosed: () -> Unit) {
    val palette = GamePalette
    val closing by TouchControls.panelClosing
    val visible = remember { MutableTransitionState(false) }
    visible.targetState = !closing
    LaunchedEffect(visible.currentState, visible.isIdle, closing) {
        if (closing && visible.isIdle && !visible.currentState) onClosed()
    }
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val maxCardHeight = maxHeight - 24.dp
        AnimatedVisibility(
            visibleState = visible,
            enter = fadeIn(tween(220)),
            exit = fadeOut(tween(200)),
        ) {
            Box(
                Modifier
                    .fillMaxSize()
                    .background(Color(0x59000000))
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                        onClickLabel = "Close",
                        onClick = { TouchControls.closePanel() },
                    ),
            )
        }
        AnimatedVisibility(
            visibleState = visible,
            modifier = Modifier.align(Alignment.TopCenter),
            enter = slideInVertically(tween(420, easing = PortalEmphasized)) { -it / 5 } +
                fadeIn(tween(260, easing = PortalEmphasizedDecelerate)) +
                scaleIn(tween(420, easing = PortalEmphasized), initialScale = 0.96f),
            exit = slideOutVertically(tween(220, easing = PortalEmphasizedAccelerate)) { -it / 6 } +
                fadeOut(tween(180, easing = PortalEmphasizedAccelerate)) +
                scaleOut(tween(220, easing = PortalEmphasizedAccelerate), targetScale = 0.97f),
        ) {
            PanelCard(palette, maxCardHeight)
        }
    }
}

@Composable
private fun PanelCard(palette: PortalPalette, maxHeight: Dp) {
    val context = LocalContext.current
    val shape = RoundedCornerShape(CardCorner)
    val appId by TouchControls.appId
    val gameName by TouchControls.gameName
    val gameRunning by TouchControls.gameRunning
    val layout by TouchControls.layout
    val physicalPad by TouchControls.physicalPad
    // Read so the switch follows changes made from the handle.
    TouchControls.controllerOn.value
    TouchControls.showWithPad.value
    val haptics by TouchControls.haptics
    val rumble by TouchControls.rumble
    val analog by TouchControls.analog
    val visible = TouchControls.controllerVisible

    var ownSettings by remember(appId) { mutableStateOf(GameSettings.hasAppConfig(context, appId)) }
    var config by remember(appId, ownSettings) { mutableStateOf(GameSettings.load(context, appId)) }
    fun update(next: GameConfig) {
        config = next
        GameSettings.save(context, appId, next)
    }

    Column(
        modifier = Modifier
            .padding(top = 12.dp, start = 16.dp, end = 16.dp)
            .widthIn(max = PanelWidth)
            .fillMaxWidth()
            .heightIn(max = maxHeight)
            .gameCard(palette, shape)
            // Taps on the card's own blank space must not reach the scrim.
            .pointerInput(Unit) { detectTapGestures { } }
            .verticalScroll(rememberScrollState())
            .padding(start = 20.dp, end = 20.dp, top = 18.dp, bottom = 18.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(
                    text = gameName ?: if (gameRunning) "Game" else "Steam",
                    fontSize = 17.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = palette.textPrimary,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = when {
                        appId != null -> "Running · Steam app $appId"
                        gameRunning -> "Running"
                        else -> "No game running · settings apply to the next one"
                    },
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Medium,
                    color = palette.textMuted,
                )
            }
            PanelButton(text = "Done", primary = false, palette = palette, onClick = { TouchControls.closePanel() })
        }

        SectionHeading("On-screen controller", palette)
        ToggleRow(
            title = "Show controller",
            subtitle = when {
                physicalPad && !visible -> "Hidden while a controller is connected"
                physicalPad -> "Shown alongside your controller"
                !gameRunning -> "Appears when a game starts"
                else -> null
            },
            checked = visible,
            palette = palette,
            onChange = { TouchControls.toggleController() },
        )
        SliderRow(
            title = "Opacity",
            value = layout.opacity,
            range = 0.15f..1f,
            valueText = "${(layout.opacity * 100).roundToInt()}%",
            palette = palette,
            onChange = { TouchControls.updateLayout(layout.copy(opacity = it), persist = false) },
            onFinished = { TouchControls.updateLayout(TouchControls.layout.value, persist = true) },
        )
        SliderRow(
            title = "Size",
            value = layout.scale,
            range = ControllerLayout.MIN_SCALE..ControllerLayout.MAX_SCALE,
            valueText = "${(layout.scale * 100).roundToInt()}%",
            palette = palette,
            onChange = { TouchControls.updateLayout(layout.copy(scale = it), persist = false) },
            onFinished = { TouchControls.updateLayout(TouchControls.layout.value, persist = true) },
        )
        ToggleRow("Button vibration", "A tap you can feel on every press", haptics, palette) {
            TouchControls.setHaptics(it)
        }
        ToggleRow("Rumble", "The tablet vibrates when a game rumbles", rumble, palette) {
            TouchControls.setRumble(it)
        }
        ToggleRow("Analog triggers", "Press LT and RT lower down to press harder", analog, palette) {
            TouchControls.setAnalogTriggers(it)
        }
        Spacer(Modifier.height(10.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            PanelButton(text = "Edit layout", primary = true, palette = palette, onClick = { TouchControls.startEditing() })
            Text(
                text = if (TouchControls.appLayout.value) "Using this game's own layout" else "Move, resize and hide controls",
                fontSize = 11.sp,
                fontWeight = FontWeight.Medium,
                color = palette.textMuted,
            )
        }

        SectionHeading("Game settings", palette)
        val id = appId
        if (id != null) {
            ToggleRow(
                title = "Only for this game",
                subtitle = if (ownSettings) "${gameName ?: "This game"} keeps its own settings" else "Off: every game shares these settings",
                checked = ownSettings,
                palette = palette,
            ) { own ->
                GameSettings.setAppConfig(context, id, own)
                ownSettings = own
            }
        }
        OptionRow(
            title = "Video memory",
            subtitle = "What Windows games are told the GPU has",
            options = listOf(2048, 3072, 4096, 6144, 8192),
            selected = config.vramMb,
            label = { "${it / 1024f}".removeSuffix(".0") + " GB" },
            palette = palette,
        ) { update(config.copy(vramMb = it)) }
        OptionRow(
            title = "Frame-rate cap",
            subtitle = "Steadier pacing and less heat (DirectX games)",
            options = listOf(0, 30, 45, 60, 90, 120),
            selected = config.fpsLimit,
            label = { if (it == 0) "Off" else "$it" },
            palette = palette,
        ) { update(config.copy(fpsLimit = it)) }
        OptionRow(
            title = "Proton",
            subtitle = "Which installed Proton for ARM64 runs Windows games",
            options = listOf("", "stable", "experimental"),
            selected = config.proton.takeIf { it == "stable" || it == "experimental" } ?: "",
            label = {
                when (it) {
                    "stable" -> "Stable"
                    "experimental" -> "Experimental"
                    else -> "Newest"
                }
            },
            palette = palette,
        ) { update(config.copy(proton = it)) }
        OptionRow(
            title = "32-bit Windows code",
            subtitle = "Translator for 32-bit games and launchers",
            options = listOf("", "box64"),
            selected = config.wow64,
            label = { if (it == "box64") "Box64" else "FEX" },
            palette = palette,
        ) { update(config.copy(wow64 = it)) }
        ToggleRow("FEX code cache", "Keep translated x86 code between runs for faster starts", config.fexCache, palette) {
            update(config.copy(fexCache = it))
        }
        ToggleRow("Frame-rate counter", "DXVK's FPS readout in the corner", config.hud, palette) {
            update(config.copy(hud = it))
        }
        Spacer(Modifier.height(10.dp))
        Text(
            text = "Game settings apply the next time a game starts. A game's Steam launch options override them.",
            fontSize = 11.sp,
            fontWeight = FontWeight.Medium,
            color = palette.textMuted,
            lineHeight = 15.sp,
        )
    }
}

/** Floating bar shown while the controller layout is being edited. */
@Composable
fun EditBar() {
    val palette = GamePalette
    val shape = RoundedCornerShape(CardCorner)
    val layout by TouchControls.layout
    val selected by TouchControls.selectedState
    val appId by TouchControls.appId
    val gameName by TouchControls.gameName
    val appLayout by TouchControls.appLayout
    var confirmReset by remember { mutableStateOf(false) }
    LaunchedEffect(confirmReset) {
        if (confirmReset) {
            delay(3500)
            confirmReset = false
        }
    }
    Column(
        modifier = Modifier
            .padding(top = 12.dp, start = 16.dp, end = 16.dp)
            .widthIn(max = EditBarWidth)
            .gameCard(palette, shape)
            .padding(start = 18.dp, end = 14.dp, top = 14.dp, bottom = 10.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Edit controller", fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = palette.textPrimary)
                Text(
                    text = "Drag to move. Tap a control to resize or hide it.",
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Medium,
                    color = palette.textMuted,
                )
            }
            PanelButton(
                text = if (confirmReset) "Tap to reset" else "Reset",
                primary = false,
                palette = palette,
                onClick = {
                    if (confirmReset) {
                        confirmReset = false
                        TouchControls.resetLayout()
                    } else {
                        confirmReset = true
                    }
                },
            )
            Spacer(Modifier.width(4.dp))
            PanelButton(text = "Done", primary = true, palette = palette, onClick = { TouchControls.stopEditing() })
        }
        val control = selected
        if (control != null) {
            val state = layout.element(control)
            Spacer(Modifier.height(6.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = control.title,
                    modifier = Modifier.weight(1f),
                    fontSize = 13.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = palette.accent,
                )
                PanelButton(
                    text = if (state.visible) "Hide" else "Show",
                    primary = false,
                    palette = palette,
                    onClick = { TouchControls.setElementVisible(control, !state.visible) },
                )
            }
            SliderRow(
                title = "${control.title} size",
                value = state.scale,
                range = ControllerLayout.MIN_ELEMENT_SCALE..ControllerLayout.MAX_ELEMENT_SCALE,
                valueText = "${(state.scale * 100).roundToInt()}%",
                palette = palette,
                onChange = { TouchControls.setElementScale(control, it, persist = false) },
                onFinished = { TouchControls.updateLayout(TouchControls.layout.value, persist = true) },
            )
        }
        Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            Box(Modifier.weight(1f)) {
                SliderRow(
                    title = "Opacity",
                    value = layout.opacity,
                    range = 0.15f..1f,
                    valueText = "${(layout.opacity * 100).roundToInt()}%",
                    palette = palette,
                    onChange = { TouchControls.updateLayout(layout.copy(opacity = it), persist = false) },
                    onFinished = { TouchControls.updateLayout(TouchControls.layout.value, persist = true) },
                )
            }
            Box(Modifier.weight(1f)) {
                SliderRow(
                    title = "Overall size",
                    value = layout.scale,
                    range = ControllerLayout.MIN_SCALE..ControllerLayout.MAX_SCALE,
                    valueText = "${(layout.scale * 100).roundToInt()}%",
                    palette = palette,
                    onChange = { TouchControls.updateLayout(layout.copy(scale = it), persist = false) },
                    onFinished = { TouchControls.updateLayout(TouchControls.layout.value, persist = true) },
                )
            }
        }
        if (appId != null) {
            ToggleRow(
                title = "Layout only for ${gameName ?: "this game"}",
                subtitle = if (appLayout) "Other games keep the shared layout" else "Off: every game shares this layout",
                checked = appLayout,
                palette = palette,
            ) { TouchControls.setAppLayout(it) }
        }
    }
}

@Composable
private fun SectionHeading(text: String, palette: PortalPalette) {
    Text(
        text = text.uppercase(),
        modifier = Modifier.padding(top = 18.dp, bottom = 4.dp),
        color = palette.textSecondary.copy(alpha = 0.75f),
        fontSize = 10.sp,
        fontWeight = FontWeight.SemiBold,
        letterSpacing = 1.2.sp,
        maxLines = 1,
    )
}

@Composable
private fun ToggleRow(
    title: String,
    subtitle: String?,
    checked: Boolean,
    palette: PortalPalette,
    onChange: (Boolean) -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
                role = Role.Switch,
                onClick = { onChange(!checked) },
            )
            .padding(vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f).padding(end = 12.dp)) {
            Text(title, fontSize = 14.sp, fontWeight = FontWeight.Medium, color = palette.textPrimary)
            if (subtitle != null) {
                Text(subtitle, fontSize = 11.sp, fontWeight = FontWeight.Medium, color = palette.textMuted, lineHeight = 14.sp)
            }
        }
        Switch(
            checked = checked,
            onCheckedChange = onChange,
            colors = SwitchDefaults.colors(
                checkedThumbColor = PortalColors.Ivory,
                checkedTrackColor = palette.accent,
                checkedBorderColor = palette.accent,
                uncheckedThumbColor = palette.textMuted,
                uncheckedTrackColor = palette.trackFill,
                uncheckedBorderColor = palette.textMuted.copy(alpha = 0.5f),
            ),
        )
    }
}

@Composable
private fun SliderRow(
    title: String,
    value: Float,
    range: ClosedFloatingPointRange<Float>,
    valueText: String,
    palette: PortalPalette,
    onChange: (Float) -> Unit,
    onFinished: () -> Unit,
) {
    Column(Modifier.fillMaxWidth().padding(top = 6.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(title, modifier = Modifier.weight(1f), fontSize = 14.sp, fontWeight = FontWeight.Medium, color = palette.textPrimary)
            Text(valueText, fontSize = 12.sp, fontWeight = FontWeight.Medium, color = palette.textMuted)
        }
        Slider(
            value = value.coerceIn(range.start, range.endInclusive),
            onValueChange = onChange,
            onValueChangeFinished = onFinished,
            valueRange = range,
            modifier = Modifier.height(32.dp),
            colors = SliderDefaults.colors(
                thumbColor = PortalColors.Ivory,
                activeTrackColor = palette.accent,
                inactiveTrackColor = PortalColors.Ivory.copy(alpha = 0.14f),
            ),
        )
    }
}

@Composable
private fun <T> OptionRow(
    title: String,
    subtitle: String,
    options: List<T>,
    selected: T,
    label: (T) -> String,
    palette: PortalPalette,
    onSelect: (T) -> Unit,
) {
    Column(Modifier.fillMaxWidth().padding(top = 8.dp)) {
        Text(title, fontSize = 14.sp, fontWeight = FontWeight.Medium, color = palette.textPrimary)
        Text(subtitle, fontSize = 11.sp, fontWeight = FontWeight.Medium, color = palette.textMuted)
        Spacer(Modifier.height(6.dp))
        // A value set by hand in the file that the panel does not offer shows the first option.
        val current = if (selected in options) selected else options.first()
        SlidingSegmentedControl(
            options = options,
            selected = current,
            onSelect = onSelect,
            label = label,
            palette = palette,
            height = 38.dp,
        )
    }
}
