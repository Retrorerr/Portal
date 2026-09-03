# Host-only behavioral checks for capture-kcminit-stack.ps1.
#
# This file deliberately extracts only function-definition ASTs.  It never
# dot-sources or invokes the probe script's top-level code, so the test cannot
# reach a real adb executable or a device.

$ErrorActionPreference = "Stop"

function Assert-Test([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

function Assert-ByteSequenceEqual([byte[]]$Actual, [byte[]]$Expected, [string]$Message) {
    $same = ($null -ne $Actual -and $null -ne $Expected -and $Actual.Length -eq $Expected.Length)
    if ($same) {
        for ($i = 0; $i -lt $Expected.Length; $i++) {
            if ($Actual[$i] -ne $Expected[$i]) { $same = $false; break }
        }
    }
    Assert-Test $same $Message
}

function Get-CaptureScriptText([string]$ScriptName) {
    $path = Join-Path $PSScriptRoot ("..\scripts\" + $ScriptName)
    return [IO.File]::ReadAllText([IO.Path]::GetFullPath($path))
}

function Get-ProbeScriptText {
    return Get-CaptureScriptText "capture-kcminit-stack.ps1"
}

function Get-CaptureFunctionAst([string]$ScriptName, [string]$Name) {
    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseInput(
        (Get-CaptureScriptText $ScriptName), [ref]$tokens, [ref]$errors)
    Assert-Test ($errors.Count -eq 0) "$ScriptName does not parse: $($errors | Out-String)"
    $function = $ast.Find({ param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq $Name
    }, $true)
    Assert-Test ($null -ne $function) "function '$Name' was not found in $ScriptName"
    return $function
}

function Get-ProbeFunctionAst([string]$Name) {
    return Get-CaptureFunctionAst "capture-kcminit-stack.ps1" $Name
}

function Get-ProbeFunctionText([string]$Name) {
    return (Get-ProbeFunctionAst $Name).Extent.Text
}

function Install-CaptureFunction([string]$ScriptName, [string]$Name) {
    $function = Get-CaptureFunctionAst $ScriptName $Name
    $parameterText = if ($function.Parameters.Count -gt 0) {
        "param(" + (($function.Parameters | ForEach-Object { $_.Extent.Text }) -join ",") + ")"
    } else { "" }
    $bodyText = $function.Body.Extent.Text
    $bodyText = $bodyText.Substring(1, $bodyText.Length - 2)
    $definitionText = $parameterText + "`n" + $bodyText
    # The test copy is the production function body, with only process and
    # device dependencies redirected to the in-memory harness.  No top-level
    # capture code (ADB, APK, staging, or cleanup) is executed.
    $definitionText = $definitionText -replace '\$adb\b', '$global:ProbeAdb'
    $definitionText = $definitionText -replace '\$DeviceId\b', '$global:ProbeDeviceId'
    $definitionText = $definitionText -replace '\$guestLibrary\b', '$global:ProbeGuestLibrary'
    Set-Item -LiteralPath ("Function:\script:" + $Name) -Value ([scriptblock]::Create($definitionText))
}

function Get-CaptureFinallyAst([string]$ScriptName) {
    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseInput(
        (Get-CaptureScriptText $ScriptName), [ref]$tokens, [ref]$errors)
    Assert-Test ($errors.Count -eq 0) "$ScriptName does not parse: $($errors | Out-String)"
    $finally = $ast.Find({ param($node)
        $node -is [System.Management.Automation.Language.TryStatementAst] -and $null -ne $node.Finally
    }, $true) | Select-Object -First 1
    Assert-Test ($null -ne $finally) "finally block was not found in $ScriptName"
    return $finally.Finally
}

function Invoke-CaptureFinally([string]$ScriptName, [string]$RunRoot) {
    $finally = Get-CaptureFinallyAst $ScriptName
    $bodyText = $finally.Extent.Text
    $definitionText = "param()`n" + $bodyText
    # Redirect only the test's dependencies.  This executes the production
    # finally block with an empty preflight state and a mock adb, so a
    # preflight exception can be proven not to stop a user's running app.
    $definitionText = $definitionText -replace '\$adb\b', '$global:ProbeAdb'
    $definitionText = $definitionText -replace '\$DeviceId\b', '$global:ProbeDeviceId'
    $definitionText = $definitionText -replace '\$statesCaptured\b', '$global:ProbeStatesCaptured'
    $definitionText = $definitionText -replace '\$mutationStarted\b', '$global:ProbeMutationStarted'
    $definitionText = $definitionText -replace '\$launched\b', '$global:ProbeLaunched'
    $definitionText = $definitionText -replace '\$cleanupErrors\b', '$global:ProbeCleanupErrors'
    $definitionText = $definitionText -replace '\$states\b', '$global:ProbeStates'
    $definitionText = $definitionText -replace '\$runRoot\b', '$global:ProbeRunRoot'
    $definitionText = $definitionText -replace '\$targets\b', '$global:ProbeTargets'
    Set-Item -LiteralPath Function:\script:Invoke-ProductionFinally -Value ([scriptblock]::Create($definitionText))
    $global:ProbeStatesCaptured = $false
    $global:ProbeMutationStarted = $false
    $global:ProbeLaunched = $false
    $global:ProbeCleanupErrors = [Collections.Generic.List[string]]::new()
    $global:ProbeStates = @{}
    $global:ProbeTargets = @()
    $global:ProbeRunRoot = $RunRoot
    Invoke-ProductionFinally
}

function Invoke-ProductionArtifactRoot([string]$ScriptName, [string]$RepoRoot) {
    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseInput(
        (Get-CaptureScriptText $ScriptName), [ref]$tokens, [ref]$errors)
    Assert-Test ($errors.Count -eq 0) "$ScriptName does not parse: $($errors | Out-String)"
    $assignment = $ast.Find({ param($node)
        $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and
            $node.Left.Extent.Text -eq '$ArtifactRoot'
    }, $true) | Select-Object -First 1
    Assert-Test ($null -ne $assignment) "artifact-root assignment was not found in $ScriptName"
    $assignmentText = $assignment.Extent.Text -replace '\(Get-Date -Format "[^"]+"\)', '$global:ProbeFormattedDate'
    $definition = [scriptblock]::Create("param([string]`$RepoRoot)`n$assignmentText`nreturn `$ArtifactRoot")
    Set-Item -LiteralPath Function:\script:Invoke-GeneratedArtifactRoot -Value $definition
    $global:ProbeFormattedDate = "fixed-clock"
    return Invoke-GeneratedArtifactRoot -RepoRoot $RepoRoot
}

function New-HostHarness {
    # These are script-scoped because extracted functions resolve variables
    # dynamically when called.  The mock adb is a scriptblock, not a process.
    $script:DeviceId = "mock-device"
    $script:AdbCalls = [Collections.Generic.List[object]]::new()
    $script:MockMode = "happy"
    $script:MockIdentity = @{}
    $script:MockBytes = @{}
    $script:MockStates = @{}
    $script:MockModes = @{}
    $script:MockSignalLog = [Text.Encoding]::UTF8.GetBytes("")
    $script:MockScreen = [byte[]](0x89, 0x50, 0x4e, 0x47)
    $script:MockFinalStopCode = 0
    $script:MockLaunchStarted = $false
    $script:MockKillSeen = $false

    # The extracted Guest*/Put*/Save* functions call & $adb.  A scriptblock
    # works with PowerShell's call operator and lets tests decode the exact
    # guest command without starting adb.
    $script:adb = {
        # Use automatic $args/$input so both native-style arguments and the
        # base64 body piped by PutBytes are observable without a process.
        $argv = @($args | ForEach-Object { [string]$_ })
        $stdin = @($input | ForEach-Object { [string]$_ }) -join ""
        $global:LASTEXITCODE = 0
        [void]$script:AdbCalls.Add($argv)

        $isExecOut = $argv -contains "exec-out"
        $isShell = $argv -contains "shell"
        $remote = if ($isShell) {
            $index = [Array]::IndexOf($argv, "shell")
            if ($index -ge 0 -and $index + 1 -lt $argv.Count) { $argv[$index + 1] } else { "" }
        } else { "" }

        if ($isExecOut -and ($argv -contains "screencap")) {
            if ($script:MockMode -eq "empty-screen") { return }
            [Console]::OpenStandardOutput().Write($script:MockScreen, 0, $script:MockScreen.Length)
            return
        }

        if ($isExecOut -and ($argv -contains "base64")) {
            $path = $argv[$argv.Count - 1]
            $bytes = if ($script:MockBytes.ContainsKey($path)) { $script:MockBytes[$path] } else { [byte[]]@() }
            return [Convert]::ToBase64String($bytes)
        }

        if ($isShell -and $remote -match "am force-stop") {
            if ($script:MockFinalStopCode -ne 0) {
                $global:LASTEXITCODE = $script:MockFinalStopCode
                return "force-stop failed"
            }
            return ""
        }

        if ($isShell -and $remote -match "am start") {
            $script:MockLaunchStarted = $true
            return "Starting: Intent { cmp=app.polarbear/.NativeActivity }"
        }

        if ($isShell -and $remote -match "ps -A") {
            return "PID PPID STAT COMM ARGS`n410 1 S kcminit_startup /usr/bin/kcminit_startup`n411 410 S kcminit_startup /usr/bin/kcminit_startup"
        }

        if ($isShell -and $remote -match "printf %s ([A-Za-z0-9+/=]+)") {
            $encoded = $Matches[1]
            $command = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($encoded))
            if ($command -match "kill -USR2") {
                $script:MockKillSeen = $true
                if ($script:MockMode -eq "signal-mismatch") {
                    $global:LASTEXITCODE = 42
                    return "identity-mismatch"
                }
                return ""
            }
            if ($command -match "cat /proc/(\d+)/wchan") { return "poll_schedule_timeout" }
            if ($command -match "cat /proc/(\d+)/syscall") { return "0 0 0 0" }
            if ($command -match "cat /proc/(\d+)/maps") { return "maps" }
            if ($command -match "stat -c '%a' '([^']+)'") { return $script:MockModes[$Matches[1]] }
            if ($command -match "chmod (\d+) '([^']+)'") {
                $script:MockModes[$Matches[2]] = $Matches[1]
                return ""
            }
            if ($command -match "rm -f '([^']+)'") {
                $path = $Matches[1]
                $script:MockStates[$path] = "absent"
                $script:MockBytes.Remove($path)
                $script:MockModes.Remove($path)
                return ""
            }
            if ($command -match "if \[ -L '([^']+)' \]") {
                $path = $Matches[1]
                return $(if ($script:MockStates[$path] -eq "symlink") { "symlink" } elseif ($script:MockStates[$path] -eq "file") { "file" } elseif ($script:MockStates[$path] -eq "other") { "other" } else { "absent" })
            }
            if ($command -match "readlink '/proc/(\d+)/exe'") {
                $targetPid = [int]$Matches[1]
                # Interpret the small, deterministic /proc command generated
                # by the production GuestIdentity function.  In particular,
    # printf output is accumulated exactly as a shell would, so
                # a missing separator in production becomes a malformed
                # identity here instead of being hidden by a canned string.
                return Invoke-FakeGuestIdentityCommand -Command $command -TargetPid $targetPid
            }
            return ""
        }

        if ($isShell -and $remote -match "base64 -d > ([^\s']+)") {
            $path = $Matches[1]
            $script:MockBytes[$path] = [Convert]::FromBase64String($stdin.Trim())
            $script:MockStates[$path] = "file"
            return ""
        }

        return ""
    }
    # FunctionDefinitionAst instances retain their own script scope when they
    # are installed from this helper. Mirror the capture script's unscoped
    # variables in global scope so the extracted functions resolve the mock,
    # while the top-level capture script remains completely unexecuted.
    $global:ProbeAdb = $script:adb
    $global:ProbeDeviceId = $script:DeviceId
    $global:ProbeGuestLibrary = "files/arch/usr/local/lib/localdesktop-kcminit-stack-probe.so"

    # Extract and define only the functions under test.  The source file's
    # parameter block, adb lookup, staging, launch, and finally block remain
    # unexecuted.
    foreach ($name in @("RemoteCommand", "GuestText", "Guest", "GuestBytes", "PutBytes", "PutFile", "HashBytes", "GuestState", "GuestMode", "RemoveGuest", "SaveCommand", "GuestIdentity", "AddCleanup", "Assert-ActivityOutput")) {
        Install-CaptureFunction "capture-kcminit-stack.ps1" $name
    }
}

function Invoke-FakeGuestIdentityCommand([string]$Command, [int]$TargetPid) {
    $proc = $script:MockIdentity[$TargetPid]
    if ($null -eq $proc) {
        $proc = [pscustomobject]@{
            Exe = "/usr/bin/kcminit_startup"
            Cmdline = "/usr/bin/kcminit_startup --wait"
            Ppid = "410"
            ProbeLoaded = "1"
            Sigusr2Caught = "1"
            Starttime = "12345"
        }
    }
    $output = [Text.StringBuilder]::new()
    # Each append below corresponds to a command fragment present in the
    # production function.  Newlines are only emitted when the production
    # command actually contains printf '\n' at that position.
    [void]$output.Append("exe=")
    [void]$output.Append([string]$proc.Exe)
    # readlink writes a trailing newline just like the Android toybox
    # command used by the production probe.
    [void]$output.Append([char]10)
    [void]$output.Append("cmdline=")
    [void]$output.Append(([string]$proc.Cmdline).Replace([char]0, [char]32))
    if ($Command -match "printf '\\n'; printf 'ppid='") { [void]$output.Append([char]10) }
    [void]$output.Append("ppid=")
    [void]$output.Append([string]$proc.Ppid)
    # sed emits the selected status line with its own newline.
    [void]$output.Append([char]10)
    if ($Command -match "printf 'starttime='") {
        [void]$output.Append("starttime=")
        [void]$output.Append([string]$proc.Starttime)
        # cut emits the selected field with a trailing newline.
        [void]$output.Append([char]10)
    }
    if ($Command -match "printf '\\n'; printf 'probe_loaded='") { [void]$output.Append([char]10) }
    [void]$output.Append("probe_loaded=")
    [void]$output.Append([string]$proc.ProbeLoaded)
    if ($Command -match "printf '\\n'; printf 'sigusr2_caught='") { [void]$output.Append([char]10) }
    [void]$output.Append("sigusr2_caught=")
    [void]$output.Append([string]$proc.Sigusr2Caught)
    if ($Command -match "sigusr2_caught=.*?printf '\\n'; fi") { [void]$output.Append([char]10) }
    return $output.ToString()
}

function Test-IdentityFieldBoundaries {
    New-HostHarness
    $script:MockIdentity[411] = [pscustomobject]@{
        Exe = "/usr/bin/kcminit_startup"
        Cmdline = "/usr/bin/kcminit_startup --wait"
        Ppid = "410"
        ProbeLoaded = "1"
        Sigusr2Caught = "1"
        Starttime = "12345"
    }
    $identity = (@(GuestIdentity 411) -join "`n").Trim()
    $expected = @(
        "exe=/usr/bin/kcminit_startup",
        "cmdline=/usr/bin/kcminit_startup --wait",
        "ppid=410",
        "probe_loaded=1",
        "sigusr2_caught=1"
    ) -join "`n"
    Assert-Test ($identity -eq $expected) "GuestIdentity emitted malformed field boundaries: [$identity]"
}

function Test-KWinIdentityFieldBoundaries {
    New-HostHarness
    $script:ProbeGuestLibrary = "files/arch/usr/local/lib/localdesktop-kwin-stack-probe.so"
    Install-CaptureFunction "capture-kwin-stack.ps1" "GuestIdentity"
    $script:MockIdentity[811] = [pscustomobject]@{
        Exe = "/usr/bin/kwin_wayland"
        Cmdline = "/usr/bin/kwin_wayland --wayland-fd 17 --socket wayland-1"
        Ppid = "700"
        ProbeLoaded = "1"
        Sigusr2Caught = "1"
        Starttime = "98765"
    }
    $identity = (@(GuestIdentity 811) -join "`n").Trim()
    $expected = @(
        "exe=/usr/bin/kwin_wayland",
        "cmdline=/usr/bin/kwin_wayland --wayland-fd 17 --socket wayland-1",
        "ppid=700",
        "starttime=98765",
        "probe_loaded=1",
        "sigusr2_caught=1"
    ) -join "`n"
    Assert-Test ($identity -eq $expected) "KWin GuestIdentity emitted malformed field boundaries: [$identity]"
}

function Test-ReservedPidAssignmentIsAbsent {
    $source = Get-ProbeScriptText
    Assert-Test ($source -notmatch '(?im)^\s*\$pid\s*=') "probe script assigns reserved PowerShell `$PID"
}

function Test-RestoreBytesAndModes {
    New-HostHarness
    $path = "files/arch/usr/local/bin/kcminit_startup"
    $before = [Text.Encoding]::UTF8.GetBytes("original wrapper")
    $script:MockBytes[$path] = $before
    $script:MockStates[$path] = "file"
    $script:MockModes[$path] = "750"
    $backup = Join-Path ([IO.Path]::GetTempPath()) ("kcminit-before-" + [Guid]::NewGuid().ToString("N"))
    [IO.File]::WriteAllBytes($backup, $before)
    try {
        $script:entry = [pscustomobject]@{ Name = "kcminit_startup"; Path = $path; State = "file"; Mode = "750"; Backup = $backup; Hash = (HashBytes $before) }
        $script:states = @{ kcminit_startup = $entry }
        PutFile $backup $path
        Guest "chmod 750 '$path'"
        Assert-Test ((HashBytes $script:MockBytes[$path]) -eq $entry.Hash) "restored bytes changed"
        Assert-Test ($script:MockModes[$path] -eq "750") "restored mode changed"
    } finally {
        if (Test-Path -LiteralPath $backup) { Remove-Item -LiteralPath $backup -Force }
    }
}

function Test-PreflightDoesNotStopApp {
    $root = Join-Path ([IO.Path]::GetTempPath()) ("capture-finally-" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $root -Force | Out-Null
    try {
        New-HostHarness
        $before = $script:AdbCalls.Count
        Invoke-CaptureFinally "capture-kcminit-stack.ps1" $root
        Assert-Test ($script:AdbCalls.Count -eq $before) "kcminit preflight cleanup invoked adb before launch/mutation"

        $before = $script:AdbCalls.Count
        Invoke-CaptureFinally "capture-kwin-stack.ps1" $root
        Assert-Test ($script:AdbCalls.Count -eq $before) "KWin preflight cleanup invoked adb before launch/mutation"
    } finally {
        if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
    }
}

function Test-ArtifactFreshnessContract {
    $root = Join-Path ([IO.Path]::GetTempPath()) ("capture-artifact-" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $root -Force | Out-Null
    try {
        # Freeze the clock while invoking the production default expression.
        # If two automatic runs in the same formatted second collide, this
        # test reproduces that collision deterministically.
        $first = Invoke-ProductionArtifactRoot "capture-kcminit-stack.ps1" $root
        $second = Invoke-ProductionArtifactRoot "capture-kcminit-stack.ps1" $root
        Assert-Test ($first -ne $second) "kcminit artifact roots collide for the same clock tick: $first"

        $first = Invoke-ProductionArtifactRoot "capture-kwin-stack.ps1" $root
        $second = Invoke-ProductionArtifactRoot "capture-kwin-stack.ps1" $root
        Assert-Test ($first -ne $second) "KWin artifact roots collide for the same clock tick: $first"
    } finally {
        if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
    }
}

function Test-SignalEvidenceContract {
    $source = Get-ProbeScriptText
    Assert-Test ($source -match 'kill -USR2') "SIGUSR2 action disappeared"
    Assert-Test ($source -match 'probe-child-pid\.txt') "child PID evidence disappeared"
    # A signal exit code is not handler evidence. Require the capture script
    # to validate a PID-specific signal record and complete map/backtrace
    # sections before reporting a result.
    Assert-Test ($source -match '(?i)kcminit-stack-probe signal=.*pid=') "PID-specific signal evidence check missing"
    Assert-Test ($source -match 'backtrace_begin.*backtrace_end|maps_begin.*maps_end') "complete stack evidence check missing"
    Assert-Test ($source.IndexOf('kcminit-stack-probe signal=') -lt $source.IndexOf('result.txt')) "result is written before signal evidence is validated"
}

function New-FakeAdbBinary {
    param(
        [Parameter(Mandatory)][string]$PayloadPath,
        [Parameter(Mandatory)][string]$Root
    )
    # Save-AdbBinary starts a native process.  This is a deliberately named,
    # local fixture command, never the Android SDK adb executable.  The child
    # PowerShell writes the payload as bytes to stdout, which lets the
    # production capture path be tested end to end without a device.
    $path = Join-Path $Root "fake-adb.cmd"
    $body = @(
        "@echo off",
        'powershell.exe -NoLogo -NoProfile -NonInteractive -Command "$b=[IO.File]::ReadAllBytes($env:LOCALDESKTOP_PROBE_PAYLOAD); [Console]::OpenStandardOutput().Write($b,0,$b.Length)"',
        "exit /b %ERRORLEVEL%"
    ) -join "`r`n"
    [IO.File]::WriteAllText($path, $body, [Text.Encoding]::ASCII)
    return $path
}

function Invoke-ProductionPngCapture {
    param(
        [Parameter(Mandatory)][byte[]]$Payload,
        [Parameter(Mandatory)][string]$Root
    )
    $payloadPath = Join-Path $Root "adb-payload.bin"
    $outputPath = Join-Path $Root "screenshot.png"
    [IO.File]::WriteAllBytes($payloadPath, $Payload)
    $fakeAdb = New-FakeAdbBinary -PayloadPath $payloadPath -Root $Root
    $previousPayload = $env:LOCALDESKTOP_PROBE_PAYLOAD
    $env:LOCALDESKTOP_PROBE_PAYLOAD = $payloadPath
    try {
        $global:ProbeAdb = $fakeAdb
        Install-CaptureFunction "capture-kwin-stack.ps1" "Save-AdbBinary"
        Save-AdbBinary @("-s", "mock-device", "exec-out", "screencap", "-p") $outputPath
        return [IO.File]::ReadAllBytes($outputPath)
    } finally {
        if ($null -eq $previousPayload) { Remove-Item Env:LOCALDESKTOP_PROBE_PAYLOAD -ErrorAction SilentlyContinue }
        else { $env:LOCALDESKTOP_PROBE_PAYLOAD = $previousPayload }
    }
}

function Test-BinaryScreenshotContract {
    # Exercise the production Save-AdbBinary function through a fake native
    # executable.  This catches both PowerShell's text-redirection corruption
    # and an over-permissive PNG check; no local Test-PngMagic copy can mask a
    # regression here.
        $root = Join-Path ([IO.Path]::GetTempPath()) ("capture-png-" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $root -Force | Out-Null
    try {
        $valid = [byte[]](0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0xff, 0x10)
        $captured = Invoke-ProductionPngCapture -Payload $valid -Root $root
        Assert-ByteSequenceEqual $captured $valid "production screenshot capture changed valid PNG bytes"

        $corrupt = [byte[]](0xff, 0xfe, 0xfd, 0xff, 0x50, 0x00, 0x4e, 0x00)
        $corruptFailed = $false
        try { [void](Invoke-ProductionPngCapture -Payload $corrupt -Root $root) } catch { $corruptFailed = $true }
        Assert-Test $corruptFailed "production screenshot validator accepted UTF-16-corrupted PNG"

        $truncated = [byte[]](0x89, 0x50, 0x4e, 0x47)
        $truncatedFailed = $false
        try { [void](Invoke-ProductionPngCapture -Payload $truncated -Root $root) } catch { $truncatedFailed = $true }
        Assert-Test $truncatedFailed "production screenshot validator accepted truncated PNG"
    } finally {
        if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
    }
}

$tests = @(
    [pscustomobject]@{ Name = "IdentityFieldBoundaries"; Body = ${function:Test-IdentityFieldBoundaries} },
    [pscustomobject]@{ Name = "KWinIdentityFieldBoundaries"; Body = ${function:Test-KWinIdentityFieldBoundaries} },
    [pscustomobject]@{ Name = "ReservedPidAssignmentIsAbsent"; Body = ${function:Test-ReservedPidAssignmentIsAbsent} },
    [pscustomobject]@{ Name = "RestoreBytesAndModes"; Body = ${function:Test-RestoreBytesAndModes} },
    [pscustomobject]@{ Name = "PreflightDoesNotStopApp"; Body = ${function:Test-PreflightDoesNotStopApp} },
    [pscustomobject]@{ Name = "ArtifactFreshnessContract"; Body = ${function:Test-ArtifactFreshnessContract} },
    [pscustomobject]@{ Name = "SignalEvidenceContract"; Body = ${function:Test-SignalEvidenceContract} },
    [pscustomobject]@{ Name = "BinaryScreenshotContract"; Body = ${function:Test-BinaryScreenshotContract} }
)

$failures = [Collections.Generic.List[string]]::new()
foreach ($test in $tests) {
    try {
        & $test.Body
        Write-Output "PASS $($test.Name)"
    } catch {
        [void]$failures.Add("FAIL $($test.Name): $($_.Exception.Message)")
        Write-Output $failures[$failures.Count - 1]
    }
}
if ($failures.Count -ne 0) {
    throw ($failures -join [Environment]::NewLine)
}
