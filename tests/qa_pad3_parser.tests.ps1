<#
.SYNOPSIS
    Regression tests for the strict, fixture-only Pad 3 Plasma QA parser.

.DESCRIPTION
    These tests never invoke adb.  They exercise the parser with timestamped
    host-log fixtures and generated PNGs so stale markers, disabled output,
    black frames, and incomplete lifecycle/stability evidence cannot regress
    into a release pass.
#>

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$script:Failures = New-Object 'System.Collections.Generic.List[string]'

function Assert-QaTrue {
    param(
        [bool]$Condition,
        [Parameter(Mandatory)][string]$Name
    )
    if (-not $Condition) {
        [void]$script:Failures.Add("Expected true: $Name")
    }
}

function Assert-QaFalse {
    param(
        [bool]$Condition,
        [Parameter(Mandatory)][string]$Name
    )
    Assert-QaTrue (-not $Condition) $Name
}

function Assert-QaContains {
    param(
        [object[]]$Values,
        [Parameter(Mandatory)][string]$Expected,
        [Parameter(Mandatory)][string]$Name
    )
    Assert-QaTrue (@($Values) -contains $Expected) $Name
}

$scriptPath = Join-Path $PSScriptRoot "..\scripts\qa-pad3-loop.ps1"
. $scriptPath -FunctionsOnly

$runStart = [Int64]1700000000000

function New-QaFixtureLog {
    param(
        [switch]$IncludePresentation = $true,
        [switch]$IncludeLifecycle = $true,
        [switch]$IncludeStability = $true,
        [switch]$LatestOutputDisabled,
        [switch]$HostPresentedFalse,
        [switch]$IncludeStaleMarker,
        [switch]$IncludeGenericPresentation,
        [Int64]$StabilityOffsetMs = 65000,
        [Int64]$TimestampOffsetMs = 0
    )

    $lines = New-Object 'System.Collections.Generic.List[string]'
    $base = $runStart + $TimestampOffsetMs
    [void]$lines.Add("$($base + 1000) host stage=wayland-readiness stage=kwin-identified generation=7 client=client-1 surface=55 title=KDE Wayland Compositor WL-0")
    [void]$lines.Add("$($base + 1100) host stage=output-configured generation=7 surface=55 enabled=true")
    [void]$lines.Add(('' + ($base + 1200) + ' host log=DEBUG xdg_toplevel#15.set_title("KDE Wayland Compositor WL-0- Output disabled")'))
    [void]$lines.Add(('' + ($base + 1300) + ' host log=DEBUG xdg_toplevel#15.set_title("KDE Wayland Compositor WL-0 — Press right control key to grab pointer")'))
    if ($LatestOutputDisabled) {
        [void]$lines.Add(('' + ($base + 1400) + ' host log=DEBUG xdg_toplevel#15.set_title("KDE Wayland Compositor WL-0- Output disabled")'))
    }
    if ($IncludePresentation) {
        $hostPresentedField = if ($HostPresentedFalse) { ' host_presented=false' } else { '' }
        [void]$lines.Add("$($base + 2000) host stage=android-frame-presented generation=7 surface=55 evidence=egl-android-display-present android_present_ns=123456$hostPresentedField surfaces=1 clients=1")
    } elseif ($IncludeGenericPresentation) {
        [void]$lines.Add("$($base + 2000) host stage=plasma-ready generation=7 surface=55 evidence=egl-android-display-present android_present_ns=123456")
    }
    if ($IncludeLifecycle) {
        [void]$lines.Add("$($base + 3000) host stage=qa-lifecycle generation=7 kwin_alive=true plasma_alive=true")
    }
    if ($IncludeStability) {
        [void]$lines.Add("$($base + $StabilityOffsetMs) host stage=qa-stability generation=7 status=pass stable_for_ms=60000 kwin_alive=true plasma_alive=true output_enabled=true")
    }
    if ($IncludeStaleMarker) {
        [void]$lines.Add("$($runStart - 1000) host stage=plasma-ready timestamp_ms=$($runStart - 2000) generation=7 evidence=egl-android-display-present android_present_ns=123456")
    }
    return [string]::Join("`n", $lines)
}

function New-QaLifecycleScenarioContent {
    return @(
        "$($runStart + 80000) collector source=qa-collector stage=qa-lifecycle-scenario scenario=background-resume run_id=qa-run-1 status=pass verified=true artifact=artifacts/qa/scenarios/background-resume.json pre_session_id=session-a post_session_id=session-a pre_generation=7 post_generation=7 background_ms=$($runStart + 70000) foreground_ms=$($runStart + 75000) pre_host_presented=true post_host_presented=true pre_screenshot_nonblack=true post_screenshot_nonblack=true",
        "$($runStart + 100000) collector source=qa-collector stage=qa-lifecycle-scenario scenario=force-close-reopen run_id=qa-run-1 status=pass verified=true artifact=artifacts/qa/scenarios/force-close-reopen.json pre_session_id=session-a post_session_id=session-b pre_generation=7 post_generation=8 force_close_ms=$($runStart + 90000) reopen_ms=$($runStart + 95000) pre_host_presented=true post_host_presented=true pre_screenshot_nonblack=true post_screenshot_nonblack=true"
    ) -join "`n"
}

function New-QaPng {
    param(
        [Parameter(Mandatory)][string]$Path,
        [switch]$Black
    )

    Add-Type -AssemblyName System.Drawing -ErrorAction Stop
    $bitmap = [System.Drawing.Bitmap]::new(32, 32)
    $graphics = $null
    try {
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        if ($Black) {
            $graphics.Clear([System.Drawing.Color]::Black)
        } else {
            $graphics.Clear([System.Drawing.Color]::MidnightBlue)
            $graphics.FillRectangle([System.Drawing.Brushes]::White, 4, 4, 24, 24)
        }
        $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    } finally {
        if ($null -ne $graphics) { $graphics.Dispose() }
        $bitmap.Dispose()
    }
}

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("qa-pad3-parser-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null
$goodPng = Join-Path $tempRoot "good.png"
$blackPng = Join-Path $tempRoot "black.png"

try {
    New-QaPng -Path $goodPng
    New-QaPng -Path $blackPng -Black

    $positive = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent (New-QaFixtureLog) `
        -LifecycleScenarioContent (New-QaLifecycleScenarioContent) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaTrue $positive.Passed "complete current-run evidence passes strict release gate"
    Assert-QaTrue $positive.HostPresented "complete fixture reports HostPresented=true"
    Assert-QaTrue $positive.OutputEnabled "complete fixture reports output enabled"
    Assert-QaTrue $positive.StabilityPassed "complete fixture reports 60-second stability"
    Assert-QaTrue $positive.BackgroundForeground "complete fixture reports background/resume scenario"
    Assert-QaTrue $positive.ForceCloseReopen "complete fixture reports force-close/reopen scenario"

    $realFormatting = (New-QaFixtureLog) `
        -replace 'client=client-1 surface=55', 'client=Some(InnerClientId { id: 0, serial: 1 }) surface=Some(55)' `
        -replace 'surface=55 evidence=', 'surface=Some(55) evidence='
    $realShape = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent $realFormatting `
        -LifecycleScenarioContent (New-QaLifecycleScenarioContent) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaTrue $realShape.Passed "real host-event Some(...) surface formatting remains parseable"

    $hostPresentedFalse = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent (New-QaFixtureLog -HostPresentedFalse) `
        -LifecycleScenarioContent (New-QaLifecycleScenarioContent) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $hostPresentedFalse.Passed "HostPresented=false cannot pass release"
    Assert-QaFalse $hostPresentedFalse.HostPresented "HostPresented=false remains false"

    $presentationRevoked = (New-QaFixtureLog) + "`n" + (($runStart + 3000).ToString() + ' host stage=android-frame-presented generation=7 surface=55 host_presented=false evidence=egl-android-display-present android_present_ns=123456')
    $revoked = Test-QaHostPresentation -HostLogContent $presentationRevoked -RunStartTimestampMs $runStart
    Assert-QaFalse $revoked.Passed "a later false presentation attempt clears an earlier pass"

    $missingBackgroundScenario = Test-QaLifecycleScenarios `
        -Content (((New-QaLifecycleScenarioContent) -split "`n" | Where-Object { $_ -notmatch 'scenario=background-resume' }) -join "`n") `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $missingBackgroundScenario.BackgroundForeground "missing background/resume scenario fails closed"
    Assert-QaTrue $missingBackgroundScenario.ForceCloseReopen "present force-close/reopen scenario remains independently verifiable"

    $missingReopenScenario = Test-QaLifecycleScenarios `
        -Content (((New-QaLifecycleScenarioContent) -split "`n" | Where-Object { $_ -notmatch 'scenario=force-close-reopen' }) -join "`n") `
        -RunStartTimestampMs $runStart
    Assert-QaTrue $missingReopenScenario.BackgroundForeground "present background/resume scenario remains independently verifiable"
    Assert-QaFalse $missingReopenScenario.ForceCloseReopen "missing force-close/reopen scenario fails closed"

    $noPresentation = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent (New-QaFixtureLog -IncludePresentation:$false) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $noPresentation.Passed "hostPresented=false cannot pass release"
    Assert-QaFalse $noPresentation.HostPresented "hostPresented=false remains false"
    Assert-QaContains $noPresentation.MissingCriteria "HostPresented" "missing HostPresented is reported"

    $disabled = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent (New-QaFixtureLog -LatestOutputDisabled) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $disabled.Passed "latest disabled output cannot pass release"
    Assert-QaFalse $disabled.OutputEnabled "latest disabled output is not enabled"
    Assert-QaContains $disabled.MissingCriteria "OutputEnabled" "disabled output is reported"

    $staleMarker = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent (New-QaFixtureLog -IncludePresentation:$false -IncludeStaleMarker) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $staleMarker.Passed "stale plasma-ready marker cannot pass release"
    Assert-QaFalse $staleMarker.HostPresented "stale marker does not set HostPresented"
    $markerCheck = Test-WaylandReadinessMarker `
        -HostLogContent (New-QaFixtureLog -IncludePresentation:$false -IncludeStaleMarker) `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $markerCheck.Passed "marker-only readiness check fails closed"

    $genericPresentation = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent (New-QaFixtureLog -IncludePresentation:$false -IncludeGenericPresentation) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $genericPresentation.Passed "generic/plasma-ready presentation cannot pass release"
    Assert-QaFalse $genericPresentation.HostPresented "generic presentation does not set HostPresented"

    $black = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent (New-QaFixtureLog) `
        -ScreenshotPath $blackPng `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $black.Passed "black screenshot cannot pass release"
    Assert-QaFalse $black.ScreenshotNonBlack "black screenshot is rejected"
    Assert-QaContains $black.MissingCriteria "ScreenshotNonBlack" "black screenshot is reported"

    $incomplete = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent (New-QaFixtureLog -IncludeLifecycle:$false -IncludeStability:$false) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $incomplete.Passed "missing lifecycle/stability cannot pass release"
    Assert-QaContains $incomplete.MissingCriteria "KWinAlive" "missing KWin lifecycle is reported"
    Assert-QaContains $incomplete.MissingCriteria "PlasmaAlive" "missing Plasma lifecycle is reported"
    Assert-QaContains $incomplete.MissingCriteria "Stability" "missing stability is reported"
    Assert-QaContains $incomplete.MissingCriteria "BackgroundForeground" "missing background/resume is reported"
    Assert-QaContains $incomplete.MissingCriteria "ForceCloseReopen" "missing force-close/reopen is reported"

    $stale = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent (New-QaFixtureLog -TimestampOffsetMs (-60000)) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $stale.Passed "stale current-run log evidence cannot pass release"
    Assert-QaContains $stale.MissingCriteria "CurrentRunKWinSurface" "stale identity is reported"

    $rawProcessNames = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent (New-QaFixtureLog -IncludeLifecycle:$false) `
        -ProcessSnapshotContent "kwin_wayland plasmashell" `
        -ProcessSnapshotTimestampMs ($runStart + 65000) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $rawProcessNames.Passed "raw process names cannot substitute for lifecycle evidence"

    $oldSnapshotThenExit = New-QaFixtureLog
    $oldSnapshotThenExit += "`n$($runStart + 70000) collector stage=qa-process-snapshot generation=7 kwin_alive=true plasma_alive=true"
    $oldSnapshotThenExit += "`n$($runStart + 80000) host stage=desktop-exit status=139"
    $snapshotExit = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent $oldSnapshotThenExit `
        -LifecycleScenarioContent (New-QaLifecycleScenarioContent) `
        -ProcessSnapshotContent "stage=qa-process-snapshot generation=7 kwin_alive=true plasma_alive=true" `
        -ProcessSnapshotTimestampMs ($runStart + 70000) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $snapshotExit.Passed "snapshot older than a later exit cannot revive lifecycle"
    Assert-QaFalse $snapshotExit.KWinAlive "later exit clears KWin alive state"
    Assert-QaFalse $snapshotExit.PlasmaAlive "later exit clears Plasma alive state"

    $generationMismatch = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent ((New-QaFixtureLog) -replace 'stage=qa-lifecycle generation=7', 'stage=qa-lifecycle generation=99') `
        -LifecycleScenarioContent (New-QaLifecycleScenarioContent) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $generationMismatch.Passed "lifecycle generation mismatch fails closed"
    Assert-QaContains $generationMismatch.MissingCriteria "KWinAlive" "generation mismatch removes KWin lifecycle proof"

    $earlyStability = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent (New-QaFixtureLog -StabilityOffsetMs 40000) `
        -LifecycleScenarioContent (New-QaLifecycleScenarioContent) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $earlyStability.Passed "stability duration must end 60 seconds after first presentation"
    Assert-QaFalse $earlyStability.StabilityPassed "early stability sample is rejected"

    $stabilityReset = New-QaFixtureLog
    $stabilityReset += "`n$($runStart + 66000) host stage=qa-stability generation=7 status=fail stable_for_ms=60000 kwin_alive=false plasma_alive=false output_enabled=false"
    $resetEvidence = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent $stabilityReset `
        -LifecycleScenarioContent (New-QaLifecycleScenarioContent) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $resetEvidence.Passed "a later failed stability sample resets a previous pass"
    Assert-QaFalse $resetEvidence.StabilityPassed "failed stability sample cannot be skipped"

    $bareUptime = New-QaFixtureLog
    $bareUptime += "`n$($runStart + 66000) host stage=qa-stability generation=7 uptime_ms=60000"
    $bareUptimeEvidence = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent $bareUptime `
        -LifecycleScenarioContent (New-QaLifecycleScenarioContent) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $bareUptimeEvidence.StabilityPassed "bare uptime is not stability proof"

    $unknownStatus = New-QaFixtureLog
    $unknownStatus += "`n$($runStart + 66000) host stage=qa-stability generation=7 stable_for_ms=60000 kwin_alive=true plasma_alive=true output_enabled=true"
    $unknownStatusEvidence = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent $unknownStatus `
        -LifecycleScenarioContent (New-QaLifecycleScenarioContent) `
        -ScreenshotPath $goodPng `
        -RunStartTimestampMs $runStart
    Assert-QaFalse $unknownStatusEvidence.StabilityPassed "missing status is not stability proof"

    $partial = Assert-ReleaseGates `
        -ExpectedWidth 32 `
        -ExpectedHeight 32 `
        -ScreenshotPath $goodPng `
        -HostLogContent (New-QaFixtureLog) `
        -RunStartTimestampMs $runStart
    Assert-QaTrue $partial.PartialPassed "resolution/KWin/crash checks remain an explicit partial result"
    Assert-QaFalse $partial.ReleasePassed "partial checks cannot be promoted to release"

    $noBoundary = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent (New-QaFixtureLog) `
        -ScreenshotPath $goodPng
    Assert-QaFalse $noBoundary.Passed "missing run boundary fails closed"
} finally {
    $tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    $resolvedTempRoot = $null
    if (Test-Path -LiteralPath $tempRoot -PathType Container) {
        $resolvedTempRoot = (Resolve-Path -LiteralPath $tempRoot).Path
    }
    if ($resolvedTempRoot -and
        $resolvedTempRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolvedTempRoot) -match '^qa-pad3-parser-[0-9a-f]{32}$') {
        Remove-Item -LiteralPath $resolvedTempRoot -Recurse -Force
    }
}

if ($script:Failures.Count -gt 0) {
    $script:Failures | ForEach-Object { Write-Error $_ }
    exit 1
}

Write-Host "QA parser regression tests passed ($((Get-Date).ToString('s')))." -ForegroundColor Green
