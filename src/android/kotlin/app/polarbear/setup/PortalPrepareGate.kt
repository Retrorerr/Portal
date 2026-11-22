package app.polarbear.setup

// The setup stage for the Android setting the desktop depends on, as the
// first thing an installed desktop shows when that setting is not on yet. It
// is the same card and stage as first-run setup; the Return screen takes over
// once the setting is on (or the user chooses to go on).

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.ExperimentalSharedTransitionApi
import androidx.compose.animation.SharedTransitionLayout
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import app.polarbear.setup.components.LocalPortalVeil
import app.polarbear.setup.components.rememberPrepareState

@OptIn(ExperimentalSharedTransitionApi::class)
@Composable
internal fun PortalPrepareGate(
    launchMarkModifier: Modifier,
    onCleared: () -> Unit,
    // Debug preview: behaves as if the setting were off and unreadable.
    preview: Boolean = false,
) {
    val palette = resolvePalette(AppearanceMode.System)
    val veil = LocalPortalVeil.current
    SideEffect { veil.ink = palette.textPrimary }
    val cardBounds = remember { mutableStateOf(Rect.Zero) }
    var rootOrigin by remember { mutableStateOf(Offset.Zero) }
    val prepare = rememberPrepareState(preview = preview)

    Box(Modifier.fillMaxSize().onGloballyPositioned { rootOrigin = it.positionInRoot() }) {
        PortalAmbientBackground(
            palette = palette,
            cardBounds = { cardBounds.value.translate(-rootOrigin) },
            readyPrelude = false,
        )
        SetupCard(palette = palette, cardBounds = cardBounds) { metrics ->
            SharedTransitionLayout(Modifier.fillMaxSize()) {
                AnimatedVisibility(visible = true) {
                    StageScope(
                        shared = this@SharedTransitionLayout,
                        visibility = this,
                        metrics = metrics,
                        palette = palette,
                        launchMarkModifier = launchMarkModifier,
                    ).PrepareStage(state = prepare, onContinue = onCleared, returning = true)
                }
            }
        }
    }
}
