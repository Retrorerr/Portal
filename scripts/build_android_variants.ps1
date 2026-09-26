[CmdletBinding()]
param(
    # Validate the previously built artifacts without invoking xbuild.
    [switch]$ValidateOnly,
    # Build only Portal Debug. This is the default for device testing where
    # Stable must keep using its last explicitly produced APK.
    [switch]$DebugOnly
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $repoRoot 'manifest.yaml'
$debugManifestPath = Join-Path $repoRoot 'manifest.debug.yaml'
$xbuildManifestPath = Join-Path $repoRoot 'patches\xbuild\xbuild\Cargo.toml'
$targetDir = Join-Path $repoRoot 'target'

# This mapping is intentionally explicit. Stable and Debug are separate Android
# applications; never infer the target package from a file name or UI label.
$stable = [pscustomobject]@{
    Name = 'Stable'
    ManifestPath = $manifestPath
    Package = 'app.polarbear.portal'
    Label = 'Portal'
    Icon = 'assets/portal-icon.png'
    GradleRoot = Join-Path $repoRoot 'target\x\release\android\gradle'
    ApkPath = Join-Path $targetDir 'Portal-stable-unsigned.apk'
    Debuggable = $false
}
$debug = [pscustomobject]@{
    Name = 'DEBUG'
    ManifestPath = $debugManifestPath
    Package = 'app.polarbear'
    Label = 'Portal Debug'
    Icon = 'assets/portal-debug-icon.png'
    GradleRoot = Join-Path $repoRoot 'target\x\debug\android\gradle'
    ApkPath = Join-Path $targetDir 'Portal-Debug.apk'
    Debuggable = $true
}

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
    if ($fromPath) {
        return $fromPath.Source
    }
    throw 'Android build-tools aapt.exe is required to verify APK package identity and debuggability.'
}

function Assert-ManifestVariant {
    param([Parameter(Mandatory)]$Variant)

    if (-not (Test-Path -LiteralPath $Variant.ManifestPath -PathType Leaf)) {
        throw "$($Variant.Name) manifest is missing: $($Variant.ManifestPath)"
    }
    $text = [System.IO.File]::ReadAllText($Variant.ManifestPath)
    $package = [regex]::Match($text, '(?m)^\s*package:\s*([A-Za-z0-9_.]+)\s*$').Groups[1].Value
    $label = [regex]::Match($text, '(?m)^\s+label:\s*"([^"]+)"\s*$').Groups[1].Value
    $icon = [regex]::Match($text, '(?m)^icon:\s*(\S+)\s*$').Groups[1].Value
    if ($package -ne $Variant.Package -or $label -ne $Variant.Label -or $icon -ne $Variant.Icon) {
        throw "$($Variant.Name) manifest identity mismatch. Expected package=$($Variant.Package), label='$($Variant.Label)', icon=$($Variant.Icon); got package=$package, label='$label', icon=$icon."
    }

    $iconPath = Join-Path $repoRoot $Variant.Icon
    if (-not (Test-Path -LiteralPath $iconPath -PathType Leaf)) {
        throw "$($Variant.Name) icon asset is missing: $iconPath"
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

function Assert-GeneratedVariant {
    param([Parameter(Mandatory)]$Variant)

    $appDir = Join-Path $Variant.GradleRoot 'app'
    $gradlePath = Join-Path $appDir 'build.gradle'
    $manifestPath = Join-Path $appDir 'src\main\AndroidManifest.xml'
    foreach ($required in @($gradlePath, $manifestPath)) {
        if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
            throw "$($Variant.Name) generated Android project is missing: $required"
        }
    }

    $gradleText = [System.IO.File]::ReadAllText($gradlePath)
    $applicationId = [regex]::Match($gradleText, "(?m)^\s*applicationId\s+'([^']+)'\s*$").Groups[1].Value
    if ($applicationId -ne $Variant.Package) {
        throw "$($Variant.Name) generated Gradle applicationId is '$applicationId', expected '$($Variant.Package)'."
    }

    [xml]$xmlManifest = [System.IO.File]::ReadAllText($manifestPath)
    $androidNamespace = 'http://schemas.android.com/apk/res/android'
    $applicationNode = $xmlManifest.SelectSingleNode('/manifest/application')
    $generatedLabel = $applicationNode.GetAttribute('label', $androidNamespace)
    if ($generatedLabel -ne $Variant.Label) {
        throw "$($Variant.Name) generated Android label is '$generatedLabel', expected '$($Variant.Label)'."
    }

    Assert-AdaptiveIconResources -GradleRoot $Variant.GradleRoot
}

function Assert-ApkVariant {
    param(
        [Parameter(Mandatory)]$Variant,
        [Parameter(Mandatory)][string]$AaptPath
    )

    if (-not (Test-Path -LiteralPath $Variant.ApkPath -PathType Leaf)) {
        throw "$($Variant.Name) APK is missing: $($Variant.ApkPath)"
    }
    $apkItem = Get-Item -LiteralPath $Variant.ApkPath
    if ($apkItem.Length -le 0) {
        throw "$($Variant.Name) APK is empty: $($Variant.ApkPath)"
    }

    $badging = @(& $AaptPath dump badging $Variant.ApkPath 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "aapt could not inspect $($Variant.ApkPath): $($badging -join "`n")"
    }
    $badgingText = $badging -join "`n"
    $package = [regex]::Match($badgingText, "(?m)^package: name='([^']+)'\s").Groups[1].Value
    $label = [regex]::Match($badgingText, "(?m)^application-label:'([^']*)'").Groups[1].Value
    $isDebuggable = $badgingText -match '(?m)^application-debuggable\s*$'
    if ($package -ne $Variant.Package -or $label -ne $Variant.Label -or $isDebuggable -ne $Variant.Debuggable) {
        throw "$($Variant.Name) APK identity mismatch at '$($Variant.ApkPath)'. Expected package=$($Variant.Package), label='$($Variant.Label)', debuggable=$($Variant.Debuggable); got package=$package, label='$label', debuggable=$isDebuggable."
    }
}

function Assert-VariantIconsDiffer {
    $stableForeground = Join-Path $stable.GradleRoot 'app\src\main\res\mipmap-xxxhdpi\ic_launcher_foreground.png'
    $debugForeground = Join-Path $debug.GradleRoot 'app\src\main\res\mipmap-xxxhdpi\ic_launcher_foreground.png'
    foreach ($icon in @($stableForeground, $debugForeground)) {
        if (-not (Test-Path -LiteralPath $icon -PathType Leaf)) {
            throw "Generated launcher foreground is missing: $icon"
        }
    }

    $stableHash = (Get-FileHash -LiteralPath $stableForeground -Algorithm SHA256).Hash
    $debugHash = (Get-FileHash -LiteralPath $debugForeground -Algorithm SHA256).Hash
    if ($stableHash -eq $debugHash) {
        throw 'Stable and DEBUG launcher artwork are identical; refusing to publish ambiguously branded APKs.'
    }
}

function Assert-AllVariants {
    param([Parameter(Mandatory)][string]$AaptPath)
    Assert-ManifestVariant -Variant $stable
    Assert-ManifestVariant -Variant $debug
    Assert-GeneratedVariant -Variant $stable
    Assert-GeneratedVariant -Variant $debug
    Assert-VariantIconsDiffer
    Assert-ApkVariant -Variant $stable -AaptPath $AaptPath
    Assert-ApkVariant -Variant $debug -AaptPath $AaptPath
}

$ndkBin = Join-Path $env:LOCALAPPDATA 'Android\Sdk\ndk\27.2.12479018\toolchains\llvm\prebuilt\windows-x86_64\bin'
$gradleBin = Join-Path $env:LOCALAPPDATA 'Gradle\gradle-8.14.3\bin'
foreach ($candidate in @($ndkBin, $gradleBin)) {
    if (Test-Path -LiteralPath $candidate) {
        $env:Path = "$candidate;$env:Path"
    }
}

Assert-ManifestVariant -Variant $stable
Assert-ManifestVariant -Variant $debug
$aaptPath = Resolve-AndroidAapt

if ($ValidateOnly) {
    Assert-AllVariants -AaptPath $aaptPath
    Write-Host 'Validated Stable app.polarbear.portal and DEBUG app.polarbear APK identities and distinct icons.'
    exit 0
}

$originalManifest = [System.IO.File]::ReadAllBytes($manifestPath)
Push-Location $repoRoot
try {
    if (-not $DebugOnly) {
        Write-Host 'Building optimized Stable Portal (app.polarbear.portal)...'
        & cargo run --manifest-path $xbuildManifestPath -- build --release --platform android --arch arm64 --format apk
        if ($LASTEXITCODE -ne 0) {
            throw "Stable xbuild failed with exit code $LASTEXITCODE"
        }

        Assert-GeneratedVariant -Variant $stable
        $stableUnsigned = Join-Path $stable.GradleRoot 'app\build\outputs\apk\release\app-release-unsigned.apk'
        if (-not (Test-Path -LiteralPath $stableUnsigned -PathType Leaf)) {
            throw "Stable unsigned APK is missing: $stableUnsigned"
        }
        Copy-Item -LiteralPath $stableUnsigned -Destination $stable.ApkPath -Force
        Assert-ApkVariant -Variant $stable -AaptPath $aaptPath
    }

    Write-Host 'Building DEBUG Portal (app.polarbear)...'
    Copy-Item -LiteralPath $debugManifestPath -Destination $manifestPath -Force
    Assert-ManifestVariant -Variant $debug
    & cargo run --manifest-path $xbuildManifestPath -- build --debug --features portal-debug --platform android --arch arm64 --format apk
    if ($LASTEXITCODE -ne 0) {
        throw "DEBUG xbuild failed with exit code $LASTEXITCODE"
    }

    Assert-GeneratedVariant -Variant $debug
    $debugGradleApk = Join-Path $debug.GradleRoot 'app\build\outputs\apk\debug\app-debug.apk'
    if (-not (Test-Path -LiteralPath $debugGradleApk -PathType Leaf)) {
        throw "DEBUG APK is missing: $debugGradleApk"
    }
    Copy-Item -LiteralPath $debugGradleApk -Destination $debug.ApkPath -Force
    Assert-ApkVariant -Variant $debug -AaptPath $aaptPath
    if (-not $DebugOnly) {
        Assert-VariantIconsDiffer
    }
}
finally {
    [System.IO.File]::WriteAllBytes($manifestPath, $originalManifest)
    Pop-Location
}

if ($DebugOnly) {
    Write-Host "Built and verified DEBUG only: $($debug.ApkPath) -> $($debug.Package) (debuggable)"
} else {
    Write-Host 'Built and verified:'
    Write-Host "  Stable unsigned: $($stable.ApkPath) -> $($stable.Package) (not debuggable)"
    Write-Host "  DEBUG:           $($debug.ApkPath) -> $($debug.Package) (debuggable)"
}
Write-Host 'This script only builds APKs. It never installs, uninstalls, or clears app data.'
