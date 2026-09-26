package app.polarbear.setup

// Host-only invariants. Run with Kotlin/JVM; no device or UI automation.
fun main() {
    val capacities = listOf(
        StorageCapacity(128_000_000_000L, 91_000_000_000L),
        StorageCapacity(512_000_000_000L, 400_000_000_000L),
        StorageCapacity(16_000_000_000L, 2_000_000_000L),
        StorageCapacity(16_000_000_000L, 0L),
    )
    for (mask in 0 until (1 shl OPTIONAL_APPS.size)) {
        val ids = OPTIONAL_APPS.filterIndexed { index, _ -> mask and (1 shl index) != 0 }.map { it.id }.toSet()
        val requested = projectedInstallBytes(ids)
        for (capacity in capacities) {
            val result = capacity.projection(requested)
            check(result.usedBytes + result.portalBytes + result.freeAfterBytes == result.totalBytes)
            check(result.freeAfterBytes >= 0 && result.portalBytes <= capacity.availableBytes)
            check(result.portalBytes + result.shortfallBytes == requested)
        }
        for (app in OPTIONAL_APPS.filter { it.id !in ids }) {
            check(projectedInstallBytes(ids + app.id) - requested == app.installedMb * 1_000_000L)
        }
    }
    check(OPTIONAL_APP_IDS == setOf("chatgpt", "libreoffice", "vlc", "gimp", "krita", "inkscape", "thunderbird"))
    check(projectedInstallBytes(setOf("chatgpt")) - projectedInstallBytes(emptySet()) == 1_900_000_000L)
    check("okular" !in OPTIONAL_APP_IDS && "kate" !in OPTIONAL_APP_IDS)
    check(projectedInstallBytes(setOf("okular", "kate")) == projectedInstallBytes(emptySet()))
    check(selectedAppsSummary(emptySet()) == "Optional desktop applications")
    check(selectedAppsSummary(setOf("libreoffice", "vlc", "gimp", "krita")) == "LibreOffice, VLC +2")
    check(projectedInstallBytes(setOf("unknown")) == projectedInstallBytes(emptySet()))

    // These deliberately span small/large and low/high-density Android
    // displays. Each choice remains a ratio of densityDpi / 160 rather than a
    // fixed tablet scale, and keeps the required Compact < Balanced < Large
    // ordering on every device profile.
    val displays = listOf(
        InstallDisplayMetrics(densityDpi = 72, logicalWidthPx = 1, logicalHeightPx = 32_768),
        InstallDisplayMetrics(densityDpi = 160, logicalWidthPx = 800, logicalHeightPx = 480),
        InstallDisplayMetrics(densityDpi = 240, logicalWidthPx = 854, logicalHeightPx = 480),
        InstallDisplayMetrics(densityDpi = 420, logicalWidthPx = 2560, logicalHeightPx = 1600),
        InstallDisplayMetrics(densityDpi = 695, logicalWidthPx = 32_768, logicalHeightPx = 1800),
    )
    for (display in displays) {
        val compact = display.initialOutputScale(InterfaceSize.Compact)
        val balanced = display.initialOutputScale(InterfaceSize.Balanced)
        val large = display.initialOutputScale(InterfaceSize.Large)
        check(compact < balanced && balanced < large)
        check(kotlin.math.abs(balanced - display.densityDpi / 160.0) < 1e-9)
        check(kotlin.math.abs(compact / balanced - InterfaceSize.Compact.deviceScaleMultiplier) < 1e-9)
        check(kotlin.math.abs(large / balanced - InterfaceSize.Large.deviceScaleMultiplier) < 1e-9)
    }
    // Screen extent never selects a device preset: the same density yields the
    // same initial choice scales on a narrow phone and a wide tablet.
    val sameDensityPhone = InstallDisplayMetrics(420, 1080, 2400)
    val sameDensityTablet = InstallDisplayMetrics(420, 2560, 1600)
    InterfaceSize.entries.forEach { size ->
        check(sameDensityPhone.initialOutputScale(size) == sameDensityTablet.initialOutputScale(size))
    }
    // The upper supported density preserves all three choices below KScreen's
    // persisted output-scale limit instead of silently clamping a selection.
    check(kotlin.math.abs(displays.last().initialOutputScale(InterfaceSize.Balanced) - 695.0 / 160.0) < 1e-9)
    check(kotlin.math.abs(displays.last().initialOutputScale(InterfaceSize.Large) - 4.9953125) < 1e-9)
    check(displays.last().initialOutputScale(InterfaceSize.Large) <= 5.0)

    // These bounds mirror native InstallPlan validation; invalid display
    // snapshots fail before Compose submits any plan over JNI.
    check(runCatching { InstallDisplayMetrics(71, 800, 480) }.isFailure)
    check(runCatching { InstallDisplayMetrics(696, 800, 480) }.isFailure)
    check(runCatching { InstallDisplayMetrics(160, 0, 480) }.isFailure)
    check(runCatching { InstallDisplayMetrics(160, 800, 32_769) }.isFailure)

    val plan = InstallPlan.fromSelections(
        appearance = AppearanceMode.System,
        systemIsDark = true,
        interfaceSize = InterfaceSize.Large,
        displayMetrics = displays[3],
        selectedAppIds = setOf("vlc", "gimp"),
    )
    check(plan.committedAppearance == CommittedAppearance.Dark)
    check(plan.selectedAppIds == listOf("gimp", "vlc"))
    check(
        plan.toNativeJson() ==
            "{\"version\":1,\"appearance\":\"system\",\"committedAppearance\":\"dark\",\"interfaceSize\":\"large\",\"densityDpi\":420,\"logicalWidthPx\":2560,\"logicalHeightPx\":1600,\"selectedAppIds\":[\"gimp\",\"vlc\"]}",
    )
    check(AppearanceMode.System.committedAppearance(false) == CommittedAppearance.Light)
    check(AppearanceMode.Dark.committedAppearance(false) == CommittedAppearance.Dark)
    check(AppearanceMode.Light.committedAppearance(true) == CommittedAppearance.Light)
    check(
        runCatching {
            InstallPlan.fromSelections(
                appearance = AppearanceMode.Light,
                systemIsDark = true,
                interfaceSize = InterfaceSize.Balanced,
                displayMetrics = displays.first(),
                selectedAppIds = setOf("not-a-package"),
            )
        }.isFailure,
    )
    check(runCatching { StorageCapacity(0L, 0L) }.isFailure)
    check(runCatching { StorageCapacity(10L, 11L) }.isFailure)
    println("PASS: 128 optional-app selections across 4 capacities and 5 display baselines")
}
