package app.polarbear.gaming

fun main() {
    val stick = ControllerPointers(stick = true)
    stick.beginGesture()
    check(stick.down(4, 100f, 80f, 1000))
    check(stick.stickPointer == 4 && !stick.stickClick)
    // Another finger must not take over the stick when its owner lifts.
    check(!stick.down(9, 20f, 30f, 1010))
    stick.up(4, 1050)
    check(stick.stickPointer == -1 && !stick.stickClick)
    check(stick.touches.keys == setOf(9))
    stick.up(9, 1060)
    check(stick.touches.isEmpty())
    // A deliberate second tap clicks L3/R3, and its release also clears it.
    stick.beginGesture(); stick.down(1, 0f, 0f, 1100)
    check(stick.stickClick)
    stick.up(1, 1150)
    check(!stick.stickClick && stick.stickPointer == -1)
    // Lost UP: a move whose pointer set no longer contains the owner releases it.
    stick.beginGesture(); stick.down(7, 90f, 0f, 2000); stick.down(8, 0f, 10f, 2010)
    stick.retain { it == 8 }
    check(stick.stickPointer == -1 && !stick.stickClick && stick.touches.keys == setOf(8))
    stick.clear()
    check(stick.touches.isEmpty() && stick.stickPointer == -1)
    // A new DOWN is a new stream even if Android never cancelled the old one.
    stick.down(3, 100f, 100f, 2100)
    stick.beginGesture(); stick.down(12, 0f, 0f, 2200)
    check(stick.stickPointer == 12 && stick.touches.keys == setOf(12))
    check(!stick.stickClick)
    // Cancellation and focus loss cannot prime an accidental stick click.
    stick.up(12, 2250, cancelled = true)
    stick.beginGesture(); stick.down(15, 0f, 0f, 2300)
    check(!stick.stickClick)
    stick.clear()
    stick.down(15, 0f, 0f, 2400)
    check(!stick.stickClick)
    stick.up(15, 2450)
    stick.beginGesture(); stick.down(0, 0f, 0f, 3000)
    check(!stick.stickClick)
    // Buttons/triggers retain independent fingers and release only the lifted one.
    val buttons = ControllerPointers(stick = false)
    buttons.beginGesture(); buttons.down(2, 1f, 2f, 1000); buttons.down(5, 3f, 4f, 1001)
    buttons.up(2, 1010)
    check(buttons.touches.keys == setOf(5))
    buttons.retain { false }
    check(buttons.touches.isEmpty() && buttons.stickPointer == -1)
    buttons.down(9, 1f, 1f, 1100); buttons.clear()
    check(buttons.touches.isEmpty())
    println("Controller pointers: release, cancellation, lost pointers, multi-touch ownership, new streams and stick clicks passed")
}
