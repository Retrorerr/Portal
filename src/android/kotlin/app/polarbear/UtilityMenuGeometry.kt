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
        val margin = 8f*density
        val spacingRadius = if (count > 1) (tileWidth + 8f*density) / (2 * sin(Math.PI / count)).toFloat() else 0f
        // The least ring that keeps the tiles apart and clear of the close button.
        val leastRadius = maxOf(tileWidth + 16f*density, spacingRadius)
        // The ring on a unit circle, from the top. An odd count reaches further
        // up than down, so it is fitted by its own bounds, not a full circle's.
        val unit = (0 until count).map { index ->
            val angle = Math.toRadians((-90.0 + index*360.0/count))
            Point(cos(angle).toFloat(), sin(angle).toFloat())
        }
        val up = -(unit.minOfOrNull { it.y } ?: 0f).coerceAtMost(0f)
        val down = (unit.maxOfOrNull { it.y } ?: 0f).coerceAtLeast(0f)
        val left = -(unit.minOfOrNull { it.x } ?: 0f).coerceAtMost(0f)
        val right = (unit.maxOfOrNull { it.x } ?: 0f).coerceAtLeast(0f)
        // Where the screen is short (a phone on its side), the ring draws in
        // before the menu gives way to the compact card.
        fun fits(room: Float, reach: Float) = if (reach > 0f) (room - tileWidth - margin*2) / reach else Float.MAX_VALUE
        val ringRadius = maxOf(120f*density, leastRadius)
        val radius = minOf(ringRadius, fits(area.height, up + down), fits(area.width, left + right))
        val compact = radius < leastRadius
        // Not coerceIn: a ring that just fits can leave the bounds a rounding error apart.
        val center = if (compact) Point((area.left+area.right)/2, (area.top+area.bottom)/2) else Point(
            anchor.x.coerceAtLeast(area.left + left*radius + tileWidth/2 + margin).coerceAtMost(area.right - right*radius - tileWidth/2 - margin),
            anchor.y.coerceAtLeast(area.top + up*radius + tileHeight/2 + margin).coerceAtMost(area.bottom - down*radius - tileHeight/2 - margin))
        val ring = if (compact) ringRadius else radius
        val offsets = unit.map { Point(it.x*ring, it.y*ring) }
        return Menu(center, offsets, compact, tileWidth, tileHeight)
    }
}
