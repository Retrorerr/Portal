package app.polarbear

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.RoundRect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
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
    val pop = animateFloatAsState(
        if (visible) 1f else 0f,
        if (visible) spring(dampingRatio = 0.53f, stiffness = 380f) else tween(140),
        label = "coffee cup",
    )
    // Keep the slot measured even when the cup and steam are hidden.
    val slot = Modifier.padding(end = 8.dp).size(width = 22.dp, height = 28.dp)
    val showCup by remember { derivedStateOf { pop.value > 0.01f } }
    if (!showCup) {
        Spacer(slot)
        return
    }

    val steam by rememberInfiniteTransition(label = "coffee steam").animateFloat(
        0f, 1f, infiniteRepeatable(tween(2200, easing = LinearEasing)), label = "steam phase",
    )
    Spacer(slot.graphicsLayer {
        val progress = pop.value.coerceAtLeast(0f)
        translationX = (1f - progress) * 18.dp.toPx()
        scaleX = progress
        scaleY = progress
        alpha = progress.coerceAtMost(1f)
        transformOrigin = TransformOrigin(1f, 0.85f)
    }.drawWithCache {
        val unit = 1.dp.toPx()
        val rim = CornerRadius(1.5f * unit)
        val base = CornerRadius(5.5f * unit)
        val cup = Path().apply {
            addRoundRect(RoundRect(Rect(3f * unit, 14f * unit, 16f * unit, 25f * unit), rim, rim, base, base))
        }
        val handle = Stroke(1.8f * unit, cap = StrokeCap.Round)
        val smile = Stroke(0.8f * unit, cap = StrokeCap.Round)
        val segments = 10
        val steamColors = List(segments) { segment ->
            val fade = sin(PI.toFloat() * (segment + 0.5f) / segments)
            palette.textMuted.copy(alpha = 0.7f * fade)
        }
        onDrawBehind {
            val phase = steam
            drawSteam(phase, 7.2f * unit, unit, steamColors)
            drawSteam(phase + 0.45f, 11.8f * unit, unit, steamColors)
            drawArc(
                palette.accent, startAngle = -80f, sweepAngle = 160f, useCenter = false,
                topLeft = Offset(13f * unit, 16.2f * unit), size = Size(6f * unit, 6f * unit), style = handle,
            )
            drawPath(cup, palette.accent)
            val face = PortalColors.Charcoal
            drawCircle(face, 0.8f * unit, Offset(7.3f * unit, 18.6f * unit))
            drawCircle(face, 0.8f * unit, Offset(11.7f * unit, 18.6f * unit))
            drawArc(
                face, startAngle = 20f, sweepAngle = 140f, useCenter = false,
                topLeft = Offset(8.2f * unit, 18.6f * unit), size = Size(2.6f * unit, 2.4f * unit), style = smile,
            )
        }
    })
}

private fun DrawScope.drawSteam(phase: Float, centerX: Float, unit: Float, colors: List<Color>) {
    val width = 1.3f * unit
    fun point(along: Float) = Offset(
        centerX + width * sin(2f * PI.toFloat() * (along * 1.1f - phase)),
        (12f - along * 10f) * unit,
    )
    var start = point(0f)
    for (segment in colors.indices) {
        val end = point((segment + 1f) / colors.size)
        drawLine(colors[segment], start, end, width, StrokeCap.Round)
        start = end
    }
}
