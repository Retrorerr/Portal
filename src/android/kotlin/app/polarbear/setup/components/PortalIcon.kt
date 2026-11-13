package app.polarbear.setup.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

internal enum class PortalGlyph { Veil, Report, Files, Apps, Keyboard, Controller, Options, Settings, Close, Import, Export, Parent, Home }

/** The same small stroke, corner and proportions across Portal's utility actions. */
@Composable
internal fun PortalIcon(glyph: PortalGlyph, color: Color, modifier: Modifier = Modifier, size: Dp = 20.dp) {
    Canvas(modifier.size(size)) {
        scale(this.size.width / 24f, this.size.height / 24f, pivot = Offset.Zero) {
            val stroke = Stroke(1.5f, cap = StrokeCap.Round)
            fun line(x: Float, y: Float, x2: Float, y2: Float) = drawLine(color, Offset(x,y), Offset(x2,y2), 1.5f, StrokeCap.Round)
            fun box(x: Float, y: Float, w: Float, h: Float, r: Float = 2f) = drawRoundRect(color,
                Offset(x,y), Size(w,h), CornerRadius(r), style = stroke)
            when (glyph) {
                PortalGlyph.Files -> drawPath(Path().apply {
                    moveTo(2f,6f); lineTo(9f,6f); lineTo(11f,8f); lineTo(22f,8f)
                    lineTo(22f,20f); lineTo(2f,20f); close()
                }, color, style = stroke)
                PortalGlyph.Apps -> for (x in listOf(3f,14f)) for (y in listOf(3f,14f)) box(x,y,7f,7f)
                PortalGlyph.Keyboard -> {
                    box(2f,5f,20f,14f)
                    for (y in listOf(9f,12f)) for (x in listOf(6f,10f,14f,18f)) line(x,y,x+1f,y)
                    line(7f,16f,17f,16f)
                }
                PortalGlyph.Veil -> { box(4f,2f,16f,20f,3f); line(8f,7f,12f,11f); line(12f,11f,16f,7f); line(8f,16f,16f,16f) }
                PortalGlyph.Report -> {
                    line(6f,3f,6f,21f); line(6f,4f,18f,4f); line(18f,4f,15f,8.5f)
                    line(15f,8.5f,18f,13f); line(18f,13f,6f,13f)
                }
                PortalGlyph.Controller -> {
                    box(2f,6f,20f,12f,5f); line(6f,12f,10f,12f); line(8f,10f,8f,14f)
                    line(15f,10f,17f,10f); line(17f,14f,19f,14f)
                }
                PortalGlyph.Options -> for ((y,x) in listOf(5f to 8f,12f to 16f,19f to 10f)) {
                    line(3f,y,x-2f,y); line(x+2f,y,21f,y); box(x-2f,y-2f,4f,4f,1f)
                }
                PortalGlyph.Settings -> {
                    val gear = Path().apply {
                        moveTo(10f, 2f)
                        lineTo(14f, 2f); lineTo(14.7f, 5f); lineTo(16.5f, 6f)
                        lineTo(19.5f, 5.2f); lineTo(21.5f, 8.8f); lineTo(19.3f, 11f)
                        lineTo(19.3f, 13f); lineTo(21.5f, 15.2f); lineTo(19.5f, 18.8f)
                        lineTo(16.5f, 18f); lineTo(14.7f, 19f); lineTo(14f, 22f)
                        lineTo(10f, 22f); lineTo(9.3f, 19f); lineTo(7.5f, 18f)
                        lineTo(4.5f, 18.8f); lineTo(2.5f, 15.2f); lineTo(4.7f, 13f)
                        lineTo(4.7f, 11f); lineTo(2.5f, 8.8f); lineTo(4.5f, 5.2f)
                        lineTo(7.5f, 6f); lineTo(9.3f, 5f)
                        close()
                    }
                    drawPath(gear, color, style = stroke)
                    drawCircle(color, radius = 3f, center = Offset(12f, 12f), style = stroke)
                }
                PortalGlyph.Close -> { line(6f,6f,18f,18f); line(6f,18f,18f,6f) }
                PortalGlyph.Import, PortalGlyph.Export -> {
                    line(3f,16f,3f,21f); line(3f,21f,21f,21f); line(21f,21f,21f,16f)
                    line(12f,3f,12f,15f)
                    val y = if (glyph == PortalGlyph.Import) 15f else 3f
                    val tail = if (glyph == PortalGlyph.Import) 10f else 8f
                    line(7f,tail,12f,y); line(12f,y,17f,tail)
                }
                PortalGlyph.Parent -> { line(5f,13f,12f,6f); line(12f,6f,19f,13f); line(12f,6f,12f,21f) }
                PortalGlyph.Home -> { line(2f,11f,12f,3f); line(12f,3f,22f,11f); line(5f,9f,5f,21f)
                    line(5f,21f,19f,21f); line(19f,21f,19f,9f); line(10f,21f,10f,15f); line(10f,15f,14f,15f); line(14f,15f,14f,21f) }
            }
        }
    }
}
