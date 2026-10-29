package app.polarbear.setup

import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.onGloballyPositioned
import app.polarbear.ComposeOverlay
import app.polarbear.FileTransfers
import app.polarbear.setup.components.PortalVeilMotion

/** Observe taps without acquiring swipes or a scrolling card's pointer stream. */
@Composable
internal fun Modifier.dismissPortalPanelOutside(veil: PortalVeilMotion): Modifier {
    val coordinates = remember { arrayOfNulls<LayoutCoordinates>(1) }
    return onGloballyPositioned { coordinates[0] = it }.pointerInput(veil) {
        awaitEachGesture {
            val down = awaitFirstDown(requireUnconsumed = false,pass = PointerEventPass.Initial)
            val panel = ComposeOverlay.activePanel ?: return@awaitEachGesture
            val scene = coordinates[0]?.takeIf { it.isAttached } ?: return@awaitEachGesture
            if (veil.panelKey != panel || veil.panelBounds.contains(scene.localToRoot(down.position))) return@awaitEachGesture
            var tap = true
            while (true) {
                val event = awaitPointerEvent(PointerEventPass.Initial)
                val change = event.changes.firstOrNull { it.id == down.id } ?: break
                if (change.isConsumed || event.changes.size != 1 || (change.position-down.position).getDistance() > viewConfiguration.touchSlop ||
                    change.uptimeMillis-down.uptimeMillis >= viewConfiguration.longPressTimeoutMillis) tap = false
                if (!change.pressed) {
                    if (tap && ComposeOverlay.activePanel == panel) {
                        if (panel == "files") FileTransfers.closeSave()
                        ComposeOverlay.activePanel = null
                        // This tap dismisses the card; it cannot also activate
                        // an update button or the reveal affordance beneath it.
                        event.changes.forEach { it.consume() }
                    }
                    break
                }
            }
        }
    }
}
