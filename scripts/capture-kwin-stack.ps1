[CmdletBinding()]
param(
    [string]$DeviceId = "f105b146",
    [string]$RepoRoot = "",
    [string]$ArtifactRoot = "",
    [int]$WaitSeconds = 30,
    [ValidateRange(0, 600)]
    [int]$DelaySeconds = 90
)

# Diagnostic-only KWin probe. It does not install an APK or clear app data.
# The existing KWin wrapper and every probe path are backed up before staging,
# and all touched paths are restored and read-back verified in finally.
$ErrorActionPreference = "Stop"
$adb = Join-Path $env:LOCALAPPDATA "Android\Sdk\platform-tools\adb.exe"
if (-not (Test-Path -LiteralPath $adb)) { throw "adb not found: $adb" }
if ([string]::IsNullOrWhiteSpace($RepoRoot)) { $RepoRoot = Split-Path -Parent $PSScriptRoot }
if ([string]::IsNullOrWhiteSpace($ArtifactRoot)) {
    $ArtifactRoot = Join-Path $RepoRoot ("artifacts\qa\kwin-stack-" + (Get-Date -Format "yyyyMMdd-HHmmss"))
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

$probeSource = Join-Path $RepoRoot "assets\localdesktop-kwin-stack-probe.c"
$wrapperSource = Join-Path $RepoRoot "assets\localdesktop-kwin-stack-wrapper.sh"
if (-not (Test-Path -LiteralPath $probeSource) -or -not (Test-Path -LiteralPath $wrapperSource)) {
    throw "KWin probe assets are missing"
}

$guestWrapper = "files/arch/usr/local/bin/kwin_wayland"
$guestOriginalWrapper = "files/arch/usr/local/lib/localdesktop-kwin-wrapper.before"
$guestSource = "files/arch/usr/local/lib/localdesktop-kwin-stack-probe.c"
$guestLibrary = "files/arch/usr/local/lib/localdesktop-kwin-stack-probe.so"
$guestLibraryTmp = "files/arch/usr/local/lib/localdesktop-kwin-stack-probe.so.tmp"
$guestLog = "files/arch/var/lib/localdesktop/kwin-stack.log"
$guestLaunchConfigDir = "files/arch/etc/localdesktop"
$guestLaunchConfig = "$guestLaunchConfigDir/localdesktop.toml"
$targets = @(
    @{ Name = "kwin_wrapper"; Path = $guestWrapper },
    @{ Name = "original_wrapper_copy"; Path = $guestOriginalWrapper },
    @{ Name = "probe_source"; Path = $guestSource },
    @{ Name = "probe_library"; Path = $guestLibrary },
    @{ Name = "probe_library_tmp"; Path = $guestLibraryTmp },
    @{ Name = "probe_log"; Path = $guestLog },
    # setup_plasma_wayland rewrites kwin_wayland during every provisioning
    # pass.  Keep the probe preload in the launch contract instead of relying
    # on a wrapper that setup is allowed to replace.
    @{ Name = "launch_config"; Path = $guestLaunchConfig }
)
$states = @{}
$statesCaptured = $false
$mutationStarted = $false
$launched = $false
$launchConfigDirState = $null
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
    # Keep guest command output LF-normalized even when this script runs under
    # Windows PowerShell. Several identity records use ^/$ anchors and must
    # not acquire host CRLF separators while crossing the ADB boundary.
    return ((($out | ForEach-Object { [string]$_ }) -join ([char]10)).Replace("`r", "")).Trim()
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
    # A guest process can close an fd between the directory walk and the
    # fdinfo read.  PowerShell 5.1 promotes that remote stderr to a native
    # error while ErrorActionPreference=Stop, even for an intentionally
    # best-effort capture.  Preserve the exit code/output contract but keep
    # this expected /proc race from aborting the diagnostic before SIGUSR2.
    $previousErrorAction = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $out = & $adb -s $DeviceId shell $remote 2>&1
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorAction
    }
    Set-Content -LiteralPath $HostPath -Value (($out | ForEach-Object { [string]$_ }) -join ([char]10)) -Encoding utf8
    if (-not $AllowFailure -and $code -ne 0) { throw "capture command failed ($code): $Command" }
}
function Save-AdbText([string[]]$Arguments, [string]$HostPath, [switch]$AllowFailure) {
    $out = & $adb @Arguments 2>&1
    $code = $LASTEXITCODE
    Set-Content -LiteralPath $HostPath -Value (($out | ForEach-Object { [string]$_ }) -join ([char]10)) -Encoding utf8
    if (-not $AllowFailure -and $code -ne 0) { throw "ADB capture failed ($code): $($Arguments -join ' ')" }
}
function GuestIdentity([int]$TargetProcessId) {
    $cmd = "if [ -r '/proc/$TargetProcessId/cmdline' ] && [ -r '/proc/$TargetProcessId/maps' ]; then printf 'exe='; readlink '/proc/$TargetProcessId/exe'; printf 'cmdline='; tr '\000' ' ' < '/proc/$TargetProcessId/cmdline'; printf '\n'; printf 'ppid='; sed -n 's/^PPid:[[:space:]]*//p' '/proc/$TargetProcessId/status'; printf 'starttime='; cut -d ' ' -f22 '/proc/$TargetProcessId/stat'; printf 'probe_loaded='; if grep -F '$guestLibrary' '/proc/$TargetProcessId/maps' >/dev/null 2>&1; then printf 1; else printf 0; fi; printf '\n'; printf 'sigusr2_caught='; if grep -E '^SigCgt:[[:space:]]*[0-9a-fA-F]*[89a-fA-F][0-9a-fA-F]{2}$' '/proc/$TargetProcessId/status' >/dev/null 2>&1; then printf 1; else printf 0; fi; printf '\n'; fi"
    return GuestText $cmd
}
function AddCleanup([string]$Message) { [void]$cleanupErrors.Add($Message) }
function Assert-ActivityOutput([object[]]$Output, [string]$Action) {
    $text = (($Output | ForEach-Object { [string]$_ }) -join ([Environment]::NewLine))
    if ($text -match '(?im)(^|\s)(error|exception|unknown package|unable to)\b') { throw "$Action reported an error: $text" }
}
function Save-AdbBinary([string[]]$Arguments, [string]$HostPath) {
    $psi = [Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $adb
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $quoted = foreach ($argument in $Arguments) {
        if ($argument -match '[\s"]') { '"' + $argument.Replace('"', '\"') + '"' } else { $argument }
    }
    $psi.Arguments = $quoted -join " "
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $psi
    if (-not $process.Start()) { throw "could not start adb binary capture" }
    $file = [IO.File]::Open($HostPath, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try { $process.StandardOutput.BaseStream.CopyTo($file) } finally { $file.Dispose() }
    $stderr = $process.StandardError.ReadToEnd()
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) { throw "adb binary capture failed ($($process.ExitCode)): $stderr" }
    $magic = [IO.File]::ReadAllBytes($HostPath)
    $expected = [byte[]](137, 80, 78, 71, 13, 10, 26, 10)
    if ($magic.Length -lt $expected.Length) {
        throw "screencap output is not a valid PNG"
    }
    $prefix = [byte[]]$magic[0..($expected.Length - 1)]
    if (-not ([Linq.Enumerable]::SequenceEqual($prefix, $expected))) {
        throw "screencap output is not a valid PNG"
    }
}

try {
    $deviceState = & $adb -s $DeviceId get-state 2>&1
    $deviceCode = $LASTEXITCODE
    Set-Content -LiteralPath (Join-Path $runRoot "device-state.txt") -Value (($deviceState | ForEach-Object { [string]$_ }) -join ([Environment]::NewLine)) -Encoding utf8
    if ($deviceCode -ne 0) { throw "device unavailable: $DeviceId" }

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
    if ($states["kwin_wrapper"].State -ne "file") { throw "existing KWin wrapper is required for an architecture-preserving probe" }

    $nonce = [Guid]::NewGuid().ToString("N")
    $defaultLaunch = "export PIPEWIRE_RUNTIME_DIR=/tmp PULSE_SERVER=unix:/tmp/pulse/native; XDG_RUNTIME_DIR=/tmp WAYLAND_DISPLAY=wayland-0 XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=KDE /usr/local/bin/startplasma-localdesktop 2>&1"
    # Build the probe library before exporting LD_PRELOAD.  The managed setup
    # stage replaces kwin_wayland after this script stages its temporary
    # wrapper, so the launch command itself must compile/load the probe for
    # the real KWin exec.  A failed compile aborts this diagnostic run rather
    # than silently producing an uninstrumented stack.
    $probeLaunch = "if [ ! -r /usr/local/lib/localdesktop-kwin-stack-probe.so ]; then command -v gcc >/dev/null 2>&1 && gcc -shared -fPIC -fno-omit-frame-pointer -O0 -g -Wall -Wextra -o /usr/local/lib/localdesktop-kwin-stack-probe.so.tmp /usr/local/lib/localdesktop-kwin-stack-probe.c -ldl && chmod 0755 /usr/local/lib/localdesktop-kwin-stack-probe.so.tmp && mv -f /usr/local/lib/localdesktop-kwin-stack-probe.so.tmp /usr/local/lib/localdesktop-kwin-stack-probe.so || exit 127; fi; export LOCALDESKTOP_KWIN_STACK_NONCE=$nonce LOCALDESKTOP_KWIN_STACK_LOG=/var/lib/localdesktop/kwin-stack.log LD_PRELOAD=/usr/local/lib/localdesktop-kwin-stack-probe.so; $defaultLaunch"
    $escapedProbeLaunch = $probeLaunch.Replace('\', '\\').Replace('"', '\"')
    $launchConfigEntry = $states["launch_config"]
    $launchConfigDirState = GuestState $guestLaunchConfigDir
    if ($launchConfigDirState -in @("symlink", "other", "file")) {
        throw "refusing unexpected launch config directory state: $launchConfigDirState"
    }
    if ($launchConfigDirState -eq "absent") {
        Guest "mkdir -p '$guestLaunchConfigDir'"
    }
    $launchLine = 'launch = "' + $escapedProbeLaunch + '"'
    if ($launchConfigEntry.State -eq "file") {
        $configText = [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($launchConfigEntry.Backup))
        if ($configText -match '(?m)^\s*launch\s*=') {
            $configText = [Regex]::Replace($configText, '(?m)^\s*launch\s*=.*$', $launchLine, 1)
        } elseif ($configText -match '(?m)^\s*\[command\]\s*$') {
            $configText = [Regex]::Replace($configText, '(?m)^\s*\[command\]\s*$', "[command]`n$launchLine", 1)
        } else {
            $configText = $configText.TrimEnd() + "`n`n[command]`n$launchLine`n"
        }
    } else {
        $configText = "[user]`nusername = `"root`"`n`n[command]`n$launchLine`n"
    }
    PutBytes ([Text.Encoding]::UTF8.GetBytes($configText)) $guestLaunchConfig
    Guest "chmod 0644 '$guestLaunchConfig'"
    $temporaryWrapper = [IO.File]::ReadAllText($wrapperSource).Replace("__LOCALDESKTOP_KWIN_STACK_NONCE__", $nonce)
    $mutationStarted = $true
    PutFile $probeSource $guestSource
    PutFile $states["kwin_wrapper"].Backup $guestOriginalWrapper
    Guest "chmod $($states["kwin_wrapper"].Mode) '$guestOriginalWrapper'"
    PutBytes ([Text.Encoding]::UTF8.GetBytes($temporaryWrapper)) $guestWrapper
    Guest "chmod 0755 '$guestWrapper'"
    RemoveGuest $guestLibrary
    RemoveGuest $guestLibraryTmp
    RemoveGuest $guestLog

    $stop = & $adb -s $DeviceId shell am force-stop app.polarbear 2>&1
    $stopCode = $LASTEXITCODE
    Set-Content -LiteralPath (Join-Path $runRoot "force-stop.txt") -Value (($stop | ForEach-Object { [string]$_ }) -join ([Environment]::NewLine)) -Encoding utf8
    if ($stopCode -ne 0) { throw "initial force-stop failed ($stopCode)" }
    Assert-ActivityOutput $stop "initial force-stop"
    Set-Content -LiteralPath (Join-Path $runRoot "launch-start-utc.txt") -Value ((Get-Date).ToUniversalTime().ToString("o")) -Encoding utf8
    $launch = & $adb -s $DeviceId shell am start -n "app.polarbear/android.app.NativeActivity" 2>&1
    $launchCode = $LASTEXITCODE
    Set-Content -LiteralPath (Join-Path $runRoot "launch.txt") -Value (($launch | ForEach-Object { [string]$_ }) -join ([Environment]::NewLine)) -Encoding utf8
    if ($launchCode -ne 0) { throw "launch failed ($launchCode)" }
    Assert-ActivityOutput $launch "launch"
    $launched = $true

    $candidateIdentityPath = Join-Path $runRoot "candidate-identities.txt"
    Set-Content -LiteralPath $candidateIdentityPath -Value "KWin candidates (nonce=$nonce)" -Encoding utf8
    $kwinPid = $null
    $kwinParentPid = $null
    $kwinStarttime = $null
    for ($i = 0; $i -lt ($WaitSeconds * 2); $i++) {
        $rows = & $adb -s $DeviceId shell run-as app.polarbear ps -A -o pid,ppid,stat,comm,args 2>&1
        $rowsCode = $LASTEXITCODE
        if ($rowsCode -ne 0) { throw "ps failed ($rowsCode)" }
        foreach ($line in $rows) {
            $rowText = ([string]$line).Trim()
            $rowMatch = [regex]::Match($rowText, '^\s*(\d+)\s+(\d+)\s+(.*)$')
            if (-not $rowMatch.Success) { continue }
            $candidatePid = [int]$rowMatch.Groups[1].Value
            $candidatePpid = [int]$rowMatch.Groups[2].Value
            $candidateArgs = $rowMatch.Groups[3].Value
            if ($candidateArgs -notmatch '(?i)kwin_wayland\s+--wayland-fd\s+\d+' -or $candidateArgs -match '(?i)kwin_wayland_wrapper') { continue }
            try {
                $identity = GuestIdentity $candidatePid
                Add-Content -LiteralPath $candidateIdentityPath -Value ("ps_pid=$candidatePid ps_ppid=$candidatePpid args=$candidateArgs" + [Environment]::NewLine + $identity + [Environment]::NewLine) -Encoding utf8
                if (($identity -match '(?m)^exe=.*(kwin_wayland|libproot_loader\.so)$') -and ($identity -match '(?m)^cmdline=/usr/bin/kwin_wayland --wayland-fd \d+ --socket \S+.*\s*$') -and ($identity -match '(?m)^probe_loaded=1$') -and ($identity -match '(?m)^sigusr2_caught=1$')) {
                    $kwinPid = $candidatePid
                    $kwinParentPid = $candidatePpid
                    $startMatch = [regex]::Match($identity, '(?m)^starttime=(\d+)$')
                    if (-not $startMatch.Success) { throw "KWin starttime missing for PID $candidatePid" }
                    $kwinStarttime = $startMatch.Groups[1].Value
                    Set-Content -LiteralPath (Join-Path $runRoot "kwin-identity-before.txt") -Value $identity -Encoding utf8
                    break
                }
            } catch { }
        }
        if ($kwinPid) { break }
        Start-Sleep -Milliseconds 500
    }
    if (-not $kwinPid) {
        SaveCommand "ps -A -o pid,ppid,stat,comm,args" (Join-Path $runRoot "processes-no-kwin.txt") -AllowFailure
        throw "probe-loaded real KWin process not found within $WaitSeconds seconds"
    }
    Set-Content -LiteralPath (Join-Path $runRoot "kwin-pid.txt") -Value $kwinPid -Encoding utf8
    Set-Content -LiteralPath (Join-Path $runRoot "kwin-parent-pid.txt") -Value $kwinParentPid -Encoding utf8
    Set-Content -LiteralPath (Join-Path $runRoot "kwin-starttime.txt") -Value $kwinStarttime -Encoding utf8
    SaveCommand "ps -A -o pid,ppid,stat,comm,args" (Join-Path $runRoot "processes-before-signal.txt")
    $kwinFdCommand = 'ls -l /proc/' + $kwinPid + '/fd; for fd in /proc/' + $kwinPid + '/fd/*; do n=$(basename "$fd"); printf "fd=%s target=" "$n"; readlink "$fd"; cat "/proc/' + $kwinPid + '/fdinfo/$n"; done'
    SaveCommand $kwinFdCommand (Join-Path $runRoot "kwin-fds-before-signal.txt") -AllowFailure
    SaveCommand "cat /proc/net/unix" (Join-Path $runRoot "unix-before-signal.txt") -AllowFailure
    $kcminitRows = & $adb -s $DeviceId shell run-as app.polarbear ps -A -o pid,ppid,stat,comm,args 2>&1
    foreach ($line in $kcminitRows) {
        $rowText = ([string]$line).Trim()
        $rowMatch = [regex]::Match($rowText, '^\s*(\d+)\s+(\d+)\s+(.*)$')
        if ($rowMatch.Success -and $rowMatch.Groups[3].Value -match 'kcminit_startup') {
            $clientPid = $rowMatch.Groups[1].Value
            $clientFdCommand = 'printf "kcminit_pid=' + $clientPid + '\n"; ls -l /proc/' + $clientPid + '/fd; for fd in /proc/' + $clientPid + '/fd/*; do n=$(basename "$fd"); printf "fd=%s target=" "$n"; readlink "$fd"; cat "/proc/' + $clientPid + '/fdinfo/$n"; done'
             SaveCommand $clientFdCommand (Join-Path $runRoot ("kcminit-fds-" + $clientPid + ".txt")) -AllowFailure
        }
    }

    # A stack taken immediately after exec only describes early initialization.
    # Hold the same PID and starttime for a bounded late sample so the capture
    # represents the persistent session state seen after the genuine host
    # presentation window, not a transient startup race.
    Set-Content -LiteralPath (Join-Path $runRoot "delay-plan.txt") -Value @(
        "delay_seconds=$DelaySeconds",
        "delay_started_utc=$((Get-Date).ToUniversalTime().ToString('o'))",
        "target_pid=$kwinPid",
        "target_starttime=$kwinStarttime"
    ) -Encoding utf8
    $delaySamples = [Collections.Generic.List[string]]::new()
    for ($elapsed = 0; $elapsed -lt $DelaySeconds; $elapsed += 5) {
        $sleepSeconds = [Math]::Min(5, $DelaySeconds - $elapsed)
        if ($sleepSeconds -gt 0) { Start-Sleep -Seconds $sleepSeconds }
        $identity = GuestIdentity $kwinPid
        $identityOneLine = $identity.Replace("`r", "").Replace("`n", " ").Trim()
        $delaySamples.Add("elapsed_seconds=$($elapsed + $sleepSeconds) $identityOneLine")
        if (($identity -notmatch "(?m)^starttime=$kwinStarttime$") -or
            ($identity -notmatch "(?m)^ppid=$kwinParentPid$") -or
            ($identity -notmatch '(?m)^probe_loaded=1$') -or
            ($identity -notmatch '(?m)^sigusr2_caught=1$')) {
            throw "KWin PID identity changed during delayed capture: $kwinPid"
        }
    }
    Set-Content -LiteralPath (Join-Path $runRoot "delay-samples.txt") -Value $delaySamples -Encoding utf8
    Set-Content -LiteralPath (Join-Path $runRoot "delay-complete-utc.txt") -Value ((Get-Date).ToUniversalTime().ToString("o")) -Encoding utf8
    Set-Content -LiteralPath (Join-Path $runRoot "kwin-identity-after-delay.txt") -Value (GuestIdentity $kwinPid) -Encoding utf8
    SaveCommand "ps -A -o pid,ppid,stat,comm,args" (Join-Path $runRoot "processes-after-delay.txt")
    SaveCommand "cat /proc/$kwinPid/wchan" (Join-Path $runRoot "wchan-before-signal-after-delay.txt") -AllowFailure
    SaveCommand "cat /proc/$kwinPid/syscall" (Join-Path $runRoot "syscall-before-signal-after-delay.txt") -AllowFailure
    SaveCommand "cat /proc/net/unix" (Join-Path $runRoot "unix-before-signal-after-delay.txt") -AllowFailure
    # Correlate the late KWin sample with the client that was observed blocked
    # in the earlier kcminit trace. Capture every current kcminit process,
    # including its thread table, wait channel, syscall and Wayland fds. This
    # is observation-only and remains app-UID/guest-root scoped.
    $lateKcminitRows = & $adb -s $DeviceId shell run-as app.polarbear ps -A -o pid,ppid,stat,comm,args 2>&1
    $lateKcminitCode = $LASTEXITCODE
    Set-Content -LiteralPath (Join-Path $runRoot "kcminit-processes-after-delay.txt") -Value (($lateKcminitRows | ForEach-Object { [string]$_ }) -join ([char]10)) -Encoding utf8
    if ($lateKcminitCode -ne 0) { throw "late kcminit process capture failed ($lateKcminitCode)" }
    foreach ($line in $lateKcminitRows) {
        $rowText = ([string]$line).Trim()
        $rowMatch = [regex]::Match($rowText, '^\s*(\d+)\s+(\d+)\s+(.*)$')
        if (-not $rowMatch.Success -or $rowMatch.Groups[3].Value -notmatch 'kcminit_startup') { continue }
        $lateClientPid = [int]$rowMatch.Groups[1].Value
        $lateClientCommand = 'printf "kcminit_pid=' + $lateClientPid + '\n"; cat /proc/' + $lateClientPid + '/wchan; printf "syscall="; cat /proc/' + $lateClientPid + '/syscall; printf "threads=\n"; ps -T -p ' + $lateClientPid + ' -o pid,tid,stat,wchan,comm,args; printf "fds=\n"; ls -l /proc/' + $lateClientPid + '/fd; for fd in /proc/' + $lateClientPid + '/fd/*; do n=$(basename "$fd"); printf "fd=%s target=" "$n"; readlink "$fd"; cat "/proc/' + $lateClientPid + '/fdinfo/$n"; done'
        SaveCommand $lateClientCommand (Join-Path $runRoot ("kcminit-late-" + $lateClientPid + ".txt")) -AllowFailure
    }
    # These are app-UID logs only; no global logcat clear or unrelated process
    # inspection is performed. They correlate hostPresented/output title events
    # with the exact late KWin PID and guest generation.
    Save-AdbText @("-s", $DeviceId, "logcat", "--uid=10487", "-d", "-v", "threadtime") (Join-Path $runRoot "host-log-after-delay.txt") -AllowFailure
    foreach ($guestName in @("plasma.log", "kwin.log", "guest.log", "plasma-ready", "plasma-failed", "kwin-crash")) {
        $guestPath = "files/arch/var/lib/localdesktop/$guestName"
        try {
            [IO.File]::WriteAllBytes((Join-Path $runRoot ("guest-$guestName")), (GuestBytes $guestPath))
        } catch {
            Set-Content -LiteralPath (Join-Path $runRoot ("guest-$guestName.missing")) -Value $_.Exception.Message -Encoding utf8
        }
    }

    $signalCommand = "if readlink '/proc/$kwinPid/exe' | grep -E 'kwin_wayland|libproot_loader\.so' >/dev/null 2>&1 && tr '\000' ' ' < '/proc/$kwinPid/cmdline' | grep -E '^/usr/bin/kwin_wayland --wayland-fd [0-9]+ --socket [^ ]+' >/dev/null 2>&1 && grep -F '$guestLibrary' '/proc/$kwinPid/maps' >/dev/null 2>&1 && grep -E '^PPid:[[:space:]]+$kwinParentPid$' '/proc/$kwinPid/status' >/dev/null 2>&1 && cut -d ' ' -f22 '/proc/$kwinPid/stat' | grep -Fx '$kwinStarttime' >/dev/null 2>&1 && grep -E '^SigCgt:[[:space:]]*[0-9a-fA-F]*[89a-fA-F][0-9a-fA-F]{2}$' '/proc/$kwinPid/status' >/dev/null 2>&1; then kill -USR2 '$kwinPid'; else printf 'identity-mismatch\n' >&2; exit 42; fi"
    $signalRemote = RemoteCommand $signalCommand
    $signalOut = & $adb -s $DeviceId shell $signalRemote 2>&1
    $signalCode = $LASTEXITCODE
    Set-Content -LiteralPath (Join-Path $runRoot "signal.txt") -Value (($signalOut | ForEach-Object { [string]$_ }) -join ([Environment]::NewLine)) -Encoding utf8
    if ($signalCode -ne 0) { throw "SIGUSR2 identity validation failed ($signalCode) for PID $kwinPid" }
    Start-Sleep -Seconds 2
    $stackBytes = GuestBytes $guestLog
    [IO.File]::WriteAllBytes((Join-Path $runRoot "kwin-stack.log"), $stackBytes)
    $stackText = [Text.Encoding]::UTF8.GetString($stackBytes)
    if ($stackText -notmatch "kwin-stack-probe signal=12 pid=$kwinPid") { throw "no PID-specific SIGUSR2 record for KWin $kwinPid" }
    foreach ($marker in @("maps_begin", "maps_end", "backtrace_begin", "backtrace_end")) {
        if ($stackText -notmatch [regex]::Escape($marker)) { throw "KWin stack capture is incomplete: $marker" }
    }
    SaveCommand "cat /proc/$kwinPid/wchan" (Join-Path $runRoot "wchan-after-signal.txt") -AllowFailure
    SaveCommand "cat /proc/$kwinPid/syscall" (Join-Path $runRoot "syscall-after-signal.txt") -AllowFailure
    $kwinFdCommandAfter = 'ls -l /proc/' + $kwinPid + '/fd; for fd in /proc/' + $kwinPid + '/fd/*; do n=$(basename "$fd"); printf "fd=%s target=" "$n"; readlink "$fd"; cat "/proc/' + $kwinPid + '/fdinfo/$n"; done'
    SaveCommand $kwinFdCommandAfter (Join-Path $runRoot "kwin-fds-after-signal.txt") -AllowFailure
    SaveCommand "cat /proc/net/unix" (Join-Path $runRoot "unix-after-signal.txt") -AllowFailure
    SaveCommand "cat /proc/$kwinPid/maps" (Join-Path $runRoot "kwin-maps-after-signal.txt") -AllowFailure
    Save-AdbBinary @("-s", $DeviceId, "exec-out", "screencap", "-p") (Join-Path $runRoot "screenshot.png")
    Set-Content -LiteralPath (Join-Path $runRoot "result.txt") -Value @("signal=SIGUSR2", "kwin_pid=$kwinPid", "kwin_parent_pid=$kwinParentPid", "kwin_starttime=$kwinStarttime", "delay_seconds=$DelaySeconds", "stack_complete=true", "cleanup=pending") -Encoding utf8
}
finally {
    if ($mutationStarted -or $launched) {
        try {
            $stop = & $adb -s $DeviceId shell am force-stop app.polarbear 2>&1
            $stopCode = $LASTEXITCODE
            Set-Content -LiteralPath (Join-Path $runRoot "force-stop-final.txt") -Value (($stop | ForEach-Object { [string]$_ }) -join ([Environment]::NewLine)) -Encoding utf8
            if ($stopCode -ne 0) { AddCleanup "final force-stop failed ($stopCode)" }
            try { Assert-ActivityOutput $stop "final force-stop" } catch { AddCleanup $_.Exception.Message }
        } catch { AddCleanup "final force-stop exception: $($_.Exception.Message)" }
    }
    if ($statesCaptured) {
        foreach ($entry in $states.Values) {
            try {
                $current = GuestState $entry.Path
                if ($current -in @("symlink", "other")) { throw "refusing restore over current $current object" }
                if ($entry.State -eq "file") {
                    PutFile $entry.Backup $entry.Path
                    Guest "chmod $($entry.Mode) '$($entry.Path)'"
                    if ((GuestState $entry.Path) -ne "file") { throw "restored state is not file" }
                    $actualHash = HashBytes (GuestBytes $entry.Path)
                    if ($actualHash -ne $entry.Hash) { throw "hash mismatch expected $($entry.Hash), got $actualHash" }
                    if ((GuestMode $entry.Path) -ne $entry.Mode) { throw "mode mismatch" }
                } else {
                    RemoveGuest $entry.Path
                }
            } catch { AddCleanup "restore failed for $($entry.Path): $($_.Exception.Message)" }
        }
    }
    if ($launchConfigDirState -eq "absent") {
        try {
            $currentDirState = GuestState $guestLaunchConfigDir
            if ($currentDirState -eq "absent") {
                # Already restored by the guest cleanup; keep this check
                # explicit so an unexpected setup-created directory is not
                # mistaken for a clean restoration.
            } elseif ($currentDirState -eq "other") {
                $remaining = GuestText "find '$guestLaunchConfigDir' -mindepth 1 -maxdepth 1 -print -quit"
                if ([string]::IsNullOrWhiteSpace($remaining)) {
                    Guest "rmdir '$guestLaunchConfigDir'"
                    if ((GuestState $guestLaunchConfigDir) -ne "absent") {
                        throw "launch config directory remained after empty-directory removal"
                    }
                } else {
                    throw "launch config directory contains unexpected entry: $remaining"
                }
            } else {
                throw "launch config directory restored to unexpected state: $currentDirState"
            }
        } catch { AddCleanup "launch config directory restore failed: $($_.Exception.Message)" }
    }
    $ok = $statesCaptured -and $cleanupErrors.Count -eq 0
    $okText = $ok.ToString().ToLowerInvariant()
    $removedText = if ($ok) { "verified" } else { "not-verified" }
    Set-Content -LiteralPath (Join-Path $runRoot "restored.txt") -Value (@("wrapper_restored=$okText", "probe_paths_removed=$removedText", "data_clear=false", "cleanup_errors=$($cleanupErrors.Count)") + @($cleanupErrors)) -Encoding utf8
}
if ($cleanupErrors.Count -ne 0) { throw ("KWin probe cleanup failed: " + ($cleanupErrors -join "; ")) }
Set-Content -LiteralPath (Join-Path $runRoot "result.txt") -Value @("signal=SIGUSR2", "kwin_pid=$kwinPid", "kwin_parent_pid=$kwinParentPid", "kwin_starttime=$kwinStarttime", "delay_seconds=$DelaySeconds", "stack_complete=true", "cleanup=verified") -Encoding utf8
Write-Output "KWin probe complete: $runRoot"
