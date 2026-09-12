package app.polarbear.setup

// Setup-only estimates in decimal MB of installed footprint, not download size.
// They deliberately do not query provisioning or change native runtime behavior.
data class EssentialApp(val id: String, val name: String, val blurb: String, val installedMb: Int)

val ESSENTIAL_APPS = listOf(
    EssentialApp("libreoffice", "LibreOffice", "Office suite", 770),
    EssentialApp("vlc", "VLC", "Media player", 215),
    EssentialApp("gimp", "GIMP", "Image editor", 300),
    EssentialApp("krita", "Krita", "Digital painting", 590),
    EssentialApp("inkscape", "Inkscape", "Vector graphics", 260),
    EssentialApp("thunderbird", "Thunderbird", "Email client", 355),
    EssentialApp("okular", "Okular", "Document viewer", 110),
    EssentialApp("kate", "Kate", "Text editor", 75),
)
val DEFAULT_ESSENTIALS: Set<String> = emptySet()
const val MINIMAL_INSTALLED_MB = 5400L

fun selectedApps(ids: Set<String>) = ESSENTIAL_APPS.filter { it.id in ids }
fun projectedInstallBytes(ids: Set<String>): Long =
    (MINIMAL_INSTALLED_MB + selectedApps(ids).sumOf { it.installedMb }) * 1_000_000L

fun selectedAppsSummary(ids: Set<String>): String {
    val names = selectedApps(ids).map { it.name }
    return if (names.isEmpty()) "Optional desktop applications"
    else names.take(2).joinToString(", ") + if (names.size > 2) " +${names.size - 2}" else ""
}

/** Available means allocatable to the app, excluding filesystem reservations.
 * The neutral segment includes those reservations so all three parts sum to total.
 */
data class StorageCapacity(val totalBytes: Long, val availableBytes: Long) {
    init {
        require(totalBytes > 0 && availableBytes in 0..totalBytes)
    }
    val usedBytes: Long get() = totalBytes - availableBytes
    fun projection(installBytes: Long): StorageProjection {
        val allocated = installBytes.coerceIn(0L, availableBytes)
        return StorageProjection(usedBytes, allocated, availableBytes - allocated,
            (installBytes - allocated).coerceAtLeast(0L), totalBytes)
    }
}
data class StorageProjection(
    val usedBytes: Long, val portalBytes: Long, val freeAfterBytes: Long,
    val shortfallBytes: Long, val totalBytes: Long,
)
