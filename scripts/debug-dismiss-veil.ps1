<#
.SYNOPSIS
    Debug-only Portal veil dismissal for automated UI testing (app.polarbear).

.DESCRIPTION
    Every automated Plasma UI test must run unveiled: the Portal reveal veil
    stays fullscreen and input-consuming until the reveal commits, so clicks,
    scrolls and drags issued while it is attached are NOT valid tests.

    Required sequence for every automated UI run:
      1. launch Debug
      2. wait for READY with short polling (plasma-ready marker)
      3. invoke this hook (Debug-only broadcast, inert on Stable/release)
      4. verify "overlay removed" in logcat
      5. only then begin Plasma UI testing

    The hook performs the same state transition as a completed human reveal
    (reveal-committed -> overlay-removed) once the desktop-ready latch holds;
    normal swipe behavior is unchanged.
#>
[CmdletBinding()]
param(
    [string]$DeviceId = "",
    [string]$PackageName = "app.polarbear",
    [int]$ReadyTimeoutSeconds = 90,
    [int]$RemovedTimeoutSeconds = 10
)

$ErrorActionPreference = "Stop"

function Get-AdbDeviceId {
    param([string]$Preferred = "")
    if ($Preferred) { return $Preferred }
    if ($env:ANDROID_SERIAL) { return $env:ANDROID_SERIAL }
    $devs = & adb devices 2>$null | Select-String "\tdevice$"
    if ($devs) {
        $first = if ($devs -is [array]) { $devs[0].Line } else { $devs.Line }
        return ($first -split "\s+")[0]
    }
    throw "No adb device found"
}

$DeviceId = Get-AdbDeviceId -Preferred $DeviceId
Write-Host "[veil] waiting for READY on $PackageName..." -ForegroundColor Cyan

$runStart = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$ready = $null
$deadline = (Get-Date).AddSeconds($ReadyTimeoutSeconds)
while ((Get-Date) -lt $deadline) {
    $text = & adb -s $DeviceId shell "run-as $PackageName cat files/runtime-B/var/lib/localdesktop/plasma-ready 2>&1" 2>&1 | Out-String
    if ($text -match 'timestamp_ms=(\d+)') {
        $ts = [int64]$Matches[1]
        if ($ts -ge ($runStart - 120000)) {
            $ready = $text.Trim()
            break
        }
    }
    Start-Sleep -Milliseconds 500
}
if (-not $ready) { throw "[veil] READY not observed within ${ReadyTimeoutSeconds}s; refusing to dismiss" }
Write-Host "[veil] READY: $ready" -ForegroundColor Green

& adb -s $DeviceId shell am broadcast -a app.polarbear.DEBUG_DISMISS_VEIL 2>&1 | Out-String | Write-Host
$removedDeadline = (Get-Date).AddSeconds($RemovedTimeoutSeconds)
while ((Get-Date) -lt $removedDeadline) {
    $hits = & adb -s $DeviceId shell logcat -d 2>&1 | Select-String "overlay removed"
    if ($hits) {
        Write-Host "[veil] overlay removed confirmed" -ForegroundColor Green
        return
    }
    Start-Sleep -Milliseconds 500
}
throw "[veil] 'overlay removed' not observed within ${RemovedTimeoutSeconds}s"
