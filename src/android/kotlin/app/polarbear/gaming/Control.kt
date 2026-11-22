package app.polarbear.gaming

// The on-screen controller's controls, as plain data: the layout and touch
// logic built on them run off the device too (tests/kotlin/GamepadTest.kt).

/** How a control reads touches and what it draws. */
enum class ControlKind { STICK, DPAD, FACE, BUMPER, TRIGGER, BUTTON }

/** Bits in xpad button order (src/core/gamepad.rs BUTTONS). */
object PadBits {
    const val A = 1 shl 0
    const val B = 1 shl 1
    const val X = 1 shl 2
    const val Y = 1 shl 3
    const val LB = 1 shl 4
    const val RB = 1 shl 5
    const val BACK = 1 shl 6
    const val START = 1 shl 7
    const val GUIDE = 1 shl 8
    const val L3 = 1 shl 9
    const val R3 = 1 shl 10
}

/**
 * Every on-screen control: its saved key, its name, and what it drives.
 * Axes index the pad state laid out like GamepadBridge's (0..3 sticks,
 * 4/5 triggers, 8/9 hat). Sizes and places come from [PadLayout].
 */
enum class Control(
    val key: String,
    val title: String,
    val kind: ControlKind,
    val bit: Int = 0,
) {
    LEFT_STICK("lstick", "Left stick", ControlKind.STICK, bit = PadBits.L3),
    RIGHT_STICK("rstick", "Right stick", ControlKind.STICK, bit = PadBits.R3),
    DPAD("dpad", "D-pad", ControlKind.DPAD),
    FACE("face", "A B X Y", ControlKind.FACE),
    LB("lb", "LB", ControlKind.BUMPER, bit = PadBits.LB),
    RB("rb", "RB", ControlKind.BUMPER, bit = PadBits.RB),
    LT("lt", "LT", ControlKind.TRIGGER),
    RT("rt", "RT", ControlKind.TRIGGER),
    BACK("back", "View", ControlKind.BUTTON, bit = PadBits.BACK),
    GUIDE("guide", "Guide", ControlKind.BUTTON, bit = PadBits.GUIDE),
    START("start", "Menu", ControlKind.BUTTON, bit = PadBits.START),
    L3("l3", "L3", ControlKind.BUTTON, bit = PadBits.L3),
    R3("r3", "R3", ControlKind.BUTTON, bit = PadBits.R3);

    companion object {
        fun byKey(key: String): Control? = entries.firstOrNull { it.key == key }
    }
}

/**
 * Where one control sits once the user moved, resized or hid it. Positions
 * are the control's centre as a fraction of the screen, so a layout keeps
 * its shape through rotation; NaN means the default spot.
 */
data class ElementState(
    val x: Float = Float.NaN,
    val y: Float = Float.NaN,
    val scale: Float = 1f,
    val visible: Boolean = true,
) {
    val placed: Boolean get() = !x.isNaN() && !y.isNaN()
}
