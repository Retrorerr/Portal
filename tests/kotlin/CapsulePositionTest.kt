package app.polarbear

fun main() {
    val bounds = CapsulePosition.bounds(1000f, 700f, 120f, 48f, 24f, 30f, 20f, 25f, 8f)
    check(bounds.clamp(-50f, -50f) == 32f to 38f)
    check(bounds.clamp(2000f, 2000f) == 852f to 619f)
    check(CapsulePosition.remembered(bounds, 1000f, 700f, 120f, 0.5f, -1f) == 440f to 38f)
    val small = CapsulePosition.bounds(300f, 200f, 180f, 48f, 0f, 40f, 0f, 240f, 8f)
    check(small.clamp(500f, 500f) == 112f to 48f)
    val tiny = CapsulePosition.bounds(80f, 30f, 120f, 48f, 30f, 40f, 30f, 40f, 8f)
    check(tiny.clamp(-100f, 300f) == 0f to 0f)
    check(CapsulePosition.remembered(bounds, 1000f, 700f, 120f, Float.NaN, Float.POSITIVE_INFINITY) == 440f to 38f)
    val rotated = CapsulePosition.bounds(700f, 1000f, 180f, 48f, 0f, 30f, 0f, 25f, 8f)
    val position = CapsulePosition.remembered(rotated, 700f, 1000f, 180f, 0.98f, 0.98f)
    check(position == 512f to 919f)
    println("Capsule bounds, rotation, keyboard and saved-position checks passed")
}
