[CmdletBinding()]
param(
    [string]$DeviceId = $env:ANDROID_SERIAL,
    [string]$PackageName = "app.polarbear",
    [string]$ActivityName = ".PortalActivity",
    [ValidateRange(1, 20)]
    [int]$Runs = 3,
    [ValidateRange(10, 300)]
    [int]$TimeoutSeconds = 120,
    [ValidateRange(50, 2000)]
    # A full device screenshot is several MB on the Pad 3. Keep the default
    # probe interval low enough to avoid making the measurement its own load.
    [int]$PanelPollMilliseconds = 1000,
    [ValidateRange(5, 300)]
    [int]$TraceSeconds = 45,
    [ValidateRange(100, 5000)]
    [int]$TimelineIntervalMilliseconds = 1000,
    [switch]$CapturePerfetto,
    [switch]$CaptureTimeline,
    [switch]$DismissVeil,
    [string]$OutputRoot
)

$ErrorActionPreference = "Stop"

function Invoke-AdbText {
    param([Parameter(Mandatory)][string[]]$Arguments)

    $output = & adb -s $script:DeviceId @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "adb $($Arguments -join ' ') failed: $($output -join ' ')"
    }
    return ($output -join "`n").Trim()
}

function Invoke-AdbToFile {
    param(
        [Parameter(Mandatory)][string[]]$Arguments,
        [Parameter(Mandatory)][string]$Path
    )

    $output = & adb -s $script:DeviceId @Arguments 2>&1
    $output | Set-Content -LiteralPath $Path -Encoding UTF8
    if ($LASTEXITCODE -ne 0) {
        throw "adb $($Arguments -join ' ') failed; see $Path"
    }
}

function Invoke-AdbExecOutToFile {
    param(
        [Parameter(Mandatory)][string[]]$Arguments,
        [Parameter(Mandatory)][string]$Path
    )

    $output = & adb -s $script:DeviceId @Arguments 2>&1
    $exitCode = $LASTEXITCODE
    $output | Set-Content -LiteralPath $Path -Encoding UTF8
    if ($exitCode -ne 0) {
        throw "adb exec-out $($Arguments -join ' ') failed; see $Path"
    }
}

function Get-DeviceEpochMilliseconds {
    $value = Invoke-AdbText @("shell", "date", "+%s%3N")
    $parsed = 0L
    if ([long]::TryParse($value, [ref]$parsed)) {
        return $parsed
    }
    return [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
}

function Get-GuestFile {
    param([Parameter(Mandatory)][string]$Path)

    $output = & adb -s $script:DeviceId shell run-as $script:PackageName cat $Path 2>$null
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        return ""
    }
    return ($output -join "`n")
}

function Remove-GuestLaunchMarkers {
    $stateDir = "/data/data/$script:PackageName/files/runtime-B/var/lib/localdesktop"
    $markerPaths = @(
        "$stateDir/plasma-ready",
        "$stateDir/plasma-failed",
        "$stateDir/kwin-crash"
    )
    foreach ($path in $markerPaths) {
        & adb -s $script:DeviceId shell run-as $script:PackageName rm -f $path 2>$null | Out-Null
    }
}

function Get-ProcessIdText {
    param([Parameter(Mandatory)][string]$Name)

    # PRoot guest commands inherit the Android process name only after exec.
    # A run-as `pidof` can therefore return the PRoot loader while its command
    # line still contains the requested guest name. Resolve the exact Android
    # comm field instead so startup samples describe the real process.
    $output = & adb -s $script:DeviceId shell ps -A -o pid=,comm=,args= 2>$null
    foreach ($line in $output) {
        if ($line -match "^\s*(\d+)\s+\[?$([regex]::Escape($Name))\]?\s+") {
            return $Matches[1]
        }
    }
    return ""
}

function Get-GuestProcessStats {
    param([AllowNull()][AllowEmptyString()][string]$TargetPid)

    if ([string]::IsNullOrWhiteSpace($TargetPid)) {
        return ""
    }
    $output = & adb -s $script:DeviceId shell ps -p $TargetPid -o pid,etime,time,%cpu,rss,stat,wchan,comm 2>$null
    return (($output -join " | ").Trim())
}

function Get-ProcessThreadStats {
    param([AllowNull()][AllowEmptyString()][string]$TargetPid)

    if ([string]::IsNullOrWhiteSpace($TargetPid)) {
        return @()
    }
    # Toybox ps exposes per-thread CPU time and wait channel without requiring
    # ptrace/root. Keep this separate from the process timeline so the monitor
    # does not hide which Qt/QML thread owns the startup CPU.
    $output = & adb -s $script:DeviceId shell ps -T -p $TargetPid -o TID,CMD,ELAPSED,TIME,%CPU,S,WCHAN 2>$null
    return @($output | Where-Object { $_ -match '^\s*\d+\s+' })
}

function Wait-ProcessGone {
    param([int]$Seconds = 15)

    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do {
        $appPid = Get-ProcessIdText -Name $script:PackageName
        $kwinPid = Get-ProcessIdText -Name "kwin_wayland"
        $plasmaPid = Get-ProcessIdText -Name "plasmashell"
        if ([string]::IsNullOrWhiteSpace($appPid) -and
            [string]::IsNullOrWhiteSpace($kwinPid) -and
            [string]::IsNullOrWhiteSpace($plasmaPid)) {
            return
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)

    throw "Portal or guest desktop processes remained after force-stop: app='$appPid' kwin='$kwinPid' plasmashell='$plasmaPid'"
}

function Capture-Screenshot {
    param([Parameter(Mandatory)][string]$Path)

    # Keep the native stdout byte stream intact; PowerShell's object pipeline
    # would otherwise turn a PNG into text and corrupt the panel probe.
    & adb -s $script:DeviceId exec-out screencap -p 2>$null > $Path
    if ($LASTEXITCODE -ne 0) {
        return $false
    }
    return ((Test-Path -LiteralPath $Path) -and ((Get-Item -LiteralPath $Path).Length -gt 1024))
}

function Test-PanelVisible {
    param([Parameter(Mandatory)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return $false
    }

    $bitmap = $null
    try {
        $bitmap = [System.Drawing.Bitmap]::new($Path)
        if ($bitmap.Width -lt 200 -or $bitmap.Height -lt 200) {
            return $false
        }

        # The Plasma panel is a nearly full-width dark strip at the physical
        # bottom of the Pad. A dark row alone is not enough: the pre-Plasma
        # frame is also black and can contain the large debug cursor. Require
        # a luminance boundary above the strip plus panel pixels in the strip.
        $bottomY = [Math]::Max(0, $bitmap.Height - [Math]::Max(24, [int]($bitmap.Height * 0.01)))
        $aboveY = [Math]::Max(0, $bitmap.Height - [Math]::Max(48, [int]($bitmap.Height * 0.05)))
        $step = [Math]::Max(1, [int]($bitmap.Width / 96))
        $samples = 0
        $bottomLuminanceTotal = 0.0
        $aboveLuminanceTotal = 0.0
        $bottomBright = 0
        $bottomColored = 0
        for ($x = $step; $x -lt ($bitmap.Width - $step); $x += $step) {
            $bottomColor = $bitmap.GetPixel($x, $bottomY)
            $aboveColor = $bitmap.GetPixel($x, $aboveY)
            $bottomMaximum = [Math]::Max($bottomColor.R, [Math]::Max($bottomColor.G, $bottomColor.B))
            $bottomMinimum = [Math]::Min($bottomColor.R, [Math]::Min($bottomColor.G, $bottomColor.B))
            $bottomPixelLuminance = (0.2126 * $bottomColor.R) + (0.7152 * $bottomColor.G) + (0.0722 * $bottomColor.B)
            $abovePixelLuminance = (0.2126 * $aboveColor.R) + (0.7152 * $aboveColor.G) + (0.0722 * $aboveColor.B)
            $samples++
            $bottomLuminanceTotal += $bottomPixelLuminance
            $aboveLuminanceTotal += $abovePixelLuminance
            if ($bottomPixelLuminance -gt 75) {
                $bottomBright++
            }
            if (($bottomMaximum - $bottomMinimum) -gt 25) {
                $bottomColored++
            }
        }
        if ($samples -le 0) {
            return $false
        }
        $bottomMean = $bottomLuminanceTotal / $samples
        $aboveMean = $aboveLuminanceTotal / $samples
        $boundary = $aboveMean - $bottomMean
        $hasPanelPixels = (($bottomBright / $samples) -ge 0.01) -or (($bottomColored / $samples) -ge 0.10)
        return ($bottomMean -lt 90 -and $boundary -ge 25 -and $hasPanelPixels)
    }
    catch {
        return $false
    }
    finally {
        if ($null -ne $bitmap) {
            $bitmap.Dispose()
        }
    }
}

function Start-PerfettoTrace {
    param(
        [Parameter(Mandatory)][string]$RemotePath,
        [Parameter(Mandatory)][int]$DurationSeconds
    )

    try {
        $output = Invoke-AdbText @(
            "shell", "perfetto", "--background-wait",
            "-o", $RemotePath,
            "-t", "${DurationSeconds}s",
            "--app", $script:PackageName,
            "sched", "freq", "idle", "am", "wm", "gfx", "view", "binder_driver", "hal", "dalvik"
        )
        $perfettoProcessId = 0
        if ([int]::TryParse($output.Trim(), [ref]$perfettoProcessId)) {
            return $perfettoProcessId
        }
    }
    catch {
        Write-Warning "Perfetto could not be started: $($_.Exception.Message)"
    }
    return $null
}

function Stop-PerfettoTrace {
    param([AllowNull()][int]$PerfettoProcessId)

    if ($null -eq $PerfettoProcessId -or $PerfettoProcessId -le 0) {
        return
    }
    & adb -s $script:DeviceId shell kill -TERM $PerfettoProcessId 2>$null | Out-Null
    Start-Sleep -Milliseconds 750
}

function Get-StartupEvents {
    param([Parameter(Mandatory)][long]$AfterEpochMilliseconds)

    $text = Get-GuestFile "files/diagnostics/host.log"
    $events = @()
    foreach ($line in ($text -split "`r?`n")) {
        if ($line -match "^(?<timestamp>\d+) host stage=(?<stage>\S+) (?<detail>.*)$") {
            $timestamp = [long]$Matches.timestamp
            if ($timestamp -ge $AfterEpochMilliseconds) {
                $events += [PSCustomObject]@{
                    TimestampMs = $timestamp
                    Stage = $Matches.stage
                    Detail = $Matches.detail
                }
            }
        }
    }
    return $events
}

function Get-PlasmaStartupTraceEvents {
    param([Parameter(Mandatory)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return @()
    }

    $events = @()
    $lineIndex = 0
    foreach ($line in (Get-Content -LiteralPath $Path)) {
        $lineIndex++
        if ($line -match "PORTAL_STARTUP\s+timestamp_ms=\s*(?<timestamp>\d+)\s+event=\s*(?<event>\S+)(?<detail>.*)$") {
            $events += [PSCustomObject]@{
                TimestampMs = [long]$Matches.timestamp
                Event = $Matches.event
                Detail = $Matches.detail.Trim()
                LineIndex = $lineIndex
                Line = $line
            }
        }
    }
    return $events
}

function Get-FirstPlasmaStartupTraceEvent {
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][object[]]$Events,
        [Parameter(Mandatory)][string]$Event,
        [string]$DetailPattern
    )

    $matching = @($Events | Where-Object {
        $_.Event -eq $Event -and ([string]::IsNullOrWhiteSpace($DetailPattern) -or $_.Detail -match $DetailPattern)
    } | Sort-Object TimestampMs, LineIndex)
    if ($matching.Count -eq 0) {
        return $null
    }
    return $matching[0]
}

function Get-PlasmaStartupTraceDelta {
    param(
        [AllowNull()][object]$StartEvent,
        [AllowNull()][object]$EndEvent
    )

    if ($null -eq $StartEvent -or $null -eq $EndEvent) {
        return $null
    }
    return ([long]$EndEvent.TimestampMs - [long]$StartEvent.TimestampMs)
}

if ([string]::IsNullOrWhiteSpace($DeviceId)) {
    $deviceLines = & adb devices 2>$null | Select-Object -Skip 1 | Where-Object { $_ -match "\sdevice\s*$" }
    $DeviceId = ($deviceLines | Select-Object -First 1).ToString().Split("`t")[0]
}
if ([string]::IsNullOrWhiteSpace($DeviceId)) {
    throw "No authorized ADB device found. Pass -DeviceId explicitly."
}
$script:DeviceId = $DeviceId
$script:PackageName = $PackageName

if ([string]::IsNullOrWhiteSpace($OutputRoot)) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutputRoot = Join-Path (Get-Location) (Join-Path "artifacts/launch-monitor" $stamp)
}
New-Item -ItemType Directory -Force -Path $OutputRoot | Out-Null

$activity = "$PackageName/$ActivityName"
$deviceModel = Invoke-AdbText @("shell", "getprop", "ro.product.model")
$androidRelease = Invoke-AdbText @("shell", "getprop", "ro.build.version.release")
$androidSdk = Invoke-AdbText @("shell", "getprop", "ro.build.version.sdk")

$runResults = @()
for ($run = 1; $run -le $Runs; $run++) {
    $runDir = Join-Path $OutputRoot ("run-{0:D2}" -f $run)
    New-Item -ItemType Directory -Force -Path $runDir | Out-Null
    Write-Host "Run $run/${Runs}: force-stopping Portal and waiting for guest shutdown"

    Invoke-AdbText @("shell", "am", "force-stop", $PackageName) | Out-Null
    Wait-ProcessGone
    Remove-GuestLaunchMarkers
    Invoke-AdbText @("shell", "logcat", "-c") | Out-Null
    Invoke-AdbText @("shell", "dumpsys", "gfxinfo", $PackageName, "reset") | Out-Null

    $startEpoch = Get-DeviceEpochMilliseconds
    $launchClock = [System.Diagnostics.Stopwatch]::StartNew()
    $remoteTrace = "/data/misc/perfetto-traces/portal-cold-launch-$([Guid]::NewGuid().ToString('N')).pftrace"
    $perfettoPid = $null
    if ($CapturePerfetto) {
        $perfettoPid = Start-PerfettoTrace -RemotePath $remoteTrace -DurationSeconds $TraceSeconds
    }

    $activityOutput = Invoke-AdbText @("shell", "am", "start", "-W", "-n", $activity)
    $activityElapsed = $launchClock.ElapsedMilliseconds
    $activityOutput | Set-Content -LiteralPath (Join-Path $runDir "am-start.txt") -Encoding UTF8

    $readyAt = $null
    $readyElapsed = $null
    $veilDismissAt = $null
    $panelAt = $null
    $panelElapsed = $null
    $plasmashellAt = $null
    $plasmashellPid = $null
    $lastScreenshot = Join-Path $runDir "last.png"
    $panelScreenshot = Join-Path $runDir "panel-visible.png"
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $timelinePath = Join-Path $runDir "timeline.csv"
    $threadTimelinePath = Join-Path $runDir "plasmashell-threads.tsv"
    $nextTimelineAt = 0L
    if ($CaptureTimeline) {
        "ElapsedMs,DeviceEpochMs,ReadyMarkerSeen,PlasmashellPid,PlasmashellStats,PanelVisible" |
            Set-Content -LiteralPath $timelinePath -Encoding UTF8
        "ElapsedMs`tRawPsThread" |
            Set-Content -LiteralPath $threadTimelinePath -Encoding UTF8
    }

    while ([DateTime]::UtcNow -lt $deadline) {
        $readyText = Get-GuestFile "/data/data/$PackageName/files/runtime-B/var/lib/localdesktop/plasma-ready"
        if ($null -eq $readyAt -and $readyText -match "timestamp_ms=(\d+)") {
            $readyAt = [long]$Matches[1]
            $readyElapsed = $launchClock.ElapsedMilliseconds
            Write-Host ("  plasma-ready at {0} ms (marker timestamp {1})" -f $readyElapsed, $readyAt)
            if ($DismissVeil) {
                Invoke-AdbText @("shell", "am", "broadcast", "-a", "app.polarbear.DEBUG_DISMISS_VEIL", "--receiver-foreground") | Out-Null
                $veilDismissAt = $launchClock.ElapsedMilliseconds
                Write-Host ("  debug veil dismissed at {0} ms" -f $veilDismissAt)
            }
        }

        if ($null -eq $plasmashellAt) {
            $candidatePid = Get-ProcessIdText -Name "plasmashell"
            if (-not [string]::IsNullOrWhiteSpace($candidatePid)) {
                $plasmashellPid = $candidatePid
                $plasmashellAt = $launchClock.ElapsedMilliseconds
            }
        }

        if (-not $CaptureTimeline -and
            $null -ne $readyAt -and $DismissVeil -and
            ($launchClock.ElapsedMilliseconds -ge ($veilDismissAt + 100))) {
            $captured = Capture-Screenshot -Path $lastScreenshot
            if ($captured -and (Test-PanelVisible -Path $lastScreenshot)) {
                if ($null -eq $panelAt) {
                    Copy-Item -LiteralPath $lastScreenshot -Destination $panelScreenshot -Force
                    $panelAt = Get-DeviceEpochMilliseconds
                    $panelElapsed = $launchClock.ElapsedMilliseconds
                    Write-Host ("  panel-visible at {0} ms" -f $panelElapsed)
                }
            }
        }

        if ($CaptureTimeline -and $launchClock.ElapsedMilliseconds -ge $nextTimelineAt) {
            # Name the artifact from the time the capture actually starts. A
            # high-resolution screencap can take longer than the requested
            # interval; naming it from the scheduled target otherwise causes
            # later samples to overwrite the earlier evidence.
            $timelineCaptureStartedAt = $launchClock.ElapsedMilliseconds
            $timelineShot = Join-Path $runDir ("timeline-{0:D6}.png" -f $timelineCaptureStartedAt)
            $timelineCaptured = Capture-Screenshot -Path $timelineShot
            if ($timelineCaptured) {
                Copy-Item -LiteralPath $timelineShot -Destination $lastScreenshot -Force
                if ($null -ne $readyAt -and $DismissVeil -and
                    ($launchClock.ElapsedMilliseconds -ge ($veilDismissAt + 100)) -and
                    (Test-PanelVisible -Path $timelineShot) -and ($null -eq $panelAt)) {
                    Copy-Item -LiteralPath $timelineShot -Destination $panelScreenshot -Force
                    $panelAt = Get-DeviceEpochMilliseconds
                    $panelElapsed = $launchClock.ElapsedMilliseconds
                    Write-Host ("  panel-visible at {0} ms" -f $panelElapsed)
                }
            }
            $timelineRow = [PSCustomObject]@{
                ElapsedMs = $launchClock.ElapsedMilliseconds
                DeviceEpochMs = Get-DeviceEpochMilliseconds
                ReadyMarkerSeen = $null -ne $readyAt
                PlasmashellPid = $plasmashellPid
                PlasmashellStats = Get-GuestProcessStats -TargetPid $plasmashellPid
                PanelVisible = $null -ne $panelAt
            }
            $timelineRow |
                ConvertTo-Csv -NoTypeInformation |
                Select-Object -Skip 1 |
                Add-Content -LiteralPath $timelinePath -Encoding UTF8
            foreach ($threadLine in (Get-ProcessThreadStats -TargetPid $plasmashellPid)) {
                "{0}`t{1}" -f $launchClock.ElapsedMilliseconds, $threadLine |
                    Add-Content -LiteralPath $threadTimelinePath -Encoding UTF8
            }
            # Do not catch up missed intervals: back-to-back screencaps would
            # become an observer-induced workload on the same startup path.
            $nextTimelineAt = $launchClock.ElapsedMilliseconds + $TimelineIntervalMilliseconds
        }

        if ($null -ne $readyAt -and $null -ne $panelAt) {
            break
        }
        Start-Sleep -Milliseconds $PanelPollMilliseconds
    }

    if (Test-Path -LiteralPath $lastScreenshot) {
        Copy-Item -LiteralPath $lastScreenshot -Destination (Join-Path $runDir "last-observed.png") -Force
    }
    $launchClock.Stop()

    Stop-PerfettoTrace -PerfettoProcessId $perfettoPid
    if ($null -ne $perfettoPid) {
        try {
            Invoke-AdbText @("pull", $remoteTrace, (Join-Path $runDir "startup.pftrace")) | Out-Null
        }
        catch {
            Write-Warning "Perfetto trace pull failed for run ${run}: $($_.Exception.Message)"
        }
        & adb -s $DeviceId shell rm -f $remoteTrace 2>$null | Out-Null
    }

    Invoke-AdbToFile @("logcat", "-d") (Join-Path $runDir "logcat.txt")
    Invoke-AdbToFile @("shell", "dumpsys", "window", "windows") (Join-Path $runDir "window.txt")
    Invoke-AdbToFile @("shell", "dumpsys", "gfxinfo", $PackageName) (Join-Path $runDir "gfxinfo.txt")
    Invoke-AdbToFile @("shell", "dumpsys", "meminfo", $PackageName) (Join-Path $runDir "meminfo.txt")
    Invoke-AdbToFile @("shell", "ps", "-A", "-o", "PID,PPID,NAME,ARGS") (Join-Path $runDir "processes.txt")
    Invoke-AdbToFile @("shell", "run-as", $PackageName, "cat", "files/diagnostics/host.log") (Join-Path $runDir "host.log")
    Invoke-AdbToFile @("shell", "run-as", $PackageName, "cat", "files/diagnostics/guest.log") (Join-Path $runDir "guest.log")
    Invoke-AdbToFile @("shell", "run-as", $PackageName, "cat", "files/runtime-B/var/lib/localdesktop/plasma.log") (Join-Path $runDir "plasma.log")
    Invoke-AdbToFile @("shell", "run-as", $PackageName, "cat", "files/runtime-B/var/lib/localdesktop/kwin.log") (Join-Path $runDir "kwin.log")
    Invoke-AdbToFile @("shell", "run-as", $PackageName, "cat", "files/runtime-B/var/lib/localdesktop/plasma-ready") (Join-Path $runDir "plasma-ready.txt")

    $events = Get-StartupEvents -AfterEpochMilliseconds $startEpoch
    $events | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $runDir "startup-events.json") -Encoding UTF8
    $plasmaTraceEvents = @(Get-PlasmaStartupTraceEvents -Path (Join-Path $runDir "plasma.log"))
    $plasmaTraceEvents | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $runDir "plasma-startup-trace.json") -Encoding UTF8
    $shellCoronaLoad = Get-FirstPlasmaStartupTraceEvent -Events $plasmaTraceEvents -Event "shellcorona-load-entry"
    $panelViewConstructed = Get-FirstPlasmaStartupTraceEvent -Events $plasmaTraceEvents -Event "panelview-constructed"
    $panelContainmentUiReady = Get-FirstPlasmaStartupTraceEvent -Events $plasmaTraceEvents -Event "panel-containment-ui-ready"
    $desktopUiReady = Get-FirstPlasmaStartupTraceEvent -Events $plasmaTraceEvents -Event "desktop-containment-ui-ready"

    # PanelView can emit a transient visible=true while its QML view is being
    # constructed, before ContainmentView applies the panel containment's own
    # uiReady gate. Count the first visible event at/after that gate instead.
    $panelFirstVisible = $null
    if ($panelContainmentUiReady) {
        $panelFirstVisible = @(
            $plasmaTraceEvents |
                Where-Object {
                    $_.Event -eq "panelview-visible-changed" -and
                    $_.Detail -match "visible\s*=\s*true" -and
                    $_.TimestampMs -ge $panelContainmentUiReady.TimestampMs
                } |
                Sort-Object TimestampMs, LineIndex |
                Select-Object -First 1
        ) | Select-Object -First 1
    }
    $activityTotal = ($activityOutput | Select-String -Pattern "TotalTime:\s*(\d+)").Matches.Groups[1].Value

    $result = [PSCustomObject]@{
        Run = $run
        Device = $DeviceId
        Model = $deviceModel
        Android = "$androidRelease (API $androidSdk)"
        Package = $PackageName
        Activity = $activity
        ActivityTotalTimeMs = if ($activityTotal) { [int]$activityTotal } else { $null }
        HostLaunchToAmReturnMs = $activityElapsed
        LaunchToPlasmaReadyMs = $readyElapsed
        LaunchToVeilDismissMs = $veilDismissAt
        LaunchToPanelVisibleMs = $panelElapsed
        LaunchToPlasmashellProcessMs = $plasmashellAt
        ShellCoronaLoadEpochMs = if ($shellCoronaLoad) { $shellCoronaLoad.TimestampMs } else { $null }
        PanelViewConstructedEpochMs = if ($panelViewConstructed) { $panelViewConstructed.TimestampMs } else { $null }
        PanelContainmentUiReadyEpochMs = if ($panelContainmentUiReady) { $panelContainmentUiReady.TimestampMs } else { $null }
        PanelFirstVisibleEpochMs = if ($panelFirstVisible) { $panelFirstVisible.TimestampMs } else { $null }
        DesktopUiReadyEpochMs = if ($desktopUiReady) { $desktopUiReady.TimestampMs } else { $null }
        PlasmashellStartToPanelViewConstructedMs = Get-PlasmaStartupTraceDelta -StartEvent $shellCoronaLoad -EndEvent $panelViewConstructed
        PanelViewConstructedToPanelUiReadyMs = Get-PlasmaStartupTraceDelta -StartEvent $panelViewConstructed -EndEvent $panelContainmentUiReady
        PlasmashellStartToPanelFirstVisibleMs = Get-PlasmaStartupTraceDelta -StartEvent $shellCoronaLoad -EndEvent $panelFirstVisible
        PlasmashellStartToDesktopUiReadyMs = Get-PlasmaStartupTraceDelta -StartEvent $shellCoronaLoad -EndEvent $desktopUiReady
        PlasmaReadyMarkerEpochMs = $readyAt
        PanelVisibleEpochMs = $panelAt
        PlasmashellPid = $plasmashellPid
        Perfetto = Test-Path -LiteralPath (Join-Path $runDir "startup.pftrace")
        Timeline = if ($CaptureTimeline) { $timelinePath } else { $null }
        ArtifactDirectory = $runDir
    }
    $runResults += $result
    $result | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $runDir "result.json") -Encoding UTF8
    Write-Host ("  result: ready={0} ms panel={1} ms plasmashell={2} ms panel-construct={3} ms panel-ui-ready={4} ms desktop-ui-ready={5} ms" -f `
        $readyElapsed, $panelElapsed, $plasmashellAt, $result.PlasmashellStartToPanelViewConstructedMs, `
        $result.PanelViewConstructedToPanelUiReadyMs, $result.PlasmashellStartToDesktopUiReadyMs)
}

$runResults | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputRoot "summary.json") -Encoding UTF8
$runResults | Format-Table Run, ActivityTotalTimeMs, LaunchToPlasmaReadyMs, LaunchToPanelVisibleMs, LaunchToPlasmashellProcessMs, Perfetto
Write-Host "Artifacts: $OutputRoot"
