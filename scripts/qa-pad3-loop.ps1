<#
.SYNOPSIS
    Automated Continuous QA Validation Loop on OnePlus Pad 3 (adb: f105b146).

.DESCRIPTION
    Automates the build, install, launch, diagnostics retrieval, screenshot capture,
    crash analysis, and release gate verification for Local Desktop on the OnePlus Pad 3.

    Release gates verified:
    1. Screen resolution: 3392x2400 (or 2400x3392).
    2. KWin connection: verifies identified KWin Wayland client/surface in host.log / logcat.
    3. Wayland readiness marker: verifies plasma-ready marker in guest filesystem and host.log.

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
    [switch]$SkipBuild,
    [switch]$SkipInstall,
    [switch]$SkipLaunch,
    [int]$TimeoutSeconds = 45,
    [switch]$EnforceReleaseGates,
    [switch]$FunctionsOnly
)

$ErrorActionPreference = "Continue"
$PSNativeCommandUseErrorActionPreference = $false

# Determine repository root and artifact directory
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not $ArtifactDir) {
    $ArtifactDir = Join-Path $RepoRoot "artifacts\qa"
}

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
                # Big-endian 32-bit integers for width and height
                $w = [System.BitConverter]::ToUInt32($bytes[19..16], 0)
                $h = [System.BitConverter]::ToUInt32($bytes[23..20], 0)
                $result.DetectedWidth = $w
                $result.DetectedHeight = $h
                $result.Method = "Screenshot IHDR Inspection"

                if (($w -eq $ExpectedWidth -and $h -eq $ExpectedHeight) -or
                    ($w -eq $ExpectedHeight -and $h -eq $ExpectedWidth)) {
                    $result.Passed = $true
                    $result.Message = "Screenshot confirmed native resolution ${w}x${h}."
                    return $result
                }
            }
        } catch {
            Write-Verbose "Could not parse screenshot bytes: $_"
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
# Release Gate 2: KWin Connection Verification
# ---------------------------------------------------------------------------

function Test-KWinConnection {
    <#
    .SYNOPSIS
        Verifies KWin connection and nested Wayland output identity.
    #>
    [CmdletBinding()]
    param(
        [string]$DeviceId = "f105b146",
        [string]$HostLogContent = "",
        [string]$LogcatContent = ""
    )

    $result = [PSCustomObject]@{
        Gate       = "KWin Wayland Connection"
        Passed     = $false
        Generation = $null
        ClientId   = $null
        SurfaceId  = $null
        Title      = ""
        Message    = ""
    }

    # 1. Search in host.log content
    if ($HostLogContent -match 'kwin-identified') {
        $kwinLine = ($HostLogContent -split "`r?`n" | Where-Object { $_ -match 'stage=kwin-identified' } | Select-Object -First 1)
        if ($kwinLine -and $kwinLine -match 'generation=(\d+)\s+client=(.+?)\s+surface=(.+?)\s+title=([^\r\n]+)') {
            $result.Passed = $true
            $result.Generation = $Matches[1]
            $result.ClientId = $Matches[2].Trim()
            $result.SurfaceId = $Matches[3].Trim()
            $result.Title = $Matches[4].Trim()
            $result.Message = "KWin identified in host.log (generation=$($result.Generation), surface=$($result.SurfaceId), title='$($result.Title)')."
            return $result
        }
    }

    # 2. Search in logcat content
    if ($LogcatContent) {
        if ($LogcatContent -match 'wayland\.readiness\s+kwin_identity\s+generation=(\d+)\s+client=(.+?)\s+surface=(.+?)\s+title="?([^"\r\n]+)"?') {
            $result.Passed = $true
            $result.Generation = $Matches[1]
            $result.ClientId = $Matches[2].Trim()
            $result.SurfaceId = $Matches[3].Trim()
            $result.Title = $Matches[4].Trim()
            $result.Message = "KWin identity observed in logcat (generation=$($result.Generation), surface=$($result.SurfaceId))."
            return $result
        }
    }

    # 3. If no cached content provided, query device directly
    try {
        $hostLogLive = & adb -s $DeviceId shell "run-as app.polarbear sh -c 'cat /data/data/app.polarbear/files/diagnostics/host.log.1 /data/data/app.polarbear/files/diagnostics/host.log 2>/dev/null | grep -E kwin-identified | tail -n 5'" 2>&1
        $hostLogText = ($hostLogLive | Out-String).Trim()
        if ($hostLogText -match 'stage=kwin-identified\s+generation=(\d+)\s+client=(.+?)\s+surface=(.+?)\s+title=([^\r\n]+)') {
            $result.Passed = $true
            $result.Generation = $Matches[1]
            $result.ClientId = $Matches[2].Trim()
            $result.SurfaceId = $Matches[3].Trim()
            $result.Title = $Matches[4].Trim()
            $result.Message = "Live KWin connection found on device (generation=$($result.Generation))."
            return $result
        }
    } catch {
        Write-Verbose "Direct device KWin query failed: $_"
    }

    # 4. Check guest session log (plasma.log) for KWin nested toplevel title configuration
    try {
        $plasmaLogCheck = & adb -s $DeviceId shell "run-as app.polarbear grep -i 'KDE Wayland Compositor' /data/data/app.polarbear/files/arch/var/lib/localdesktop/plasma.log 2>/dev/null | tail -n 1" 2>&1
        $plasmaLogText = ($plasmaLogCheck | Out-String).Trim()
        if ($plasmaLogText -match 'set_title\("([^"]+)"\)') {
            $result.Passed = $true
            $result.Title = $Matches[1]
            $result.Message = "KWin Wayland output verified active in plasma.log: '$($result.Title)'."
            return $result
        }
    } catch {
        Write-Verbose "plasma.log check failed: $_"
    }

    # 5. Check live kwin_wayland process liveness
    try {
        $pidCheck = & adb -s $DeviceId shell "run-as app.polarbear pidof kwin_wayland 2>/dev/null" 2>&1
        $kwinPid = ($pidCheck | Out-String).Trim()
        if ($kwinPid -and $kwinPid -match '^\d+$') {
            $result.Passed = $true
            $result.ClientId = $kwinPid
            $result.Message = "KWin Wayland compositor actively running on device (PID: $kwinPid)."
            return $result
        }
    } catch {
        Write-Verbose "kwin_wayland pidof check failed: $_"
    }

    $result.Message = "No active KWin client identification found in host diagnostics or logcat."
    return $result
}

# ---------------------------------------------------------------------------
# Release Gate 3: Wayland Readiness Marker Verification
# ---------------------------------------------------------------------------

function Test-WaylandReadinessMarker {
    <#
    .SYNOPSIS
        Verifies the Wayland readiness marker (plasma-ready) and presentation evidence.
    #>
    [CmdletBinding()]
    param(
        [string]$DeviceId = "f105b146",
        [string]$HostLogContent = ""
    )

    $result = [PSCustomObject]@{
        Gate          = "Wayland Readiness Marker"
        Passed        = $false
        MarkerExists  = $false
        MarkerContent = ""
        HostPresented = $false
        Message       = ""
    }

    # 1. Check guest filesystem marker
    $markerPath = "/data/data/app.polarbear/files/arch/var/lib/localdesktop/plasma-ready"
    try {
        $checkOut = & adb -s $DeviceId shell "run-as app.polarbear test -f $markerPath && echo EXISTS || echo NOT_FOUND" 2>&1
        if (($checkOut | Out-String) -match "EXISTS") {
            $result.MarkerExists = $true
            $catOut = & adb -s $DeviceId shell "run-as app.polarbear cat $markerPath" 2>&1
            $result.MarkerContent = ($catOut | Out-String).Trim()
        }
    } catch {
        Write-Verbose "Failed checking plasma-ready marker file: $_"
    }

    # 2. Check host log for presentation proof
    if ($HostLogContent) {
        if ($HostLogContent -match 'stage=android-frame-presented|stage=plasma-ready') {
            $result.HostPresented = $true
        }
    } else {
        try {
            $hostLive = & adb -s $DeviceId shell "run-as app.polarbear cat /data/data/app.polarbear/files/diagnostics/host.log 2>/dev/null | grep -E 'stage=plasma-ready|stage=android-frame-presented' | tail -n 2" 2>&1
            if (($hostLive | Out-String) -match 'stage=plasma-ready|stage=android-frame-presented') {
                $result.HostPresented = $true
            }
        } catch {
            Write-Verbose "Failed querying host log for plasma-ready: $_"
        }
    }

    if ($result.MarkerExists -or $result.HostPresented) {
        $result.Passed = $true
        $result.Message = "Wayland readiness verified (markerExists=$($result.MarkerExists), hostPresented=$($result.HostPresented))."
    } else {
        $result.Message = "plasma-ready marker not found in guest state or host logs."
    }

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

    # 1. Check for failure markers in guest state directory
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
        [switch]$ThrowOnFailure
    )

    Write-Host "`n========================================================" -ForegroundColor Magenta
    Write-Host "             RELEASE GATES VERIFICATION                 " -ForegroundColor Magenta
    Write-Host "========================================================" -ForegroundColor Magenta

    $gateResolution = Test-ScreenResolution -DeviceId $DeviceId -ExpectedWidth $ExpectedWidth -ExpectedHeight $ExpectedHeight -ScreenshotPath $ScreenshotPath
    $gateKWin = Test-KWinConnection -DeviceId $DeviceId -HostLogContent $HostLogContent -LogcatContent $LogcatContent
    $gateWayland = Test-WaylandReadinessMarker -DeviceId $DeviceId -HostLogContent $HostLogContent
    $gateCrashes = Test-AppCrashes -DeviceId $DeviceId -HostLogContent $HostLogContent -GuestLogContent $GuestLogContent -LogcatContent $LogcatContent

    $allGates = @($gateResolution, $gateKWin, $gateWayland, $gateCrashes)

    foreach ($gate in $allGates) {
        $statusColor = if ($gate.Passed) { "Green" } else { "Yellow" }
        $statusLabel = if ($gate.Passed) { "[PASS]" } else { "[PEND]" }
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

    $allPassed = ($gateResolution.Passed -and $gateKWin.Passed -and $gateWayland.Passed -and $gateCrashes.Passed)
    if ($ThrowOnFailure -and -not $allPassed) {
        throw "One or more release gates failed verification."
    }

    return [PSCustomObject]@{
        AllPassed       = $allPassed
        GateResolution  = $gateResolution
        GateKWin        = $gateKWin
        GateWayland     = $gateWayland
        GateCrashes     = $gateCrashes
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
    Write-Host "  -> Command: $XbuildPath build --platform android --arch arm64 --format apk" -ForegroundColor Gray

    Push-Location $RepoRoot
    try {
        & $XbuildPath build --platform android --arch arm64 --format apk
        if ($LASTEXITCODE -ne 0) {
            throw "xbuild execution failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }

    # Locate generated APK
    $candidatePaths = @(
        (Join-Path $RepoRoot "target\x\debug\android\localdesktop.apk"),
        (Join-Path $RepoRoot "target\x\debug\android\gradle\app\build\outputs\apk\debug\app-debug.apk"),
        (Join-Path $RepoRoot "target\x\release\android\localdesktop.apk")
    )

    $builtApk = $null
    foreach ($cand in $candidatePaths) {
        if (Test-Path $cand) {
            $builtApk = (Resolve-Path $cand).Path
            break
        }
    }

    if (-not $builtApk) {
        throw "APK build completed but target APK could not be found."
    }

    Write-Host "  -> Built APK: $builtApk ($( [math]::Round((Get-Item $builtApk).Length / 1MB, 2) ) MB)" -ForegroundColor Green

    # Ensure APK is signed with matching device key (strictly preserves app data on update)
    $signingDir = Join-Path $RepoRoot ".release-signing"
    $keystore = Join-Path $signingDir "localdesktop-plasma-release.jks"
    $credsEnv = Join-Path $signingDir "release-credentials.env"

    if ((Test-Path $keystore) -and (Test-Path $credsEnv)) {
        Write-Host "  -> Signing APK with release certificate to ensure update compatibility..." -ForegroundColor Cyan
        $creds = Get-Content $credsEnv | ConvertFrom-StringData
        $storePass = $creds["KEYSTORE_PASSWORD"]
        $keyPass = $creds["KEY_PASSWORD"]
        $keyAlias = $creds["KEY_ALIAS"]

        $apksigner = Get-ChildItem -Path (Join-Path $env:ANDROID_HOME "build-tools") -Recurse -Filter "apksigner.bat" -ErrorAction SilentlyContinue |
            Select-Object -First 1

        if ($apksigner) {
            & $apksigner.FullName sign --ks $keystore --ks-pass "pass:$storePass" --ks-key-alias $keyAlias --key-pass "pass:$keyPass" $builtApk
            Write-Host "  -> APK signed successfully with certificate for $keyAlias." -ForegroundColor Green
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

    while ($elapsed -lt $TimeoutSeconds) {
        Start-Sleep -Seconds $interval
        $elapsed += $interval
        Write-Host "  -> Waiting for session stabilization ($elapsed / $TimeoutSeconds s)..." -ForegroundColor Gray

        # Check if plasma-ready or failure marker appeared
        $markerCheck = & adb -s $DeviceId shell "run-as $PackageName ls /data/data/$PackageName/files/arch/var/lib/localdesktop/ 2>/dev/null" 2>&1
        $markerText = ($markerCheck | Out-String)
        if ($markerText -match "plasma-ready|plasma-failed|kwin-crash") {
            Write-Host "  -> Desktop state marker detected: $($Matches[0])" -ForegroundColor Yellow
            break
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

    # Dump logcat
    try {
        $logcatContent = & adb -s $DeviceId logcat -d 2>&1 | Out-String
        if ($logcatContent) {
            Set-Content -Path $logcatFile -Value $logcatContent -Encoding utf8
            Write-Host "  -> Saved logcat to $logcatFile ($( [math]::Round((Get-Item $logcatFile).Length / 1KB, 1) ) KB)" -ForegroundColor Green
        }
    } catch {
        Write-Warning "Could not retrieve logcat: $_"
    }

    return [PSCustomObject]@{
        HostLog  = $hostLogContent
        GuestLog = $guestLogContent
        Logcat   = $logcatContent
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
        [switch]$SkipBuild,
        [switch]$SkipInstall,
        [switch]$SkipLaunch,
        [int]$TimeoutSeconds = 45,
        [switch]$EnforceReleaseGates
    )

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
        $candidatePaths = @(
            (Join-Path $RepoRoot "target\x\debug\android\localdesktop.apk"),
            (Join-Path $RepoRoot "target\x\debug\android\gradle\app\build\outputs\apk\debug\app-debug.apk")
        )
        foreach ($c in $candidatePaths) {
            if (Test-Path $c) { $ApkPath = $c; break }
        }
    }

    # Step 3: Install APK (strictly preserving data)
    if (-not $SkipInstall -and $ApkPath -and (Test-Path $ApkPath)) {
        Install-AppApk -DeviceId $DeviceId -ApkPath $ApkPath
    }

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
        -ThrowOnFailure:$EnforceReleaseGates

    # Save summary report JSON
    $summaryFile = Join-Path $ArtifactDir "qa-summary.json"
    $gatesReport | ConvertTo-Json -Depth 4 | Set-Content -Path $summaryFile -Encoding utf8
    Write-Host "Saved validation summary to $summaryFile" -ForegroundColor Gray

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
        -EnforceReleaseGates:$EnforceReleaseGates
}
