<#
.SYNOPSIS
    Installs the EloraNotes collaboration server on this Windows machine.

.DESCRIPTION
    Asks for the name people see in their sidebar, the port, and the password everyone connects
    with, then installs the binary, creates the notes folder, generates the certificate, opens the
    port in Windows Firewall and registers a task that starts the server at boot and keeps it
    running.

    If a server is already installed it says what it found and asks whether to update it (keeping
    the settings, the password, the notes and the certificate), uninstall it while keeping the
    notes, or uninstall it and delete the notes and certificate. Set $env:ELORA_ACTION to update,
    uninstall or purge to skip that question.

    Run it from an Administrator PowerShell, after downloading install-elora-server.ps1 from the
    EloraNotes release page:
        powershell -ExecutionPolicy Bypass -File .\install-elora-server.ps1

    For a scripted install, answer the questions through the environment:
        ELORA_NAME, ELORA_PORT, ELORA_PASSWORD, ELORA_ACTION (update, uninstall or purge) and
        ELORA_VERSION (for example 0.10.1; the default is the latest release).
#>

$ErrorActionPreference = "Stop"
# The progress bar makes Windows PowerShell 5.1 download many times slower.
$ProgressPreference = "SilentlyContinue"
# Windows PowerShell 5.1 can still default to TLS 1.0/1.1, which GitHub refuses.
try {
    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch { }

$ReleasesUrl = "https://github.com/noxilith/EloraNotes/releases"
if ($env:ELORA_VERSION) {
    $ReleaseBase = "$ReleasesUrl/download/v$($env:ELORA_VERSION.TrimStart('v'))"
} else {
    $ReleaseBase = "$ReleasesUrl/latest/download"
}
if ($env:ELORA_RELEASE_BASE) { $ReleaseBase = $env:ELORA_RELEASE_BASE }
$Asset = "elora-server-windows-x86_64.exe"
$InstallDir = Join-Path $env:ProgramData "EloraNotes Server"
$Binary = Join-Path $InstallDir "elora-server.exe"
$Runner = Join-Path $InstallDir "run-server.cmd"
$LogFile = Join-Path $InstallDir "server.log"
$ConfigPath = Join-Path $InstallDir "config.json"
$TaskName = "EloraNotes Server"
$FirewallRule = "EloraNotes Server"
$HealthWait = 15
if ($env:ELORA_HEALTH_WAIT) { $HealthWait = [int]$env:ELORA_HEALTH_WAIT }
$env:ELORA_SERVER_DIR = $InstallDir

# --- helpers --------------------------------------------------------------------------------

function Fail($Message) {
    Write-Host "error: $Message" -ForegroundColor Red
    exit 1
}

# Windows PowerShell turns anything a native program writes to stderr into an error record, and
# with ErrorActionPreference = Stop the script dies on it -- even when the text was headed for
# $null. "schtasks: the task does not exist" is how that shows up here, and it is not an error.
# Native calls therefore go through this helper, which keeps their messages out of PowerShell's
# error stream and hands back the exit code and the text instead.
#
# Two more things it does. A password piped to a program travels as text in $OutputEncoding,
# which Windows PowerShell 5.1 sets to ASCII: every non-ASCII character would arrive as "?" and
# the password clients type would never match, so UTF-8 is used for the pipe. And a program that
# cannot be started has no exit code: it is reported as -1, never as an exception and never as
# whatever the previous program left in $LASTEXITCODE.
function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$Arguments = @(),
        [string]$StandardInput
    )
    $previousPreference = $ErrorActionPreference
    $previousEncoding = $global:OutputEncoding
    $ErrorActionPreference = "Continue"
    $global:LASTEXITCODE = $null
    $lines = @()
    $code = $null
    $launchError = $null
    try {
        $global:OutputEncoding = New-Object System.Text.UTF8Encoding($false)
        if ($PSBoundParameters.ContainsKey("StandardInput")) {
            $lines = @($StandardInput | & $FilePath @Arguments 2>&1 | ForEach-Object { "$_" })
        } else {
            $lines = @(& $FilePath @Arguments 2>&1 | ForEach-Object { "$_" })
        }
        $code = $global:LASTEXITCODE
    } catch {
        $launchError = $_.Exception.Message
    } finally {
        $ErrorActionPreference = $previousPreference
        $global:OutputEncoding = $previousEncoding
    }
    if ($null -eq $code) { $code = -1 }
    [pscustomobject]@{
        ExitCode = $code
        Output   = $lines
        Error    = $launchError
    }
}

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

# A port a person may pick: 1024 and up, so the server needs no special rights. Returns the
# number, or $null if the text is not one.
function ConvertTo-ServerPort($Text) {
    $port = 0
    if ([int]::TryParse("$Text".Trim(), [ref]$port) -and $port -ge 1024 -and $port -le 65535) {
        return $port
    }
    return $null
}

# The settings the server wrote, or $null when there is no readable file. The password it holds
# is never read out of it.
function Read-ServerConfig($Path) {
    if (-not (Test-Path $Path)) { return $null }
    try {
        $saved = Get-Content -Raw -Path $Path | ConvertFrom-Json
        $port = ConvertTo-ServerPort $saved.port
        if (-not $saved.name -or -not $port) { return $null }
        [pscustomobject]@{
            Name     = "$($saved.name)"
            Port     = $port
            NotesDir = "$($saved.notes_dir)"
            Bind     = "$($saved.bind)"
        }
    } catch {
        return $null
    }
}

# The checksum a release's SHA256SUMS text lists for one file name, or $null.
function Get-ListedSha256($Text, $Name) {
    foreach ($line in ($Text -split "\r?\n")) {
        if ($line -match '^([0-9a-fA-F]{64})\s+\*?(.+?)\s*$' -and $Matches[2] -eq $Name) {
            return $Matches[1].ToLowerInvariant()
        }
    }
    return $null
}

# The processes that are this install's server, found by the file they run from so that another
# elora-server.exe on the machine is never touched.
function Get-ServerProcess {
    Get-Process -ErrorAction SilentlyContinue | Where-Object {
        try { $_.Path -eq $Binary } catch { $false }
    }
}

function Stop-Server {
    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    }
    Get-ServerProcess | Stop-Process -Force -ErrorAction SilentlyContinue
    # Windows will not overwrite the executable of a program that is still running.
    for ($i = 0; $i -lt 20 -and (Get-ServerProcess); $i++) { Start-Sleep -Milliseconds 250 }
}

# True when something on this machine already accepts connections on the port.
function Test-PortInUse([int]$Port) {
    $client = New-Object Net.Sockets.TcpClient
    try {
        $wait = $client.BeginConnect("127.0.0.1", $Port, $null, $null)
        if ($wait.AsyncWaitHandle.WaitOne(500) -and $client.Connected) {
            $client.EndConnect($wait)
            return $true
        }
        return $false
    } catch {
        return $false
    } finally {
        $client.Close()
    }
}

# Up means accepting connections, not "the task is running": a server that cannot read its
# settings or bind its port is gone a moment after the task starts.
function Wait-ServerListening([int]$Port, [int]$Seconds) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        if (Test-PortInUse $Port) { return $true }
        Start-Sleep -Milliseconds 500
    }
    return $false
}

# The settings file holds the password in plain text and the folder holds the private key, and
# C:\ProgramData is readable by every account on the machine. Only SYSTEM (which runs the task)
# and Administrators may open this folder. The well-known SIDs are used so it works in any language.
function Protect-InstallDir {
    $result = Invoke-Native "icacls" @($InstallDir, "/inheritance:r", "/grant:r",
        "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F")
    if ($result.ExitCode -ne 0) {
        foreach ($line in $result.Output) { Write-Host "  $line" }
        Fail "could not restrict access to $InstallDir (the password is stored there)"
    }
}

# A scheduled task, because a Windows service needs a program that speaks the service protocol.
# The settings matter: a task made the plain way is stopped after 72 hours, is not started on
# battery power, is stopped when the laptop is unplugged, and is never restarted if the server
# exits, which is a server that quietly disappears after three days.
function Register-ServerTask {
    $action = New-ScheduledTaskAction -Execute $Runner -WorkingDirectory $InstallDir
    $trigger = New-ScheduledTaskTrigger -AtStartup
    $principal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
    $settings = New-ScheduledTaskSettingsSet `
        -AllowStartIfOnBatteries `
        -DontStopIfGoingOnBatteries `
        -StartWhenAvailable `
        -MultipleInstances IgnoreNew `
        -ExecutionTimeLimit ([TimeSpan]::Zero) `
        -RestartCount 999 `
        -RestartInterval (New-TimeSpan -Minutes 1)
    Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger `
        -Principal $principal -Settings $settings `
        -Description "Serves the EloraNotes shared library. Started at boot and restarted if it stops." `
        -Force | Out-Null
}

function Set-ServerFirewallRule([int]$Port) {
    Remove-ServerFirewallRule
    New-NetFirewallRule -DisplayName $FirewallRule -Direction Inbound -Protocol TCP `
        -LocalPort $Port -Action Allow -Profile Any -Program $Binary | Out-Null
}

function Remove-ServerFirewallRule {
    Get-NetFirewallRule -DisplayName $FirewallRule -ErrorAction SilentlyContinue |
        Remove-NetFirewallRule -ErrorAction SilentlyContinue
}

# The machine's own addresses other computers can use, without the virtual adapters that WSL,
# Hyper-V, Docker and VPN software add.
function Get-LanAddress {
    Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
        Where-Object {
            $_.IPAddress -notlike "127.*" -and $_.IPAddress -notlike "169.254.*" -and
            $_.InterfaceAlias -notmatch "vEthernet|WSL|Loopback|VirtualBox|VMware|Hyper-V|Docker|Bluetooth"
        } |
        Select-Object -ExpandProperty IPAddress
}

function Read-PlainPassword($Prompt) {
    $secure = Read-Host $Prompt -AsSecureString
    $bstr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure)
    try {
        [Runtime.InteropServices.Marshal]::PtrToStringBSTR($bstr)
    } finally {
        [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
    }
}

function Get-LogTail {
    if (Test-Path $LogFile) { Get-Content -Path $LogFile -Tail 15 -ErrorAction SilentlyContinue }
}

# --- the installer --------------------------------------------------------------------------

function Invoke-Installer {
    if (-not (Test-IsAdministrator)) {
        Fail "open PowerShell as Administrator and run this script again"
    }
    if (-not [Environment]::Is64BitOperatingSystem) {
        Fail "the EloraNotes server is only built for 64-bit Windows"
    }

    # Questions are only asked when there is somewhere to ask them; a scripted run gets its
    # answers from the environment and takes the defaults for the rest.
    $interactive = $true
    try { if ([Console]::IsInputRedirected) { $interactive = $false } } catch { }
    function Ask($Question, $Default) {
        if (-not $interactive) { return $Default }
        $label = $Question
        if ($Default) { $label = "$Question [$Default]" }
        $answer = Read-Host $label
        if ([string]::IsNullOrWhiteSpace($answer)) { return $Default }
        return $answer.Trim()
    }

    Write-Host "EloraNotes server setup" -ForegroundColor Cyan
    Write-Host ""

    # --- is a server already installed here? ------------------------------------------------
    # Checked before anything is asked or downloaded: an update must never look like a fresh
    # install, and uninstalling has to know where the old files and the startup task are.
    $taskExists = [bool](Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue)
    $dataExists = Test-Path $ConfigPath
    $existing = $dataExists -or (Test-Path $Binary) -or $taskExists
    $cur = Read-ServerConfig $ConfigPath

    # ELORA_ACTION=update|uninstall|purge skips the question (for scripted installs).
    $action = $env:ELORA_ACTION
    if ($action -and @("update", "uninstall", "purge") -notcontains $action) {
        Fail "ELORA_ACTION must be 'update', 'uninstall' or 'purge', not '$action'"
    }

    # Asked to remove a server that is not here, it must not install one instead.
    if (-not $existing -and ($action -eq "uninstall" -or $action -eq "purge")) {
        Write-Host "EloraNotes server is not installed on this machine; nothing to remove."
        return
    }

    $notesDir = Join-Path $InstallDir "notes"
    if ($cur -and $cur.NotesDir) { $notesDir = $cur.NotesDir }

    if ($existing) {
        $noteCount = 0
        if (Test-Path $notesDir) {
            $noteCount = @(Get-ChildItem -Path $notesDir -Filter *.md -File -ErrorAction SilentlyContinue).Count
        }
        $code = ""
        $oldVersion = ""
        if (Test-Path $Binary) {
            $ver = Invoke-Native $Binary @("version")
            if ($ver.ExitCode -eq 0 -and $ver.Output.Count -gt 0) { $oldVersion = $ver.Output[0] }
            if ($dataExists) {
                $fp = Invoke-Native $Binary @("fingerprint")
                if ($fp.ExitCode -eq 0 -and $fp.Output.Count -gt 0) { $code = $fp.Output[0] }
            }
        }

        Write-Host "EloraNotes server is already installed on this machine:"
        if ($taskExists) {
            Write-Host "  service:  scheduled task `"$TaskName`" (starts at boot)"
        } else {
            Write-Host "  service:  none found"
        }
        if (Test-Path $Binary) {
            $shown = $Binary
            if ($oldVersion) { $shown = "$Binary ($oldVersion)" }
            Write-Host "  binary:   $shown"
        }
        if ($dataExists) {
            $detail = ""
            if ($cur) { $detail = "  (`"$($cur.Name)`", port $($cur.Port))" }
            Write-Host "  settings: $InstallDir$detail"
            Write-Host "  notes:    $noteCount in $notesDir"
        }
        if ($code) {
            Write-Host "  code:     $code (clients already trust this certificate)"
        }

        if (-not $action) {
            if ($interactive) {
                Write-Host ""
                Write-Host "What would you like to do?"
                Write-Host "  1) Update it, keeping the settings, the notes and the certificate  (recommended)"
                Write-Host "  2) Uninstall it, keeping the notes on disk"
                Write-Host "  3) Uninstall it and delete the notes and certificate"
                Write-Host "  4) Cancel"
                while ($true) {
                    $choice = Ask "Choice" "1"
                    switch ($choice) {
                        "1" { $action = "update" }
                        "2" { $action = "uninstall" }
                        "3" { $action = "purge" }
                        "4" { $action = "cancel" }
                        default { Write-Host "  Type 1, 2, 3 or 4." }
                    }
                    if ($action) { break }
                }
            } else {
                $action = "update"
                Write-Host "No console to ask in, so the existing install is updated."
            }
        }

        if ($action -eq "cancel") {
            Write-Host "Nothing was changed."
            return
        }

        if ($action -eq "uninstall" -or $action -eq "purge") {
            Write-Host ""
            Write-Host "Uninstalling..."
            Stop-Server
            if ($taskExists) {
                Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
                if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
                    Fail "could not remove the scheduled task `"$TaskName`""
                }
                Write-Host "  Removed the scheduled task `"$TaskName`""
            }
            foreach ($path in @($Binary, $Runner)) {
                if (Test-Path $path) {
                    Remove-Item -Force $path -ErrorAction SilentlyContinue
                    Write-Host "  Removed $path"
                }
            }
            Remove-ServerFirewallRule
            Write-Host "  Closed the firewall port"

            if ($action -eq "purge") {
                if (Test-Path $InstallDir) {
                    Remove-Item -Recurse -Force $InstallDir
                    Write-Host "  Deleted $InstallDir"
                }
                # Notes that hand-edited settings keep somewhere else are not this installer's to delete.
                if ($cur -and $cur.NotesDir -and -not $cur.NotesDir.StartsWith($InstallDir, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path $cur.NotesDir)) {
                    Write-Host "  Left $($cur.NotesDir) alone: the settings pointed the notes outside the server folder."
                }
                Write-Host ""
                Write-Host "The server and everything it held are gone."
            } else {
                Write-Host ""
                Write-Host "The server is gone; the notes are still in $notesDir."
                Write-Host "Copy that folder somewhere safe if you want to keep them."
            }
            Write-Host "Run this installer again to set a server up from scratch."
            return
        }

        Write-Host ""
        Write-Host "Updating it; the notes and the certificate stay where they are."
    }

    # --- the binary -------------------------------------------------------------------------
    # Fetched, checked and test-run before anything on this machine is touched, so a failed
    # download can never replace a server that works.
    $staged = Join-Path ([IO.Path]::GetTempPath()) "elora-server-staged.exe"
    Remove-Item -Force $staged -ErrorAction SilentlyContinue
    $localBinary = $null
    if ($PSScriptRoot) {
        foreach ($name in @("elora-server.exe", $Asset)) {
            $candidate = Join-Path $PSScriptRoot $name
            if (Test-Path $candidate) { $localBinary = $candidate; break }
        }
    }
    if ($localBinary) {
        Write-Host "Using $localBinary"
        Copy-Item -Force $localBinary $staged
    } else {
        Write-Host "Downloading the server from the EloraNotes releases..."
        try {
            Invoke-WebRequest -Uri "$ReleaseBase/$Asset" -OutFile $staged -UseBasicParsing
        } catch {
            Fail "could not download the server: $($_.Exception.Message). Put elora-server.exe next to this script instead."
        }
        # The release lists the checksum of every file it carries; the download has to match it.
        $sums = Join-Path ([IO.Path]::GetTempPath()) "elora-server-SHA256SUMS"
        try {
            Invoke-WebRequest -Uri "$ReleaseBase/SHA256SUMS" -OutFile $sums -UseBasicParsing
        } catch {
            Remove-Item -Force $staged -ErrorAction SilentlyContinue
            Fail "could not download the checksum list, so the download cannot be verified: $($_.Exception.Message)"
        }
        $want = Get-ListedSha256 (Get-Content -Raw -Path $sums) $Asset
        Remove-Item -Force $sums -ErrorAction SilentlyContinue
        if (-not $want) {
            Remove-Item -Force $staged -ErrorAction SilentlyContinue
            Fail "the release's checksum list has no entry for $Asset"
        }
        $have = (Get-FileHash -Algorithm SHA256 -Path $staged).Hash.ToLowerInvariant()
        if ($have -ne $want) {
            Remove-Item -Force $staged -ErrorAction SilentlyContinue
            Fail "the downloaded server does not match its published checksum (expected $want, got $have); nothing was installed"
        }
        Write-Host "Checked the download against the release's checksum."
    }
    if (-not (Test-Path $staged) -or (Get-Item $staged).Length -lt 1000000) {
        Fail "the downloaded server looks incomplete; check the connection and try again"
    }
    $check = Invoke-Native $staged @("version")
    if ($check.ExitCode -ne 0 -or $check.Output.Count -eq 0) {
        Remove-Item -Force $staged -ErrorAction SilentlyContinue
        Fail "the downloaded server does not run on this machine (wrong CPU architecture or a corrupt download); nothing was changed"
    }
    $newVersion = $check.Output[0]

    # --- what the server should be called ---------------------------------------------------
    # An existing install keeps its settings, its password included, unless a person asks to
    # change them or the environment supplies new ones: an update is about the program, not the setup.
    $reconfigure = $true
    if ($cur) {
        $reconfigure = $false
        if ($env:ELORA_NAME -or $env:ELORA_PORT -or $env:ELORA_PASSWORD) {
            $reconfigure = $true
        } elseif ($interactive) {
            Write-Host ""
            $keep = Ask "Keep the current settings (name `"$($cur.Name)`", port $($cur.Port), same password)? [Y/n]" "y"
            if ($keep -match '^[nN]') { $reconfigure = $true }
        }
    }

    $password = ""
    if ($reconfigure) {
        $defaultName = "EloraNotes"
        $defaultPort = "7331"
        if ($cur) { $defaultName = $cur.Name; $defaultPort = "$($cur.Port)" }

        if ($env:ELORA_NAME) { $name = $env:ELORA_NAME } else {
            $name = Ask "Server name (shown in everyone's sidebar)" $defaultName
        }
        $name = ($name -replace "[\r\n]+", " ").Trim()
        if (-not $name) { $name = "EloraNotes" }

        $port = $null
        if ($env:ELORA_PORT) {
            $port = ConvertTo-ServerPort $env:ELORA_PORT
            if (-not $port) { Fail "ELORA_PORT '$($env:ELORA_PORT)' must be a number from 1024 to 65535" }
        }
        while (-not $port) {
            $port = ConvertTo-ServerPort (Ask "Port" $defaultPort)
            if (-not $port) {
                if (-not $interactive) { Fail "the port must be a number from 1024 to 65535" }
                Write-Host "  Pick a port between 1024 and 65535."
            }
        }

        if ($env:ELORA_PASSWORD) {
            $password = $env:ELORA_PASSWORD
            Remove-Item Env:\ELORA_PASSWORD -ErrorAction SilentlyContinue
        } elseif ($interactive) {
            $password = Read-PlainPassword "Password everyone will connect with"
            $again = Read-PlainPassword "Repeat the password"
            if ($password -ne $again) { Fail "the passwords did not match" }
        } else {
            Fail "there is no console to ask for the password; set ELORA_PASSWORD"
        }
        if ([string]::IsNullOrEmpty($password)) { Fail "the password cannot be empty" }
    } else {
        $name = $cur.Name
        $port = $cur.Port
    }

    # --- stop the old server, then check the port is free -----------------------------------
    # Windows refuses to overwrite the executable of a running program, and a running server
    # would also hold its old settings in memory; both need it stopped before anything changes.
    Stop-Server
    if (Test-PortInUse $port) {
        Remove-Item -Force $staged -ErrorAction SilentlyContinue
        Fail "port $port is already in use by another program; stop it or run this installer again and pick another port"
    }

    # --- the binary -------------------------------------------------------------------------
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Protect-InstallDir
    # A just-stopped server can hold its file for a moment, so the copy is retried.
    for ($attempt = 1; $attempt -le 5; $attempt++) {
        try {
            Copy-Item -Force $staged $Binary -ErrorAction Stop
            break
        } catch {
            if ($attempt -eq 5) {
                Fail "could not replace ${Binary}: $($_.Exception.Message). Stop the server first with: Stop-ScheduledTask -TaskName `"$TaskName`""
            }
            Start-Sleep -Seconds 2
        }
    }
    Remove-Item -Force $staged -ErrorAction SilentlyContinue
    # A file saved from a browser carries a "downloaded from the internet" mark; clear it.
    try { Unblock-File -Path $Binary -ErrorAction SilentlyContinue } catch { }

    # --- settings + certificate -------------------------------------------------------------
    # Only written when they are new or changed; an update leaves them untouched. When they are
    # rewritten, a notes folder or bind address edited in by hand is kept.
    if ($reconfigure) {
        Write-Host "Writing the settings and the certificate..."
        $initArgs = @("init", "--name", $name, "--port", "$port", "--password-stdin")
        if ($cur -and $cur.NotesDir) { $initArgs += @("--notes-dir", $cur.NotesDir) }
        if ($cur -and $cur.Bind) { $initArgs += @("--bind", $cur.Bind) }
        $init = Invoke-Native $Binary $initArgs -StandardInput $password
        $password = ""
        if ($init.ExitCode -ne 0) {
            foreach ($line in $init.Output) { Write-Host "  $line" }
            Fail "elora-server init failed"
        }
    }
    # Once more, so a file the server has just written is covered too.
    Protect-InstallDir

    # --- start at boot ----------------------------------------------------------------------
    # The runner sets the server's folder (the task runs as SYSTEM, whose profile is somewhere
    # else) and keeps the last two runs' output, which is the only place a crash shows up.
    $previousLog = Join-Path $InstallDir "server.previous.log"
    $runnerContent = @"
@echo off
set "ELORA_SERVER_DIR=$InstallDir"
if exist "$LogFile" move /y "$LogFile" "$previousLog" >nul
"$Binary" run >> "$LogFile" 2>&1
"@
    Set-Content -Path $Runner -Value $runnerContent -Encoding ASCII

    try {
        Register-ServerTask
    } catch {
        Fail "could not register the startup task: $($_.Exception.Message)"
    }
    try {
        Start-ScheduledTask -TaskName $TaskName
    } catch {
        Fail "registered the startup task, but could not start it: $($_.Exception.Message)"
    }

    # --- firewall ---------------------------------------------------------------------------
    try {
        Set-ServerFirewallRule $port
    } catch {
        Write-Host "warning: could not open port $port in Windows Firewall ($($_.Exception.Message)); allow TCP $port yourself" -ForegroundColor Yellow
    }

    # --- is it really up? -------------------------------------------------------------------
    $up = Wait-ServerListening $port $HealthWait
    if (-not $up) {
        Write-Host ""
        Write-Host "The server did not start answering on port $port. Its last output:" -ForegroundColor Red
        foreach ($line in (Get-LogTail)) { Write-Host "  $line" }
        Fail "see $LogFile; once it is fixed, start it with: Start-ScheduledTask -TaskName `"$TaskName`""
    }

    # --- what to tell everyone else ---------------------------------------------------------
    $fingerprint = ""
    $fp = Invoke-Native $Binary @("fingerprint")
    if ($fp.ExitCode -eq 0 -and $fp.Output.Count -gt 0) { $fingerprint = $fp.Output[0] }
    $addresses = @(Get-LanAddress)
    $after = Read-ServerConfig $ConfigPath
    $notesShown = Join-Path $InstallDir "notes"
    if ($after -and $after.NotesDir) { $notesShown = $after.NotesDir }

    Write-Host ""
    Write-Host "EloraNotes server `"$name`" is running on port $port ($newVersion)." -ForegroundColor Green
    foreach ($address in $addresses) {
        Write-Host "  Same network:  $address  port $port"
    }
    Write-Host "  Other networks: this machine's public IP or a DNS name, port $port"
    Write-Host "  Router: forward TCP $port to this machine once."
    if ($fingerprint) {
        Write-Host "  Certificate code clients confirm: $fingerprint"
    }
    Write-Host ""
    Write-Host "Server binary: $Binary"
    Write-Host "Notes live in $notesShown."
    Write-Host "Its output is kept in $LogFile."
    Write-Host "Manage it with: Start-ScheduledTask -TaskName `"$TaskName`" / Stop-ScheduledTask -TaskName `"$TaskName`""
    Write-Host "Update later by running this installer again; to remove it, run it again and choose Uninstall."
}

# Dot-sourced by the tests, which exercise the helpers without installing anything.
if ($env:ELORA_INSTALLER_LIBRARY -ne "1") {
    Invoke-Installer
}
