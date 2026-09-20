[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$ArtifactRoot,
    [string]$OutputName = "plasma-preload-analysis.json"
)

$ErrorActionPreference = "Stop"

function Get-Median {
    param([double[]]$Values)

    if (-not $Values -or $Values.Count -eq 0) {
        return $null
    }
    $ordered = @($Values | Sort-Object)
    $middle = [int][Math]::Floor($ordered.Count / 2)
    if (($ordered.Count % 2) -eq 1) {
        return $ordered[$middle]
    }
    return ($ordered[$middle - 1] + $ordered[$middle]) / 2
}

function Get-Field {
    param(
        [Parameter(Mandatory)][string]$Line,
        [Parameter(Mandatory)][string]$Name
    )

    $match = [regex]::Match($Line, "(?:^|\s)$([regex]::Escape($Name))=\s*(\S+)")
    if (-not $match.Success) {
        return $null
    }
    return $match.Groups[1].Value
}

$runs = @(
    Get-ChildItem -LiteralPath $ArtifactRoot -Directory |
        Where-Object Name -Match '^run-\d+$' |
        Sort-Object Name
)
if ($runs.Count -eq 0) {
    throw "No run-NN directories found under $ArtifactRoot"
}

$runSummaries = @()
$preloadRows = @()
$fullCreateRows = @()
$stallRows = @()

foreach ($run in $runs) {
    $logPath = Join-Path $run.FullName "plasma.log"
    if (-not (Test-Path -LiteralPath $logPath)) {
        continue
    }

    $lines = Get-Content -LiteralPath $logPath
    $raw = Get-Content -LiteralPath $logPath -Raw
    $startup = @()
    $preloads = @()
    $fullCreates = @()
    $stalls = @()
    foreach ($line in $lines) {
        if ($line -match 'PORTAL_STARTUP timestamp_ms=\s*(\d+) event=\s*(\S+)') {
            $startup += [pscustomobject]@{
                TimestampMs = [long]$Matches[1]
                Event = $Matches[2]
                Line = $line
            }
        }

        if ($line -match 'PORTAL_EVENT_LOOP timestamp_ms=\s*(\d+) event=stall gap_ms=\s*(\d+)') {
            $stall = [pscustomobject]@{
                Run = $run.Name
                TimestampMs = [long]$Matches[1]
                GapMs = [int]$Matches[2]
            }
            $stalls += $stall
            $stallRows += $stall
        }
    }

    # Qt's categorized debug output may wrap one PORTAL_PRELOAD record over
    # several physical lines. Parse from one marker to the next marker so
    # duration_ms and delay_ms remain attached to their originating record.
    $preloadPattern = 'PORTAL_PRELOAD\s+timestamp_ms=\s*(\d+)\s+phase=\s*(\S+)\s+plugin=\s*(\S+)(?<tail>.*?)(?=PORTAL_PRELOAD\s+timestamp_ms|PORTAL_STARTUP\s+timestamp_ms|PORTAL_EVENT_LOOP\s+timestamp_ms|\z)'
    foreach ($match in [regex]::Matches($raw, $preloadPattern, [System.Text.RegularExpressions.RegexOptions]::Singleline)) {
        $record = ($match.Value -replace '\s+', ' ').Trim()
        $row = [pscustomobject]@{
            TimestampMs = [long]$match.Groups[1].Value
            Phase = $match.Groups[2].Value
            Plugin = $match.Groups[3].Value
            AppletId = Get-Field -Line $record -Name "applet_id"
            Weight = Get-Field -Line $record -Name "weight"
            DelayMs = Get-Field -Line $record -Name "delay_ms"
            DurationMs = Get-Field -Line $record -Name "duration_ms"
            Result = Get-Field -Line $record -Name "result"
        }
        $preloads += $row
        if ($row.Phase -eq "end" -and $null -ne $row.DurationMs) {
            $preloadRows += [pscustomobject]@{
                Run = $run.Name
                Plugin = $row.Plugin
                AppletId = $row.AppletId
                DurationMs = [double]$row.DurationMs
                TimestampMs = $row.TimestampMs
            }
        }
        if ($row.Phase -eq "full-create-end" -and $null -ne $row.DurationMs) {
            $fullCreateRows += [pscustomobject]@{
                Run = $run.Name
                Plugin = $row.Plugin
                AppletId = $row.AppletId
                DurationMs = [double]$row.DurationMs
                TimestampMs = $row.TimestampMs
            }
        }
    }

    $load = $startup | Where-Object Event -eq "shellcorona-load-entry" | Select-Object -First 1
    $constructed = $startup | Where-Object Event -eq "panelview-constructed" | Select-Object -First 1
    $panelReady = $startup | Where-Object Event -eq "panel-containment-ui-ready" | Select-Object -First 1
    $desktopReady = $startup | Where-Object Event -eq "desktop-containment-ui-ready" | Select-Object -First 1
    $gatedVisible = $startup |
        Where-Object {
            $_.Event -eq "panelview-visible-changed" -and
            $_.TimestampMs -ge $panelReady.TimestampMs -and
            $_.Line -match 'visible=\s*true'
        } |
        Select-Object -First 1

    $relative = {
        param($event)
        if ($null -eq $event -or $null -eq $load) { return $null }
        return [long]($event.TimestampMs - $load.TimestampMs)
    }

    $scheduled = @($preloads | Where-Object Phase -eq "scheduled")
    $weights = @($preloads | Where-Object Phase -eq "schedule" | ForEach-Object {
        if ($null -ne $_.Weight) {
            [pscustomobject]@{ Plugin = $_.Plugin; AppletId = $_.AppletId; Weight = [int]$_.Weight }
        }
    })

    $runSummaries += [pscustomobject]@{
        Run = $run.Name
        ShellCoronaLoadMs = & $relative $load
        PanelConstructedMs = & $relative $constructed
        PanelContainmentUiReadyMs = & $relative $panelReady
        PanelFirstGatedVisibleMs = & $relative $gatedVisible
        DesktopContainmentUiReadyMs = & $relative $desktopReady
        PreloadEndCount = @($preloads | Where-Object Phase -eq "end").Count
        ScheduledCount = $scheduled.Count
        ScheduledPlugins = @($scheduled | ForEach-Object Plugin)
        Weights = $weights
        EventLoopStallCount = @($lines | Select-String "PORTAL_EVENT_LOOP.*event=stall").Count
        EventLoopMaxGapMs = if ($stalls.Count) { ($stalls | Measure-Object GapMs -Maximum).Maximum } else { $null }
    }
}

$preloadSummary = @(
    $preloadRows | Group-Object Plugin | ForEach-Object {
        [pscustomobject]@{
            Plugin = $_.Name
            Count = $_.Count
            MedianMs = Get-Median @($_.Group | ForEach-Object DurationMs)
            MinMs = ($_.Group | Measure-Object DurationMs -Minimum).Minimum
            MaxMs = ($_.Group | Measure-Object DurationMs -Maximum).Maximum
            Samples = @($_.Group | ForEach-Object { [math]::Round($_.DurationMs, 1) })
        }
    } | Sort-Object Plugin
)

$fullCreateSummary = @(
    $fullCreateRows | Group-Object Plugin | ForEach-Object {
        [pscustomobject]@{
            Plugin = $_.Name
            Count = $_.Count
            MedianMs = Get-Median @($_.Group | ForEach-Object DurationMs)
            MinMs = ($_.Group | Measure-Object DurationMs -Minimum).Minimum
            MaxMs = ($_.Group | Measure-Object DurationMs -Maximum).Maximum
            Samples = @($_.Group | ForEach-Object { [math]::Round($_.DurationMs, 1) })
        }
    } | Sort-Object Plugin
)

$result = [pscustomobject]@{
    ArtifactRoot = (Resolve-Path -LiteralPath $ArtifactRoot).Path
    Runs = $runSummaries
    PreloadForExpansion = $preloadSummary
    FullRepresentationCreation = $fullCreateSummary
    EventLoopStalls = [pscustomobject]@{
        Count = $stallRows.Count
        MedianGapMs = Get-Median @($stallRows | ForEach-Object GapMs)
        MaxGapMs = if ($stallRows.Count) { ($stallRows | Measure-Object GapMs -Maximum).Maximum } else { $null }
        Samples = $stallRows
    }
}

$outputPath = Join-Path $ArtifactRoot $OutputName
$result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $outputPath -Encoding UTF8
$result | Select-Object Runs,PreloadForExpansion,FullRepresentationCreation,EventLoopStalls | ConvertTo-Json -Depth 8
Write-Host "Analysis: $outputPath"
