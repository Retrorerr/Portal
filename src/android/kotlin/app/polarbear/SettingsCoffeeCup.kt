package app.polarbear

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.RoundRect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.unit.dp
import app.polarbear.setup.PortalColors
import app.polarbear.setup.PortalPalette
import kotlin.math.PI
import kotlin.math.sin

/** A steaming cup that pops out from behind the switch while the desktop holds the screen awake. */
@Composable
internal fun SettingsCoffeeCup(visible: Boolean, palette: PortalPalette) {
    val pop by animateFloatAsState(
        if (visible) 1f else 0f,
        if (visible) spring(dampingRatio = 0.38f, stiffness = 380f) else tween(140),
        label = "coffee cup",
    )
    // The slot keeps its size, so the row's text never reflows as the cup comes and goes.
    Box(Modifier.padding(end = 8.dp).size(width = 22.dp, height = 28.dp)) {
        if (pop <= 0.01f) return@Box
        val steam = steamPhase()
        Canvas(
            Modifier.fillMaxSize().graphicsLayer {
                // Starts tucked under the switch's left edge and springs out past its resting place.
                translationX = (1f - pop) * 18.dp.toPx()
                scaleX = pop.coerceAtLeast(0f)
                scaleY = pop.coerceAtLeast(0f)
                alpha = pop.coerceIn(0f, 1f)
                transformOrigin = TransformOrigin(1f, 0.85f)
            },
        ) {
            val unit = 1.dp.toPx()
            drawSteam(steam.value, 7.2f * unit, unit, palette)
            drawSteam(steam.value + 0.45f, 11.8f * unit, unit, palette)
            drawCup(unit, palette)
        }
    }
}

@Composable
private fun steamPhase(): State<Float> = rememberInfiniteTransition(label = "coffee steam").animateFloat(
    0f, 1f, infiniteRepeatable(tween(2200, easing = LinearEasing), RepeatMode.Restart), label = "steam phase",
)

private fun DrawScope.drawCup(unit: Float, palette: PortalPalette) {
    val body = Rect(3f * unit, 14f * unit, 16f * unit, 25f * unit)
    val handle = Stroke(1.8f * unit, cap = StrokeCap.Round)
    drawArc(
        palette.accent, startAngle = -80f, sweepAngle = 160f, useCenter = false,
        topLeft = Offset(13f * unit, 16.2f * unit), size = Size(6f * unit, 6f * unit), style = handle,
    )
    drawPath(
        Path().apply {
            addRoundRect(
                RoundRect(
                    body,
                    topLeft = CornerRadius(1.5f * unit), topRight = CornerRadius(1.5f * unit),
                    bottomLeft = CornerRadius(5.5f * unit), bottomRight = CornerRadius(5.5f * unit),
                ),
            )
        },
        palette.accent,
    )
    // A sleepy little face.
    val face = PortalColors.Charcoal
    drawCircle(face, 0.8f * unit, Offset(7.3f * unit, 18.6f * unit))
    drawCircle(face, 0.8f * unit, Offset(11.7f * unit, 18.6f * unit))
    drawArc(
        face, startAngle = 20f, sweepAngle = 140f, useCenter = false,
        topLeft = Offset(8.2f * unit, 18.6f * unit), size = Size(2.6f * unit, 2.4f * unit),
        style = Stroke(0.8f * unit, cap = StrokeCap.Round),
    )
}

/** One wisp: the wave travels upward and fades in and out along its length, so the loop has no seam. */
private fun DrawScope.drawSteam(phase: Float, centerX: Float, unit: Float, palette: PortalPalette) {
    val bottom = 12f * unit
    val length = 10f * unit
    val segments = 10
    var previous: Offset? = null
    for (step in 0..segments) {
        val along = step / segments.toFloat()
        val point = Offset(
            centerX + 1.3f * unit * sin(2f * PI.toFloat() * (along * 1.1f - phase)),
            bottom - along * length,
        )
        previous?.let { start ->
            val fade = sin(PI.toFloat() * (along - 0.5f / segments))
            drawLine(palette.textMuted.copy(alpha = 0.7f * fade), start, point, 1.3f * unit, StrokeCap.Round)
        }
        previous = point
    }
}
