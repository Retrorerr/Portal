package app.polarbear

/** Session generations are terminal: a delayed start/stop cannot revive or end another desktop. */
internal class SessionServiceOwnership {
    var owner: Long = 0L
        private set
    var active: Boolean = false
        private set

    fun update(generation: Long, running: Boolean): Boolean {
        if (generation <= 0L || generation < owner) return false
        if (generation == owner) {
            if (!active || running) return false
            active = false
            return true
        }
        owner = generation
        active = running
        return true
    }
}
