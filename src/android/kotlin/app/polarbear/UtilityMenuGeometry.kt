package app.polarbear

import kotlin.math.cos
import kotlin.math.sin

/** Pure placement and release policy shared by the native touch host and Compose. */
internal object UtilityMenuGeometry {
    data class Point(val x: Float, val y: Float)
    data class Area(val left: Float, val top: Float, val right: Float, val bottom: Float) {
        val width get() = (right-left).coerceAtLeast(0f)
        val height get() = (bottom-top).coerceAtLeast(0f)
    }
    data class Menu(val center: Point, val offsets: List<Point>, val compact: Boolean, val tileWidth: Float, val tileHeight: Float)
    fun dock(area: Area, right: Boolean, y: Float, density: Float, fontScale: Float = 1f): Point {
        val half = (26f + 8f*(fontScale-1f).coerceAtLeast(0f)) * density
        return Point(if (right) area.right - 20f*density else area.left + 20f*density,
            y.coerceIn(area.top + half.coerceAtMost(area.height/2), (area.bottom-half).coerceAtLeast(area.top+area.height.coerceAtMost(half*2)/2)))
    }
    fun released(area: Area, position: Point, velocity: Point, density: Float, fontScale: Float = 1f): Pair<Point, Boolean> {
        val projectedX = position.x + velocity.x.coerceIn(-2400*density,2400*density)*0.13f
        val projectedY = position.y + velocity.y.coerceIn(-2000*density,2000*density)*0.13f
        val right = projectedX >= (area.left+area.right)/2
        return dock(area, right, projectedY, density, fontScale) to right
    }
    fun menu(area: Area, anchor: Point, count: Int, density: Float, fontScale: Float): Menu {
        val tileWidth = (88f + 48f*(fontScale-1f).coerceAtLeast(0f))*density
        val tileHeight = tileWidth
        val spacingRadius = if (count > 1) (tileWidth + 8f*density) / (2 * sin(Math.PI / count)).toFloat() else 0f
        val radius = maxOf(120f*density, tileWidth + 16f*density, spacingRadius)
        val halfW = radius + tileWidth/2 + 8*density
        val halfH = radius + tileHeight/2 + 8*density
        val compact = area.width < halfW*2 || area.height < halfH*2
        val center = if (compact) Point((area.left+area.right)/2, (area.top+area.bottom)/2) else Point(
            anchor.x.coerceIn(area.left+halfW, area.right-halfW),
            anchor.y.coerceIn(area.top+halfH, area.bottom-halfH))
        val offsets = (0 until count).map { index ->
            val angle = Math.toRadians((-90.0 + index*360.0/count))
            Point(cos(angle).toFloat()*radius, sin(angle).toFloat()*radius)
        }
        return Menu(center, offsets, compact, tileWidth, tileHeight)
    }
}
