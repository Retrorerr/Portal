package app.polarbear.setup.components

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density

/** Keep accessibility text-size changes continuous within Portal's own surfaces. */
@Composable
internal fun PortalUiDensity(content: @Composable () -> Unit) {
    val target = LocalDensity.current
    val fontScale by animateFloatAsState(target.fontScale,tween(360,easing = PortalEmphasized),label = "Portal text scale")
    CompositionLocalProvider(LocalDensity provides Density(target.density,fontScale),content = content)
}
