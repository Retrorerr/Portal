[CmdletBinding()]
param(
    [string]$DeviceId = "",
    [string]$RepoRoot = "",
    [string]$ArtifactRoot = "",
    [int]$WaitSeconds = 30
)

# Diagnostic-only probe. No APK install, data clear, or production asset
# change is performed. Every app-private target is inspected and backed up
# before staging, and is restored and read-back-verified in finally.
$ErrorActionPreference = "Stop"
$adb = Join-Path $env:LOCALAPPDATA "Android\Sdk\platform-tools\adb.exe"
if (-not (Test-Path -LiteralPath $adb)) { throw "adb not found: $adb" }
if (-not $DeviceId) {
    $DeviceId = if ($env:ANDROID_SERIAL) { $env:ANDROID_SERIAL } else {
        $devs = & $adb devices | Select-String "\tdevice$"
        if ($devs) {
            $first = if ($devs -is [array]) { $devs[0].Line } else { $devs.Line }
            ($first -split "\s+")[0]
        } else { "f105b146" }
    }
}
if ([string]::IsNullOrWhiteSpace($RepoRoot)) { $RepoRoot = Split-Path -Parent $PSScriptRoot }
if ([string]::IsNullOrWhiteSpace($ArtifactRoot)) {
    $ArtifactRoot = Join-Path $RepoRoot ("artifacts\qa\kcminit-stack-" + (Get-Date -Format "yyyyMMdd-HHmmss"))
}
$runRoot = [IO.Path]::GetFullPath($ArtifactRoot)
if (Test-Path -LiteralPath $runRoot) {
    if (-not (Get-Item -LiteralPath $runRoot).PSIsContainer) { throw "artifact root is not a directory: $runRoot" }
    if (@(Get-ChildItem -LiteralPath $runRoot -Force).Count -ne 0) {
        throw "artifact root must be new or empty: $runRoot"
    }
} else {
    New-Item -ItemType Directory -Force -Path $runRoot | Out-Null
}

$probeSource = Join-Path $RepoRoot "assets\localdesktop-kcminit-stack-probe.c"
$wrapperSource = Join-Path $RepoRoot "assets\localdesktop-kcminit-stack-wrapper.sh"
if (-not (Test-Path -LiteralPath $probeSource) -or -not (Test-Path -LiteralPath $wrapperSource)) {
    throw "probe assets are missing"
}

$targets = @(
    @{ Name = "kcminit_startup"; Path = "files/arch/usr/local/bin/kcminit_startup" },
    @{ Name = "probe-source"; Path = "files/arch/usr/local/lib/localdesktop-kcminit-stack-probe.c" },
    @{ Name = "probe-library"; Path = "files/arch/usr/local/lib/localdesktop-kcminit-stack-probe.so" },
    @{ Name = "probe-library-tmp"; Path = "files/arch/usr/local/lib/localdesktop-kcminit-stack-probe.so.tmp" },
    @{ Name = "probe-log"; Path = "files/arch/var/lib/localdesktop/kcminit-stack.log" }
)
$states = @{}
$statesCaptured = $false
$mutationStarted = $false
$launched = $false
$cleanupErrors = [Collections.Generic.List[string]]::new()

function RemoteCommand([string]$Command) {
    $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($Command))
    return "run-as app.polarbear sh -c 'printf %s $encoded | base64 -d | sh'"
}
function GuestText([string]$Command) {
    $remote = RemoteCommand $Command
    $out = & $adb -s $DeviceId shell $remote 2>&1
    $code = $LASTEXITCODE
    if ($code -ne 0) { throw "guest command failed ($code): $Command" }
    return (($out | ForEach-Object { [string]$_ }) -join ([char]10)).Trim()
}
function Guest([string]$Command) {
    $remote = RemoteCommand $Command
    & $adb -s $DeviceId shell $remote > $null
    $code = $LASTEXITCODE
    if ($code -ne 0) { throw "guest command failed ($code): $Command" }
}
function GuestBytes([string]$Path) {
    $lines = & $adb -s $DeviceId exec-out run-as app.polarbear base64 $Path 2>$null
    $code = $LASTEXITCODE
    if ($code -ne 0) { throw "guest read failed ($code): $Path" }
    $text = (($lines | ForEach-Object { [string]$_ }) -join "").Trim()
    try { return ,([Convert]::FromBase64String($text)) } catch { throw "invalid base64 from guest path: $Path" }
}
function PutBytes([byte[]]$Bytes, [string]$Path) {
    $encoded = [Convert]::ToBase64String($Bytes)
    # The redirection is inside guest sh -c and the target is one of the fixed
    # paths in $targets, never process/device output.
    $remote = "run-as app.polarbear sh -c 'base64 -d > $Path'"
    $encoded | & $adb -s $DeviceId shell $remote
    $code = $LASTEXITCODE
    if ($code -ne 0) { throw "guest write failed ($code): $Path" }
}
function PutFile([string]$HostPath, [string]$GuestPath) { PutBytes ([IO.File]::ReadAllBytes($HostPath)) $GuestPath }
function HashBytes([byte[]]$Bytes) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return (($sha.ComputeHash($Bytes) | ForEach-Object { $_.ToString("x2") }) -join "") } finally { $sha.Dispose() }
}
function GuestState([string]$Path) {
    $q = "'$Path'"
    $state = GuestText "if [ -L $q ]; then printf symlink; elif [ -f $q ]; then printf file; elif [ -e $q ]; then printf other; else printf absent; fi"
    if ($state -notin @("file", "absent", "symlink", "other")) { throw "unexpected state '$state' for $Path" }
    return $state
}
function GuestMode([string]$Path) {
    $mode = GuestText "stat -c '%a' '$Path'"
    if ($mode -notmatch '^\d{3,4}$') { throw "could not determine mode for $Path (got '$mode')" }
    return $mode
}
function RemoveGuest([string]$Path) {
    Guest "rm -f '$Path'"
    if ((GuestState $Path) -ne "absent") { throw "guest path remained after removal: $Path" }
}
function SaveCommand([string]$Command, [string]$HostPath, [switch]$AllowFailure) {
    $remote = RemoteCommand $Command
    $out = & $adb -s $DeviceId shell $remote 2>&1
    $code = $LASTEXITCODE
    Set-Content -LiteralPath $HostPath -Value (($out | ForEach-Object { [string]$_ }) -join ([char]10)) -Encoding utf8
    if (-not $AllowFailure -and $code -ne 0) { throw "capture command failed ($code): $Command" }
}
function GuestIdentity([int]$TargetProcessId) {
    $cmd = "if [ -r '/proc/$TargetProcessId/cmdline' ] && [ -r '/proc/$TargetProcessId/maps' ]; then printf 'exe='; readlink '/proc/$TargetProcessId/exe'; printf 'cmdline='; tr '\000' ' ' < '/proc/$TargetProcessId/cmdline'; printf '\n'; printf 'ppid='; sed -n 's/^PPid:[[:space:]]*//p' '/proc/$TargetProcessId/status'; printf 'probe_loaded='; if grep -F 'files/arch/usr/local/lib/localdesktop-kcminit-stack-probe.so' '/proc/$TargetProcessId/maps' >/dev/null 2>&1; then printf 1; else printf 0; fi; printf '\n'; printf 'sigusr2_caught='; if grep -E '^SigCgt:[[:space:]]*[0-9a-fA-F]*[89a-fA-F][0-9a-fA-F]{2}$' '/proc/$TargetProcessId/status' >/dev/null 2>&1; then printf 1; else printf 0; fi; printf '\n'; fi"
    return GuestText $cmd
}
function AddCleanup([string]$Message) { [void]$cleanupErrors.Add($Message) }
function Assert-ActivityOutput([object[]]$Output, [string]$Action) {
    $text = (($Output | ForEach-Object { [string]$_ }) -join "`n")
    if ($text -match '(?im)(^|\s)(error|exception|unknown package|unable to)\b') {
        throw "$Action reported an error: $text"
    }
}

$parentPid = $null
$childPid = $null
try {
    $stateOut = & $adb -s $DeviceId get-state 2>&1
    $stateCode = $LASTEXITCODE
    Set-Content -LiteralPath (Join-Path $runRoot "device-state.txt") -Value (($stateOut | ForEach-Object { [string]$_ }) -join "`n") -Encoding utf8
    if ($stateCode -ne 0) { throw "device unavailable: $DeviceId" }

    # Inspect and backup all targets before any write/removal. Symlinks and
    # non-regular objects are refused rather than followed or deleted.
    foreach ($target in $targets) {
        $state = GuestState $target.Path
        if ($state -in @("symlink", "other")) { throw "refusing pre-existing $state target: $($target.Path)" }
        $entry = [pscustomobject]@{ Name = $target.Name; Path = $target.Path; State = $state; Mode = $null; Backup = $null; Hash = $null }
        if ($state -eq "file") {
            $entry.Mode = GuestMode $target.Path
            $entry.Backup = Join-Path $runRoot ($target.Name + ".before")
            $bytes = GuestBytes $target.Path
            [IO.File]::WriteAllBytes($entry.Backup, $bytes)
            $entry.Hash = HashBytes $bytes
        }
        $states[$target.Name] = $entry
    }
    Set-Content -LiteralPath (Join-Path $runRoot "target-state.txt") -Value (@($states.Values | ForEach-Object { "name=$($_.Name) path=$($_.Path) state=$($_.State) mode=$($_.Mode) sha256=$($_.Hash)" })) -Encoding utf8
    $statesCaptured = $true

    PutFile $probeSource $targets[1].Path
    PutFile $wrapperSource $targets[0].Path
    Guest "chmod 0755 '$($targets[0].Path)'"
    RemoveGuest $targets[2].Path
    RemoveGuest $targets[3].Path
    RemoveGuest $targets[4].Path

    $stop = & $adb -s $DeviceId shell am force-stop app.polarbear 2>&1
    $stopCode = $LASTEXITCODE
    Set-Content -LiteralPath (Join-Path $runRoot "force-stop.txt") -Value (($stop | ForEach-Object { [string]$_ }) -join "`n") -Encoding utf8
    if ($stopCode -ne 0) { throw "initial force-stop failed ($stopCode)" }
    Assert-ActivityOutput $stop "initial force-stop"
    Set-Content -LiteralPath (Join-Path $runRoot "launch-start-utc.txt") -Value ((Get-Date).ToUniversalTime().ToString("o")) -Encoding utf8
    $launch = & $adb -s $DeviceId shell am start -n "app.polarbear/android.app.NativeActivity" 2>&1
    $launchCode = $LASTEXITCODE
    Set-Content -LiteralPath (Join-Path $runRoot "launch.txt") -Value (($launch | ForEach-Object { [string]$_ }) -join "`n") -Encoding utf8
    if ($launchCode -ne 0) { throw "launch failed ($launchCode)" }
    Assert-ActivityOutput $launch "launch"

    # This constructor marker identifies the exec-side parent only. The
    # signal target must be its fork child, not the first marker PID.
    for ($i = 0; $i -lt ($WaitSeconds * 2); $i++) {
        Start-Sleep -Milliseconds 500
        try { $logText = [Text.Encoding]::UTF8.GetString((GuestBytes $targets[4].Path)) } catch { $logText = "" }
        $m = [regex]::Match($logText, "kcminit-stack-probe-start role=exec-parent pid=(\d+)")
        if ($m.Success) { $parentPid = [int]$m.Groups[1].Value; break }
    }
    if (-not $parentPid) { throw "probe parent did not load within $WaitSeconds seconds" }
    Set-Content -LiteralPath (Join-Path $runRoot "probe-parent-pid.txt") -Value $parentPid -Encoding utf8

    $candidateIdentityPath = Join-Path $runRoot "candidate-identities.txt"
    Set-Content -LiteralPath $candidateIdentityPath -Value "candidate identities (parent=$parentPid)" -Encoding utf8
    for ($i = 0; $i -lt ($WaitSeconds * 2); $i++) {
        $rows = & $adb -s $DeviceId shell run-as app.polarbear ps -A -o pid,ppid,stat,comm,args 2>&1
        $rowsCode = $LASTEXITCODE
        if ($rowsCode -ne 0) { throw "ps failed ($rowsCode)" }
        foreach ($line in $rows) {
            $rowText = ([string]$line).Trim()
            if ($rowText -notmatch '^\s*(\d+)\s+(\d+)\s+(.*)$') { continue }
            $candidatePid = [int]$Matches[1]; $candidatePpid = [int]$Matches[2]; $candidateArgs = $Matches[3]
            if ($candidatePid -eq $parentPid -or $candidatePpid -ne $parentPid -or $candidateArgs -notmatch 'kcminit_startup') { continue }
            try {
                $identity = GuestIdentity $candidatePid
                Add-Content -LiteralPath $candidateIdentityPath -Value ("ps_pid=$candidatePid ps_ppid=$candidatePpid args=$candidateArgs`n$identity`n") -Encoding utf8
                # Under PRoot /proc/exe is the loader, while /proc/cmdline
                # retains the guest executable. Require both identities.
                if (($identity -match '(?m)^exe=.*(kcminit_startup|libproot_loader\.so)') -and ($identity -match '(?m)^cmdline=.*kcminit_startup') -and ($identity -match "(?m)^ppid=$parentPid$") -and ($identity -match '(?m)^probe_loaded=1$') -and ($identity -match '(?m)^sigusr2_caught=1$')) {
                    $childPid = $candidatePid
                    Set-Content -LiteralPath (Join-Path $runRoot "probe-child-identity.txt") -Value $identity -Encoding utf8
                    break
                }
            } catch { }
        }
        if ($childPid) { break }
        Start-Sleep -Milliseconds 500
    }
    if (-not $childPid) {
        SaveCommand "ps -A -o pid,ppid,stat,comm,args" (Join-Path $runRoot "processes-no-child.txt") -AllowFailure
        throw "probe-loaded kcminit child not found within $WaitSeconds seconds"
    }
    Set-Content -LiteralPath (Join-Path $runRoot "probe-child-pid.txt") -Value $childPid -Encoding utf8
    SaveCommand "ps -A -o pid,ppid,stat,comm,args" (Join-Path $runRoot "processes-before-signal.txt")
    Set-Content -LiteralPath (Join-Path $runRoot "parent-identity.txt") -Value (GuestIdentity $parentPid) -Encoding utf8
    SaveCommand "cat /proc/$childPid/wchan" (Join-Path $runRoot "wchan-before-signal.txt") -AllowFailure
    SaveCommand "cat /proc/$childPid/syscall" (Join-Path $runRoot "syscall-before-signal.txt") -AllowFailure

    # Identity is rechecked atomically in the guest shell immediately before
    # kill, including parent relation, executable/cmdline, and probe mapping.
    $signalCommand = "if readlink '/proc/$childPid/exe' | grep -E 'kcminit_startup|libproot_loader\.so' >/dev/null 2>&1 && tr '\000' ' ' < '/proc/$childPid/cmdline' | grep -F 'kcminit_startup' >/dev/null 2>&1 && grep -F 'files/arch/usr/local/lib/localdesktop-kcminit-stack-probe.so' '/proc/$childPid/maps' >/dev/null 2>&1 && grep -E '^PPid:[[:space:]]+$parentPid$' '/proc/$childPid/status' >/dev/null 2>&1 && grep -E '^SigCgt:[[:space:]]*[0-9a-fA-F]*[89a-fA-F][0-9a-fA-F]{2}$' '/proc/$childPid/status' >/dev/null 2>&1; then kill -USR2 '$childPid'; else printf 'identity-mismatch\n' >&2; exit 42; fi"
    $signalRemote = RemoteCommand $signalCommand
    $signalOut = & $adb -s $DeviceId shell $signalRemote 2>&1
    $signalCode = $LASTEXITCODE
    Set-Content -LiteralPath (Join-Path $runRoot "signal.txt") -Value (($signalOut | ForEach-Object { [string]$_ }) -join "`n") -Encoding utf8
    if ($signalCode -ne 0) { throw "SIGUSR2 identity validation failed ($signalCode) for PID $childPid" }
    Start-Sleep -Seconds 2
    $stackBytes = GuestBytes $targets[4].Path
    [IO.File]::WriteAllBytes((Join-Path $runRoot "kcminit-stack.log"), $stackBytes)
    SaveCommand "cat /proc/$childPid/wchan" (Join-Path $runRoot "wchan-after-signal.txt") -AllowFailure
    SaveCommand "cat /proc/$childPid/syscall" (Join-Path $runRoot "syscall-after-signal.txt") -AllowFailure
    SaveCommand "cat /proc/$childPid/maps" (Join-Path $runRoot "maps-after-signal.txt") -AllowFailure
    & $adb -s $DeviceId exec-out screencap -p > (Join-Path $runRoot "screenshot.png")
    $screenCode = $LASTEXITCODE
    if ($screenCode -ne 0) { throw "screenshot failed ($screenCode)" }
    Set-Content -LiteralPath (Join-Path $runRoot "result.txt") -Value "signal=SIGUSR2`nprobe_parent_pid=$parentPid`nprobe_child_pid=$childPid`nwrapper_restored=pending" -Encoding utf8
}
finally {
    try {
        $stop = & $adb -s $DeviceId shell am force-stop app.polarbear 2>&1
        $stopCode = $LASTEXITCODE
        Set-Content -LiteralPath (Join-Path $runRoot "force-stop-final.txt") -Value (($stop | ForEach-Object { [string]$_ }) -join "`n") -Encoding utf8
        if ($stopCode -ne 0) { AddCleanup "final force-stop failed ($stopCode)" }
        try { Assert-ActivityOutput $stop "final force-stop" } catch { AddCleanup $_.Exception.Message }
    } catch { AddCleanup "final force-stop exception: $($_.Exception.Message)" }
    if ($statesCaptured) {
        foreach ($entry in $states.Values) {
            try {
                if ($entry.State -eq "file") {
                    PutFile $entry.Backup $entry.Path
                    Guest "chmod $($entry.Mode) '$($entry.Path)'"
                    if ((GuestState $entry.Path) -ne "file") { throw "restored state is not file" }
                    $actual = HashBytes (GuestBytes $entry.Path)
                    if ($actual -ne $entry.Hash) { throw "hash mismatch expected $($entry.Hash), got $actual" }
                    if ((GuestMode $entry.Path) -ne $entry.Mode) { throw "mode mismatch" }
                } else {
                    RemoveGuest $entry.Path
                }
            } catch { AddCleanup "restore failed for $($entry.Path): $($_.Exception.Message)" }
        }
    }
    $ok = $statesCaptured -and $cleanupErrors.Count -eq 0
    $okText = $ok.ToString().ToLowerInvariant()
    $removedText = if ($ok) { "verified" } else { "not-verified" }
    Set-Content -LiteralPath (Join-Path $runRoot "restored.txt") -Value (@("wrapper_restored=$okText", "probe_paths_removed=$removedText", "data_clear=false", "cleanup_errors=$($cleanupErrors.Count)") + @($cleanupErrors)) -Encoding utf8
}
if ($cleanupErrors.Count -ne 0) { throw ("probe cleanup failed: " + ($cleanupErrors -join "; ")) }
Write-Output "kcminit probe complete: $runRoot"
