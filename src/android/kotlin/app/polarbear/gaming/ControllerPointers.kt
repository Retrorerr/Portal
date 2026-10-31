package app.polarbear.gaming

/** Pointer ownership survives split multi-touch events, but never a new gesture. */
internal class ControllerPointers(private val stick: Boolean) {
    internal class Touch(var x: Float, var y: Float)
    private val tracked = linkedMapOf<Int, Touch>()
    val touches: Map<Int, Touch> get() = tracked
    var stickPointer = -1; private set
    var stickClick = false; private set
    private var lastStickUp: Long? = null

    fun beginGesture() { clear(resetClickHistory = false) }

    /** True only when this finger takes ownership of an idle stick. */
    fun down(id: Int, x: Float, y: Float, time: Long): Boolean {
        tracked[id] = Touch(x, y)
        if (!stick || stickPointer >= 0) return false
        stickPointer = id
        stickClick = lastStickUp?.let { time - it in 0..280L } == true
        return true
    }

    fun up(id: Int, time: Long, cancelled: Boolean = false) {
        tracked.remove(id)
        if (id == stickPointer) {
            stickPointer = -1
            stickClick = false
            lastStickUp = if (cancelled) null else time
        }
    }

    /** Recover if Android drops an individual pointer without its UP event. */
    fun retain(present: (Int) -> Boolean) {
        val iterator = tracked.keys.iterator()
        while (iterator.hasNext()) {
            val id = iterator.next()
            if (!present(id)) {
                iterator.remove()
                if (id == stickPointer) {
                    stickPointer = -1; stickClick = false; lastStickUp = null
                }
            }
        }
    }

    fun clear(resetClickHistory: Boolean = true) {
        tracked.clear()
        stickPointer = -1
        stickClick = false
        if (resetClickHistory) lastStickUp = null
    }
}
