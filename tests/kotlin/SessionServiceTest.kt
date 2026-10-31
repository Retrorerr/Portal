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
}
