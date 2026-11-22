package app.polarbear

import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.*
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.unit.IntSize
import kotlinx.coroutines.flow.first
import androidx.compose.ui.semantics.clearAndSetSemantics
import app.polarbear.setup.components.*

internal val LocalUtilityContentActive = compositionLocalOf { true }

/**
 * [LocalUtilityContentActive] as a state that keeps its identity: read it in
 * event handlers, and a page with many controls (the keyboard's keys) is not
 * recomposed as it turns on and off at the end of every morph.
 */
internal val LocalUtilityContentActiveState = staticCompositionLocalOf<State<Boolean>> { mutableStateOf(true) }

/** Retain the measured keyboard so repeated openings never rebuild its keys. */
@Composable
internal fun UtilitySurfaceContent(
    target: Pair<UtilityPage,Boolean>,
    /** Build the keys ahead of time; read in an effect, so its changes never recompose. */
    prepareKeyboard: () -> Boolean,
    modifier: Modifier = Modifier, onResizing: (Boolean) -> Unit = {},
    onTargetSize: (UtilityPage, IntSize) -> Unit = { _, _ -> },
    /** Nothing around the surface moves any more: its travel and the keyboard's own motion have stopped. */
    settled: () -> Boolean = { true },
    content: @Composable (Pair<UtilityPage,Boolean>) -> Unit,
) {
    var keyboardCreated by remember { mutableStateOf(false) }
    var gamingCreated by remember { mutableStateOf(false) }
    var settingsCreated by remember { mutableStateOf(false) }
    LaunchedEffect(target.first) {
        if (target.first == UtilityPage.Keyboard) keyboardCreated = true
        if (target.first == UtilityPage.Gaming) gamingCreated = true
        if (target.first == UtilityPage.Settings) settingsCreated = true
    }
    val prepareNow = rememberUpdatedState(prepareKeyboard)
    LaunchedEffect(Unit) {
        snapshotFlow { prepareNow.value() }.first { it }
        keyboardCreated = true
    }
    val states = remember { listOf(UtilityPage.Dock to false,UtilityPage.Menu to false,UtilityPage.Menu to true,
        UtilityPage.Keyboard to false,UtilityPage.Keyboard to true,UtilityPage.Gaming to false,UtilityPage.Settings to false) }
    val keyboardIndex = states.indexOf(UtilityPage.Keyboard to false)
    var wanted by remember { mutableStateOf(IntSize.Zero) }
    var measuredSize by remember { mutableStateOf(IntSize.Zero) }
    val resizing = wanted != measuredSize
    SideEffect { onResizing(resizing) }
    val resizingNow = rememberUpdatedState(resizing)
    val settledNow = rememberUpdatedState(settled)
    val opacities = arrayOfNulls<State<Float>>(states.size)
    Layout(content = {
        states.forEachIndexed { index,state -> key(state) {
            val selected = state == target
            val alpha = animateFloatAsState(if (selected) 1f else 0f,
                if (selected) tween(300,delayMillis = 100,easing = PortalEmphasizedDecelerate) else tween(140,easing = PortalEmphasizedAccelerate),
                label = "utility ${state.first} content")
            opacities[index] = alpha
            val scale by animateFloatAsState(if (selected) 1f else 0.985f,tween(420,easing = PortalEmphasized),label = "utility content settle")
            val selectedNow = rememberUpdatedState(selected)
            // Takes input as soon as it has faded in and stopped resizing.
            val interactive = remember { derivedStateOf { selectedNow.value && !resizingNow.value && alpha.value > 0.98f } }
            // Its controls are published to accessibility services only once
            // everything has come to rest. Publishing a page of keys costs the
            // main thread tens of milliseconds while any service is connected
            // (Portal's own keyboard service included); done mid-travel, it
            // dropped the last frames of every morph.
            val published by remember { derivedStateOf { interactive.value && settledNow.value() } }
            CompositionLocalProvider(LocalUtilityContentActive provides interactive.value,
                LocalUtilityContentActiveState provides interactive) {
                Box(Modifier.graphicsLayer { this.alpha = alpha.value; scaleX = scale; scaleY = scale }
                    .then(if (!published) Modifier.clearAndSetSemantics {} else Modifier)) {
                    val created = when (state.first) {
                        UtilityPage.Keyboard -> keyboardCreated
                        UtilityPage.Gaming -> gamingCreated
                        UtilityPage.Settings -> settingsCreated
                        else -> true
                    }
                    if (selected || created) content(state)
                }
            }
        } }
    },modifier = modifier.onSizeChanged { measuredSize = it }.animateContentSize(spring(dampingRatio = Spring.DampingRatioNoBouncy,stiffness = 380f),alignment = Alignment.Center)) { children,constraints ->
        val measured = children.map { it.measure(constraints.copy(minWidth = 0,minHeight = 0)) }
        val selected = measured[states.indexOf(target)]
        wanted = IntSize(selected.width,selected.height)
        onTargetSize(target.first, wanted)
        layout(selected.width,selected.height) {
            fun place(item: androidx.compose.ui.layout.Placeable) =
                item.placeRelative((selected.width-item.width)/2,(selected.height-item.height)/2)
            // The full keyboard, once built, stays laid out and drawn beneath
            // the other pages while it is hidden: unplaced, its keys laid out
            // and drew again from scratch on the frame it came back, which
            // stalled the morph that brings it in.
            val keyboard = measured[keyboardIndex]
            if (keyboardCreated && (opacities[keyboardIndex]?.value ?: 0f) <= 0.001f) place(keyboard)
            measured.forEachIndexed { index,item ->
                // Other retained pages are unplaced while hidden: they draw
                // nothing, take no input and expose no accessibility nodes,
                // while keeping their layout.
                if ((opacities[index]?.value ?: 0f) > 0.001f) place(item)
            }
        }
    }
}
