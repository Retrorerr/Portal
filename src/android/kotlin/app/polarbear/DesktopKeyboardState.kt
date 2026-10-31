package app.polarbear

/** Linux evdev keys, independent of Android's IME and hardware key mapping. */
internal data class DesktopKey(val label: String, val code: Int, val shifted: String = label, val weight: Float = 1f)

internal object DesktopKeyboardLayout {
    val modifiers = setOf(42, 54, 29, 56, 125, 100)
    val numbers = "1234567890".mapIndexed { i, c -> DesktopKey(c.toString(), i + 2, "!@#$%^&*()"[i].toString()) }
    private fun letters(text: String, codes: List<Int>) = text.mapIndexed { i, c -> DesktopKey(c.toString(), codes[i], c.uppercase()) }
    val letters = listOf(
        listOf(DesktopKey("Tab", 15, weight = 1.5f)) + letters("qwertyuiop", (16..25).toList()),
        listOf(DesktopKey("Caps", 58, weight = 1.2f)) + letters("asdfghjkl", (30..38).toList()) + DesktopKey("Enter", 28, weight = 1.3f),
        listOf(DesktopKey("Shift", 42, weight = 1.5f)) + letters("zxcvbnm", (44..50).toList()) + listOf(DesktopKey(",", 51, "<"), DesktopKey(".", 52, ">"), DesktopKey("/", 53, "?")),
    )
    val symbols = listOf(
        listOf(DesktopKey("Tab", 15), DesktopKey("`", 41, "~"), DesktopKey("-", 12, "_"), DesktopKey("=", 13, "+"), DesktopKey("[", 26, "{"), DesktopKey("]", 27, "}"), DesktopKey("\\", 43, "|")),
        listOf(DesktopKey("Esc", 1), DesktopKey(";", 39, ":"), DesktopKey("'", 40, "\""), DesktopKey(",", 51, "<"), DesktopKey(".", 52, ">"), DesktopKey("/", 53, "?"), DesktopKey("Enter", 28, weight = 1.5f)),
        listOf(DesktopKey("Shift", 42, weight = 1.5f), DesktopKey("Caps", 58), DesktopKey("AltGr", 100), DesktopKey("Menu", 127), DesktopKey("Ins", 110), DesktopKey("Del", 111)),
    )
    val bottom = listOf(DesktopKey("Esc", 1), DesktopKey("Ctrl", 29), DesktopKey("Super", 125), DesktopKey("Alt", 56), DesktopKey("Space", 57, weight = 3.5f), DesktopKey("←", 105), DesktopKey("↑", 103), DesktopKey("↓", 108), DesktopKey("→", 106))
    val functions = (1..12).map { DesktopKey("F$it", if (it <= 10) 58 + it else 76 + it) }
    val navigation = listOf(DesktopKey("Ins", 110), DesktopKey("Del", 111), DesktopKey("Home", 102), DesktopKey("End", 107), DesktopKey("PgUp", 104), DesktopKey("PgDn", 109))
    val system = listOf(DesktopKey("Caps", 58), DesktopKey("AltGr", 100), DesktopKey("Menu", 127), DesktopKey("PrtSc", 99), DesktopKey("Scroll", 70), DesktopKey("Pause", 119))
    val numpad = listOf(
        listOf(DesktopKey("Num", 69), DesktopKey("÷", 98), DesktopKey("×", 55), DesktopKey("−", 74)),
        listOf(DesktopKey("7", 71), DesktopKey("8", 72), DesktopKey("9", 73), DesktopKey("+", 78)),
        listOf(DesktopKey("4", 75), DesktopKey("5", 76), DesktopKey("6", 77), DesktopKey("Enter", 96)),
        listOf(DesktopKey("1", 79), DesktopKey("2", 80), DesktopKey("3", 81), DesktopKey(".", 83)),
        listOf(DesktopKey("0", 82, weight = 3f), DesktopKey("=", 117)),
    )
}

/** Own every press, including cancellation, sticky modifiers and multi-touch chords. */
internal class DesktopKeyboardState(private val send: (Int, Boolean) -> Unit, private val changed: () -> Unit = {}) {
    private data class ModifierState(var latch: Int = 0, var held: Boolean = false, var used: Boolean = false)
    private val modifiers = DesktopKeyboardLayout.modifiers.associateWith { ModifierState() }
    private val held = mutableSetOf<Int>()
    private val pressed = mutableSetOf<Int>()
    private val locks = mutableSetOf<Int>()
    val capsLock get() = 58 in locks
    fun level(code: Int): Int = modifiers[code]?.let { if (it.held) maxOf(1, it.latch) else it.latch } ?: if (code in locks) 2 else 0
    fun isPressed(code: Int) = code in pressed
    private fun down(code: Int) { if (held.add(code)) send(code, true) }
    private fun up(code: Int) { if (held.remove(code)) send(code, false) }
    fun press(code: Int) {
        if (!pressed.add(code)) return
        val modifier = modifiers[code]
        if (modifier != null) {
            // A scroll may start here too. Stage physical modifiers until a
            // chord needs them or a successful tap arms the sticky state.
            modifier.held = true; modifier.used = false
            if (pressed.any { it !in modifiers }) { modifier.used = true; down(code) }
        } else {
            if (code in setOf(58, 69, 70) && !locks.add(code)) locks.remove(code)
            modifiers.forEach { (key, state) ->
                if (state.held || state.latch > 0) {
                    if (state.held) state.used = true
                    down(key)
                }
            }
            down(code)
        }
        changed()
    }
    fun release(code: Int, cancelled: Boolean = false) {
        if (!pressed.remove(code)) return
        val modifier = modifiers[code]
        if (modifier != null) {
            modifier.held = false
            modifier.latch = when {
                cancelled && !modifier.used -> modifier.latch
                modifier.used -> if (modifier.latch == 2) 2 else 0
                else -> (modifier.latch + 1) % 3
            }
            if (modifier.latch == 0) up(code) else down(code)
        } else {
            up(code)
            if (pressed.none { it !in modifiers }) modifiers.forEach { (key, state) ->
                if (state.latch == 1 && !state.held) { state.latch = 0; up(key) }
            }
        }
        changed()
    }
    fun tap(code: Int) { press(code); release(code) }
    fun shortcut(vararg codes: Int) {
        // Never alter a chord already under the user's fingers.
        if (pressed.isNotEmpty()) return
        reset()
        codes.forEach(::down)
        codes.reversed().forEach(::up)
        changed()
    }
    fun reset() {
        if (held.isEmpty() && pressed.isEmpty() && modifiers.values.none { it.latch != 0 || it.held }) return
        held.toList().asReversed().forEach(::up)
        pressed.clear()
        modifiers.values.forEach { it.latch = 0; it.held = false; it.used = false }
        changed()
    }
}
