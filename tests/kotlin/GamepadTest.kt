package app.polarbear.gaming

import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.max

// The on-screen controller off the device: where its controls sit on real
// screens, whole-screen and floating, and how fingers drive the pad.

private class Screen(val name: String, val width: Float, val height: Float, val mm: Float, val left: Float = 0f, val bottom: Float = 0f)

private val screens = listOf(
    Screen("OnePlus 15 landscape", 2772f, 1272f, 17.71f),
    Screen("OnePlus 15 portrait", 1272f, 2772f, 17.71f),
    Screen("OnePlus Pad 3 landscape", 3392f, 2400f, 12.4f),
    Screen("OnePlus Pad 3 portrait", 2400f, 3392f, 12.4f),
    Screen("Fold cover with cutout and bar", 2316f, 904f, 14.9f, left = 80f, bottom = 40f),
)

private fun layout(screen: Screen, wholeScreen: Boolean, scale: Float = 1f, elements: Map<Control, ElementState> = emptyMap()) =
    PadLayout(screen.width, screen.height, screen.left, 0f, 0f, screen.bottom, screen.mm, scale, wholeScreen, elements = elements)

/** Circles a control's shape is made of: (x, y, r); a shoulder is a rectangle instead. */
private fun circles(pad: PadLayout, spot: PadSpot): List<FloatArray> = when (spot.control.kind) {
    ControlKind.FACE -> (0 until 4).map { floatArrayOf(pad.faceButtonX(spot, it), pad.faceButtonY(spot, it), spot.a) }
    ControlKind.BUMPER, ControlKind.TRIGGER -> emptyList()
    else -> listOf(floatArrayOf(spot.x, spot.y, spot.r))
}

/** The gap between two controls' shapes, px; negative when they overlap. */
private fun gap(pad: PadLayout, a: PadSpot, b: PadSpot): Float {
    val shoulder = { spot: PadSpot -> spot.control.kind == ControlKind.BUMPER || spot.control.kind == ControlKind.TRIGGER }
    if (shoulder(a) && shoulder(b)) {
        val dx = abs(a.x - b.x) - (a.w + b.w) / 2f
        val dy = abs(a.y - b.y) - (a.h + b.h) / 2f
        return max(dx, dy)
    }
    if (shoulder(a)) return gap(pad, b, a)
    var best = Float.MAX_VALUE
    for (circle in circles(pad, a)) {
        val distance = if (shoulder(b)) pad.distance(b, circle[0], circle[1]) - circle[2]
        else circles(pad, b).minOf { hypot(circle[0] - it[0], circle[1] - it[1]) - circle[2] - it[2] }
        best = minOf(best, distance)
    }
    return best
}

private fun checkLayouts() {
    for (screen in screens) for (whole in listOf(true, false)) {
        // Every Size the settings offer: the touchpad page's, and the floating layout's (ControllerLayout's range).
        val scales = if (whole) listOf(PadLayout.MIN_USER_SCALE, 1f, PadLayout.MAX_USER_SCALE) else listOf(0.6f, 1f, 1.6f)
        for (scale in scales) {
            val pad = layout(screen, whole, scale)
            val where = "${screen.name}, ${if (whole) "whole screen" else "floating"}, size $scale"
            check(pad.spots.size == Control.entries.size) { where }
            for (spot in pad.spots) {
                check(spot.x - spot.w / 2f >= screen.left - 0.5f && spot.x + spot.w / 2f <= screen.width + 0.5f) { "$where: ${spot.control} off screen" }
                check(spot.y - spot.h / 2f >= -0.5f && spot.y + spot.h / 2f <= screen.height - screen.bottom + 0.5f) { "$where: ${spot.control} off screen" }
            }
            for (i in pad.spots.indices) for (j in 0 until i) {
                val a = pad.spots[i]
                val b = pad.spots[j]
                check(gap(pad, a, b) > 0f) { "$where: ${a.control} overlaps ${b.control} by ${-gap(pad, a, b) / screen.mm} mm" }
            }
        }
    }
    // Size grows the pad up to the room the screen has, and no further: a phone's is full at 100%.
    for (screen in screens) for (whole in listOf(true, false)) {
        val base = layout(screen, whole)
        val room = base.room
        val where = "${screen.name}, ${if (whole) "whole screen" else "floating"}"
        check(room >= 1f) { where }
        check(abs(layout(screen, whole, room).s - base.s * room) < 1e-3f) { "$where: Size grows to its room" }
        check(abs(layout(screen, whole, room + 0.3f).s - layout(screen, whole, room).s) < 1e-3f) { "$where: past its room Size does nothing" }
    }
    for (screen in screens.take(2)) check(layout(screen, false).room == 1f) { "${screen.name}: a phone's pad fills it at 100%" }
    check(layout(screens[2], false).room > 1.4f) { "a tablet held sideways has room to grow" }
    // A thumb's controls are the same size on a phone and a tablet held sideways, within the fit.
    val phone = layout(screens[0], wholeScreen = false).spot(Control.LEFT_STICK)!!
    val tablet = layout(screens[2], wholeScreen = false).spot(Control.LEFT_STICK)!!
    check(phone.w / screens[0].mm in 26f..30f && tablet.w / screens[2].mm in 26f..33f)
}

private fun checkPicks() {
    val phone = screens[0]
    val whole = layout(phone, wholeScreen = true)
    val floating = layout(phone, wholeScreen = false)
    val stick = floating.spot(Control.LEFT_STICK)!!
    // Floating, the open screen below the middle row is the game's; on the whole screen it is the right stick's.
    val openX = floating.middle + 2f * phone.mm
    val openY = phone.height * 0.85f
    check(floating.pick(openX, openY) == null)
    check(whole.pick(openX, openY)?.control == Control.RIGHT_STICK)
    // A landing a little past a control's edge still takes it; further out, it does not.
    check(floating.pick(stick.x + stick.r + stick.reach * 0.9f, stick.y)?.control == Control.LEFT_STICK)
    check(floating.pick(stick.x, stick.y + stick.r + stick.reach * 1.1f)?.control != Control.LEFT_STICK)
    // A thumb landing in the middle of the face diamond, between the buttons, is the cluster's.
    val face = floating.spot(Control.FACE)!!
    check(floating.pick(face.x, face.y)?.control == Control.FACE)
    // Moved, resized and hidden controls.
    val moved = layout(phone, wholeScreen = false, elements = mapOf(
        Control.LEFT_STICK to ElementState(x = 0.7f, y = 0.5f),
        Control.FACE to ElementState(scale = 1.5f),
        Control.GUIDE to ElementState(visible = false),
    ))
    val left = moved.spot(Control.LEFT_STICK)!!
    check(abs(left.x - 0.7f * phone.width) < 0.5f && abs(left.y - 0.5f * phone.height) < 0.5f)
    // Floating, a stick dragged onto the other half still takes its thumb.
    check(moved.pick(left.x, left.y)?.control == Control.LEFT_STICK)
    check(abs(moved.spot(Control.FACE)!!.w - floating.spot(Control.FACE)!!.w * 1.5f) < 0.5f)
    val guide = moved.spot(Control.GUIDE)!!
    check(guide.hidden && moved.pick(guide.x, guide.y)?.control != Control.GUIDE)
    // The layout editor still picks a hidden control, to show it again.
    check(moved.pick(guide.x, guide.y, hidden = true)?.control == Control.GUIDE)
}

private fun checkFeel() {
    val phone = screens[0]
    for (whole in listOf(true, false)) {
        val pad = layout(phone, whole)
        val input = PadInput(phone.mm).also { it.layout = pad }
        val source = Any()
        val stick = pad.spot(Control.LEFT_STICK)!!
        var time = 1000L
        // Lands on the stick: the thumb's spot is its middle, so nothing leans yet.
        check(input.land(source, 1, stick.x, stick.y, time))
        input.move(1, stick.x, stick.y)
        check(input.publish() == PadFeel.Grab && input.axes[0] == 0f && input.axes[1] == 0f)
        // Pushed right past the rim: full tilt right, and the rim is felt.
        input.move(1, stick.x + stick.b * 1.4f, stick.y)
        check(input.publish() == PadFeel.Rim)
        check(abs(input.axes[0] - 1f) < 0.001f && abs(input.axes[1]) < 0.001f)
        // Past the rim the base follows, so turning back answers at once.
        input.move(1, stick.x + stick.b * 1.4f - stick.b * 0.5f, stick.y)
        input.publish()
        check(input.axes[0] < 0.6f)
        input.lift(1, time + 400, cancelled = false)
        input.publish()
        check(input.axes.all { it == 0f } && input.buttons == 0)
        // A quick tap and then a hold clicks it.
        time += 1000
        input.land(source, 2, stick.x, stick.y, time); input.move(2, stick.x, stick.y); input.publish()
        input.lift(2, time + 80, cancelled = false); input.publish()
        input.land(source, 3, stick.x, stick.y, time + 200); input.move(3, stick.x, stick.y)
        check(input.publish() == PadFeel.Click && input.buttons and PadBits.L3 != 0)
        input.lift(3, time + 300, cancelled = false)
        input.publish()
        check(input.buttons == 0)
        // A cancelled tap never primes a click.
        time += 1000
        input.land(source, 4, stick.x, stick.y, time); input.lift(4, time + 50, cancelled = true)
        input.land(source, 5, stick.x, stick.y, time + 150); input.move(5, stick.x, stick.y)
        check(input.publish() != PadFeel.Click && input.buttons == 0)
        input.liftAll(null, time + 200)
        input.publish()
    }

    // Floating, a thumb landing on a stick's rim keeps the base by its home and leans outward.
    val floating = layout(phone, wholeScreen = false)
    val input = PadInput(phone.mm).also { it.layout = floating }
    val stick = floating.spot(Control.LEFT_STICK)!!
    val left = Any()
    val right = Any()
    check(input.land(left, 1, stick.x + stick.r, stick.y, 0L))
    input.move(1, stick.x + stick.r, stick.y)
    input.publish()
    check(input.axes[0] > 0.2f && abs(input.axes[1]) < 0.001f)
    check(abs(input.leftStick.originX - stick.x - floating.drift(stick)) < 0.5f)
    // Dragged far away, the base still stays within its drift.
    input.move(1, stick.x - stick.r * 4f, stick.y)
    input.publish()
    check(abs(input.leftStick.originX - (stick.x - floating.drift(stick))) < 0.5f && abs(input.axes[0] + 1f) < 0.001f)
    // A second thumb on a held stick is meant for nothing (it belongs to the desktop).
    check(!input.land(right, 2, stick.x, stick.y, 10L))

    // Each view's stream is its own: one ending leaves the other's fingers held.
    val other = floating.spot(Control.RIGHT_STICK)!!
    check(input.land(right, 3, other.x, other.y, 20L))
    input.move(3, other.x, other.y + other.b * 2f)
    input.publish()
    check(input.axes[3] > 0.99f && input.axes[0] < -0.99f)
    input.liftAll(left, 30L)
    input.publish()
    check(input.axes[0] == 0f && input.axes[3] > 0.99f)
    // A finger Android dropped without its UP lets go once its view's next event lacks it.
    input.dropMissing(right, 40L) { false }
    input.publish()
    check(input.axes.all { it == 0f } && input.fingerCount == 0)
    // An id still held under one view that lands under another lost its UP: the old one goes.
    input.land(left, 7, stick.x, stick.y, 50L)
    check(input.land(right, 7, other.x, other.y, 60L) && input.fingerCount == 1 && !input.leftStick.held && input.rightStick.held)
    input.liftAll(null, 70L)

    // D-pad: a step on each new direction; the main directions win near a diagonal.
    val dpad = floating.spot(Control.DPAD)!!
    check(input.land(left, 10, dpad.x + dpad.r * 0.8f, dpad.y, 100L))
    input.move(10, dpad.x + dpad.r * 0.8f, dpad.y)
    check(input.publish() == PadFeel.Step && input.hatX == 1 && input.hatY == 0)
    input.move(10, dpad.x + dpad.r * 0.8f, dpad.y + dpad.r * 0.3f)
    input.publish()
    check(input.hatX == 1 && input.hatY == 0)
    input.move(10, dpad.x + dpad.r * 0.6f, dpad.y + dpad.r * 0.6f)
    input.publish()
    check(input.hatX == 1 && input.hatY == 1)
    input.lift(10, 200L, cancelled = false)

    // Face buttons: one under the thumb, both when it sits between two.
    val face = floating.spot(Control.FACE)!!
    val ax = floating.faceButtonX(face, 0)
    val ay = floating.faceButtonY(face, 0)
    check(input.land(left, 11, ax, ay, 300L))
    input.move(11, ax, ay)
    check(input.publish() == PadFeel.Press && input.buttons == PadBits.A)
    val bx = floating.faceButtonX(face, 1)
    val by = floating.faceButtonY(face, 1)
    input.move(11, (ax + bx) / 2f, (ay + by) / 2f)
    input.publish()
    check(input.buttons == PadBits.A or PadBits.B)
    input.lift(11, 400L, cancelled = false)
    input.publish()

    // Triggers: full at once, or analog by how far down the finger is.
    val lt = floating.spot(Control.LT)!!
    check(input.land(left, 12, lt.x, lt.y - lt.h * 0.4f, 500L))
    input.publish()
    check(input.axes[4] == 1f)
    input.analogTriggers = true
    input.publish()
    val light = input.axes[4]
    input.move(12, lt.x, lt.y + lt.h * 0.4f)
    input.publish()
    check(light < 0.5f && input.axes[4] > 0.9f)
    input.liftAll(null, 600L)
    input.publish()
    check(input.axes.all { it == 0f } && input.buttons == 0)

    // Every button reaches its bit and lets go.
    for (control in listOf(Control.LB, Control.RB, Control.BACK, Control.GUIDE, Control.START, Control.L3, Control.R3)) {
        val spot = floating.spot(control)!!
        check(input.land(left, 20, spot.x, spot.y, 700L)) { "$control not taken" }
        check(input.publish() == PadFeel.Press && input.buttons == control.bit) { "$control" }
        input.lift(20, 710L, cancelled = false)
        input.publish()
        check(input.buttons == 0)
    }
}

fun main() {
    checkLayouts()
    checkPicks()
    checkFeel()
    println("Gamepad: layouts on phones, tablets and covers (whole screen and floating), picks, edits, sticks, clicks, d-pad, face buttons, triggers and per-view streams passed")
}
