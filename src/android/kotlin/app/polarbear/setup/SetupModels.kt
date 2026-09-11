package app.polarbear.setup

// SPIKE-ONLY (branch compose-setup-spike): local preview models for the
// CONFIGURE screen. All values are mock; nothing here touches package
// management, storage queries, or Rust.

data class EssentialApp(
    val id: String,
    val name: String,
    val blurb: String,
    val sizeMb: Int,
)

val ESSENTIAL_APPS: List<EssentialApp> = listOf(
    EssentialApp("libreoffice", "LibreOffice", "Office suite", 650),
    EssentialApp("vlc", "VLC", "Media player", 180),
    EssentialApp("gimp", "GIMP", "Image editor", 250),
    EssentialApp("krita", "Krita", "Digital painting", 500),
    EssentialApp("inkscape", "Inkscape", "Vector graphics", 220),
    EssentialApp("thunderbird", "Thunderbird", "Email client", 300),
    EssentialApp("okular", "Okular", "Document viewer", 90),
    EssentialApp("kate", "Kate", "Text editor", 60),
)

val DEFAULT_ESSENTIALS: Set<String> = setOf("libreoffice", "vlc")

const val MINIMAL_DOWNLOAD_MB: Int = 1900
const val MINIMAL_INSTALLED_MB: Int = 5400
const val MOCK_FREE_GB: String = "91"

fun selectedApps(ids: Set<String>): List<EssentialApp> =
    ESSENTIAL_APPS.filter { it.id in ids }

fun extrasDownloadMb(ids: Set<String>): Int =
    selectedApps(ids).sumOf { it.sizeMb }

fun extrasInstalledMb(ids: Set<String>): Int =
    selectedApps(ids).sumOf { (it.sizeMb * 1.18).toInt() }

fun formatGb(mb: Int): String {
    return if (mb >= 1000) {
        val whole = mb / 1000
        val tenth = (mb % 1000) / 100
        "$whole.$tenth"
    } else {
        "$mb MB"
    }
}

fun formatStorageLine(ids: Set<String>): Triple<String, String, String> {
    val download = formatGb(MINIMAL_DOWNLOAD_MB + extrasDownloadMb(ids))
    val installed = formatGb(MINIMAL_INSTALLED_MB + extrasInstalledMb(ids))
    return Triple(download, installed, MOCK_FREE_GB)
}

fun formatExtrasDelta(ids: Set<String>): String {
    val mb = extrasDownloadMb(ids)
    if (mb < 100) {
        return "+$mb MB"
    }
    val whole = mb / 1000
    val tenth = (mb % 1000) / 100
    return "+$whole.$tenth GB"
}
