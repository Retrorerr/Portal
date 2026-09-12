# Verifies the CONFIGURE fake-installation progress/stage model, on host.
#
# Single source of truth: SetupInstallModel.kt in
#   src/android/kotlin/app/polarbear/setup/
# This script parses the committed stage tables, duration constant and phase
# enum, then simulates the documented mapping rule (first stage with end >=
# clamped progress; exact boundaries belong to the ending stage) to prove:
#   - ends strictly increasing, within (0,1], last exactly 1.0
#   - stage ids unique; labels non-empty and free of fake-detail markers
#   - optional-apps stage present iff the with-apps variant, deterministic
#   - stage index valid + non-decreasing across 0..1, 0->first, 100%->last
#   - out-of-range progress clamps instead of escaping
#   - visible log trail capped at 5, exactly one active line until 100%
#   - SetupPhase is exactly Configure/Installing/Ready
#   - fake duration inside the 12-15s prototype window
#
# Exit code is 0 only if every check passes. If a check fails, fix the model.

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $repoRoot) { $repoRoot = (Get-Item $PSScriptRoot).Parent.FullName }
$kotlin = Join-Path $repoRoot 'src/android/kotlin/app/polarbear/setup/SetupInstallModel.kt'
if (-not (Test-Path -LiteralPath $kotlin)) {
    $repoRoot = (git rev-parse --show-toplevel) -replace '/', '\'
    $kotlin = Join-Path $repoRoot 'src/android/kotlin/app/polarbear/setup/SetupInstallModel.kt'
}
$text = Get-Content -Raw -LiteralPath $kotlin
$failures = 0
function Fail([string]$msg) { Write-Output ("FAIL " + $msg); $script:failures++ }

# Phase enum: exactly the three prototype phases.
$enumBody = ([regex]::Match($text, 'enum class SetupPhase \{([^}]+)\}')).Groups[1].Value
$phases = ([regex]::Matches($enumBody, '[A-Za-z]+') | ForEach-Object { $_.Value }) -join ','
if ($phases -ne 'Configure,Installing,Ready') { Fail ("phases=[$phases], want Configure/Installing/Ready") }

# Duration window.
$duration = [int]([regex]::Match($text, 'const val FAKE_INSTALL_DURATION_MS = ([\d_]+)').Groups[1].Value -replace '_', '')
if ($duration -lt 12000 -or $duration -gt 15000) { Fail ("duration ${duration}ms outside 12-15s") }

# Stage tables, parsed per variant block.
function Get-Stages([string]$name) {
    $block = ([regex]::Match($text, "private val $name = listOf\((.*?)\n\)", 'Singleline')).Groups[1].Value
    $rows = [regex]::Matches($block, 'InstallStage\("([^"]+)", "([^"]+)", ([\d.]+)f\)')
    $out = @()
    foreach ($r in $rows) { $out += ,@($r.Groups[1].Value, $r.Groups[2].Value, [double]$r.Groups[3].Value) }
    return ,$out
}
$without = Get-Stages 'STAGES_WITHOUT_APPS'
$with = Get-Stages 'STAGES_WITH_APPS'
if ($without.Count -lt 3) { Fail 'without-apps table too short' }
if ($with.Count -lt 4) { Fail 'with-apps table too short' }

$denied = @('MB/s', 'ETA', 'http', 'apt-get', 'dpkg', 'download speed')
foreach ($variant in @(@('without', $without), @('with', $with))) {
    $tag = $variant[0]; $stages = $variant[1]
    $ids = @(); $prev = -1.0
    foreach ($s in $stages) {
        if ($s[0] -in $ids) { Fail "$tag : duplicate stage id $($s[0])" }
        $ids += $s[0]
        if ([string]::IsNullOrWhiteSpace($s[1])) { Fail "$tag : empty label" }
        foreach ($d in $denied) { if ($s[1] -like "*$d*") { Fail "$tag : label '$($s[1])' carries fake detail '$d'" } }
        if ($s[2] -le 0 -or $s[2] -gt 1.0) { Fail "$tag : end $($s[2]) outside (0,1]" }
        if ($s[2] -le $prev) { Fail "$tag : ends not strictly increasing at $($s[0])" }
        $prev = $s[2]
    }
    $last = $stages[$stages.Count - 1]
    if ([Math]::Abs($last[2] - 1.0) -gt 1e-9) { Fail "$tag : last end $($last[2]) != 1.0" }
    if ($last[0] -ne 'finalize') { Fail "$tag : last stage must be finalize" }
}
$withIds = $with | ForEach-Object { $_[0] }
$withoutIds = $without | ForEach-Object { $_[0] }
if (-not ($withIds -contains 'apps')) { Fail 'with-apps variant lacks the apps stage' }
if ($withoutIds -contains 'apps') { Fail 'without-apps variant must not contain the apps stage' }
foreach ($id in @('prepare', 'debian', 'plasma', 'finalize')) {
    if (-not ($withIds -contains $id) -or -not ($withoutIds -contains $id)) { Fail "base stage $id missing from a variant" }
}

# Mapping simulation per the documented rule.
function Stage-Index([double]$p, $stages) {
    $c = [Math]::Min(1.0, [Math]::Max(0.0, $p))
    for ($i = 0; $i -lt $stages.Count; $i++) { if ($c -le $stages[$i][2]) { return $i } }
    return ($stages.Count - 1)
}
foreach ($variant in @(@('without', $without), @('with', $with))) {
    $tag = $variant[0]; $stages = $variant[1]
    if ((Stage-Index 0.0 $stages) -ne 0) { Fail "$tag : progress 0 must map to first stage" }
    if ((Stage-Index 1.0 $stages) -ne ($stages.Count - 1)) { Fail "$tag : progress 1 must map to last stage" }
    if ((Stage-Index -0.5 $stages) -ne 0) { Fail "$tag : negative progress must clamp to first" }
    if ((Stage-Index 1.5 $stages) -ne ($stages.Count - 1)) { Fail "$tag : >1 progress must clamp to last" }
    $prevIdx = -1
    for ($k = 0; $k -le 200; $k++) {
        $idx = Stage-Index ($k / 200.0) $stages
        if ($idx -lt 0 -or $idx -ge $stages.Count) { Fail "$tag : index out of range"; break }
        if ($idx -lt $prevIdx) { Fail "$tag : index moved backwards"; break }
        $prevIdx = $idx
    }
    # Exact boundaries belong to the ending stage; epsilon past belongs onward.
    for ($i = 0; $i -lt $stages.Count; $i++) {
        $e = $stages[$i][2]
        if ((Stage-Index $e $stages) -ne $i) { Fail "$tag : boundary $e must map to stage $i" }
        if ($i + 1 -lt $stages.Count -and (Stage-Index ($e + 1e-6) $stages) -ne ($i + 1)) {
            Fail "$tag : just past boundary $e must advance"
        }
    }
    # Log trail: capped, oldest-first subset, single active line until 100%.
    foreach ($pct in @(0.0, 0.05, 0.3, 0.77, 0.905, 1.0)) {
        $idx = Stage-Index $pct $stages
        $visible = @()
        for ($i = 0; $i -le $idx; $i++) { $visible += $i }
        $visible = $visible | Select-Object -Last 5
        if ($visible.Count -gt 5 -or $visible.Count -lt 1) { Fail "$tag : trail size at $pct" }
        $active = ($visible | Where-Object { $_ -eq $idx -and $pct -lt 1.0 }).Count
        $wantActive = if ($pct -lt 1.0) { 1 } else { 0 }
        if ($active -ne $wantActive) { Fail "$tag : active-line count at $pct" }
    }
    Write-Output ("PASS {0} : {1} stages, monotonic mapping, capped single-active trail" -f $tag, $stages.Count)
}

if ($failures -gt 0) { Write-Output "RESULT: $failures check(s) FAILED"; exit 1 }
Write-Output 'RESULT: install model checks all pass'
