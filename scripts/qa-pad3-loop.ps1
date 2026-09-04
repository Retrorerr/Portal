<#
.SYNOPSIS
    Automated Continuous QA Validation Loop on OnePlus Pad 3 (adb: f105b146).

.DESCRIPTION
    Automates the build, install, launch, diagnostics retrieval, screenshot capture,
    crash analysis, and release gate verification for Portal on the OnePlus Pad 3.

    The release result is intentionally stricter than a partial diagnostics check. A
    release pass requires all of the following evidence from the current run:
    an identified KWin surface, an enabled output, a genuine host presentation,
    live KWin and Plasma lifecycle evidence, a non-black screenshot, and at least
    60 seconds of explicit stability evidence. A release-labelled run also needs
    independently collected, verified background/resume and force-close/reopen
    scenario evidence. Markers, generic presentation text, process-name checks,
    and stale logs cannot satisfy the release gate.

    Partial checks report useful diagnostics but are never promoted to a release pass.

    Partial/release checks include:
    1. Screen resolution: 3392x2400 (or 2400x3392).
    2. Current-run KWin connection: verifies an identified KWin Wayland client/surface in host.log.
    3. Current-run host presentation: verifies a correlated Android display-present event.
    4. Crash/error diagnostics.

    Release-only checks additionally require output-enabled, KWin/Plasma lifecycle,
    screenshot-content, and stability evidence.

.PARAMETER DeviceId
    Target adb device ID (default: "f105b146" for OnePlus Pad 3).

.PARAMETER ArtifactDir
    Output directory for logs, screenshots, and validation reports.

.PARAMETER SkipBuild
    Skip APK compilation if an existing build should be validated.

.PARAMETER SkipInstall
    Skip APK installation (strictly preserving existing app/guest data).

.PARAMETER SkipLaunch
    Skip launching the NativeActivity.

.PARAMETER TimeoutSeconds
    Maximum wait time for desktop startup and diagnostics generation.

.PARAMETER StabilitySeconds
    Minimum explicit stability interval required for a release pass (default: 60).

.PARAMETER LifecycleScenarioContent
    Timestamped, independently collected and verified background/resume plus
    force-close/reopen evidence. Missing content fails the release gate.

.PARAMETER EnforceReleaseGates
    If set, exits with non-zero exit code if release gates fail.

.PARAMETER FunctionsOnly
    Only load helper functions into the session without running the loop.

.EXAMPLE
    .\scripts\qa-pad3-loop.ps1
    Runs the full continuous validation loop on the OnePlus Pad 3.

.EXAMPLE
    . .\scripts\qa-pad3-loop.ps1 -FunctionsOnly
    Test-ScreenResolution -DeviceId "f105b146"
    Test-KWinConnection -DeviceId "f105b146"
    Test-WaylandReadinessMarker -DeviceId "f105b146"
#>

[CmdletBinding()]
param(
    [string]$DeviceId = "f105b146",
    [string]$PackageName = "app.polarbear",
    [string]$ActivityName = "android.app.NativeActivity",
    [int]$ExpectedWidth = 3392,
    [int]$ExpectedHeight = 2400,
    [string]$ArtifactDir = "",
    [string]$ApkPath = "",
    [string]$LifecycleScenarioContent = "",
    [switch]$SkipBuild,
    [switch]$SkipInstall,
    [switch]$SkipLaunch,
    [int]$TimeoutSeconds = 120,
    [ValidateRange(60, 86400)]
    [int]$StabilitySeconds = 60,
    [switch]$EnforceReleaseGates,
    [switch]$FunctionsOnly
)

$ErrorActionPreference = "Continue"
$PSNativeCommandUseErrorActionPreference = $false

# Determine repository root and artifact directory
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

# ---------------------------------------------------------------------------
# Helper Environment & Setup Functions
# ---------------------------------------------------------------------------

function Initialize-BuildEnvironment {
    [CmdletBinding()]
    param()

    Write-Host "[*] Initializing build environment..." -ForegroundColor Cyan

    # 1. Android SDK
    if (-not $env:ANDROID_HOME -or -not (Test-Path $env:ANDROID_HOME)) {
        $sdkCandidates = @(
            "C:\Users\masob\AppData\Local\Android\Sdk",
            (Join-Path $env:LOCALAPPDATA "Android\Sdk")
        )
        foreach ($candidate in $sdkCandidates) {
            if (Test-Path $candidate) {
                $env:ANDROID_HOME = $candidate
                break
            }
        }
    }
    if ($env:ANDROID_HOME) {
        Write-Host "    ANDROID_HOME: $env:ANDROID_HOME" -ForegroundColor Gray
    } else {
        Write-Warning "ANDROID_HOME could not be detected."
    }

    # 2. Android NDK Toolchain (provides clang for C/Rust dependencies like ring)
    if ($env:ANDROID_HOME) {
        $ndkDir = Join-Path $env:ANDROID_HOME "ndk"
        if (Test-Path $ndkDir) {
            $latestNdk = Get-ChildItem -Path $ndkDir -Directory | Sort-Object Name -Descending | Select-Object -First 1
            if ($latestNdk) {
                $env:ANDROID_NDK_HOME = $latestNdk.FullName
                $env:ANDROID_NDK_ROOT = $latestNdk.FullName
                $llvmBin = Join-Path $latestNdk.FullName "toolchains\llvm\prebuilt\windows-x86_64\bin"
                if ((Test-Path $llvmBin) -and ($env:PATH -notlike "*$llvmBin*")) {
                    $env:PATH = "$llvmBin;" + $env:PATH
                    Write-Host "    Prepended NDK LLVM bin: $llvmBin" -ForegroundColor Gray
                }
            }
        }
    }

    # 3. Gradle executable
    $gradleOnPath = Get-Command gradle.bat -ErrorAction SilentlyContinue
    if (-not $gradleOnPath) {
        $gradleDists = Join-Path $env:USERPROFILE ".gradle\wrapper\dists"
        if (Test-Path $gradleDists) {
            $gradleBat = Get-ChildItem -Path $gradleDists -Recurse -Filter "gradle.bat" -ErrorAction SilentlyContinue |
                Select-Object -First 1
            if ($gradleBat) {
                $gradleBin = Split-Path -Parent $gradleBat.FullName
                $env:PATH = "$gradleBin;" + $env:PATH
                Write-Host "    Prepended Gradle bin: $gradleBin" -ForegroundColor Gray
            }
        }
    }

    # 4. Locate or compile x.exe (xbuild)
    $xbuildCandidates = @(
        (Join-Path $RepoRoot "patches\xbuild\target\debug\x.exe"),
        "C:\Users\masob\Documents\Portal\patches\xbuild\target\debug\x.exe"
    )
    $xbuildPath = $null
    foreach ($candidate in $xbuildCandidates) {
        if (Test-Path $candidate) {
            $xbuildPath = $candidate
            break
        }
    }

    if (-not $xbuildPath) {
        Write-Host "    Building xbuild tool..." -ForegroundColor Yellow
        $xbuildManifest = Join-Path $RepoRoot "patches\xbuild\Cargo.toml"
        & cargo build --manifest-path $xbuildManifest
        $defaultX = Join-Path $RepoRoot "patches\xbuild\target\debug\x.exe"
        if (Test-Path $defaultX) {
            $xbuildPath = $defaultX
        }
    }

    return $xbuildPath
}

# ---------------------------------------------------------------------------
# Release Gate 1: Resolution Verification (3392x2400)
# ---------------------------------------------------------------------------

function Test-ScreenResolution {
    <#
    .SYNOPSIS
        Verifies display resolution matches OnePlus Pad 3 native spec (3392x2400).
    #>
    [CmdletBinding()]
    param(
        [string]$DeviceId = "f105b146",
        [int]$ExpectedWidth = 3392,
        [int]$ExpectedHeight = 2400,
        [string]$ScreenshotPath = ""
    )

    $result = [PSCustomObject]@{
        Gate           = "Screen Resolution (3392x2400)"
        Passed         = $false
        DetectedWidth  = 0
        DetectedHeight = 0
        Method         = "Unknown"
        Message        = ""
    }

    # Priority 1: Check PNG screenshot dimensions if provided and valid
    if ($ScreenshotPath -and (Test-Path $ScreenshotPath)) {
        try {
            $bytes = [System.IO.File]::ReadAllBytes($ScreenshotPath)
            # Check PNG magic (89 50 4E 47 0D 0A 1A 0A) and IHDR at byte 12
            if ($bytes.Length -ge 24 -and $bytes[12] -eq 73 -and $bytes[13] -eq 72 -and $bytes[14] -eq 68 -and $bytes[15] -eq 82) {
                # PNG IHDR dimensions are big-endian; do not reverse the byte
                # order through BitConverter (which is little-endian here).
                $w = [uint32](($bytes[16] -shl 24) -bor ($bytes[17] -shl 16) -bor ($bytes[18] -shl 8) -bor $bytes[19])
                $h = [uint32](($bytes[20] -shl 24) -bor ($bytes[21] -shl 16) -bor ($bytes[22] -shl 8) -bor $bytes[23])
                $result.DetectedWidth = $w
                $result.DetectedHeight = $h
                $result.Method = "Screenshot IHDR Inspection"

                if (($w -eq $ExpectedWidth -and $h -eq $ExpectedHeight) -or
                    ($w -eq $ExpectedHeight -and $h -eq $ExpectedWidth)) {
                    $result.Passed = $true
                    $result.Message = "Screenshot confirmed native resolution ${w}x${h}."
                    return $result
                }
                $result.Message = "Screenshot dimensions ${w}x${h} do not match expected ${ExpectedWidth}x${ExpectedHeight}."
                return $result
            }
            $result.Message = "Screenshot is not a valid PNG with an IHDR header."
            return $result
        } catch {
            $result.Message = "Screenshot dimensions could not be inspected; treating them as unknown: $($_.Exception.Message)"
            return $result
        }
    }

    # Priority 2: Check device wm size via adb
    try {
        $wmOut = & adb -s $DeviceId shell wm size 2>&1
        $wmText = ($wmOut | Out-String).Trim()

        # Parse "Physical size: 2400x3392" or "Override size: ..."
        if ($wmText -match '(?:Override|Physical) size:\s*(\d+)x(\d+)') {
            $w = [int]$Matches[1]
            $h = [int]$Matches[2]
            $result.DetectedWidth = $w
            $result.DetectedHeight = $h
            $result.Method = "adb shell wm size"

            if (($w -eq $ExpectedWidth -and $h -eq $ExpectedHeight) -or
                ($w -eq $ExpectedHeight -and $h -eq $ExpectedWidth)) {
                $result.Passed = $true
                $result.Message = "Device reported native physical display size ${w}x${h}."
                return $result
            } else {
                $result.Message = "Unexpected resolution reported by wm size: ${w}x${h} (expected ${ExpectedWidth}x${ExpectedHeight})."
            }
        } else {
            $result.Message = "Could not parse wm size output: $wmText"
        }
    } catch {
        $result.Message = "Failed to query wm size: $_"
    }

    return $result
}

# ---------------------------------------------------------------------------
# Strict release-evidence parsing helpers
# ---------------------------------------------------------------------------

function Get-QaCurrentRunLines {
    <#
    .SYNOPSIS
        Returns only timestamped diagnostics emitted at or after this QA run.

    .DESCRIPTION
        Diagnostics are append-only and may contain rotated logs from earlier
        launches.  A release decision without a run boundary is therefore
        unknown by design.  Untimestamped text is not allowed to cross that
        boundary because it cannot be proven current.
    #>
    [CmdletBinding()]
    param(
        [AllowEmptyString()]
        [string]$Content = "",
        [Int64]$RunStartTimestampMs = 0
    )

    $records = New-Object 'System.Collections.Generic.List[object]'
    if ([string]::IsNullOrWhiteSpace($Content) -or $RunStartTimestampMs -le 0) {
        return @()
    }

    foreach ($line in ($Content -split "`r?`n")) {
        if ($line -match '^\s*(\d{13})\s+') {
            try {
                $timestampMs = [Int64]::Parse($Matches[1], [Globalization.CultureInfo]::InvariantCulture)
            } catch {
                continue
            }
            if ($timestampMs -ge $RunStartTimestampMs) {
                [void]$records.Add([PSCustomObject]@{
                    Text        = $line
                    TimestampMs = $timestampMs
                })
            }
        }
    }

    return @($records.ToArray())
}

function Get-QaField {
    [CmdletBinding()]
    param(
        [string]$Line = "",
        [Parameter(Mandatory)]
        [string]$Name
    )

    $pattern = '(?i)(?:^|\s)' + [Regex]::Escape($Name) + '=(?:"([^"]*)"|(\S+))'
    $match = [Regex]::Match($Line, $pattern)
    if (-not $match.Success) {
        return $null
    }
    if ($match.Groups[1].Success) {
        return $match.Groups[1].Value
    }
    return $match.Groups[2].Value
}

function Convert-QaBoolean {
    [CmdletBinding()]
    param([AllowNull()][string]$Value)

    if ([string]::IsNullOrWhiteSpace($Value)) {
        return $null
    }
    if ($Value -match '^(?i:true|1|yes|on|enabled|alive|pass|passed|ok)$') {
        return $true
    }
    if ($Value -match '^(?i:false|0|no|off|disabled|dead|fail|failed|error)$') {
        return $false
    }
    return $null
}

function Convert-QaIdentifier {
    [CmdletBinding()]
    param([AllowNull()][string]$Value)

    if ([string]::IsNullOrWhiteSpace($Value)) {
        return ""
    }
    $normalized = $Value.Trim().Trim('"')
    if ($normalized -match '^Some\((.*)\)$') {
        $normalized = $Matches[1]
    }
    $normalized = $normalized -replace '^WlSurface#', ''
    return $normalized.Trim()
}

function Get-QaKWinSurfaceIdentity {
    <#
    .SYNOPSIS
        Parses the latest current-run KWin client/surface identity.
    #>
    [CmdletBinding()]
    param(
        [AllowEmptyString()]
        [string]$HostLogContent = "",
        [Int64]$RunStartTimestampMs = 0
    )

    $result = [PSCustomObject]@{
        Passed      = $false
        Generation  = $null
        ClientId    = $null
        SurfaceId   = $null
        Title       = ""
        TimestampMs = $null
        Message     = ""
    }

    if ($RunStartTimestampMs -le 0) {
        $result.Message = "Current-run boundary is missing; stale KWin identity cannot be accepted."
        return $result
    }

    $latest = $null
    foreach ($record in (Get-QaCurrentRunLines -Content $HostLogContent -RunStartTimestampMs $RunStartTimestampMs)) {
        $line = $record.Text
        if ($line -notmatch '(?i)\bstage=kwin-identified\b') {
            continue
        }

        # A newer malformed identity supersedes an earlier one; never retain
        # a previous generation when the current event is incomplete.
        $latest = $null

        # The ClientId is a Rust Debug value and may contain spaces. Surface is
        # deliberately the next field so unrelated text cannot be mistaken for
        # an object identity.
        $match = [Regex]::Match(
            $line,
            '(?i)\bgeneration=(?<generation>\d+)\b.*?\bclient=(?<client>.+?)\s+surface=(?<surface>\S+)\s+title=(?<title>[^\r\n]+)$'
        )
        if ($match.Success) {
            $latest = [PSCustomObject]@{
                Generation  = $match.Groups['generation'].Value
                ClientId    = $match.Groups['client'].Value.Trim()
                SurfaceId   = $match.Groups['surface'].Value.Trim()
                Title       = $match.Groups['title'].Value.Trim()
                TimestampMs = $record.TimestampMs
            }
        }
    }

    if ($null -eq $latest) {
        $result.Message = "No current-run stage=kwin-identified event with a client and surface was found."
        return $result
    }

    $titleIsKwin = $latest.Title -match '^(?i:KDE Wayland Compositor)(?:\s|$)'
    $surface = Convert-QaIdentifier $latest.SurfaceId
    $client = Convert-QaIdentifier $latest.ClientId
    if (-not $titleIsKwin -or [string]::IsNullOrWhiteSpace($surface) -or [string]::IsNullOrWhiteSpace($client)) {
        $result.Message = "Current-run KWin identity is incomplete or does not identify a KWin Wayland surface."
        return $result
    }

    $result.Passed = $true
    $result.Generation = $latest.Generation
    $result.ClientId = $client
    $result.SurfaceId = $surface
    $result.Title = $latest.Title
    $result.TimestampMs = $latest.TimestampMs
    $result.Message = "Current-run KWin surface identified (generation=$($result.Generation), surface=$($result.SurfaceId))."
    return $result
}

function Get-QaOutputState {
    <#
    .SYNOPSIS
        Parses the latest explicit output enabled/disabled state in this run.

    .DESCRIPTION
        A disabled KWin title may be an intermediate startup state.  The latest
        explicit state wins, so a later active KWin title is accepted while an
        ending disabled state remains a failure.  An output state that is never
        reported is unknown and fails closed.
    #>
    [CmdletBinding()]
    param(
        [AllowEmptyString()]
        [string]$HostLogContent = "",
        [Int64]$RunStartTimestampMs = 0,
        [PSCustomObject]$KWinIdentity = $null
    )

    $result = [PSCustomObject]@{
        Known       = $false
        Enabled     = $false
        Source      = ""
        TimestampMs = $null
        Message     = ""
    }

    $events = New-Object 'System.Collections.Generic.List[object]'
    foreach ($record in (Get-QaCurrentRunLines -Content $HostLogContent -RunStartTimestampMs $RunStartTimestampMs)) {
        $line = $record.Text
        if ($null -ne $KWinIdentity -and $KWinIdentity.Passed -and $record.TimestampMs -lt $KWinIdentity.TimestampMs) {
            continue
        }
        $isOutputStage = $line -match '(?i)\bstage=output-configured\b|\bstage=output-state\b'
        if ($isOutputStage) {
            $eventGeneration = Get-QaField -Line $line -Name 'generation'
            $eventSurface = Get-QaField -Line $line -Name 'surface'
            if ($null -ne $KWinIdentity -and $KWinIdentity.Passed -and
                (($eventGeneration -and $eventGeneration -ne [string]$KWinIdentity.Generation) -or
                 ($eventSurface -and (Convert-QaIdentifier $eventSurface) -ne (Convert-QaIdentifier $KWinIdentity.SurfaceId)))) {
                continue
            }
            $state = Convert-QaBoolean (Get-QaField -Line $line -Name 'output_enabled')
            if ($null -eq $state) {
                $state = Convert-QaBoolean (Get-QaField -Line $line -Name 'enabled')
            }
            if ($null -eq $state) {
                if ($line -match '(?i)\boutput\s+(?:is\s+)?(enabled|disabled)\b') {
                    $state = Convert-QaBoolean $Matches[1]
                }
            }
            if ($null -ne $state) {
                [void]$events.Add([PSCustomObject]@{
                    Known       = $true
                    Enabled     = [bool]$state
                    Source      = "explicit-output-state"
                    TimestampMs = $record.TimestampMs
                    Line        = $line
                })
            } else {
                [void]$events.Add([PSCustomObject]@{
                    Known       = $false
                    Enabled     = $false
                    Source      = "invalid-output-state"
                    TimestampMs = $record.TimestampMs
                    Line        = $line
                })
            }
        }

        # KWin's nested output title is protocol-visible evidence.  Keep only
        # titles that explicitly describe the disabled or active state; a bare
        # compositor title is not enough to claim an enabled output.
        $titleMatches = [Regex]::Matches($line, '(?i)set_title\("([^"]+)"\)')
        foreach ($titleMatch in $titleMatches) {
            $title = $titleMatch.Groups[1].Value
            $state = $null
            if ($title -match '(?i)output\s+disabled') {
                $state = $false
            } elseif ($title -match '(?i)press\s+right\s+control\s+key\s+to\s+grab\s+pointer|output\s+enabled') {
                $state = $true
            }
            if ($null -ne $state) {
                [void]$events.Add([PSCustomObject]@{
                    Known       = $true
                    Enabled     = [bool]$state
                    Source      = "kwin-output-title"
                    TimestampMs = $record.TimestampMs
                    Line        = $line
                })
            }
        }

        # Some logger configurations preserve the title as a structured field
        # rather than the raw set_title protocol trace.
        if ($line -match '(?i)\btitle=(?<title>KDE Wayland Compositor[^\r\n]+)$') {
            $title = $Matches['title']
            $state = $null
            if ($title -match '(?i)output\s+disabled') {
                $state = $false
            } elseif ($title -match '(?i)press\s+right\s+control\s+key\s+to\s+grab\s+pointer|output\s+enabled') {
                $state = $true
            }
            if ($null -ne $state) {
                [void]$events.Add([PSCustomObject]@{
                    Known       = $true
                    Enabled     = [bool]$state
                    Source      = "kwin-output-title-field"
                    TimestampMs = $record.TimestampMs
                    Line        = $line
                })
            }
        }
    }

    if ($events.Count -eq 0) {
        $result.Message = "No current-run explicit output-enabled state was found."
        return $result
    }

    $latest = $events[$events.Count - 1]
    $result.Known = [bool]$latest.Known
    $result.Enabled = [bool]$latest.Enabled
    $result.Source = $latest.Source
    $result.TimestampMs = $latest.TimestampMs
    if ($result.Enabled) {
        $result.Message = "Current-run output is enabled ($($result.Source))."
    } else {
        $result.Message = "Current-run latest output state is disabled ($($result.Source))."
    }
    return $result
}

function Test-QaHostPresentation {
    <#
    .SYNOPSIS
        Verifies a genuine current-run Android display presentation.

    .DESCRIPTION
        Only an android-frame-presented event carrying the exact KWin
        generation/surface, the physical display-present evidence label, and a
        positive Android presentation timestamp is accepted.  plasma-ready and
        generic presentation strings are intentionally ignored.
    #>
    [CmdletBinding()]
    param(
        [AllowEmptyString()]
        [string]$HostLogContent = "",
        [Int64]$RunStartTimestampMs = 0,
        [PSCustomObject]$KWinIdentity = $null
    )

    $result = [PSCustomObject]@{
        Passed          = $false
        HostPresented   = $false
        Generation      = $null
        SurfaceId       = $null
        Evidence        = ""
        PresentationNs  = $null
        TimestampMs     = $null
        Message         = ""
    }

    if ($null -eq $KWinIdentity) {
        $KWinIdentity = Get-QaKWinSurfaceIdentity -HostLogContent $HostLogContent -RunStartTimestampMs $RunStartTimestampMs
    }
    if (-not $KWinIdentity.Passed) {
        $result.Message = "Host presentation cannot be correlated without a current-run KWin surface identity."
        return $result
    }

    foreach ($record in (Get-QaCurrentRunLines -Content $HostLogContent -RunStartTimestampMs $RunStartTimestampMs)) {
        $line = $record.Text
        if ($line -notmatch '(?i)\bstage=android-frame-presented\b') {
            continue
        }

        $generation = Get-QaField -Line $line -Name 'generation'
        $surfaceId = Get-QaField -Line $line -Name 'surface'
        $evidence = Get-QaField -Line $line -Name 'evidence'
        $presentationNs = Get-QaField -Line $line -Name 'android_present_ns'
        $hostPresented = Convert-QaBoolean (Get-QaField -Line $line -Name 'host_presented')
        $nestedTimestamp = Get-QaField -Line $line -Name 'timestamp_ms'

        if ($record.TimestampMs -lt $KWinIdentity.TimestampMs) {
            continue
        }
        if ($generation -eq [string]$KWinIdentity.Generation -and
            (Convert-QaIdentifier $surfaceId) -eq (Convert-QaIdentifier $KWinIdentity.SurfaceId)) {
            # A later matching event is authoritative. An explicit false,
            # stale, or malformed attempt must clear an earlier presentation.
            $result.Passed = $false
            $result.HostPresented = $false
            $result.Generation = $null
            $result.SurfaceId = $null
            $result.Evidence = ""
            $result.PresentationNs = $null
            $result.TimestampMs = $null
        }
        if ($hostPresented -eq $false) {
            continue
        }
        if ($nestedTimestamp -and $nestedTimestamp -match '^\d+$' -and [Int64]$nestedTimestamp -lt $RunStartTimestampMs) {
            continue
        }
        if ($generation -ne [string]$KWinIdentity.Generation) {
            continue
        }
        if ((Convert-QaIdentifier $surfaceId) -ne (Convert-QaIdentifier $KWinIdentity.SurfaceId)) {
            continue
        }
        if ($evidence -notmatch '^(?i:egl-android-display-present)$') {
            continue
        }
        if ([string]::IsNullOrWhiteSpace($presentationNs) -or $presentationNs -notmatch '(?i)(?:^|\D)(\d+)(?:\D|$)' -or [Int64]$Matches[1] -le 0) {
            continue
        }

        $result.Passed = $true
        $result.HostPresented = $true
        $result.Generation = $generation
        $result.SurfaceId = Convert-QaIdentifier $surfaceId
        $result.Evidence = $evidence
        $result.PresentationNs = [Int64]$Matches[1]
        $result.TimestampMs = $record.TimestampMs
    }

    if ($result.Passed) {
        $result.Message = "Current-run host presentation correlated to KWin generation=$($result.Generation), surface=$($result.SurfaceId)."
    } else {
        $result.Message = "No genuine current-run host presentation correlated to the identified KWin surface was found."
    }
    return $result
}

function Test-QaKWinPlasmaLifecycle {
    <#
    .SYNOPSIS
        Requires explicit current-run KWin and Plasma alive evidence.

    .DESCRIPTION
        A process name or pgrep/pidof result is not a lifecycle proof.  The
        parser accepts only structured alive fields (or a timestamped,
        structured process snapshot supplied by the caller), and a later exit
        or disconnect clears the corresponding state.
    #>
    [CmdletBinding()]
    param(
        [AllowEmptyString()]
        [string]$HostLogContent = "",
        [AllowEmptyString()]
        [string]$GuestLogContent = "",
        [AllowEmptyString()]
        [string]$ProcessSnapshotContent = "",
        [Int64]$ProcessSnapshotTimestampMs = 0,
        [Int64]$RunStartTimestampMs = 0,
        [PSCustomObject]$KWinIdentity = $null
    )

    $result = [PSCustomObject]@{
        Passed       = $false
        KWinAlive    = $null
        PlasmaAlive  = $null
        Source       = ""
        TimestampMs  = $null
        Message      = ""
    }

    if ($RunStartTimestampMs -le 0) {
        $result.Message = "Current-run boundary is missing; lifecycle evidence is unknown."
        return $result
    }
    if ($null -eq $KWinIdentity -or -not $KWinIdentity.Passed) {
        $result.Message = "Lifecycle evidence cannot be correlated without a current-run KWin identity."
        return $result
    }

    $records = New-Object 'System.Collections.Generic.List[object]'
    foreach ($record in (Get-QaCurrentRunLines -Content $HostLogContent -RunStartTimestampMs $RunStartTimestampMs)) {
        [void]$records.Add([PSCustomObject]@{
            Text        = $record.Text
            TimestampMs = $record.TimestampMs
            Source      = "host"
        })
    }
    foreach ($record in (Get-QaCurrentRunLines -Content $GuestLogContent -RunStartTimestampMs $RunStartTimestampMs)) {
        [void]$records.Add([PSCustomObject]@{
            Text        = $record.Text
            TimestampMs = $record.TimestampMs
            Source      = "guest"
        })
    }
    if ($ProcessSnapshotTimestampMs -ge $RunStartTimestampMs -and -not [string]::IsNullOrWhiteSpace($ProcessSnapshotContent)) {
        [void]$records.Add([PSCustomObject]@{
            Text        = $ProcessSnapshotContent
            TimestampMs = $ProcessSnapshotTimestampMs
            Source      = "process-snapshot"
        })
    }
    $orderedRecords = @($records | Sort-Object TimestampMs)

    $kwinAlive = $null
    $plasmaAlive = $null
    $lastSource = ""
    $lastTimestamp = $null
    foreach ($record in $orderedRecords) {
        $line = $record.Text
        if ($line -match '(?i)\bstage=qa-lifecycle\b') {
            $generation = Get-QaField -Line $line -Name 'generation'
            if ($record.TimestampMs -lt $KWinIdentity.TimestampMs -or
                [string]::IsNullOrWhiteSpace($generation) -or
                $generation -ne [string]$KWinIdentity.Generation) {
                continue
            }
            # Positive lifecycle evidence must be complete and explicit. A
            # partial/unknown record must not inherit an earlier alive state.
            $kwinValue = Get-QaField -Line $line -Name 'kwin_alive'
            $plasmaValue = Get-QaField -Line $line -Name 'plasma_alive'
            $kwinState = Convert-QaBoolean $kwinValue
            $plasmaState = Convert-QaBoolean $plasmaValue
            if ($null -ne $kwinState -and $null -ne $plasmaState) {
                $kwinAlive = [bool]$kwinState
                $plasmaAlive = [bool]$plasmaState
                $lastSource = "structured-lifecycle"
                $lastTimestamp = $record.TimestampMs
            } else {
                $kwinAlive = $null
                $plasmaAlive = $null
                $lastSource = "invalid-lifecycle"
                $lastTimestamp = $record.TimestampMs
            }
        }

        # Negative lifecycle events are fail-safe invalidations. They do not
        # claim a new positive generation, but a current-run exit/disconnect
        # can never be hidden by an older process snapshot.
        if ($line -match '(?i)\bstage=kwin-disconnected\b|\bstage=desktop-exit\b|\bstage=plasma-failed\b') {
            $kwinAlive = $false
            if ($line -match '(?i)\bstage=desktop-exit\b|\bstage=plasma-failed\b') {
                $plasmaAlive = $false
                $lastSource = "desktop-exit"
            } else {
                $lastSource = "kwin-disconnected"
            }
            $lastTimestamp = $record.TimestampMs
        }

        # A snapshot is positive evidence only with the explicit QA schema,
        # both health fields, and the identified KWin generation. It is in the
        # same timestamp-sorted stream, so an old snapshot cannot revive a
        # later exit.
        if ($record.Source -eq "process-snapshot" -and $line -match '(?i)\bstage=qa-process-snapshot\b') {
            $generation = Get-QaField -Line $line -Name 'generation'
            $kwinState = Convert-QaBoolean (Get-QaField -Line $line -Name 'kwin_alive')
            $plasmaState = Convert-QaBoolean (Get-QaField -Line $line -Name 'plasma_alive')
            if ($record.TimestampMs -ge $KWinIdentity.TimestampMs -and
                $generation -eq [string]$KWinIdentity.Generation -and
                $null -ne $kwinState -and $null -ne $plasmaState) {
                $kwinAlive = [bool]$kwinState
                $plasmaAlive = [bool]$plasmaState
                $lastSource = "structured-process-snapshot"
                $lastTimestamp = $record.TimestampMs
            } elseif ($generation -eq [string]$KWinIdentity.Generation) {
                $kwinAlive = $null
                $plasmaAlive = $null
                $lastSource = "invalid-process-snapshot"
                $lastTimestamp = $record.TimestampMs
            }
        }
    }

    $result.KWinAlive = $kwinAlive
    $result.PlasmaAlive = $plasmaAlive
    $result.Source = $lastSource
    $result.TimestampMs = $lastTimestamp
    if ($kwinAlive -eq $true -and $plasmaAlive -eq $true) {
        $result.Passed = $true
        $result.Message = "Current-run lifecycle reports KWin and Plasma alive ($($result.Source))."
    } else {
        $result.Message = "KWin/Plasma lifecycle evidence is missing or reports a dead process; process names alone are insufficient."
    }
    return $result
}

function Test-QaStability {
    <#
    .SYNOPSIS
        Requires explicit current-run stability evidence for the requested duration.
    #>
    [CmdletBinding()]
    param(
        [AllowEmptyString()]
        [string]$HostLogContent = "",
        [AllowEmptyString()]
        [string]$GuestLogContent = "",
        [Int64]$RunStartTimestampMs = 0,
        [Int64]$PresentationTimestampMs = 0,
        [string]$Generation = "",
        [ValidateRange(60, 86400)]
        [int]$MinimumStabilitySeconds = 60
    )

    $result = [PSCustomObject]@{
        Passed             = $false
        StableForSeconds   = $null
        RequiredSeconds    = $MinimumStabilitySeconds
        Source             = ""
        TimestampMs        = $null
        FirstVisibleTimestampMs = $PresentationTimestampMs
        Message            = ""
    }

    if ($RunStartTimestampMs -le 0 -or $PresentationTimestampMs -le 0 -or [string]::IsNullOrWhiteSpace($Generation)) {
        $result.Message = "Stability is unknown without a run boundary, KWin generation, and first host-presentation timestamp."
        return $result
    }

    $records = New-Object 'System.Collections.Generic.List[object]'
    foreach ($record in (Get-QaCurrentRunLines -Content $HostLogContent -RunStartTimestampMs $RunStartTimestampMs)) {
        [void]$records.Add($record)
    }
    foreach ($record in (Get-QaCurrentRunLines -Content $GuestLogContent -RunStartTimestampMs $RunStartTimestampMs)) {
        [void]$records.Add($record)
    }

    $candidate = $null
    foreach ($record in (@($records | Sort-Object TimestampMs))) {
        $line = $record.Text
        if ($line -notmatch '(?i)\bstage=qa-stability\b') {
            continue
        }

        $eventGeneration = Get-QaField -Line $line -Name 'generation'
        $statusValue = Get-QaField -Line $line -Name 'status'
        $durationValue = Get-QaField -Line $line -Name 'stable_for_ms'
        $kwinState = Convert-QaBoolean (Get-QaField -Line $line -Name 'kwin_alive')
        $plasmaState = Convert-QaBoolean (Get-QaField -Line $line -Name 'plasma_alive')
        $outputState = Convert-QaBoolean (Get-QaField -Line $line -Name 'output_enabled')

        # Every stability sample is a complete, current-generation health
        # assertion. Bare uptime, unknown status, or omitted health fields are
        # not stability evidence and invalidate an earlier candidate.
        $durationMs = 0L
        $durationOk = $durationValue -match '^\d+$' -and [Int64]::TryParse(
            $durationValue,
            [Globalization.NumberStyles]::Integer,
            [Globalization.CultureInfo]::InvariantCulture,
            [ref]$durationMs
        )
        $statusOk = $statusValue -match '^(?i:pass)$'
        $healthy = ($kwinState -eq $true -and $plasmaState -eq $true -and $outputState -eq $true)
        $current = ($eventGeneration -eq $Generation -and
            $statusOk -and $durationOk -and $healthy -and
            $record.TimestampMs -ge $PresentationTimestampMs)
        if (-not $current) {
            $candidate = $null
            continue
        }

        $requiredMs = [Int64]$MinimumStabilitySeconds * 1000L
        if ($durationMs -lt $requiredMs -or
            $record.TimestampMs -lt ($PresentationTimestampMs + $requiredMs)) {
            $candidate = $null
            continue
        }

        $candidate = [PSCustomObject]@{
            DurationMs  = $durationMs
            TimestampMs = $record.TimestampMs
        }
    }

    if ($null -ne $candidate) {
        $result.Passed = $true
        $result.StableForSeconds = [Math]::Round($candidate.DurationMs / 1000.0, 3)
        $result.Source = "explicit-current-generation-stability"
        $result.TimestampMs = $candidate.TimestampMs
    }

    if ($result.Passed) {
        $result.Message = "Explicit current-run stability evidence covers $($result.StableForSeconds)s (required $MinimumStabilitySeconds s)."
    } else {
        $result.Message = "No explicit current-run stability evidence covering at least $MinimumStabilitySeconds s was found."
    }
    return $result
}

function Test-QaLifecycleScenarios {
    <#
    .SYNOPSIS
        Validates separately collected background/resume and force-close/reopen evidence.

    .DESCRIPTION
        These scenarios span Android activity/process sessions and are not inferred from a
        host marker or a pgrep result. The collector must provide one timestamped,
        status=pass, verified=true record per scenario with pre/post session IDs,
        transition timestamps, fresh host-presentation evidence, non-black screenshots,
        and an artifact reference. Missing or malformed records fail closed. This
        parser validates the collector record shape only; it does not authenticate
        manually typed claims or replace review of the referenced artifacts.
    #>
    [CmdletBinding()]
    param(
        [AllowEmptyString()]
        [string]$Content = "",
        [Int64]$RunStartTimestampMs = 0
    )

    $result = [PSCustomObject]@{
        Passed                 = $false
        BackgroundForeground   = $false
        ForceCloseReopen       = $false
        RunId                  = $null
        Evidence               = @{}
        Message                = ""
    }

    if ($RunStartTimestampMs -le 0) {
        $result.Message = "Lifecycle scenarios are unknown without a current-run boundary."
        return $result
    }

    $latest = @{
        'background-resume'   = $null
        'force-close-reopen'  = $null
    }
    foreach ($record in (Get-QaCurrentRunLines -Content $Content -RunStartTimestampMs $RunStartTimestampMs)) {
        $line = $record.Text
        if ($line -notmatch '(?i)\bstage=qa-lifecycle-scenario\b') {
            continue
        }

        $scenario = Get-QaField -Line $line -Name 'scenario'
        if ($scenario -notin @('background-resume', 'force-close-reopen')) {
            continue
        }

        # Keep the latest record for each scenario. A later failed/unknown record
        # therefore invalidates an earlier pass instead of being skipped.
        $valid = $true
        $source = Get-QaField -Line $line -Name 'source'
        $runId = Get-QaField -Line $line -Name 'run_id'
        $artifact = Get-QaField -Line $line -Name 'artifact'
        $preSession = Get-QaField -Line $line -Name 'pre_session_id'
        $postSession = Get-QaField -Line $line -Name 'post_session_id'
        $preGeneration = Get-QaField -Line $line -Name 'pre_generation'
        $postGeneration = Get-QaField -Line $line -Name 'post_generation'
        $status = Get-QaField -Line $line -Name 'status'
        $verified = Convert-QaBoolean (Get-QaField -Line $line -Name 'verified')
        $prePresented = Convert-QaBoolean (Get-QaField -Line $line -Name 'pre_host_presented')
        $postPresented = Convert-QaBoolean (Get-QaField -Line $line -Name 'post_host_presented')
        $preScreenshot = Convert-QaBoolean (Get-QaField -Line $line -Name 'pre_screenshot_nonblack')
        $postScreenshot = Convert-QaBoolean (Get-QaField -Line $line -Name 'post_screenshot_nonblack')

        if ($source -ne 'qa-collector' -or $status -ne 'pass' -or $verified -ne $true -or
            [string]::IsNullOrWhiteSpace($runId) -or [string]::IsNullOrWhiteSpace($artifact) -or
            [string]::IsNullOrWhiteSpace($preSession) -or [string]::IsNullOrWhiteSpace($postSession) -or
            [string]::IsNullOrWhiteSpace($preGeneration) -or [string]::IsNullOrWhiteSpace($postGeneration) -or
            $prePresented -ne $true -or $postPresented -ne $true -or
            $preScreenshot -ne $true -or $postScreenshot -ne $true) {
            $valid = $false
        }

        $preTimestamp = 0L
        $postTimestamp = 0L
        if ($scenario -eq 'background-resume') {
            $preTimestampValue = Get-QaField -Line $line -Name 'background_ms'
            $postTimestampValue = Get-QaField -Line $line -Name 'foreground_ms'
        } else {
            $preTimestampValue = Get-QaField -Line $line -Name 'force_close_ms'
            $postTimestampValue = Get-QaField -Line $line -Name 'reopen_ms'
        }
        $preTimestampOk = $preTimestampValue -match '^\d+$' -and [Int64]::TryParse(
            $preTimestampValue,
            [Globalization.NumberStyles]::Integer,
            [Globalization.CultureInfo]::InvariantCulture,
            [ref]$preTimestamp
        )
        $postTimestampOk = $postTimestampValue -match '^\d+$' -and [Int64]::TryParse(
            $postTimestampValue,
            [Globalization.NumberStyles]::Integer,
            [Globalization.CultureInfo]::InvariantCulture,
            [ref]$postTimestamp
        )
        if (-not $preTimestampOk -or -not $postTimestampOk -or
            $preTimestamp -lt $RunStartTimestampMs -or $postTimestamp -le $preTimestamp -or
            $record.TimestampMs -lt $postTimestamp) {
            $valid = $false
        }
        if ($scenario -eq 'force-close-reopen' -and $preSession -eq $postSession) {
            $valid = $false
        }

        $latest[$scenario] = [PSCustomObject]@{
            Valid       = $valid
            RunId       = $runId
            Artifact    = $artifact
            TimestampMs = $record.TimestampMs
            Detail      = $line
        }
    }

    $background = $latest['background-resume']
    $reopen = $latest['force-close-reopen']
    $runIds = @(@($background, $reopen) | Where-Object { $null -ne $_ -and $_.Valid } | ForEach-Object { $_.RunId } | Select-Object -Unique)
    if ($runIds.Count -eq 1) {
        $result.RunId = $runIds[0]
    }
    $result.BackgroundForeground = $null -ne $background -and $background.Valid -and $runIds.Count -eq 1 -and $background.RunId -eq $result.RunId
    $result.ForceCloseReopen = $null -ne $reopen -and $reopen.Valid -and $runIds.Count -eq 1 -and $reopen.RunId -eq $result.RunId
    $result.Evidence = @{
        BackgroundForeground = $background
        ForceCloseReopen     = $reopen
    }
    $result.Passed = $result.BackgroundForeground -and $result.ForceCloseReopen
    if ($result.Passed) {
        $result.Message = "Verified background/resume and force-close/reopen scenarios for run_id=$($result.RunId)."
    } else {
        $missing = @()
        if (-not $result.BackgroundForeground) { $missing += 'background-resume' }
        if (-not $result.ForceCloseReopen) { $missing += 'force-close-reopen' }
        $result.Message = "Lifecycle scenario evidence is missing, stale, malformed, or unverified: $($missing -join ', ')."
    }
    return $result
}

function Test-ScreenshotNonBlack {
    <#
    .SYNOPSIS
        Rejects missing, invalid, or effectively black screenshots.
    #>
    [CmdletBinding()]
    param(
        [string]$ScreenshotPath = "",
        [double]$MinimumNonBlackFraction = 0.01,
        [double]$NonBlackLumaThreshold = 8.0
    )

    $result = [PSCustomObject]@{
        Passed             = $false
        Width              = 0
        Height             = 0
        SampledPixels      = 0
        NonBlackPixels     = 0
        NonBlackFraction   = 0.0
        MeanLuma           = 0.0
        MaxLuma            = 0.0
        Message            = ""
    }

    if ([string]::IsNullOrWhiteSpace($ScreenshotPath) -or -not (Test-Path -LiteralPath $ScreenshotPath -PathType Leaf)) {
        $result.Message = "Screenshot is missing; image content is unknown."
        return $result
    }

    $bitmap = $null
    try {
        Add-Type -AssemblyName System.Drawing -ErrorAction Stop
        $fullPath = (Resolve-Path -LiteralPath $ScreenshotPath).Path
        $bitmap = [System.Drawing.Bitmap]::new($fullPath)
        $width = $bitmap.Width
        $height = $bitmap.Height
        $result.Width = $width
        $result.Height = $height

        $step = [Math]::Max(1, [int][Math]::Floor([Math]::Sqrt(($width * [double]$height) / 250000.0)))
        $sampled = 0
        $nonBlack = 0
        $sumLuma = 0.0
        $maxLuma = 0.0
        for ($y = 0; $y -lt $height; $y += $step) {
            for ($x = 0; $x -lt $width; $x += $step) {
                $pixel = $bitmap.GetPixel($x, $y)
                $luma = if ($pixel.A -eq 0) {
                    0.0
                } else {
                    (0.2126 * $pixel.R) + (0.7152 * $pixel.G) + (0.0722 * $pixel.B)
                }
                $sampled++
                $sumLuma += $luma
                if ($luma -gt $maxLuma) { $maxLuma = $luma }
                if ($luma -gt $NonBlackLumaThreshold) { $nonBlack++ }
            }
        }
        $result.SampledPixels = $sampled
        $result.NonBlackPixels = $nonBlack
        if ($sampled -gt 0) {
            $result.NonBlackFraction = $nonBlack / [double]$sampled
            $result.MeanLuma = $sumLuma / [double]$sampled
        }
        $result.MaxLuma = $maxLuma
        $result.Passed = ($result.NonBlackFraction -ge $MinimumNonBlackFraction)
        if ($result.Passed) {
            $result.Message = "Screenshot contains non-black rendered content (fraction=$([Math]::Round($result.NonBlackFraction, 4)))."
        } else {
            $result.Message = "Screenshot is effectively black (fraction=$([Math]::Round($result.NonBlackFraction, 4)))."
        }
    } catch {
        $result.Message = "Screenshot content could not be inspected; treating it as unknown: $($_.Exception.Message)"
    } finally {
        if ($null -ne $bitmap) {
            $bitmap.Dispose()
        }
    }
    return $result
}

function Test-StrictPlasmaReleaseEvidence {
    <#
    .SYNOPSIS
        Evaluates the complete real-ARM64 Plasma release evidence contract.
    #>
    [CmdletBinding()]
    param(
        [AllowEmptyString()]
        [string]$HostLogContent = "",
        [AllowEmptyString()]
        [string]$GuestLogContent = "",
        [AllowEmptyString()]
        [string]$ProcessSnapshotContent = "",
        [Int64]$ProcessSnapshotTimestampMs = 0,
        [AllowEmptyString()]
        [string]$LifecycleScenarioContent = "",
        [string]$ScreenshotPath = "",
        [Int64]$RunStartTimestampMs = 0,
        [ValidateRange(60, 86400)]
        [int]$MinimumStabilitySeconds = 60
    )

    $identity = Get-QaKWinSurfaceIdentity -HostLogContent $HostLogContent -RunStartTimestampMs $RunStartTimestampMs
    $output = Get-QaOutputState `
        -HostLogContent $HostLogContent `
        -RunStartTimestampMs $RunStartTimestampMs `
        -KWinIdentity $identity
    $presentation = Test-QaHostPresentation -HostLogContent $HostLogContent -RunStartTimestampMs $RunStartTimestampMs -KWinIdentity $identity
    $lifecycle = Test-QaKWinPlasmaLifecycle `
        -HostLogContent $HostLogContent `
        -GuestLogContent $GuestLogContent `
        -ProcessSnapshotContent $ProcessSnapshotContent `
        -ProcessSnapshotTimestampMs $ProcessSnapshotTimestampMs `
        -RunStartTimestampMs $RunStartTimestampMs `
        -KWinIdentity $identity
    $screenshot = Test-ScreenshotNonBlack -ScreenshotPath $ScreenshotPath
    $scenarios = Test-QaLifecycleScenarios `
        -Content $LifecycleScenarioContent `
        -RunStartTimestampMs $RunStartTimestampMs
    $stability = Test-QaStability `
        -HostLogContent $HostLogContent `
        -GuestLogContent $GuestLogContent `
        -RunStartTimestampMs $RunStartTimestampMs `
        -PresentationTimestampMs $presentation.TimestampMs `
        -Generation $identity.Generation `
        -MinimumStabilitySeconds $MinimumStabilitySeconds

    # Background/resume and force-close/reopen are separate two-session
    # scenarios. They are not implied by startup health and remain false when
    # no verified collector evidence is supplied.
    $criteria = [ordered]@{
        CurrentRunKWinSurface = [bool]$identity.Passed
        OutputEnabled         = [bool]$output.Known -and [bool]$output.Enabled
        HostPresented         = [bool]$presentation.HostPresented
        KWinAlive             = $lifecycle.KWinAlive -eq $true
        PlasmaAlive           = $lifecycle.PlasmaAlive -eq $true
        ScreenshotNonBlack    = [bool]$screenshot.Passed
        Stability             = [bool]$stability.Passed
        BackgroundForeground  = [bool]$scenarios.BackgroundForeground
        ForceCloseReopen      = [bool]$scenarios.ForceCloseReopen
    }
    $missing = @($criteria.GetEnumerator() | Where-Object { -not $_.Value } | ForEach-Object { $_.Key })
    $passed = ($missing.Count -eq 0)

    $message = if ($passed) {
        "Strict real-ARM64 Plasma release evidence passed."
    } elseif ($missing.Count -gt 0) {
        "Release evidence incomplete; failed criteria: $($missing -join ', ')."
    } else {
        "Release evidence is unknown; failing closed."
    }

    return [PSCustomObject]@{
        Gate                    = "Strict ARM64 Plasma Release Evidence"
        Passed                  = $passed
        CurrentRunKWinSurface   = [bool]$identity.Passed
        Generation              = $identity.Generation
        SurfaceId               = $identity.SurfaceId
        OutputKnown             = [bool]$output.Known
        OutputEnabled           = [bool]$output.Enabled
        HostPresented           = [bool]$presentation.HostPresented
        KWinAlive               = $lifecycle.KWinAlive
        PlasmaAlive             = $lifecycle.PlasmaAlive
        ScreenshotNonBlack      = [bool]$screenshot.Passed
        StabilityPassed         = [bool]$stability.Passed
        StabilitySeconds        = $stability.StableForSeconds
        RequiredStabilitySeconds = $MinimumStabilitySeconds
        BackgroundForeground    = [bool]$scenarios.BackgroundForeground
        ForceCloseReopen        = [bool]$scenarios.ForceCloseReopen
        MissingCriteria         = $missing
        Identity                = $identity
        Output                  = $output
        Presentation            = $presentation
        Lifecycle               = $lifecycle
        Screenshot              = $screenshot
        Stability               = $stability
        LifecycleScenarios      = $scenarios
        Message                 = $message
    }
}

# ---------------------------------------------------------------------------
# Release Gate 2: KWin Connection Verification
# ---------------------------------------------------------------------------

function Test-KWinConnection {
    <#
    .SYNOPSIS
        Verifies the current-run KWin client and nested Wayland output identity.

    .DESCRIPTION
        This is a partial diagnostic check, not a release claim.  It accepts
        only a timestamped host-log identity event from the current run.  A
        logcat identity, a guest title, or a running process name cannot prove
        which surface supplied the release evidence.
    #>
    [CmdletBinding()]
    param(
        [string]$DeviceId = "f105b146",
        [string]$HostLogContent = "",
        [string]$LogcatContent = "",
        [Int64]$RunStartTimestampMs = 0
    )

    $identity = Get-QaKWinSurfaceIdentity `
        -HostLogContent $HostLogContent `
        -RunStartTimestampMs $RunStartTimestampMs
    return [PSCustomObject]@{
        Gate       = "Current-run KWin Wayland Connection"
        Passed     = [bool]$identity.Passed
        Generation = $identity.Generation
        ClientId   = $identity.ClientId
        SurfaceId  = $identity.SurfaceId
        Title      = $identity.Title
        Message    = $identity.Message
    }
}

# ---------------------------------------------------------------------------
# Release Gate 3: Wayland Readiness Marker Verification
# ---------------------------------------------------------------------------

function Test-WaylandReadinessMarker {
    <#
    .SYNOPSIS
        Verifies current-run host presentation evidence.

    .DESCRIPTION
        The guest plasma-ready marker is diagnostic context only.  It is never
        sufficient for this gate because it may be stale and does not by itself
        prove that the Android host presented the identified KWin surface.
    #>
    [CmdletBinding()]
    param(
        [string]$DeviceId = "f105b146",
        [string]$HostLogContent = "",
        [Int64]$RunStartTimestampMs = 0,
        [PSCustomObject]$KWinIdentity = $null
    )

    $result = [PSCustomObject]@{
        Gate          = "Current-run Host Presentation"
        Passed        = $false
        MarkerExists  = $false
        MarkerContent = "Marker is not release evidence."
        HostPresented = $false
        Generation    = $null
        SurfaceId     = $null
        Evidence      = ""
        Message       = ""
    }

    $presentation = Test-QaHostPresentation `
        -HostLogContent $HostLogContent `
        -RunStartTimestampMs $RunStartTimestampMs `
        -KWinIdentity $KWinIdentity
    $result.Passed = [bool]$presentation.Passed
    $result.HostPresented = [bool]$presentation.HostPresented
    $result.Generation = $presentation.Generation
    $result.SurfaceId = $presentation.SurfaceId
    $result.Evidence = $presentation.Evidence
    $result.Message = $presentation.Message

    return $result
}

# ---------------------------------------------------------------------------
# Crash & Error Diagnostics Verification
# ---------------------------------------------------------------------------

function Test-AppCrashes {
    <#
    .SYNOPSIS
        Checks for process crashes, SIGSEGV, SIGABRT, unhandled exceptions,
        and failure markers in host and guest states.
    #>
    [CmdletBinding()]
    param(
        [string]$DeviceId = "f105b146",
        [string]$HostLogContent = "",
        [string]$GuestLogContent = "",
        [string]$LogcatContent = ""
    )

    $result = [PSCustomObject]@{
        Gate          = "Crash and Error Detection"
        Passed        = $true
        CrashFound    = $false
        FailureReason = ""
        Backtrace     = ""
        Details       = @()
    }

    # 1. Check for failure markers in guest state directory only when the
    # caller did not supply cached diagnostics. Parser/regression callers must
    # never cause an implicit ADB query; a missing cache is unknown, not a
    # license to inspect a different device.
    $hasCachedDiagnostics = (-not [string]::IsNullOrWhiteSpace($HostLogContent)) -or
        (-not [string]::IsNullOrWhiteSpace($GuestLogContent)) -or
        (-not [string]::IsNullOrWhiteSpace($LogcatContent))
    if (-not $hasCachedDiagnostics) {
        try {
            $failCheck = & adb -s $DeviceId shell "run-as app.polarbear cat /data/data/app.polarbear/files/arch/var/lib/localdesktop/plasma-failed 2>/dev/null" 2>&1
            $failText = ($failCheck | Out-String).Trim()
            if ($failText -and $failText -notmatch "No such file") {
                $result.CrashFound = $true
                $result.FailureReason = "plasma-failed: $failText"
                $result.Details += "Guest recorded plasma-failed marker: $failText"
            }

            $crashCheck = & adb -s $DeviceId shell "run-as app.polarbear cat /data/data/app.polarbear/files/arch/var/lib/localdesktop/kwin-crash 2>/dev/null" 2>&1
            $crashText = ($crashCheck | Out-String).Trim()
            if ($crashText -and $crashText -notmatch "No such file") {
                $result.CrashFound = $true
                $result.FailureReason = "kwin-crash: $crashText"
                $result.Details += "Guest recorded kwin-crash marker: $crashText"
            }

            $btCheck = & adb -s $DeviceId shell "run-as app.polarbear grep -iE 'fault_address|signal=11' /data/data/app.polarbear/files/arch/var/lib/localdesktop/kwin-backtrace.log 2>/dev/null | head -n 5" 2>&1
            $btText = ($btCheck | Out-String).Trim()
            if ($btText -and $btText -notmatch "No such file") {
                $result.Backtrace = $btText
                $result.CrashFound = $true
                $result.Details += "KWin crash backtrace recorded: $btText"
            }
        } catch {
            Write-Verbose "Failed checking guest failure markers: $_"
        }
    }

    # 2. Check host log for crashes / fatal errors
    if ($HostLogContent) {
        if ($HostLogContent -match '(?:desktop-failure|panic|FATAL|setup-failed)\s+([^\r\n]+)') {
            $result.Details += "Host log error marker: $($Matches[0])"
        }
    }

    # 3. Check logcat for crashes
    if ($LogcatContent) {
        if ($LogcatContent -match 'Fatal signal (?:11|6) \((?:SIGSEGV|SIGABRT)\).*app\.polarbear|FATAL EXCEPTION.*app\.polarbear|Process app\.polarbear.*died') {
            $result.Details += "Logcat crash signature detected: $($Matches[0])"
        }
    }

    if ($result.CrashFound -or $result.Details.Count -gt 0) {
        $result.Passed = $false
        if (-not $result.FailureReason) {
            $result.FailureReason = ($result.Details -join "; ")
        }
    }

    return $result
}

# ---------------------------------------------------------------------------
# Release Gates Assertions Summary
# ---------------------------------------------------------------------------

function Assert-ReleaseGates {
    <#
    .SYNOPSIS
        Evaluates all release gates and displays a structured summary.
    #>
    [CmdletBinding()]
    param(
        [string]$DeviceId = "f105b146",
        [int]$ExpectedWidth = 3392,
        [int]$ExpectedHeight = 2400,
        [string]$ScreenshotPath = "",
        [string]$HostLogContent = "",
        [string]$GuestLogContent = "",
        [string]$LogcatContent = "",
        [string]$ProcessSnapshotContent = "",
        [Int64]$ProcessSnapshotTimestampMs = 0,
        [string]$LifecycleScenarioContent = "",
        [Int64]$RunStartTimestampMs = 0,
        [ValidateRange(60, 86400)]
        [int]$MinimumStabilitySeconds = 60,
        [switch]$ThrowOnFailure
    )

    Write-Host "`n========================================================" -ForegroundColor Magenta
    Write-Host "       RELEASE / PARTIAL GATES VERIFICATION             " -ForegroundColor Magenta
    Write-Host "========================================================" -ForegroundColor Magenta

    $gateResolution = Test-ScreenResolution -DeviceId $DeviceId -ExpectedWidth $ExpectedWidth -ExpectedHeight $ExpectedHeight -ScreenshotPath $ScreenshotPath
    $gateKWin = Test-KWinConnection `
        -DeviceId $DeviceId `
        -HostLogContent $HostLogContent `
        -LogcatContent $LogcatContent `
        -RunStartTimestampMs $RunStartTimestampMs
    $gateWayland = Test-WaylandReadinessMarker `
        -DeviceId $DeviceId `
        -HostLogContent $HostLogContent `
        -RunStartTimestampMs $RunStartTimestampMs `
        -KWinIdentity ([PSCustomObject]@{
            Passed = $gateKWin.Passed
            Generation = $gateKWin.Generation
            SurfaceId = $gateKWin.SurfaceId
        })
    $gateCrashes = Test-AppCrashes -DeviceId $DeviceId -HostLogContent $HostLogContent -GuestLogContent $GuestLogContent -LogcatContent $LogcatContent
    $gateRelease = Test-StrictPlasmaReleaseEvidence `
        -HostLogContent $HostLogContent `
        -GuestLogContent $GuestLogContent `
        -ProcessSnapshotContent $ProcessSnapshotContent `
        -ProcessSnapshotTimestampMs $ProcessSnapshotTimestampMs `
        -LifecycleScenarioContent $LifecycleScenarioContent `
        -ScreenshotPath $ScreenshotPath `
        -RunStartTimestampMs $RunStartTimestampMs `
        -MinimumStabilitySeconds $MinimumStabilitySeconds

    $allGates = @($gateResolution, $gateKWin, $gateWayland, $gateCrashes, $gateRelease)

    foreach ($gate in $allGates) {
        $statusColor = if ($gate.Passed) { "Green" } else { "Red" }
        $statusLabel = if ($gate.Passed) { "[PASS]" } else { "[FAIL]" }
        Write-Host "$statusLabel " -NoNewline -ForegroundColor $statusColor
        Write-Host "$($gate.Gate): " -NoNewline -ForegroundColor White
        if ($gate.Message) {
            Write-Host $gate.Message -ForegroundColor Gray
        } elseif ($gate.FailureReason) {
            Write-Host $gate.FailureReason -ForegroundColor Red
        } else {
            Write-Host "Verified." -ForegroundColor Gray
        }
    }

    Write-Host "========================================================`n" -ForegroundColor Magenta

    # Partial checks are deliberately reported separately.  They are useful
    # for diagnosing an ARM64 run, but can never make AllPassed/ReleasePassed
    # true without the strict evidence gate.
    $partialPassed = ($gateResolution.Passed -and $gateKWin.Passed -and $gateCrashes.Passed)
    $releasePassed = ($partialPassed -and $gateWayland.Passed -and $gateRelease.Passed)
    if ($ThrowOnFailure -and -not $releasePassed) {
        throw "One or more release gates failed verification."
    }

    return [PSCustomObject]@{
        ValidationMode  = "Release"
        AllPassed       = $releasePassed
        ReleasePassed   = $releasePassed
        PartialPassed   = $partialPassed
        GateResolution  = $gateResolution
        GateKWin        = $gateKWin
        GateWayland     = $gateWayland
        GateCrashes     = $gateCrashes
        GateRelease     = $gateRelease
    }
}

# ---------------------------------------------------------------------------
# Core QA Pipeline Execution Functions
# ---------------------------------------------------------------------------

function Test-DeviceConnection {
    <#
    .SYNOPSIS
        Checks whether the specified target device is connected via ADB.
    #>
    [CmdletBinding()]
    param([string]$DeviceId = "f105b146")

    Write-Host "[1/7] Checking device connection (adb devices)..." -ForegroundColor Cyan
    $adbDevices = & adb devices 2>&1
    Write-Host ($adbDevices | Out-String).Trim() -ForegroundColor Gray

    $matched = $false
    foreach ($line in $adbDevices) {
        if ($line -match "^$DeviceId\s+device\b") {
            $matched = $true
            break
        }
    }

    if ($matched) {
        Write-Host "  -> OnePlus Pad 3 ($DeviceId) connected and ready." -ForegroundColor Green
        return $true
    } else {
        Write-Host "  -> Target device ($DeviceId) not connected or unauthorized." -ForegroundColor Red
        return $false
    }
}

function Build-AppApk {
    <#
    .SYNOPSIS
        Builds APK using xbuild toolchain and ensures signing compatibility.
    #>
    [CmdletBinding()]
    param(
        [string]$XbuildPath,
        [string]$RepoRoot
    )

    Write-Host "[2/7] Building APK with xbuild..." -ForegroundColor Cyan
    Write-Host "  -> Command: $XbuildPath build --debug --platform android --arch arm64 --format apk" -ForegroundColor Gray
    $buildStartedUtc = (Get-Date).ToUniversalTime()

    Push-Location $RepoRoot
    try {
        & $XbuildPath build --debug --platform android --arch arm64 --format apk |
            ForEach-Object { Write-Host $_ }
        if ($LASTEXITCODE -ne 0) {
            throw "xbuild execution failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }

    # xbuild copies the debug Gradle output to this deterministic path. A
    # release APK or an arbitrary stale candidate must never be selected.
    $candidatePaths = @(
        (Join-Path $RepoRoot "target\x\debug\android\localdesktop.apk")
    )

    $builtApk = $null
    foreach ($cand in $candidatePaths) {
        if (Test-Path -LiteralPath $cand -PathType Leaf) {
            $candidateInfo = Get-Item -LiteralPath $cand
            if ($candidateInfo.LastWriteTimeUtc -lt $buildStartedUtc.AddSeconds(-2)) {
                continue
            }
            $builtApk = (Resolve-Path $cand).Path
            break
        }
    }

    if (-not $builtApk) {
        throw "APK build completed but no current-run debug APK was produced; refusing stale or release candidates."
    }

    Write-Host "  -> Built APK: $builtApk ($( [math]::Round((Get-Item $builtApk).Length / 1MB, 2) ) MB)" -ForegroundColor Green

    # Ensure APK is signed with matching device key (strictly preserves app data on update)
    $signingDir = Join-Path $RepoRoot ".release-signing"
    $keystore = Join-Path $signingDir "localdesktop-plasma-release.jks"
    $credsEnv = Join-Path $signingDir "release-credentials.env"

    $hasKeystore = Test-Path -LiteralPath $keystore -PathType Leaf
    $hasCredentials = Test-Path -LiteralPath $credsEnv -PathType Leaf
    if ($hasKeystore -xor $hasCredentials) {
        throw "Signing configuration is incomplete; both keystore and release-credentials.env are required."
    }
    if ($hasKeystore -and $hasCredentials) {
        Write-Host "  -> Signing APK with release certificate to ensure update compatibility..." -ForegroundColor Cyan
        $creds = Get-Content -Raw $credsEnv | ConvertFrom-StringData
        $storePass = $creds["KEYSTORE_PASSWORD"]
        $keyPass = $creds["KEY_PASSWORD"]
        $keyAlias = $creds["KEY_ALIAS"]

        if ([string]::IsNullOrWhiteSpace($storePass) -or [string]::IsNullOrWhiteSpace($keyPass) -or [string]::IsNullOrWhiteSpace($keyAlias)) {
            throw "Signing credentials are incomplete."
        }

        $apksigner = Get-ChildItem -Path (Join-Path $env:ANDROID_HOME "build-tools") -Recurse -Filter "apksigner.bat" -ErrorAction SilentlyContinue |
            Select-Object -First 1

        if (-not $apksigner) {
            throw "apksigner.bat was not found under ANDROID_HOME/build-tools."
        }

        # apksigner supports env: sources; secrets must not appear in argv or
        # process listings. Always verify both command exit codes.
        $storeEnvName = "LOCALDESKTOP_QA_KEYSTORE_PASSWORD"
        $keyEnvName = "LOCALDESKTOP_QA_KEY_PASSWORD"
        try {
            Set-Item -Path "Env:$storeEnvName" -Value $storePass
            Set-Item -Path "Env:$keyEnvName" -Value $keyPass
            & $apksigner.FullName sign --ks $keystore --ks-pass "env:$storeEnvName" --ks-key-alias $keyAlias --key-pass "env:$keyEnvName" $builtApk |
                ForEach-Object { Write-Host $_ }
            if ($LASTEXITCODE -ne 0) {
                throw "apksigner sign failed with exit code $LASTEXITCODE"
            }
            & $apksigner.FullName verify --verbose --print-certs $builtApk |
                ForEach-Object { Write-Host $_ }
            if ($LASTEXITCODE -ne 0) {
                throw "apksigner verify failed with exit code $LASTEXITCODE"
            }
            Write-Host "  -> APK signed and verified successfully with certificate for $keyAlias." -ForegroundColor Green
        } finally {
            Remove-Item -Path "Env:$storeEnvName" -ErrorAction SilentlyContinue
            Remove-Item -Path "Env:$keyEnvName" -ErrorAction SilentlyContinue
        }
    }

    return $builtApk
}

function Install-AppApk {
    <#
    .SYNOPSIS
        Installs APK on device preserving existing app and guest data (-r -t).
    #>
    [CmdletBinding()]
    param(
        [string]$DeviceId,
        [string]$ApkPath
    )

    Write-Host "[3/7] Installing APK (strictly preserving application data)..." -ForegroundColor Cyan
    Write-Host "  -> adb -s $DeviceId install -r -t `"$ApkPath`"" -ForegroundColor Gray

    $installOut = & adb -s $DeviceId install -r -t $ApkPath 2>&1
    $installText = ($installOut | Out-String).Trim()
    Write-Host "  $installText" -ForegroundColor Gray

    if ($installText -notmatch "Success") {
        throw "Failed to install APK: $installText"
    }

    Write-Host "  -> Installation succeeded without modifying app data." -ForegroundColor Green
}

function Start-AppActivity {
    <#
    .SYNOPSIS
        Starts NativeActivity via adb shell am start.
    #>
    [CmdletBinding()]
    param(
        [string]$DeviceId,
        [string]$PackageName,
        [string]$ActivityName
    )

    Write-Host "[4/7] Launching activity: adb shell am start -n $PackageName/$ActivityName..." -ForegroundColor Cyan
    $launchOut = & adb -s $DeviceId shell am start -n "$PackageName/$ActivityName" 2>&1
    Write-Host "  $($launchOut | Out-String).Trim()" -ForegroundColor Gray

    Start-Sleep -Seconds 2
    $pidOut = & adb -s $DeviceId shell pidof $PackageName 2>&1
    $currentPid = ($pidOut | Out-String).Trim()
    if ($currentPid) {
        Write-Host "  -> $PackageName running with PID: $currentPid" -ForegroundColor Green
    } else {
        Write-Warning "Process $PackageName not immediately found in pidof (may still be launching)."
    }
}

function Get-AppUid {
    [CmdletBinding()]
    param(
        [string]$DeviceId,
        [string]$PackageName
    )

    $uidOutput = & adb -s $DeviceId shell cmd package list packages -U $PackageName 2>&1
    if ($LASTEXITCODE -ne 0) {
        return $null
    }
    $uidText = ($uidOutput | Out-String).Trim()
    if ($uidText -match "(?i)package:$([Regex]::Escape($PackageName))\s+uid:(\d+)") {
        return [Int64]$Matches[1]
    }
    return $null
}

function Get-AppDiagnostics {
    <#
    .SYNOPSIS
        Monitors startup and pulls host.log, guest.log, and logcat into artifacts/qa.
    #>
    [CmdletBinding()]
    param(
        [string]$DeviceId,
        [string]$PackageName,
        [string]$ArtifactDir,
        [int]$TimeoutSeconds
    )

    Write-Host "[5/7] Waiting for startup and collecting diagnostics..." -ForegroundColor Cyan

    $hostLogFile = Join-Path $ArtifactDir "host.log"
    $guestLogFile = Join-Path $ArtifactDir "guest.log"
    $logcatFile = Join-Path $ArtifactDir "logcat.log"

    $elapsed = 0
    $interval = 3
    $hostLogContent = ""
    $guestLogContent = ""
    $logcatContent = ""
    $appUid = $null

    while ($elapsed -lt $TimeoutSeconds) {
        Start-Sleep -Seconds $interval
        $elapsed += $interval
        Write-Host "  -> Waiting for session stabilization ($elapsed / $TimeoutSeconds s)..." -ForegroundColor Gray

        # Markers are diagnostic context only. Continue through the full wait
        # so a release decision cannot stop before its stability interval.
        $markerCheck = & adb -s $DeviceId shell "run-as $PackageName ls /data/data/$PackageName/files/arch/var/lib/localdesktop/ 2>/dev/null" 2>&1
        $markerText = ($markerCheck | Out-String)
        if ($markerText -match "plasma-ready|plasma-failed|kwin-crash") {
            Write-Host "  -> Desktop state marker observed for diagnostics: $($Matches[0])" -ForegroundColor Yellow
        }
    }

    # Extract host.log via app-scoped run-as (including rotated host.log.1)
    try {
        $hostLogContent = & adb -s $DeviceId shell "run-as $PackageName sh -c 'cat /data/data/$PackageName/files/diagnostics/host.log.1 /data/data/$PackageName/files/diagnostics/host.log 2>/dev/null || cat /data/data/$PackageName/files/diagnostics/host.log 2>/dev/null'" 2>&1 | Out-String
        if ($hostLogContent) {
            Set-Content -Path $hostLogFile -Value $hostLogContent -Encoding utf8
            Write-Host "  -> Saved host.log to $hostLogFile ($( [math]::Round((Get-Item $hostLogFile).Length / 1KB, 1) ) KB)" -ForegroundColor Green
        }
    } catch {
        Write-Warning "Could not retrieve host.log: $_"
    }

    # Extract guest.log via app-scoped run-as
    try {
        $guestLogContent = & adb -s $DeviceId shell "run-as $PackageName cat /data/data/$PackageName/files/diagnostics/guest.log 2>/dev/null" 2>&1 | Out-String
        if ($guestLogContent) {
            Set-Content -Path $guestLogFile -Value $guestLogContent -Encoding utf8
            Write-Host "  -> Saved guest.log to $guestLogFile ($( [math]::Round((Get-Item $guestLogFile).Length / 1KB, 1) ) KB)" -ForegroundColor Green
        }
    } catch {
        Write-Warning "Could not retrieve guest.log: $_"
    }

    # Dump only this package UID's logcat. A missing UID is unknown; never
    # substitute a global device log that could contain unrelated processes.
    try {
        $appUid = Get-AppUid -DeviceId $DeviceId -PackageName $PackageName
        if ($null -eq $appUid) {
            Write-Warning "Could not resolve UID for $PackageName; refusing a global logcat fallback."
        } else {
            $logcatContent = & adb -s $DeviceId logcat "--uid=$appUid" -d 2>&1 | Out-String
            if ($LASTEXITCODE -ne 0) {
                Write-Warning "UID-scoped logcat retrieval failed with exit code $LASTEXITCODE."
                $logcatContent = ""
            } elseif ($logcatContent) {
                Set-Content -Path $logcatFile -Value $logcatContent -Encoding utf8
                Write-Host "  -> Saved UID $appUid logcat to $logcatFile ($( [math]::Round((Get-Item $logcatFile).Length / 1KB, 1) ) KB)" -ForegroundColor Green
            }
        }
    } catch {
        Write-Warning "Could not retrieve UID-scoped logcat: $_"
    }

    return [PSCustomObject]@{
        HostLog  = $hostLogContent
        GuestLog = $guestLogContent
        Logcat   = $logcatContent
        AppUid   = $appUid
    }
}

function Capture-DeviceScreenshot {
    <#
    .SYNOPSIS
        Captures screenshot safely to artifacts/qa/pad3-screenshot.png without encoding corruption.
    #>
    [CmdletBinding()]
    param(
        [string]$DeviceId,
        [string]$ArtifactDir
    )

    Write-Host "[6/7] Capturing screenshot to artifacts/qa/pad3-screenshot.png..." -ForegroundColor Cyan

    $localScreenshot = Join-Path $ArtifactDir "pad3-screenshot.png"
    $remoteTemp = "/data/local/tmp/pad3-screenshot.png"

    # Capture on device, pull binary cleanly, then clean up temp file
    & adb -s $DeviceId shell screencap -p $remoteTemp
    cmd /c "adb -s $DeviceId pull `"$remoteTemp`" `"$localScreenshot`" >nul 2>&1"
    & adb -s $DeviceId shell rm -f $remoteTemp

    if (Test-Path $localScreenshot) {
        $fileInfo = Get-Item $localScreenshot
        Write-Host "  -> Screenshot captured: $localScreenshot ($( [math]::Round($fileInfo.Length / 1MB, 2) ) MB)" -ForegroundColor Green
        return $localScreenshot
    } else {
        Write-Warning "Screenshot could not be pulled from device."
        return $null
    }
}

# ---------------------------------------------------------------------------
# Main Orchestrator
# ---------------------------------------------------------------------------

function Invoke-QaLoop {
    [CmdletBinding()]
    param(
        [string]$DeviceId = "f105b146",
        [string]$PackageName = "app.polarbear",
        [string]$ActivityName = "android.app.NativeActivity",
        [int]$ExpectedWidth = 3392,
        [int]$ExpectedHeight = 2400,
        [string]$ArtifactDir = "",
        [string]$ApkPath = "",
        [string]$LifecycleScenarioContent = "",
        [switch]$SkipBuild,
        [switch]$SkipInstall,
        [switch]$SkipLaunch,
        [ValidateRange(60, 86400)]
        [int]$TimeoutSeconds = 120,
        [ValidateRange(60, 86400)]
        [int]$StabilitySeconds = 60,
        [switch]$EnforceReleaseGates
    )

    if ([string]::IsNullOrWhiteSpace($ArtifactDir)) {
        $runLabel = (Get-Date).ToUniversalTime().ToString("yyyyMMdd-HHmmssfff") + "-" + ([Guid]::NewGuid().ToString("N").Substring(0, 8))
        $ArtifactDir = Join-Path $RepoRoot ("artifacts\qa\run-" + $runLabel)
    }

    Write-Host "`n========================================================" -ForegroundColor Cyan
    Write-Host "      OnePlus Pad 3 Continuous QA Validation Loop       " -ForegroundColor Cyan
    Write-Host "========================================================" -ForegroundColor Cyan
    Write-Host "Device ID : $DeviceId" -ForegroundColor Gray
    Write-Host "Target    : $PackageName/$ActivityName" -ForegroundColor Gray
    Write-Host "Artifacts : $ArtifactDir" -ForegroundColor Gray
    Write-Host "Timestamp : $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')`n" -ForegroundColor Gray

    # Ensure artifact directory exists
    if (-not (Test-Path $ArtifactDir)) {
        New-Item -ItemType Directory -Force -Path $ArtifactDir | Out-Null
    }

    # Step 1: Check device connection
    if (-not (Test-DeviceConnection -DeviceId $DeviceId)) {
        throw "Device $DeviceId is not available via ADB. Aborting validation loop."
    }

    # Step 2: Initialize toolchain & Build APK
    $xbuildPath = Initialize-BuildEnvironment
    if (-not $SkipBuild) {
        $ApkPath = Build-AppApk -XbuildPath $xbuildPath -RepoRoot $RepoRoot
    } elseif (-not $ApkPath) {
        throw "-SkipBuild requires an explicit -ApkPath; refusing to select a stale APK candidate."
    }

    # Step 3: Install APK (strictly preserving data)
    if (-not $SkipInstall -and $ApkPath -and (Test-Path $ApkPath)) {
        Install-AppApk -DeviceId $DeviceId -ApkPath $ApkPath
    }

    # Establish the current-run boundary immediately before launch. Evidence
    # from an earlier append-only diagnostics file cannot satisfy this run.
    $runStartTimestampMs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    Write-Host "Run boundary: $runStartTimestampMs" -ForegroundColor Gray

    # Step 4: Launch Activity
    if (-not $SkipLaunch) {
        Start-AppActivity -DeviceId $DeviceId -PackageName $PackageName -ActivityName $ActivityName
    }

    # Step 5: Wait for startup & pull diagnostics
    $diag = Get-AppDiagnostics -DeviceId $DeviceId -PackageName $PackageName -ArtifactDir $ArtifactDir -TimeoutSeconds $TimeoutSeconds

    # Step 6: Capture screenshot
    $screenshot = Capture-DeviceScreenshot -DeviceId $DeviceId -ArtifactDir $ArtifactDir

    # Step 7 & Release Gates: Evaluate gates
    Write-Host "[7/7] Evaluating release gates and crash markers..." -ForegroundColor Cyan
    $gatesReport = Assert-ReleaseGates `
        -DeviceId $DeviceId `
        -ExpectedWidth $ExpectedWidth `
        -ExpectedHeight $ExpectedHeight `
        -ScreenshotPath $screenshot `
        -HostLogContent $diag.HostLog `
        -GuestLogContent $diag.GuestLog `
        -LogcatContent $diag.Logcat `
        -LifecycleScenarioContent $LifecycleScenarioContent `
        -RunStartTimestampMs $runStartTimestampMs `
        -MinimumStabilitySeconds $StabilitySeconds

    # Save summary report JSON
    $summaryFile = Join-Path $ArtifactDir "qa-summary.json"
    $gatesReport | ConvertTo-Json -Depth 4 | Set-Content -Path $summaryFile -Encoding utf8
    Write-Host "Saved validation summary to $summaryFile" -ForegroundColor Gray

    if ($EnforceReleaseGates -and -not $gatesReport.ReleasePassed) {
        throw "One or more release gates failed verification; see $summaryFile."
    }

    Write-Host "QA Validation Loop completed.`n" -ForegroundColor Green
    return $gatesReport
}

# ---------------------------------------------------------------------------
# Entry Point
# ---------------------------------------------------------------------------

if (-not $FunctionsOnly) {
    Invoke-QaLoop `
        -DeviceId $DeviceId `
        -PackageName $PackageName `
        -ActivityName $ActivityName `
        -ExpectedWidth $ExpectedWidth `
        -ExpectedHeight $ExpectedHeight `
        -ArtifactDir $ArtifactDir `
        -ApkPath $ApkPath `
        -SkipBuild:$SkipBuild `
        -SkipInstall:$SkipInstall `
        -SkipLaunch:$SkipLaunch `
        -TimeoutSeconds $TimeoutSeconds `
        -StabilitySeconds $StabilitySeconds `
        -LifecycleScenarioContent $LifecycleScenarioContent `
        -EnforceReleaseGates:$EnforceReleaseGates
}
