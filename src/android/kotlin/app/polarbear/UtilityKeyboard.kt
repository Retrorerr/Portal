package app.polarbear

import androidx.compose.animation.*
import androidx.compose.animation.core.*
import androidx.compose.foundation.*
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalViewConfiguration
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.*
import app.polarbear.setup.PortalPalette
import app.polarbear.setup.components.PortalEmphasized
import app.polarbear.setup.components.PortalEmphasizedDecelerate
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

private enum class KeyboardPage { Letters, Symbols, Numpad }
private val KeyboardShortcuts = listOf(
    "Copy" to intArrayOf(29,46), "Paste" to intArrayOf(29,47), "Cut" to intArrayOf(29,45),
    "Undo" to intArrayOf(29,44), "Redo" to intArrayOf(29,42,44), "Select all" to intArrayOf(29,30),
    "Save" to intArrayOf(29,31), "Find" to intArrayOf(29,33), "Switch app" to intArrayOf(56,15),
    "Terminal" to intArrayOf(29,56,20), "Copy in terminal" to intArrayOf(29,42,46),
    "Paste in terminal" to intArrayOf(29,42,47), "Close app" to intArrayOf(56,62), "Desktop" to intArrayOf(125,32),
)

/** Typing stays in place; the desktop keys unfold beneath it in their own tray. */
@Composable
internal fun UtilityKeyboard(palette: PortalPalette, width: Dp, height: Dp, onBack: () -> Unit,
    grip: Modifier = Modifier,
) {
    val active = LocalUtilityContentActive.current
    var expanded by remember { mutableStateOf(false) }
    var page by remember { mutableStateOf(KeyboardPage.Letters) }
    val keyboard = SessionControls.desktopKeyboard
    val shift by remember { derivedStateOf {
        SessionControls.desktopKeyboardRevision
        keyboard.level(42) > 0 || keyboard.level(54) > 0
    } }
    val caps by remember { derivedStateOf { SessionControls.desktopKeyboardRevision; keyboard.capsLock } }
    val fontScale = LocalDensity.current.fontScale.coerceAtLeast(1f)
    val keyHeight = minOf(48.dp * fontScale.coerceAtMost(1.4f), (height - 88.dp) / 5).coerceAtLeast(32.dp)
    val keyText = (if (width < 440.dp) 12f else 14f).sp
    val shortWindow = height < 264.dp
    DisposableEffect(active) { onDispose { if (active) keyboard.reset() } }
    LaunchedEffect(active) { if (!active) keyboard.reset() }
    fun select(next: KeyboardPage) { keyboard.reset(); page = if (page == next) KeyboardPage.Letters else next }
    Column(Modifier.widthIn(max = 760.dp).width(width).heightIn(max = height)) {
        Row(Modifier.fillMaxWidth().heightIn(min = 44.dp).padding(horizontal = 8.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(if (width < 440.dp) "Keyboard" else "Desktop keyboard", Modifier.weight(1f), color = palette.textPrimary,
                fontSize = 12.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
            KeyboardAction(if (page == KeyboardPage.Symbols) "ABC" else "#+=", "Symbols", palette, active, page == KeyboardPage.Symbols) { select(KeyboardPage.Symbols) }
            KeyboardAction("123", "Number pad", palette, active, page == KeyboardPage.Numpad) { select(KeyboardPage.Numpad) }
            KeyboardAction(if (expanded) "−" else "+", "Extra desktop keys", palette, active, expanded) { expanded = !expanded }
            KeyboardAction("×", "Close keyboard", palette, active, onClick = onBack)
        }
        Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState(), enabled = active && shortWindow)
            .padding(horizontal = 6.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            AnimatedContent(page, transitionSpec = {
                (fadeIn(tween(240, delayMillis = 80, easing = PortalEmphasizedDecelerate)) +
                    scaleIn(tween(300, easing = PortalEmphasized), initialScale = 0.985f))
                    .togetherWith(fadeOut(tween(100)))
                    .using(SizeTransform { _, _ -> spring(dampingRatio = 1f, stiffness = 380f) })
            }, label = "keyboard layout") { shown ->
                val acceptsKeys = active && shown == page
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    if (shown == KeyboardPage.Numpad) {
                        KeyboardRow(listOf(DesktopKey("Tab",15), DesktopKey("Shift",42), DesktopKey("⌫",14)),
                            palette, keyboard, shift, caps, acceptsKeys, keyHeight, keyText, deferPress = shortWindow)
                        DesktopKeyboardLayout.numpad.forEach { row ->
                            KeyboardRow(row, palette, keyboard, shift, caps, acceptsKeys, keyHeight, keyText, "Numpad ", shortWindow)
                        }
                    } else {
                        KeyboardRow(DesktopKeyboardLayout.numbers + DesktopKey("⌫",14,weight = 1.5f),
                            palette, keyboard, shift, caps, acceptsKeys, keyHeight, keyText, deferPress = shortWindow)
                        (if (shown == KeyboardPage.Symbols) DesktopKeyboardLayout.symbols else DesktopKeyboardLayout.letters).forEach { row ->
                            KeyboardRow(row, palette, keyboard, shift, caps, acceptsKeys, keyHeight, keyText, deferPress = shortWindow)
                        }
                    }
                }
            }
            KeyboardRow(DesktopKeyboardLayout.bottom, palette, keyboard, shift, caps, active, keyHeight, keyText, deferPress = shortWindow)
            AnimatedVisibility(expanded,
                enter = expandVertically(spring(dampingRatio = 1f, stiffness = 380f), expandFrom = Alignment.Top) + fadeIn(tween(240, delayMillis = 80)),
                exit = shrinkVertically(spring(dampingRatio = 1f, stiffness = 380f), shrinkTowards = Alignment.Top) + fadeOut(tween(120))) {
                Column(Modifier.heightIn(max = (height - keyHeight * 5 - 92.dp).coerceAtLeast(80.dp))
                    .verticalScroll(rememberScrollState(), enabled = active && expanded).padding(top = 4.dp),
                    verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState(), enabled = active && expanded), horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                        KeyboardShortcuts.forEach { (label,codes) ->
                            KeyboardAction(label, label, palette, active && expanded) { keyboard.shortcut(*codes) }
                        }
                    }
                    DesktopKeyboardLayout.functions.chunked(if (width < 500.dp) 6 else 12).forEach { row ->
                        KeyboardRow(row, palette, keyboard, shift, caps, active && expanded, keyHeight, keyText, deferPress = true)
                    }
                    KeyboardRow(DesktopKeyboardLayout.navigation, palette, keyboard, shift, caps, active && expanded, keyHeight, keyText, deferPress = true)
                    KeyboardRow(DesktopKeyboardLayout.system, palette, keyboard, shift, caps, active && expanded, keyHeight, keyText, deferPress = true)
                    Text("Modifiers: tap for the next key · tap twice to lock · tap again to release. Hold for chords. Layout follows desktop XKB.",
                        Modifier.padding(horizontal = 5.dp, vertical = 4.dp), color = palette.textMuted, fontSize = 11.sp)
                }
            }
        }
        Box(grip.fillMaxWidth().height(24.dp).semantics { contentDescription = "Move keyboard" }, contentAlignment = Alignment.Center) {
            Box(Modifier.width(58.dp).height(4.dp).clip(RoundedCornerShape(2.dp)).background(palette.textMuted.copy(alpha = 0.5f)))
        }
    }
}

@Composable
private fun KeyboardRow(keys: List<DesktopKey>, palette: PortalPalette, state: DesktopKeyboardState, shift: Boolean,
    caps: Boolean, active: Boolean, height: Dp, textSize: TextUnit, prefix: String = "", deferPress: Boolean = false,
) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(3.dp)) {
        keys.forEach { key -> key(key.code) {
            DesktopKeyboardKey(key, palette, state, shift, caps, active, Modifier.weight(key.weight).height(height), textSize, prefix, deferPress)
        } }
    }
}

@Composable
private fun DesktopKeyboardKey(key: DesktopKey, palette: PortalPalette, state: DesktopKeyboardState, shift: Boolean,
    caps: Boolean, active: Boolean, modifier: Modifier, textSize: TextUnit, prefix: String, deferPress: Boolean,
) {
    // Ordinary strokes invalidate just their own key, rather than every key on the surface.
    val level by remember(state, key.code) { derivedStateOf { SessionControls.desktopKeyboardRevision; state.level(key.code) } }
    val pressed by remember(state, key.code) { derivedStateOf { SessionControls.desktopKeyboardRevision; state.isPressed(key.code) } }
    val uppercase = if (key.label.length == 1 && key.label[0] in 'a'..'z') shift.xor(caps) else shift
    val label = if (uppercase) key.shifted else key.label
    val description = prefix + when (key.code) {
        14 -> "Backspace"; 58 -> "Caps lock"; 105 -> "Left arrow"; 103 -> "Up arrow"; 108 -> "Down arrow"; 106 -> "Right arrow"; else -> label
    }
    KeyboardKey(label, description, palette, pressed, level, modifier, textSize, active,
        key.code in DesktopKeyboardLayout.modifiers, deferPress,
        { state.press(key.code) }, { state.release(key.code, it) }, { state.tap(key.code) })
}

@Composable
private fun KeyboardKey(label: String, description: String, palette: PortalPalette, pressed: Boolean, level: Int,
    modifier: Modifier, textSize: TextUnit, active: Boolean, modifierKey: Boolean, deferPress: Boolean,
    onDown: () -> Unit, onUp: (Boolean) -> Unit, onTap: () -> Unit,
) {
    var touching by remember { mutableStateOf(false) }
    val depressed = pressed || touching
    val holdDelay = LocalViewConfiguration.current.longPressTimeoutMillis
    val travel by animateFloatAsState(if (depressed) 1f else 0f, spring(dampingRatio = 1f, stiffness = 900f), label = "key travel")
    val fill by animateColorAsState(if (depressed || level > 0) palette.selectionHighlight else palette.surfaceTop,
        tween(100, easing = PortalEmphasized), label = "key fill")
    val latestDown by rememberUpdatedState(onDown)
    val latestUp by rememberUpdatedState(onUp)
    val shape = RoundedCornerShape(10.dp)
    BoxWithConstraints(modifier.graphicsLayer { scaleX = 1f - 0.045f * travel; scaleY = scaleX; translationY = travel * 1.dp.toPx() }
        .clip(shape).background(fill).border(1.dp, if (level > 0) palette.accent else palette.surfaceBorder, shape)
        .semantics {
            contentDescription = description; role = Role.Button
            if (!active) disabled()
            if (level > 0) stateDescription = if (level == 2) "Locked" else "Armed"
            onClick { if (active) onTap(); active }
        }
        .pointerInput(active, modifierKey, deferPress, holdDelay) {
            if (active) detectTapGestures(onPress = {
                coroutineScope {
                    touching = true
                    var sent = false
                    var released = false
                    // Only the scrollable tray defers strokes. The typing rows
                    // send key-down immediately, so a quick chord keeps its modifier.
                    if (!deferPress || modifierKey) { latestDown(); sent = true }
                    val hold = if (!sent) launch { delay(holdDelay); latestDown(); sent = true } else null
                    try { released = tryAwaitRelease() } finally {
                        hold?.cancel()
                        if (released && !sent) { latestDown(); sent = true }
                        if (sent) latestUp(!released)
                        touching = false
                    }
                }
            })
        }, contentAlignment = Alignment.Center) {
        val fittingSize = minOf(textSize.value, (maxWidth.value-4f).coerceAtLeast(1f) /
            (label.length.coerceAtLeast(1) * 0.6f * LocalDensity.current.fontScale)).sp
        Text(label, color = palette.textPrimary, fontSize = fittingSize, fontWeight = FontWeight.Medium,
            textAlign = TextAlign.Center, maxLines = 1, softWrap = false)
        // Keep the lock marker mounted so unlocking also has an exit animation.
        val marker by animateFloatAsState(if (level == 2) 1f else 0f, tween(140), label = "key lock")
        Box(Modifier.align(Alignment.BottomCenter).padding(bottom = 4.dp).size(4.dp).graphicsLayer { alpha = marker }
            .clip(RoundedCornerShape(2.dp)).background(palette.accent))
    }
}

@Composable
private fun KeyboardAction(label: String, description: String, palette: PortalPalette, enabled: Boolean, selected: Boolean = false, onClick: () -> Unit) {
    val shape = RoundedCornerShape(10.dp)
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val fill by animateColorAsState(if (selected || pressed) palette.selectionHighlight else palette.surfaceBottom,
        tween(160, easing = PortalEmphasized), label = "keyboard action")
    Box(Modifier.heightIn(min = 36.dp).widthIn(min = 36.dp).clip(shape).background(fill)
        .semantics { contentDescription = description; this.selected = selected }
        .clickable(interactionSource = interaction, indication = null, enabled = enabled, role = Role.Button, onClick = onClick)
        .padding(horizontal = 9.dp, vertical = 6.dp), contentAlignment = Alignment.Center) {
        AnimatedContent(label, transitionSpec = { fadeIn(tween(180)).togetherWith(fadeOut(tween(100))) }, label = "keyboard action label") { text ->
            Text(text, color = palette.textPrimary, fontSize = 12.sp, maxLines = 1)
        }
    }
}
