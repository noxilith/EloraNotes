# Runs install-windows.ps1 end to end with PowerShell, on any operating system.
#
#   $env:ELORA_SERVER_BINARY = "target/debug/elora-server"; pwsh packaging/server/tests/test-install-windows.ps1
#
# The Windows-only commands (Task Scheduler, the firewall, icacls) are replaced by recorders in
# windows-mocks.ps1, and the one that starts the task starts the real server, so the installer's
# own logic -- settings, updates, checksums, the health check, the task's settings -- runs for real.
# Nothing outside a temporary folder is touched.
$ErrorActionPreference = "Stop"

$here = $PSScriptRoot
$installer = Join-Path (Split-Path $here -Parent) "install-windows.ps1"
if (-not $env:ELORA_SERVER_BINARY -or -not (Test-Path $env:ELORA_SERVER_BINARY)) {
    Write-Host "set ELORA_SERVER_BINARY to a built elora-server"
    exit 2
}
$serverBinary = (Resolve-Path $env:ELORA_SERVER_BINARY).Path

$T = Join-Path ([IO.Path]::GetTempPath()) ("elora-win-installer-test-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force -Path (Join-Path $T "release"), (Join-Path $T "ProgramData") | Out-Null
$env:ELORA_TEST_DIR = $T
$env:ELORA_TEST_INSTALLER = $installer
$env:ProgramData = Join-Path $T "ProgramData"
$env:TMPDIR = $T
$env:ELORA_RELEASE_BASE = "http://example.invalid/release"
$env:ELORA_HEALTH_WAIT = "10"
$installDir = Join-Path $env:ProgramData "EloraNotes Server"
$installedBinary = Join-Path $installDir "elora-server.exe"
$configFile = Join-Path $installDir "config.json"

$script:passed = 0
$script:failed = 0
function Check($What, $Condition) {
    if ($Condition) { $script:passed++; Write-Host "  ok    $What" }
    else { $script:failed++; Write-Host "  FAIL  $What" }
}
function Section($Title) { Write-Host ""; Write-Host $Title }

function Stop-TestServer {
    $pidFile = Join-Path $T "server.pid"
    if (Test-Path $pidFile) {
        Stop-Process -Id ([int](Get-Content $pidFile)) -Force -ErrorAction SilentlyContinue
        Remove-Item -Force $pidFile -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 400
    }
}
function Test-Listening([int]$Port) {
    $client = New-Object Net.Sockets.TcpClient
    try { $client.Connect("127.0.0.1", $Port); return $true } catch { return $false } finally { $client.Close() }
}
function Get-Sha($Path) { (Get-FileHash -Algorithm SHA256 -Path $Path).Hash.ToLowerInvariant() }
function Get-FreePort {
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $listener.Start(); $port = $listener.LocalEndpoint.Port; $listener.Stop(); $port
}

# Puts a "release" in place: the binary and a checksum list (which can describe another file).
function Publish-Release($Binary, $ListedFile = $null) {
    if (-not $ListedFile) { $ListedFile = $Binary }
    Copy-Item -Force $Binary (Join-Path $T "release/elora-server-windows-x86_64.exe")
    "$(Get-Sha $ListedFile)  elora-server-windows-x86_64.exe`n0000  something-else`n" |
        Set-Content -NoNewline -Path (Join-Path $T "release/SHA256SUMS")
}

# One installer run in a process of its own, with no console to ask anything in.
function Invoke-InstallerRun([hashtable]$Environment = @{}) {
    $names = @("ELORA_NAME", "ELORA_PORT", "ELORA_PASSWORD", "ELORA_ACTION")
    foreach ($n in $names) { Remove-Item "Env:\$n" -ErrorAction SilentlyContinue }
    foreach ($k in $Environment.Keys) { Set-Item "Env:\$k" $Environment[$k] }
    $empty = Join-Path $T "empty.txt"
    Set-Content -Path $empty -Value ""
    $out = Join-Path $T "out.txt"
    $err = Join-Path $T "err.txt"
    $p = Start-Process -FilePath "pwsh" -ArgumentList @("-NoProfile", "-File", (Join-Path $here "windows-scenario.ps1")) `
        -RedirectStandardInput $empty -RedirectStandardOutput $out -RedirectStandardError $err -Wait -PassThru
    foreach ($n in $names) { Remove-Item "Env:\$n" -ErrorAction SilentlyContinue }
    $text = (Get-Content -Raw $out) + (Get-Content -Raw $err)
    $script:last = [pscustomobject]@{ Code = $p.ExitCode; Output = "$text" }
    $script:last
}
function Get-Calls { if (Test-Path (Join-Path $T "calls.log")) { Get-Content (Join-Path $T "calls.log") } else { @() } }

try {
    # --- the helpers on their own -----------------------------------------------------------
    Section "helpers"
    $env:ELORA_INSTALLER_LIBRARY = "1"
    . $installer
    . (Join-Path $here "windows-mocks.ps1")
    Remove-Item Env:\ELORA_INSTALLER_LIBRARY

    $nonAscii = [IO.File]::ReadAllBytes($installer) | Where-Object { $_ -gt 127 }
    Check "the installer is plain ASCII (Windows PowerShell 5.1 reads a file without a byte-order mark as ANSI and would garble anything else)" (-not $nonAscii)

    Check "ports from 1024 to 65535 are accepted" ((ConvertTo-ServerPort "1024") -eq 1024 -and (ConvertTo-ServerPort " 65535 ") -eq 65535)
    Check "ports outside that, words and nothing are refused" (-not (ConvertTo-ServerPort "1023") -and -not (ConvertTo-ServerPort "65536") -and -not (ConvertTo-ServerPort "abc") -and -not (ConvertTo-ServerPort ""))

    $sums = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA  wanted.exe`r`n" +
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb *starred.exe`n" +
            "0000  too-short.exe`n"
    Check "a checksum list is read (CRLF, binary-mode stars, upper case)" ((Get-ListedSha256 $sums "wanted.exe") -eq ("a" * 64) -and (Get-ListedSha256 $sums "starred.exe") -eq ("b" * 64))
    Check "a name that is not listed, or has a malformed line, gives nothing" (-not (Get-ListedSha256 $sums "nope.exe") -and -not (Get-ListedSha256 $sums "too-short.exe"))

    $cfg = Join-Path $T "sample.json"
    '{ "name": "My \"Server\" \\ ok", "port": 7777, "password": "secret", "notes_dir": "C:\\Notes dir", "bind": "0.0.0.0" }' | Set-Content $cfg
    $read = Read-ServerConfig $cfg
    Check "settings are read, escapes included" ($read.Name -eq 'My "Server" \ ok' -and $read.Port -eq 7777 -and $read.NotesDir -eq 'C:\Notes dir' -and $read.Bind -eq "0.0.0.0")
    Check "the password is not part of what is read" (-not ($read.PSObject.Properties.Name -contains "Password"))
    '{ "name": "x", "port": 80 }' | Set-Content $cfg
    Check "settings with a bad port, a missing file or broken text read as nothing" (-not (Read-ServerConfig $cfg) -and -not (Read-ServerConfig (Join-Path $T "none.json")))
    "not json" | Set-Content $cfg
    Check "broken settings read as nothing" (-not (Read-ServerConfig $cfg))

    # Invoke-Native
    $ok = Invoke-Native "sh" @("-c", "echo hello; echo oops >&2; exit 0")
    Check "a program's output and its stderr text come back, without raising an error" ($ok.ExitCode -eq 0 -and ($ok.Output -contains "hello") -and ($ok.Output -contains "oops"))
    $bad = Invoke-Native "sh" @("-c", "exit 3")
    Check "an exit code comes back" ($bad.ExitCode -eq 3)
    $null = Invoke-Native "sh" @("-c", "exit 0")
    $missing = Invoke-Native "/no/such/program" @()
    Check "a program that cannot start is a failure (-1), not the previous program's success" ($missing.ExitCode -eq -1)
    $global:OutputEncoding = [Text.Encoding]::ASCII
    $echo = Invoke-Native "cat" @() -StandardInput "pässwörd ✓"
    Check "a non-ASCII password reaches the program intact even when the session encoding is ASCII" ($echo.Output -contains "pässwörd ✓")
    Check "the session's own encoding is put back afterwards" ($global:OutputEncoding -eq [Text.Encoding]::ASCII)

    $lan = @(Get-LanAddress)
    Check "only real network addresses are offered to other people" ($lan.Count -eq 1 -and $lan[0] -eq "192.168.1.5")

    # --- a fresh install --------------------------------------------------------------------
    Publish-Release $serverBinary
    $port = Get-FreePort
    $name = 'Test "Srv" 100%'

    Section "fresh install (answers from the environment, Windows PowerShell 5.1's ASCII pipe simulated)"
    $r = Invoke-InstallerRun @{ ELORA_NAME = $name; ELORA_PORT = "$port"; ELORA_PASSWORD = "pässwörd" }
    Check "it succeeds" ($r.Code -eq 0)
    if ($r.Code -ne 0) { Write-Host $r.Output }
    Check "the download was checked against the release's checksum" ($r.Output -match "Checked the download")
    Check "the binary is installed" (Test-Path $installedBinary)
    $config = Get-Content -Raw $configFile
    Check "the settings hold the chosen name and port" ($config -match "Srv" -and $config -match "`"port`": $port")
    Check "the password with accents was stored as typed (not as question marks)" ($config -match "pässwörd")
    Check "the server really answers on the port" (Test-Listening $port)
    Check "the summary says it is running and shows the version" ($r.Output -match "is running on port $port \(elora-server")
    Check "the summary lists the real network address and not the WSL or loopback ones" ($r.Output -match "192\.168\.1\.5" -and $r.Output -notmatch "172\.20\.0\.1" -and $r.Output -notmatch "169\.254")
    Check "the password is never printed" ($r.Output -notmatch "pässwörd")
    Check "it says where the server's output goes" ($r.Output -match "server\.log")

    $task = Get-Content -Raw (Join-Path $T "task.json") | ConvertFrom-Json
    Check "the task starts at boot" ($task.Trigger.AtStartup.IsPresent -eq $true)
    Check "the task runs as SYSTEM with the highest rights" ($task.Principal.UserId -eq "SYSTEM" -and "$($task.Principal.RunLevel)" -eq "Highest")
    Check "the task is not limited to 72 hours (no execution time limit)" ($task.Settings.ExecutionTimeLimit.TotalSeconds -eq 0 -or $task.Settings.ExecutionTimeLimit.Ticks -eq 0)
    Check "the task starts and keeps running on battery power" ($task.Settings.AllowStartIfOnBatteries.IsPresent -eq $true -and $task.Settings.DontStopIfGoingOnBatteries.IsPresent -eq $true)
    Check "the task is restarted if the server exits" ($task.Settings.RestartCount -eq 999 -and $task.Settings.RestartInterval.TotalMinutes -eq 1)
    Check "the task catches up if the machine was off at boot, and never runs twice" ($task.Settings.StartWhenAvailable.IsPresent -eq $true -and "$($task.Settings.MultipleInstances)" -eq "IgnoreNew")
    Check "the task runs the runner from the server's folder" ($task.Action.Execute -like "*run-server.cmd" -and $task.Action.WorkingDirectory -eq $installDir)

    $runner = Get-Content -Raw (Join-Path $installDir "run-server.cmd")
    Check "the runner sets the server's folder and logs its output" ($runner -match 'set "ELORA_SERVER_DIR=' -and $runner -match 'server\.log" 2>&1')
    Check "the runner keeps the previous run's log" ($runner -match "server\.previous\.log")
    $calls = Get-Calls
    Check "the install folder is locked to SYSTEM and Administrators" (($calls -join "`n") -match "icacls .*EloraNotes Server /inheritance:r /grant:r \*S-1-5-18:\(OI\)\(CI\)F \*S-1-5-32-544:\(OI\)\(CI\)F")
    Check "the firewall rule is for the port and this program" ((Get-Content (Join-Path $T "firewall.txt")) -match "^$port\|.*elora-server\.exe\|Any")
    Check "the downloaded file is unblocked" (($calls -join "`n") -match "Unblock-File")
    $firstPid = [int](Get-Content (Join-Path $T "server.pid"))
    $configSum = Get-Sha $configFile

    # --- update -----------------------------------------------------------------------------
    Section "update with no answers (keeps the settings, restarts the server)"
    Start-Sleep -Seconds 1
    $r = Invoke-InstallerRun @{ ELORA_ACTION = "update" }
    Check "it succeeds" ($r.Code -eq 0)
    if ($r.Code -ne 0) { Write-Host $r.Output }
    Check "the settings file is untouched" ((Get-Sha $configFile) -eq $configSum)
    Check "no password was asked for or needed" ($r.Output -notmatch "Password everyone")
    Check "the server was restarted (a new process)" ([int](Get-Content (Join-Path $T "server.pid")) -ne $firstPid)
    Check "and it answers again" (Test-Listening $port)

    # --- a bad download never replaces a working server ------------------------------------
    Section "a corrupted download is refused"
    $corrupt = Join-Path $T "corrupt.exe"
    Copy-Item $serverBinary $corrupt
    [IO.File]::AppendAllText($corrupt, "tampered")
    Copy-Item -Force $corrupt (Join-Path $T "release/elora-server-windows-x86_64.exe")   # the listed checksum is still the good one
    $oldSha = Get-Sha $installedBinary
    $pidBefore = [int](Get-Content (Join-Path $T "server.pid"))
    $r = Invoke-InstallerRun @{ ELORA_ACTION = "update" }
    Check "the installer fails" ($r.Code -ne 0)
    Check "it says the checksum does not match" ($r.Output -match "does not match its published checksum")
    Check "the installed server is unchanged" ((Get-Sha $installedBinary) -eq $oldSha)
    Check "the running server was never stopped" ((Get-Process -Id $pidBefore -ErrorAction SilentlyContinue) -ne $null)

    Section "a download that does not run is refused before anything is touched"
    $notServer = Join-Path $T "notaserver.exe"
    [IO.File]::WriteAllBytes($notServer, (New-Object byte[] 2000000))
    chmod +x $notServer
    Publish-Release $notServer
    $r = Invoke-InstallerRun @{ ELORA_ACTION = "update" }
    Check "the installer fails" ($r.Code -ne 0)
    Check "it says nothing was changed" ($r.Output -match "nothing was changed")
    Check "the installed server is unchanged and still running" ((Get-Sha $installedBinary) -eq $oldSha -and (Get-Process -Id $pidBefore -ErrorAction SilentlyContinue))

    Section "a missing checksum list is refused"
    Publish-Release $serverBinary
    Remove-Item -Force (Join-Path $T "release/SHA256SUMS")
    $r = Invoke-InstallerRun @{ ELORA_ACTION = "update" }
    Check "the installer fails and says the download cannot be verified" ($r.Code -ne 0 -and $r.Output -match "cannot be verified")
    Publish-Release $serverBinary

    # --- reconfiguring ----------------------------------------------------------------------
    Section "changing the port (keeps a notes folder edited in by hand, replaces the firewall rule)"
    $custom = Join-Path $T "custom-notes"
    New-Item -ItemType Directory -Force -Path $custom | Out-Null
    $text = Get-Content -Raw $configFile
    $escaped = $custom.Replace("\", "\\")
    ($text -replace '"notes_dir": ".*"', "`"notes_dir`": `"$escaped`"") | Set-Content -NoNewline $configFile
    $newPort = Get-FreePort
    $r = Invoke-InstallerRun @{ ELORA_PORT = "$newPort"; ELORA_PASSWORD = "another one" }
    Check "it succeeds" ($r.Code -eq 0)
    if ($r.Code -ne 0) { Write-Host $r.Output }
    $config = Get-Content -Raw $configFile
    Check "the new port is in the settings" ($config -match "`"port`": $newPort")
    Check "the custom notes folder survived" ($config -match "custom-notes")
    Check "the firewall has only the new port" ((Get-Content (Join-Path $T "firewall.txt")) -match "^$newPort\|" -and -not ((Get-Content (Join-Path $T "firewall.txt")) -match "^$port\|"))
    Check "the server answers on the new port" (Test-Listening $newPort)
    $port = $newPort

    # --- refusals ---------------------------------------------------------------------------
    Section "refusals"
    $r = Invoke-InstallerRun @{ ELORA_NAME = "x"; ELORA_PORT = "$port" }
    Check "no password and no console: it fails and names the variable" ($r.Code -ne 0 -and $r.Output -match "ELORA_PASSWORD")
    $r = Invoke-InstallerRun @{ ELORA_NAME = "x"; ELORA_PORT = "80"; ELORA_PASSWORD = "p" }
    Check "a port below 1024 is refused" ($r.Code -ne 0 -and $r.Output -match "1024 to 65535")
    $r = Invoke-InstallerRun @{ ELORA_NAME = "x"; ELORA_PORT = "abc"; ELORA_PASSWORD = "p" }
    Check "a port that is not a number is refused" ($r.Code -ne 0 -and $r.Output -match "1024 to 65535")
    $r = Invoke-InstallerRun @{ ELORA_ACTION = "sideways" }
    Check "an unknown action is refused" ($r.Code -ne 0 -and $r.Output -match "ELORA_ACTION must be")

    # --- uninstalling -----------------------------------------------------------------------
    Section "uninstall keeps the notes and closes the port"
    "keep me" | Set-Content (Join-Path $custom "precious.md")
    $r = Invoke-InstallerRun @{ ELORA_ACTION = "uninstall" }
    Check "it succeeds" ($r.Code -eq 0)
    if ($r.Code -ne 0) { Write-Host $r.Output }
    Check "the task is gone" (-not (Test-Path (Join-Path $T "task.json")))
    Check "the binary and runner are gone" (-not (Test-Path $installedBinary) -and -not (Test-Path (Join-Path $installDir "run-server.cmd")))
    Check "the firewall rule is gone" (-not (Test-Path (Join-Path $T "firewall.txt")))
    Check "the server process was stopped" (-not (Test-Listening $port))
    Check "the notes and the settings are still there" ((Test-Path (Join-Path $custom "precious.md")) -and (Test-Path $configFile))

    Section "purge removes the settings folder"
    Stop-TestServer
    $r = Invoke-InstallerRun @{ ELORA_NAME = "Again"; ELORA_PORT = "$port"; ELORA_PASSWORD = "secret" }
    Check "a reinstall works" ($r.Code -eq 0)
    $r = Invoke-InstallerRun @{ ELORA_ACTION = "purge" }
    Check "purge succeeds" ($r.Code -eq 0)
    Check "the settings folder is gone" (-not (Test-Path $installDir))
    $r = Invoke-InstallerRun @{ ELORA_ACTION = "uninstall" }
    Check "uninstalling what is not installed says so and installs nothing" ($r.Code -eq 0 -and $r.Output -match "not installed" -and -not (Test-Path $installedBinary))

    # --- a port in use ----------------------------------------------------------------------
    Section "a port that is already in use"
    $blocker = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Any, 0)
    $blocker.Start()
    $busy = $blocker.LocalEndpoint.Port
    $r = Invoke-InstallerRun @{ ELORA_NAME = "x"; ELORA_PORT = "$busy"; ELORA_PASSWORD = "p" }
    $blocker.Stop()
    Check "it is refused and the problem is named" ($r.Code -ne 0 -and $r.Output -match "port $busy is already in use")
    Check "and nothing was installed" (-not (Test-Path $installedBinary))
}
finally {
    Stop-TestServer
    Remove-Item -Recurse -Force $T -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "$($script:passed) passed, $($script:failed) failed"
if ($script:failed -gt 0) { exit 1 }
