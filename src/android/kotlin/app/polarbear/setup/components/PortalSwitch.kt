package app.polarbear.setup.components

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.unit.dp
import app.polarbear.setup.PortalColors
import app.polarbear.setup.PortalPalette

/** Compact switch for a parent that handles taps and accessibility. */
@Composable
internal fun PortalSwitch(checked: Boolean, palette: PortalPalette) {
    val travel by animateDpAsState(if (checked) 18.dp else 0.dp, spring(dampingRatio = 0.8f, stiffness = 700f), label = "switch thumb")
    val track by animateColorAsState(if (checked) palette.accent else palette.textMuted.copy(alpha = 0.25f), tween(160), label = "switch track")
    Box(
        Modifier
            .size(width = 42.dp, height = 24.dp)
            .clip(RoundedCornerShape(50))
            .background(track)
            .padding(3.dp),
    ) {
        Box(
            Modifier
                .offset(x = travel)
                .size(18.dp)
                .shadow(2.dp, CircleShape)
                .clip(CircleShape)
                .background(PortalColors.Ivory),
        )
    }
}
