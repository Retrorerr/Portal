[CmdletBinding()]
param(
    # Install and launch the diagnostic package after building it. The package
    # is app.polarbear; the release package is app.polarbear.portal.
    [switch]$InstallDebug,
    [string]$DeviceId
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$debugManifestPath = Join-Path $repoRoot 'manifest.debug.yaml'
$xbuildManifestPath = Join-Path $repoRoot 'patches\xbuild\xbuild\Cargo.toml'

function Invoke-LocalXBuild {
    param([Parameter(Mandatory)][string[]]$Arguments)

    & cargo run --manifest-path $xbuildManifestPath -- @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Local xbuild failed with exit code $LASTEXITCODE"
    }
}

function Assert-AdaptiveIconResources {
    param([Parameter(Mandatory)][string]$GradleRoot)

    $adaptiveDir = Join-Path $GradleRoot 'app\src\main\res\mipmap-anydpi-v26'
    $launcher = Join-Path $adaptiveDir 'ic_launcher.xml'
    $foreground = Join-Path $adaptiveDir 'ic_launcher_foreground.xml'
    $monochrome = Join-Path $adaptiveDir 'ic_launcher_monochrome.xml'
    foreach ($required in @($launcher, $foreground, $monochrome)) {
        if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
            throw "Missing adaptive icon resource: $required"
        }
    }
    if ([System.IO.File]::ReadAllText($launcher) -notmatch '#191B1C') {
        throw "Adaptive icon background is not Portal charcoal: $launcher"
    }
}

$ndkBin = Join-Path $env:LOCALAPPDATA 'Android\Sdk\ndk\27.2.12479018\toolchains\llvm\prebuilt\windows-x86_64\bin'
$gradleBin = Join-Path $env:LOCALAPPDATA 'Gradle\gradle-8.14.3\bin'
$androidSdk = Join-Path $env:LOCALAPPDATA 'Android\Sdk'
if (Test-Path -LiteralPath $androidSdk -PathType Container) {
    # xbuild invokes Gradle in a generated directory. Export the SDK location
    # explicitly so that Gradle does not depend on a user-specific
    # local.properties file left behind by an earlier build.
    $env:ANDROID_HOME = $androidSdk
    $env:ANDROID_SDK_ROOT = $androidSdk
}

function Get-Aapt2 {
    $sdk = $env:ANDROID_HOME
    if ([string]::IsNullOrWhiteSpace($sdk)) {
        throw 'ANDROID_HOME is not set; Android SDK is required to verify the APK package id.'
    }
    $aapt2 = Get-ChildItem -LiteralPath (Join-Path $sdk 'build-tools') -Recurse -Filter 'aapt2.exe' |
        Sort-Object FullName |
        Select-Object -Last 1 -ExpandProperty FullName
    if ([string]::IsNullOrWhiteSpace($aapt2)) {
        throw "aapt2.exe was not found below $sdk\build-tools"
    }
    return $aapt2
}

function Assert-DebugApk {
    param([Parameter(Mandatory)][string]$ApkPath)

    $aapt2 = Get-Aapt2
    $packageLine = (& $aapt2 dump badging $ApkPath | Select-String '^package:' | Select-Object -First 1).ToString()
    if ($packageLine -notmatch "name='app\.polarbear'") {
        throw "The diagnostic APK is not app.polarbear: $packageLine"
    }
}

function Resolve-AdbDevice {
    param([string]$RequestedDevice)

    if (-not [string]::IsNullOrWhiteSpace($RequestedDevice)) {
        return $RequestedDevice
    }
    $devices = @(& adb devices | Select-Object -Skip 1 | Where-Object { $_ -match '\tdevice\s*$' } | ForEach-Object { ($_ -split '\s+')[0] })
    if ($devices.Count -ne 1) {
        throw "Pass -DeviceId explicitly; found $($devices.Count) adb devices in the device state."
    }
    return $devices[0]
}

function Install-AndLaunchDebugApk {
    param([Parameter(Mandatory)][string]$ApkPath)

    $device = Resolve-AdbDevice $DeviceId
    & adb -s $device install -r -t $ApkPath
    if ($LASTEXITCODE -ne 0) {
        throw "adb install failed with exit code $LASTEXITCODE"
    }

    $hostHash = (Get-FileHash -LiteralPath $ApkPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $devicePath = (& adb -s $device shell pm path app.polarbear | Select-String '^package:' | Select-Object -First 1).ToString().Trim() -replace '^package:', ''
    $deviceHashLine = (& adb -s $device shell sha256sum $devicePath).ToString().Trim()
    $deviceHash = ($deviceHashLine -split '\s+')[0].ToLowerInvariant()
    if ($deviceHash -ne $hostHash) {
        throw "Installed APK hash mismatch: host=$hostHash device=$deviceHash"
    }

    & adb -s $device shell am force-stop app.polarbear
    & adb -s $device shell monkey -p app.polarbear 1
    if ($LASTEXITCODE -ne 0) {
        throw "Could not launch app.polarbear on $device"
    }
    Write-Host "Installed and launched app.polarbear on $device. APK SHA-256: $hostHash"
}
foreach ($candidate in @($ndkBin, $gradleBin)) {
    if (Test-Path -LiteralPath $candidate) {
        $env:Path = "$candidate;$env:Path"
    }
}

Push-Location $repoRoot
try {
    Write-Host 'Building optimized stable Portal (app.polarbear.portal)...'
    Invoke-LocalXBuild -Arguments @('build', '--release', '--platform', 'android', '--arch', 'arm64', '--format', 'apk')

    $stableGradle = Join-Path $repoRoot 'target\x\release\android\gradle'
    Assert-AdaptiveIconResources -GradleRoot $stableGradle
    $stableUnsigned = Join-Path $repoRoot 'target\x\release\android\gradle\app\build\outputs\apk\release\app-release-unsigned.apk'
    Copy-Item -LiteralPath $stableUnsigned -Destination (Join-Path $repoRoot 'target\Portal-stable-unsigned.apk') -Force

    Write-Host 'Building diagnostic Portal Debug (app.polarbear)...'
    Invoke-LocalXBuild -Arguments @('build', '--manifest', $debugManifestPath, '--debug', '--features', 'portal-debug', '--platform', 'android', '--arch', 'arm64', '--format', 'apk')

    $debugGradle = Join-Path $repoRoot 'target\x\debug\android\gradle'
    Assert-AdaptiveIconResources -GradleRoot $debugGradle
    $debugApk = Join-Path $repoRoot 'target\x\debug\android\gradle\app\build\outputs\apk\debug\app-debug.apk'
    Assert-DebugApk -ApkPath $debugApk
    Copy-Item -LiteralPath $debugApk -Destination (Join-Path $repoRoot 'target\Portal-Debug.apk') -Force
}
finally {
    Pop-Location
}

Write-Host 'Built target\Portal-stable-unsigned.apk and target\Portal-Debug.apk.'
if ($InstallDebug) {
    Install-AndLaunchDebugApk -ApkPath (Join-Path $repoRoot 'target\Portal-Debug.apk')
}
