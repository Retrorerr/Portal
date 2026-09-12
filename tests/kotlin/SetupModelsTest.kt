package app.polarbear.setup

// Host-only invariants. Run with Kotlin/JVM; no device or UI automation.
fun main() {
    val capacities = listOf(
        StorageCapacity(128_000_000_000L, 91_000_000_000L),
        StorageCapacity(512_000_000_000L, 400_000_000_000L),
        StorageCapacity(16_000_000_000L, 2_000_000_000L),
        StorageCapacity(16_000_000_000L, 0L),
    )
    for (mask in 0 until (1 shl ESSENTIAL_APPS.size)) {
        val ids = ESSENTIAL_APPS.filterIndexed { index, _ -> mask and (1 shl index) != 0 }.map { it.id }.toSet()
        val requested = projectedInstallBytes(ids)
        for (capacity in capacities) {
            val result = capacity.projection(requested)
            check(result.usedBytes + result.portalBytes + result.freeAfterBytes == result.totalBytes)
            check(result.freeAfterBytes >= 0 && result.portalBytes <= capacity.availableBytes)
            check(result.portalBytes + result.shortfallBytes == requested)
        }
        for (app in ESSENTIAL_APPS.filter { it.id !in ids }) {
            check(projectedInstallBytes(ids + app.id) - requested == app.installedMb * 1_000_000L)
        }
    }
    check(selectedAppsSummary(emptySet()) == "Optional desktop applications")
    check(selectedAppsSummary(setOf("libreoffice", "vlc", "gimp", "krita")) == "LibreOffice, VLC +2")
    check(projectedInstallBytes(setOf("unknown")) == projectedInstallBytes(emptySet()))
    check(runCatching { StorageCapacity(0L, 0L) }.isFailure)
    check(runCatching { StorageCapacity(10L, 11L) }.isFailure)
    println("PASS: 256 selections across 4 capacities; conservation, exhaustion, per-app deltas and summaries")
}
