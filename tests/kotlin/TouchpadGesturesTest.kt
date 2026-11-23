package app.polarbear

import app.polarbear.TouchpadGestures.Point

private class Recorder(motionSlopMm: Float = TouchpadGestures.TAP_SLOP_MM) {
    val log = mutableListOf<String>()
    val gestures = TouchpadGestures(
        move = { _, _ -> if (log.lastOrNull() != "move") log += "move" },
        button = { code, pressed -> log += (if (code == TouchpadGestures.PRIMARY) "L" else "R") + if (pressed) "+" else "-" },
        scroll = { _, _, finished -> log += if (finished) "scroll-end" else "scroll" },
        pinch = { phase, _ -> log += "pinch-$phase" },
        motionSlopMm = motionSlopMm,
    )

    fun tap(at: Long, length: Long = 80) {
        gestures.down(at, Point(10f, 10f), buttonsDown = false)
        gestures.up(at + length)
    }

    fun drag(from: Long, to: Long) {
        gestures.down(from, Point(10f, 10f), buttonsDown = false)
        gestures.moveOne(Point(14f, 10f))
        gestures.moveOne(Point(20f, 12f))
        gestures.up(to)
    }

    fun expect(vararg events: String) {
        check(log == events.toList()) { "expected ${events.toList()} but got $log" }
    }
}

fun main() {
    val gap = TouchpadGestures.TAP_GAP_MS

    // A single tap presses at once and releases when no second touch follows.
    Recorder().apply {
        tap(0)
        expect("L+")
        check(gestures.nextDeadline == 80 + gap)
        gestures.timeout(80 + gap - 1)
        expect("L+")
        gestures.timeout(80 + gap)
        expect("L+", "L-")
        check(gestures.nextDeadline == 0L)
    }

    // Double-tap-and-drag holds one press for the whole drag: no double click.
    Recorder().apply {
        tap(0)
        drag(from = 150, to = 900)
        expect("L+", "move", "L-")
        check(gestures.nextDeadline == 0L)
    }

    // Two quick taps are a double click, the second press also held briefly.
    Recorder().apply {
        tap(0)
        tap(150)
        expect("L+", "L-", "L+")
        gestures.timeout(230 + gap)
        expect("L+", "L-", "L+", "L-")
    }

    // A triple click keeps alternating cleanly.
    Recorder().apply {
        tap(0); tap(140); tap(280)
        gestures.timeout(360 + gap)
        expect("L+", "L-", "L+", "L-", "L+", "L-")
    }

    // A second touch that rests becomes a drag without moving first.
    Recorder().apply {
        tap(0)
        gestures.down(150, Point(10f, 10f), buttonsDown = false)
        gestures.timeout(150 + TouchpadGestures.DRAG_HOLD_MS)
        expect("L+")
        gestures.moveOne(Point(10.5f, 10f))
        gestures.moveOne(Point(18f, 10f))
        gestures.up(1200)
        expect("L+", "move", "L-")
    }

    // After the gap the click is over and the next touch only moves.
    Recorder().apply {
        tap(0)
        gestures.timeout(80 + gap)
        drag(from = 500, to = 700)
        expect("L+", "L-", "move")
    }

    // A late touch ends the click even when the timer has not run yet.
    Recorder().apply {
        tap(0)
        drag(from = 80 + gap + 50, to = 900)
        expect("L+", "L-", "move")
    }

    // Moving or holding is not a tap.
    Recorder().apply {
        drag(from = 0, to = 100)
        gestures.down(500, Point(10f, 10f), buttonsDown = false)
        gestures.up(500 + TouchpadGestures.TAP_TIME_MS + 1)
        expect("move")
        check(gestures.nextDeadline == 0L)
    }

    // Touchscreen motion starts before the tap threshold, and still filters tiny jitter.
    Recorder(motionSlopMm = 0.3f).apply {
        gestures.down(0, Point(10f, 10f), buttonsDown = false)
        gestures.moveOne(Point(10.1f, 10f))
        expect()
        gestures.moveOne(Point(10.5f, 10f))
        expect("move")
        gestures.moveOne(Point(12f, 10f))
        gestures.up(80)
        expect("move")
    }

    // A little movement still permits a tap, including the second tap of a double click.
    Recorder(motionSlopMm = 0.3f).apply {
        gestures.down(0, Point(10f, 10f), buttonsDown = false)
        gestures.moveOne(Point(10.5f, 10f))
        gestures.up(80)
        expect("move", "L+")
        gestures.down(150, Point(10f, 10f), buttonsDown = false)
        gestures.moveOne(Point(10.5f, 10f))
        gestures.up(230)
        expect("move", "L+", "L-", "L+")
    }

    // A two-finger tap is a right click.
    Recorder().apply {
        gestures.down(0, Point(10f, 10f), buttonsDown = false)
        gestures.secondFinger(20, Point(10f, 10f), Point(30f, 10f))
        gestures.fingerLifted(Point(10f, 10f))
        gestures.up(120)
        expect("R+", "R-")
    }

    // Two fingers that travel scroll, and the scroll ends on lift.
    Recorder().apply {
        gestures.down(0, Point(10f, 10f), buttonsDown = false)
        gestures.secondFinger(20, Point(10f, 10f), Point(30f, 10f))
        gestures.moveTwo(40, Point(10f, 14f), Point(30f, 14f))
        gestures.moveTwo(60, Point(10f, 20f), Point(30f, 20f))
        gestures.fingerLifted(Point(10f, 20f))
        gestures.up(300)
        expect("scroll", "scroll", "scroll-end")
    }

    // Scroll deltas run against the fingers, like Android's own gesture distance.
    run {
        val deltas = mutableListOf<Pair<Float, Float>>()
        val gestures = TouchpadGestures(
            move = { _, _ -> }, button = { _, _ -> }, pinch = { _, _ -> },
            scroll = { h, v, finished -> if (!finished) deltas += h to v },
        )
        gestures.down(0, Point(10f, 10f), buttonsDown = false)
        gestures.secondFinger(20, Point(10f, 10f), Point(30f, 10f))
        gestures.moveTwo(40, Point(10f, 14f), Point(30f, 14f))
        gestures.moveTwo(60, Point(6f, 20f), Point(26f, 20f))
        check(deltas.isNotEmpty() && deltas.all { it.second < 0f }) { "fingers down should scroll negative: $deltas" }
        check(deltas.last().first > 0f) { "fingers left should scroll positive: $deltas" }
    }

    // Spreading two fingers pinches.
    Recorder().apply {
        gestures.down(0, Point(10f, 10f), buttonsDown = false)
        gestures.secondFinger(20, Point(10f, 10f), Point(20f, 10f))
        gestures.moveTwo(40, Point(5f, 10f), Point(25f, 10f))
        gestures.fingerLifted(Point(5f, 10f))
        expect("pinch-Begin", "pinch-Update", "pinch-End")
    }

    // A tap followed by two fingers finishes the click before scrolling.
    Recorder().apply {
        tap(0)
        gestures.down(150, Point(10f, 10f), buttonsDown = false)
        gestures.secondFinger(160, Point(10f, 10f), Point(30f, 10f))
        expect("L+", "L-")
        check(gestures.nextDeadline == 0L)
    }

    // A physical click ends a held tap; the caller reports the click itself.
    Recorder().apply {
        tap(0)
        gestures.physicalButton(TouchpadGestures.PRIMARY, true)
        expect("L+", "L-")
        gestures.down(150, Point(10f, 10f), buttonsDown = true)
        gestures.up(200)
        gestures.physicalButton(TouchpadGestures.PRIMARY, false)
        expect("L+", "L-")
        check(gestures.nextDeadline == 0L)
    }

    // The click that took control cannot tap on the desktop.
    Recorder().apply {
        gestures.startCapture(1000)
        tap(1020, length = 40)
        expect()
        tap(1200)
        expect("L+")
    }

    // Cancelling mid-drag releases the button.
    Recorder().apply {
        tap(0)
        gestures.down(150, Point(10f, 10f), buttonsDown = false)
        gestures.moveOne(Point(20f, 10f))
        gestures.cancel()
        expect("L+", "move", "L-")
        check(gestures.nextDeadline == 0L)
    }

    println("TouchpadGestures: all checks passed")
}
