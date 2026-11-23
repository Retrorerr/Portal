package app.polarbear

fun main() {
    val session = SessionServiceOwnership()
    check(!session.update(0, true))
    check(session.update(1, true) && session.active)
    check(!session.update(1, true))
    check(session.update(2, true) && session.active)
    // A detached native worker cannot stop a replacement session.
    check(!session.update(1, false) && session.owner == 2L && session.active)
    check(!session.update(1, true) && session.owner == 2L && session.active)
    check(session.update(2, false) && !session.active)
    // A queued start callback cannot revive a session that failed before service creation.
    check(!session.update(2, true) && !session.active)
    check(!session.update(2, false))
    // Ending a generation before its delayed start also permanently retires that generation.
    check(session.update(3, false) && !session.active)
    check(!session.update(3, true) && !session.active)
    check(session.update(4, true) && session.active)
    check(!session.update(3, false) && session.active)
    check(session.update(4, false) && !session.active)
    println("Desktop session service ownership tests passed")

    val swap = SessionMemoryPressure.readSwap(sequenceOf(
        "MemAvailable:     589176 kB",
        "SwapTotal:      12582908 kB",
        "SwapFree:        4031536 kB",
    ))
    check(swap == SessionMemoryPressure.Swap(12582908L, 4031536L))
    val total = 11410156L * 1024L
    check(SessionMemoryPressure.isLow(false, 589176L * 1024L, total, swap))
    check(!SessionMemoryPressure.isLow(false, total / 4, total, swap))
    check(!SessionMemoryPressure.isLow(false, total / 32, total, null))
    check(SessionMemoryPressure.isLow(true, total / 32, total, null))
    check(SessionMemoryPressure.readSwap(sequenceOf("SwapTotal: 0 kB", "SwapFree: 0 kB")) == null)
    check(SessionMemoryPressure.readSwap(sequenceOf("SwapTotal: 2 kB", "SwapFree: 3 kB")) == null)

    val memory = SessionMemoryPressure()
    check(!memory.update(true))
    check(!memory.update(false))
    repeat(2) { check(!memory.update(true)) }
    check(memory.update(true))
    check(!memory.update(true))
    repeat(5) { check(!memory.update(false)) }
    check(!memory.update(true))
    repeat(5) { check(!memory.update(false)) }
    check(memory.update(false))
    check(!memory.update(false))
    repeat(2) { check(!memory.update(true)) }
    check(memory.update(true))
    println("Desktop session memory pressure tests passed")
}
