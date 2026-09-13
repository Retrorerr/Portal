package app.polarbear.setup

// Return-to-Plasma screen for already-installed launches (cold start after
// force-close included). This is intentionally minimal: the EXACT shared
// PortalAmbientBackground (same charcoal, same seven blurred fragments, same
// drift, same 720 ms scatter, same 1.0 -> 0.83 alpha), the official Portal
// mark, and two lines of text. No card, no settings, no install controls,
// no progress indicator.
//
// READY uses the SAME path as the installer: a simple 3 second timer,
// started once this screen is presented, sets the shared readyPrelude, which
// drives the identical fragment scatter + translucency + PortalRevealVeil +
// swipe affordance through PortalLaunchTransition. No second animation
// implementation anywhere.

import android.util.Log
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay

private const val TAG = "PortalReturn"
private const val RETURN_READY_DELAY_MS = 3_000L

@Composable
internal fun PortalReturnScreen(
    launchMarkModifier: Modifier,
    onReturnReady: () -> Unit = {},
) {
    val palette = resolvePalette(AppearanceMode.System)
    var returnReady by remember { mutableStateOf(false) }
    val currentReturnReady by rememberUpdatedState(onReturnReady)
    LaunchedEffect(Unit) {
        delay(RETURN_READY_DELAY_MS)
        returnReady = true
        Log.i(TAG, "return READY; starting ambient scatter and translucent veil prelude")
        currentReturnReady()
    }
    Box(modifier = Modifier.fillMaxSize()) {
        PortalAmbientBackground(
            palette = palette,
            cardBounds = { Rect.Zero },
            readyPrelude = returnReady,
        )
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(horizontal = 32.dp),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Image(
                painter = portalMarkPainter(
                    main = palette.logoMain,
                    threshold = palette.logoThreshold,
                ),
                contentDescription = "Portal logo",
                modifier = Modifier.size(PortalDimens.LogoSize).then(launchMarkModifier),
            )
            Spacer(modifier = Modifier.height(20.dp))
            Text(
                text = "Returning to Plasma",
                fontSize = PortalDimens.TitleSize,
                fontWeight = FontWeight.SemiBold,
                color = palette.textPrimary,
                textAlign = TextAlign.Center,
            )
            Spacer(modifier = Modifier.height(8.dp))
            Text(
                text = "Restoring your desktop",
                fontSize = 14.sp,
                fontWeight = FontWeight.Medium,
                color = palette.textMuted,
                textAlign = TextAlign.Center,
            )
        }
    }
}
