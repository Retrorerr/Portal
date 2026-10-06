package app.polarbear.setup

/** The choices shown by the first-run screen, independent of Compose state. */
enum class AppearanceMode(val wireValue: String) {
    System("system"),
    Dark("dark"),
    Light("light"),
}

/** An appearance target hint; native resolves System again at commit time. */
enum class CommittedAppearance(val wireValue: String) {
    Dark("dark"),
    Light("light"),
}

/**
 * These are bounded multipliers of the Android device baseline, never
 * device-specific Plasma scale presets. The native installer applies the
 * resulting fractional scale once through KScreen/KWin when an output exists.
 */
enum class InterfaceSize(val wireValue: String, val deviceScaleMultiplier: Double) {
    Compact("compact", 0.85),
    Balanced("balanced", 1.0),
    Large("large", 1.15),
}

const val INSTALL_PLAN_VERSION = 1

/**
 * Metrics captured at acceptance time. They are deliberately part of the
 * durable plan so a retry never derives a different scale from later UI state.
 */
data class InstallDisplayMetrics(
    val densityDpi: Int,
    val logicalWidthPx: Int,
    val logicalHeightPx: Int,
) {
    init {
        // Keep the Kotlin-side acceptance gate identical to native validation;
        // malformed Android metrics should never cross JNI as a proposed plan.
        require(densityDpi in MIN_INSTALL_DENSITY_DPI..MAX_INSTALL_DENSITY_DPI) {
            "densityDpi would produce an unsupported KScreen output scale"
        }
        require(logicalWidthPx in 1..MAX_INSTALL_DISPLAY_EXTENT_PX) {
            "logicalWidthPx is outside supported bounds"
        }
        require(logicalHeightPx in 1..MAX_INSTALL_DISPLAY_EXTENT_PX) {
            "logicalHeightPx is outside supported bounds"
        }
    }

    /** Android's established Portal baseline: densityDpi / 160. */
    val deviceBaselineScale: Double get() = densityDpi.toDouble() / 160.0

    fun initialOutputScale(size: InterfaceSize): Double =
        deviceBaselineScale * size.deviceScaleMultiplier
}

private const val MIN_INSTALL_DENSITY_DPI = 72
// KScreen's persisted output scale is capped at 5.0. The selected Large size
// is 1.15x the device baseline, so 695dpi is the last integer baseline whose
// largest choice remains within that cap; never silently clamp the user's pick.
private const val MAX_INSTALL_DENSITY_DPI = 695
private const val MAX_INSTALL_DISPLAY_EXTENT_PX = 32_768

fun AppearanceMode.committedAppearance(systemIsDark: Boolean): CommittedAppearance = when (this) {
    AppearanceMode.System -> if (systemIsDark) CommittedAppearance.Dark else CommittedAppearance.Light
    AppearanceMode.Dark -> CommittedAppearance.Dark
    AppearanceMode.Light -> CommittedAppearance.Light
}

/**
 * Immutable, canonical first-run input. `fromSelections` rejects unknown app
 * IDs and normalizes ordering before crossing JNI, so presentation strings or
 * arbitrary package names can never become provisioning input.
 */
class InstallPlan private constructor(
    val version: Int,
    val appearance: AppearanceMode,
    /**
     * Selection-time target hint. When [appearance] is System, native reads
     * Android's current uiMode before applying Plasma preferences and durably
     * records that resolved target; this captured value is not authoritative.
     */
    val committedAppearance: CommittedAppearance,
    val interfaceSize: InterfaceSize,
    val displayMetrics: InstallDisplayMetrics,
    val selectedAppIds: List<String>,
) {
    val initialOutputScale: Double get() = displayMetrics.initialOutputScale(interfaceSize)

    /**
     * The values are all enums or allowlisted IDs, so this deliberately small
     * encoder needs no general-purpose JSON dependency in the UI process.
     */
    fun toNativeJson(): String = buildString {
        append("{\"version\":")
        append(version)
        append(",\"appearance\":\"")
        append(appearance.wireValue)
        append("\",\"committedAppearance\":\"")
        append(committedAppearance.wireValue)
        append("\",\"interfaceSize\":\"")
        append(interfaceSize.wireValue)
        append("\",\"densityDpi\":")
        append(displayMetrics.densityDpi)
        append(",\"logicalWidthPx\":")
        append(displayMetrics.logicalWidthPx)
        append(",\"logicalHeightPx\":")
        append(displayMetrics.logicalHeightPx)
        append(",\"selectedAppIds\":[")
        selectedAppIds.forEachIndexed { index, id ->
            if (index > 0) append(',')
            append('"')
            append(id)
            append('"')
        }
        append("]}")
    }

    companion object {
        fun fromSelections(
            appearance: AppearanceMode,
            systemIsDark: Boolean,
            interfaceSize: InterfaceSize,
            displayMetrics: InstallDisplayMetrics,
            selectedAppIds: Set<String>,
        ): InstallPlan {
            val catalog = OPTIONAL_APPS.associateBy { it.id }
            require(selectedAppIds.all { catalog[it]?.available == true }) {
                "Install plan contains an unknown or unavailable optional app"
            }
            return InstallPlan(
                version = INSTALL_PLAN_VERSION,
                appearance = appearance,
                committedAppearance = appearance.committedAppearance(systemIsDark),
                interfaceSize = interfaceSize,
                displayMetrics = displayMetrics,
                selectedAppIds = selectedAppIds.sorted(),
            )
        }
    }
}

// Estimates in decimal MB of installed footprint, not download size. Baseline
// apps are excluded because their rootfs footprint is already in minimal.
data class EssentialApp(
    val id: String,
    val name: String,
    val blurb: String,
    val installedMb: Int,
    val categoryId: String = "",
    val categoryLabel: String = "",
    /** Why this device cannot run the app; null when it can. */
    val unavailableReason: String? = null,
) {
    val available: Boolean get() = unavailableReason == null
}

/**
 * Parse the native catalog: one `id name blurb category-id category-label
 * installed-mb unavailable-reason` line per app, tab separated, in picker
 * order. Malformed lines are dropped rather than guessed at.
 */
fun parseOptionalAppCatalog(text: String): List<EssentialApp> =
    text.lineSequence()
        .filter { it.isNotBlank() }
        .mapNotNull { line ->
            val fields = line.split('	')
            if (fields.size != 7) return@mapNotNull null
            val size = fields[5].toIntOrNull() ?: return@mapNotNull null
            EssentialApp(
                id = fields[0],
                name = fields[1],
                blurb = fields[2],
                installedMb = size,
                categoryId = fields[3],
                categoryLabel = fields[4],
                unavailableReason = fields[6].ifBlank { null },
            )
        }
        .toList()

/**
 * The optional apps the pickers offer. Native code is the only source: the
 * Rust allowlist (`OptionalApp`) serves this catalog over JNI, so names,
 * sizes and availability can never drift from what native will install.
 */
object OptionalAppCatalog {
    @Volatile var loader: (() -> String?)? = null
    @Volatile private var cached: List<EssentialApp>? = null

    val apps: List<EssentialApp>
        get() = cached ?: loader?.invoke()
            ?.let(::parseOptionalAppCatalog)
            ?.takeIf { it.isNotEmpty() }
            ?.also { cached = it }
            ?: emptyList()

    /** Availability can change (graphics mode), so screens re-read it. */
    fun refresh() {
        cached = null
    }
}

val OPTIONAL_APPS: List<EssentialApp> get() = OptionalAppCatalog.apps
val OPTIONAL_APP_IDS: Set<String> get() = OPTIONAL_APPS.mapTo(linkedSetOf()) { it.id }
val DEFAULT_OPTIONAL_APP_IDS: Set<String> = emptySet()
const val MINIMAL_INSTALLED_MB = 5400L

/** Apps grouped for display, keeping catalog order within and between groups. */
fun groupedOptionalApps(apps: List<EssentialApp> = OPTIONAL_APPS): List<Pair<String, List<EssentialApp>>> =
    apps.groupBy { it.categoryLabel }.toList()

fun selectedApps(ids: Set<String>) = OPTIONAL_APPS.filter { it.id in ids }
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
