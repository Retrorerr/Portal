package app.polarbear.setup

// SPIKE-ONLY: local fake-installation phase model for the CONFIGURE prototype.
// UI-local only: no provisioning, no JNI, no Rust, no persistence. One
// monotonic progress source drives every progress-dependent visual; stage
// boundaries are explicit data so host-side checks can parse them.
//
// Mapping rule (mirrored by scripts/verify-install-model.ps1): stages occupy
// contiguous (prevEnd, end] bands; stageIndexAt returns the first stage with
// end >= clamped progress, so exact boundaries belong to the ending stage
// and 100% always resolves to the final stage (READY).

enum class SetupPhase { Configure, Installing, Ready }

/** Fake-installation wall time at normal animation scale (12-15s target). */
const val FAKE_INSTALL_DURATION_MS = 14_000

data class InstallStage(val id: String, val label: String, val end: Float)

private val STAGES_WITHOUT_APPS = listOf(
    InstallStage("prepare", "Preparing system", 0.14f),
    InstallStage("debian", "Installing Debian base", 0.62f),
    InstallStage("plasma", "Configuring Plasma", 0.86f),
    InstallStage("finalize", "Finalizing Portal", 1.0f),
)

private val STAGES_WITH_APPS = listOf(
    InstallStage("prepare", "Preparing system", 0.12f),
    InstallStage("debian", "Installing Debian base", 0.52f),
    InstallStage("plasma", "Configuring Plasma", 0.72f),
    InstallStage("apps", "Installing selected apps", 0.90f),
    InstallStage("finalize", "Finalizing Portal", 1.0f),
)

/** Deterministic stage list: the apps stage exists only when optional apps
 * were actually selected at Begin Install time. */
fun installStages(hasOptionalApps: Boolean): List<InstallStage> =
    if (hasOptionalApps) STAGES_WITH_APPS else STAGES_WITHOUT_APPS

/** First stage whose end band contains the clamped progress. */
fun stageIndexAt(progress: Float, stages: List<InstallStage>): Int {
    require(stages.isNotEmpty())
    val p = progress.coerceIn(0f, 1f)
    val i = stages.indexOfFirst { p <= it.end }
    return if (i < 0) stages.lastIndex else i
}

data class InstallLogLine(val text: String, val active: Boolean)

/** Visible trail: every entered stage, oldest first, capped; the containing
 * stage reads active until progress reaches 1. */
fun installLogLines(progress: Float, stages: List<InstallStage>, maxLines: Int = 5): List<InstallLogLine> {
    if (stages.isEmpty()) return emptyList()
    val p = progress.coerceIn(0f, 1f)
    val idx = stageIndexAt(p, stages)
    return stages.mapIndexed { i, s -> InstallLogLine(s.label, i == idx && p < 1f) }
        .filterIndexed { i, _ -> i <= idx }
        .takeLast(maxLines)
}
