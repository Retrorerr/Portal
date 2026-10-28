package app.polarbear

/** Geometry only: a remembered capsule never becomes unreachable after resizing. */
internal object CapsulePosition {
    data class Bounds(val left: Float, val top: Float, val right: Float, val bottom: Float) {
        fun clamp(x: Float, y: Float) = x.coerceIn(left, right.coerceAtLeast(left)) to
            y.coerceIn(top, bottom.coerceAtLeast(top))
    }
    fun bounds(width: Float, height: Float, capsuleWidth: Float, capsuleHeight: Float,
        leftInset: Float, topInset: Float, rightInset: Float, bottomInset: Float, margin: Float): Bounds {
        // When even the insets cannot fit, prefer a reachable capsule to an
        // inverted range (e.g. a very short split-screen with the IME open).
        val maxX = (width - capsuleWidth).coerceAtLeast(0f)
        val maxY = (height - capsuleHeight).coerceAtLeast(0f)
        val left = (leftInset + margin).coerceIn(0f, maxX)
        val top = (topInset + margin).coerceIn(0f, maxY)
        return Bounds(left, top, (maxX - rightInset - margin).coerceAtLeast(left),
            (maxY - bottomInset - margin).coerceAtLeast(top))
    }
    fun remembered(bounds: Bounds, width: Float, height: Float, capsuleWidth: Float,
        centerFraction: Float, topFraction: Float): Pair<Float, Float> {
        val center = centerFraction.takeIf { it.isFinite() } ?: 0.5f
        val top = topFraction.takeIf { it.isFinite() && it >= 0 }?.times(height) ?: bounds.top
        return bounds.clamp(center * width - capsuleWidth / 2, top)
    }
}
