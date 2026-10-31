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
import androidx.compose.ui.semantics.clearAndSetSemantics
import app.polarbear.setup.components.*

internal val LocalUtilityContentActive = compositionLocalOf { true }

/** Retain the measured keyboard so repeated openings never rebuild its keys. */
@Composable
internal fun UtilitySurfaceContent(
    target: Pair<UtilityPage,Boolean>, prepareKeyboard: Boolean, modifier: Modifier = Modifier, onResizing: (Boolean) -> Unit = {},
    onTargetSize: (UtilityPage, IntSize) -> Unit = { _, _ -> },
    content: @Composable (Pair<UtilityPage,Boolean>) -> Unit,
) {
    var keyboardCreated by remember { mutableStateOf(false) }
    var gamingCreated by remember { mutableStateOf(false) }
    LaunchedEffect(prepareKeyboard, target.first) {
        if (prepareKeyboard || target.first == UtilityPage.Keyboard) keyboardCreated = true
        if (target.first == UtilityPage.Gaming) gamingCreated = true
    }
    val states = remember { listOf(UtilityPage.Dock to false,UtilityPage.Menu to false,UtilityPage.Menu to true,
        UtilityPage.Keyboard to false,UtilityPage.Gaming to false) }
    var wanted by remember { mutableStateOf(IntSize.Zero) }
    var measuredSize by remember { mutableStateOf(IntSize.Zero) }
    val resizing = wanted != measuredSize
    SideEffect { onResizing(resizing) }
    val opacities = arrayOfNulls<State<Float>>(states.size)
    Layout(content = {
        states.forEachIndexed { index,state -> key(state) {
            val selected = state == target
            val alpha = animateFloatAsState(if (selected) 1f else 0f,
                if (selected) tween(300,delayMillis = 100,easing = PortalEmphasizedDecelerate) else tween(140,easing = PortalEmphasizedAccelerate),
                label = "utility ${state.first} content")
            opacities[index] = alpha
            val scale by animateFloatAsState(if (selected) 1f else 0.985f,tween(420,easing = PortalEmphasized),label = "utility content settle")
            val interactive by remember(selected, resizing) { derivedStateOf { selected && !resizing && alpha.value > 0.98f } }
            CompositionLocalProvider(LocalUtilityContentActive provides interactive) {
                Box(Modifier.graphicsLayer { this.alpha = alpha.value; scaleX = scale; scaleY = scale }
                    // Publish keyboard targets after the window reaches them.
                    // Enumerating every moving key each frame causes stalls
                    // with Android accessibility services connected.
                    .then(if (!selected || resizing) Modifier.clearAndSetSemantics {} else Modifier)) {
                    if ((state.first != UtilityPage.Keyboard || keyboardCreated || target.first == UtilityPage.Keyboard) &&
                        (state.first != UtilityPage.Gaming || gamingCreated || target.first == UtilityPage.Gaming)) content(state)
                }
            }
        } }
    },modifier = modifier.onSizeChanged { measuredSize = it }.animateContentSize(spring(dampingRatio = Spring.DampingRatioNoBouncy,stiffness = 380f),alignment = Alignment.Center)) { children,constraints ->
        val measured = children.map { it.measure(constraints.copy(minWidth = 0,minHeight = 0)) }
        val selected = measured[states.indexOf(target)]
        wanted = IntSize(selected.width,selected.height)
        onTargetSize(target.first, wanted)
        layout(selected.width,selected.height) {
            measured.forEachIndexed { index,item ->
                // Unplaced retained pages draw nothing, take no input and
                // expose no accessibility nodes, while keeping their layout.
                if ((opacities[index]?.value ?: 0f) > 0.001f) item.placeRelative((selected.width-item.width)/2,(selected.height-item.height)/2)
            }
        }
    }
}
