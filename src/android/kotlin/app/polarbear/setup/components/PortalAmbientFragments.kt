package app.polarbear.setup.components

import androidx.compose.animation.core.*
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathMeasure
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.dp
import app.polarbear.setup.APERTURE_PATH
import app.polarbear.setup.THRESHOLD_PATH
import app.polarbear.setup.PortalColors

/** Static, cached soft vector fragments; only their outer graphics layers drift.
 * Geometry is extracted from the canonical Portal path, never generic ellipses.
 */
@Composable
fun PortalAmbientFragments() {
    val fragments = remember {
        val aperture = PathParser().parsePathString(APERTURE_PATH).toPath()
        val measure = PathMeasure().apply { setPath(aperture, false) }
        listOf(
            Path().also { measure.getSegment(0f, measure.length * 0.56f, it) },
            Path().also { measure.getSegment(measure.length * 0.56f, measure.length, it) },
            PathParser().parsePathString(THRESHOLD_PATH).toPath(),
        )
    }
    val motion = rememberInfiniteTransition(label = "Portal atmosphere")
    // Reverse cycles have matching zero-velocity endpoints, with independent periods.
    val near = motion.animateFloat(-1f, 1f, infiniteRepeatable(tween(8500, easing = FastOutSlowInEasing), RepeatMode.Reverse), label = "near arc")
    val far = motion.animateFloat(-1f, 1f, infiniteRepeatable(tween(11500, easing = FastOutSlowInEasing), RepeatMode.Reverse), label = "far arc")
    val threshold = motion.animateFloat(-1f, 1f, infiniteRepeatable(tween(6500, easing = FastOutSlowInEasing), RepeatMode.Reverse), label = "threshold")
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val base = minOf(maxWidth, maxHeight)
        fragments.forEachIndexed { index, path ->
            val phase = when (index) { 0 -> near; 1 -> far; else -> threshold }
            val extent = base * when (index) { 0 -> 1.25f; 1 -> 1.05f; else -> 0.82f }
            val x = when (index) { 0 -> -extent * 0.48f; 1 -> maxWidth - extent * 0.50f; else -> maxWidth * 0.42f }
            val y = when (index) { 0 -> -extent * 0.12f; 1 -> maxHeight - extent * 0.58f; else -> maxHeight - extent * 0.68f }
            Canvas(Modifier.offset(x, y).size(extent)
                .graphicsLayer {
                    val t = phase.value
                    translationX = (if (index == 1) -16.dp else 22.dp).toPx() * t
                    translationY = (if (index == 2) 14.dp else 20.dp).toPx() * t
                    rotationZ = (if (index == 0) -24f else if (index == 1) 32f else -42f) + t * 3f
                    scaleX = 1f + t * 0.025f
                    scaleY = scaleX
                }
                .blur(if (index == 1) 72.dp else 48.dp)) {
                scale(size.width / 514f, size.height / 514f, Offset.Zero) {
                    drawPath(path, if (index == 2) PortalColors.Orange.copy(alpha = 0.085f)
                        else PortalColors.Ivory.copy(alpha = if (index == 0) 0.052f else 0.035f),
                        style = Stroke(width = 54f))
                }
            }
        }
    }
}
