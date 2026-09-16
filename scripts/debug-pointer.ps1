<#
.SYNOPSIS
    Debug-only Portal pointer automation through the real touchpad path
    (app.polarbear).

.DESCRIPTION
    Injects semantic pointer operations into Portal's production input path
    (broadcast -> JNI -> winit WindowEvent -> forward_anland_input -> Anland
    wire -> KWin PointerInputRedirection), identical to physical touchpad
    hardware downstream of the event queue. Coordinates are Android buffer px
    (unscaled, e.g. 3392x2400 landscape); use --es string extras (am has no
    double extra type; the receiver coerces).

    Ops:
      move x y            absolute pointer motion (repeat for drags)
      button b pressed    button 0=left 1=right 2=middle, pressed true/false
      click x y           move + left press + release
      drag x1 y1 x2 y2 ms interpolated absolute motion while held
      scroll x y          finger-scroll delta (raw buffer px)
      scroll_stop         terminate finger stream (axis-stop)

    The veil MUST already be dismissed (scripts/debug-dismiss-veil.ps1);
    verify `overlay removed` before UI testing.
#>
[CmdletBinding()]
param(
    [string]$DeviceId = "",
    [string]$PackageName = "app.polarbear",
    [Parameter(Mandatory, Position = 0)][string]$Op,
    [double]$X = 0,
    [double]$Y = 0,
    [double]$X2 = 0,
    [double]$Y2 = 0,
    [int]$Button = 0,
    [bool]$Pressed = $false,
    [int]$DurationMs = 600,
    [int]$Steps = 12
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

function Send-PointerOp {
    param([string[]]$ExtraArgs)
    $args = @("-s", $DeviceId, "shell", "am", "broadcast",
        "-a", "app.polarbear.DEBUG_POINTER") + $ExtraArgs
    & adb @args | Out-Null
}

$DeviceId = Get-AdbDeviceId -Preferred $DeviceId

switch ($Op) {
    "move" {
        Send-PointerOp @("--es", "op", "move", "--es", "x", "$X", "--es", "y", "$Y")
    }
    "button" {
        Send-PointerOp @("--es", "op", "button", "--ei", "button", "$Button",
            "--ez", "pressed", "$Pressed".ToLower())
    }
    "click" {
        Send-PointerOp @("--es", "op", "move", "--es", "x", "$X", "--es", "y", "$Y")
        Start-Sleep -Milliseconds 120
        Send-PointerOp @("--es", "op", "button", "--ei", "button", "0", "--ez", "pressed", "true")
        Start-Sleep -Milliseconds 120
        Send-PointerOp @("--es", "op", "button", "--ei", "button", "0", "--ez", "pressed", "false")
    }
    "drag" {
        Send-PointerOp @("--es", "op", "move", "--es", "x", "$X", "--es", "y", "$Y")
        Start-Sleep -Milliseconds 150
        Send-PointerOp @("--es", "op", "button", "--ei", "button", "0", "--ez", "pressed", "true")
        Start-Sleep -Milliseconds 150
        $stepMs = [Math]::Max(20, [int]($DurationMs / $Steps))
        for ($i = 1; $i -le $Steps; $i++) {
            $px = $X + ($X2 - $X) * $i / $Steps
            $py = $Y + ($Y2 - $Y) * $i / $Steps
            Send-PointerOp @("--es", "op", "move", "--es", "x", "$px", "--es", "y", "$py")
            Start-Sleep -Milliseconds $stepMs
        }
        Start-Sleep -Milliseconds 150
        Send-PointerOp @("--es", "op", "button", "--ei", "button", "0", "--ez", "pressed", "false")
    }
    "scroll" {
        Send-PointerOp @("--es", "op", "scroll", "--es", "x", "$X", "--es", "y", "$Y")
    }
    "scroll_stop" {
        Send-PointerOp @("--es", "op", "scroll_stop")
    }
    default { throw "Unknown op: $Op (want move|button|click|drag|scroll|scroll_stop)" }
}
