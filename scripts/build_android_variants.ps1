[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $repoRoot 'manifest.yaml'
$debugManifestPath = Join-Path $repoRoot 'manifest.debug.yaml'
$xbuildManifestPath = Join-Path $repoRoot 'patches\xbuild\xbuild\Cargo.toml'
$originalManifest = [System.IO.File]::ReadAllBytes($manifestPath)

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
    Copy-Item -LiteralPath $debugManifestPath -Destination $manifestPath -Force
    Invoke-LocalXBuild -Arguments @('build', '--debug', '--features', 'portal-debug', '--platform', 'android', '--arch', 'arm64', '--format', 'apk')

    $debugGradle = Join-Path $repoRoot 'target\x\debug\android\gradle'
    Assert-AdaptiveIconResources -GradleRoot $debugGradle
    $debugApk = Join-Path $repoRoot 'target\x\debug\android\gradle\app\build\outputs\apk\debug\app-debug.apk'
    Copy-Item -LiteralPath $debugApk -Destination (Join-Path $repoRoot 'target\Portal-Debug.apk') -Force
}
finally {
    [System.IO.File]::WriteAllBytes($manifestPath, $originalManifest)
    Pop-Location
}

Write-Host 'Built target\Portal-stable-unsigned.apk and target\Portal-Debug.apk.'
