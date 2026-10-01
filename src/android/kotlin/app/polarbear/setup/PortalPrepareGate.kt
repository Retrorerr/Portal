package app.polarbear.setup

// The onboarding stage as the first thing an installed desktop shows, for a
// launch where the Android setting it depends on is not on yet. It is the same
// card, stage and animations as first-run setup; the Return screen takes over
// once the setting is on (or the user chooses to go on).

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.unit.dp
import app.polarbear.setup.components.LocalPortalVeil
import app.polarbear.setup.components.PortalPrepareStage

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
    val screenWidth = LocalConfiguration.current.screenWidthDp.dp
    val compact = screenWidth < PortalDimens.CompactBreakpoint
    var cardBounds by remember { mutableStateOf(Rect.Zero) }
    var rootOrigin by remember { mutableStateOf(Offset.Zero) }

    Box(Modifier.fillMaxSize().onGloballyPositioned { rootOrigin = it.positionInRoot() }) {
        PortalAmbientBackground(
            palette = palette,
            cardBounds = { cardBounds.translate(-rootOrigin) },
            readyPrelude = false,
        )
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(vertical = 32.dp),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Column(
                modifier = Modifier
                    .onGloballyPositioned {
                        cardBounds = Rect(
                            it.positionInRoot(),
                            Size(it.size.width.toFloat(), it.size.height.toFloat()),
                        )
                    }
                    .then(
                        if (compact) {
                            Modifier.padding(horizontal = PortalDimens.CompactScreenGutter).fillMaxWidth()
                        } else {
                            Modifier.width(minOf(PortalDimens.PrepareCardWidth, screenWidth * 0.94f))
                        },
                    )
                    .portalSurface(palette, compact),
            ) {
                SetupHeaderIdentity(
                    palette = palette,
                    launchMarkModifier = launchMarkModifier,
                    title = "Before you continue",
                    compact = compact,
                )
                Spacer(Modifier.height(22.dp))
                PortalPrepareStage(
                    palette = palette,
                    onContinue = onCleared,
                    preview = preview,
                    returning = true,
                )
            }
        }
    }
}
