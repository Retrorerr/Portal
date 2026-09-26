[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
    # ADB serial is mandatory so this can never silently choose another device.
    [Parameter(Mandatory)]
    [ValidateNotNullOrEmpty()]
    [string]$DeviceId
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$debugApk = Join-Path $repoRoot 'target\Portal-Debug.apk'
$debugPackage = 'app.polarbear'

function Resolve-AndroidAapt {
    $sdkCandidates = @(
        $env:ANDROID_HOME,
        $env:ANDROID_SDK_ROOT,
        $(if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA 'Android\Sdk' })
    ) | Where-Object { $_ } | Select-Object -Unique

    foreach ($sdk in $sdkCandidates) {
        $buildToolsRoot = Join-Path $sdk 'build-tools'
        if (-not (Test-Path -LiteralPath $buildToolsRoot -PathType Container)) {
            continue
        }
        $versions = Get-ChildItem -LiteralPath $buildToolsRoot -Directory -ErrorAction SilentlyContinue |
            Sort-Object -Property @{
                Expression = {
                    try { [version]($_.Name -replace '-.*$', '') }
                    catch { [version]'0.0' }
                }
            } -Descending
        foreach ($version in $versions) {
            $candidate = Join-Path $version.FullName 'aapt.exe'
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                return $candidate
            }
        }
    }
    $fromPath = Get-Command aapt.exe -ErrorAction SilentlyContinue
    if ($fromPath) { return $fromPath.Source }
    throw 'Android build-tools aapt.exe is required to verify the DEBUG APK identity.'
}

function Assert-DebugApk {
    param([Parameter(Mandatory)][string]$AaptPath)
    if (-not (Test-Path -LiteralPath $debugApk -PathType Leaf)) {
        throw "DEBUG APK is missing: $debugApk"
    }
    $badging = @(& $AaptPath dump badging $debugApk 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "aapt could not inspect '$debugApk': $($badging -join "`n")"
    }
    $text = $badging -join "`n"
    $package = [regex]::Match($text, "(?m)^package: name='([^']+)'\s").Groups[1].Value
    $label = [regex]::Match($text, "(?m)^application-label:'([^']*)'").Groups[1].Value
    $debuggable = $text -match '(?m)^application-debuggable\s*$'
    if ($package -ne $debugPackage -or $label -ne 'Portal Debug' -or -not $debuggable) {
        throw "Refusing to install mismatched APK. Expected DEBUG / $debugPackage / Portal Debug / debuggable; got $package / $label / debuggable=$debuggable."
    }
}

function Assert-InstalledDebugIdentity {
    $packages = @(& adb -s $DeviceId shell pm list packages 2>&1)
    if ($LASTEXITCODE -ne 0) { throw "Could not read installed packages from $DeviceId." }
    if ($packages -notcontains "package:$debugPackage") { return $false }

    $details = @(& adb -s $DeviceId shell dumpsys package $debugPackage 2>&1)
    if ($LASTEXITCODE -ne 0) { throw "Could not inspect installed DEBUG package $debugPackage." }
    $debuggable = ($details -join "`n") -match '(?m)^\s*(?:pkgFlags|flags)=\[[^\]]*\bDEBUGGABLE\b'
    if (-not $debuggable) {
        throw "Installed package $debugPackage is not debuggable. Refusing to replace it."
    }
    return $true
}

function Assert-InstalledDebugBytes {
    $paths = @(& adb -s $DeviceId shell pm path $debugPackage 2>&1)
    if ($LASTEXITCODE -ne 0) { throw "Could not read installed APK path for $debugPackage." }
    $baseLine = $paths | Where-Object { $_ -match '^package:.*/base\.apk\s*$' } | Select-Object -First 1
    if (-not $baseLine) { throw "No base.apk path returned for installed package $debugPackage." }
    $remotePath = ($baseLine -replace '^package:', '').Trim()
    $remoteHashLine = @(& adb -s $DeviceId shell sha256sum $remotePath 2>&1)
    if ($LASTEXITCODE -ne 0) { throw "Could not verify installed APK bytes for $debugPackage." }
    $remoteHash = [regex]::Match(($remoteHashLine -join "`n"), '(?i)\b([0-9a-f]{64})\b').Groups[1].Value
    $localHash = (Get-FileHash -LiteralPath $debugApk -Algorithm SHA256).Hash
    if (-not $remoteHash -or $remoteHash -ne $localHash) {
        throw "Installed DEBUG APK bytes do not match '$debugApk'."
    }
}

$aaptPath = Resolve-AndroidAapt
Assert-DebugApk -AaptPath $aaptPath

$deviceState = @(& adb -s $DeviceId get-state 2>&1)
if ($LASTEXITCODE -ne 0 -or ($deviceState -join '').Trim() -ne 'device') {
    throw "ADB device '$DeviceId' is not connected and ready. No installation was attempted."
}

# This helper only updates app.polarbear (Portal Debug). Stable is entirely
# outside its scope. -r updates this exact package while preserving its data;
# signature or identity mismatches remain hard failures with no fallback.
Assert-InstalledDebugIdentity | Out-Null
$description = "$DeviceId; DEBUG $debugPackage <= $debugApk (adb install -r; preserve app data)"
if (-not $PSCmdlet.ShouldProcess($description, 'Install identity-checked Portal Debug APK')) {
    return
}

Write-Host "Installing DEBUG package $debugPackage with adb install -r..."
$installOutput = @(& adb -s $DeviceId install -r $debugApk 2>&1)
if ($LASTEXITCODE -ne 0) {
    throw "DEBUG install failed; no alternate package or destructive fallback was attempted. Details: $($installOutput -join "`n")"
}
Write-Host ($installOutput -join "`n")
if (-not (Assert-InstalledDebugIdentity)) {
    throw "DEBUG package $debugPackage is not installed after adb reported success."
}
Assert-InstalledDebugBytes
Write-Host "Verified DEBUG package $debugPackage was updated in place; its app data and the Stable app were not touched."
