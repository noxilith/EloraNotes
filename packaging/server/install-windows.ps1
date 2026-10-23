<#
.SYNOPSIS
    Installs the EloraNotes collaboration server on this Windows machine.

.DESCRIPTION
    Asks for the name people see in their sidebar, the port, and the password everyone connects
    with, then installs the binary, creates the notes folder, generates the certificate, opens the
    port in Windows Firewall and registers a task that starts the server at boot.

    If a server is already installed it says what it found and asks whether to update it,
    uninstall it while keeping the notes, or uninstall it and delete the notes and certificate.
    Set $env:ELORA_ACTION to update, uninstall or purge to skip that question.

    Run it from an Administrator PowerShell:
        powershell -ExecutionPolicy Bypass -File .\install-windows.ps1
#>

$ErrorActionPreference = "Stop"

$ReleaseUrl = "https://github.com/noxilith/EloraNotes/releases/latest/download/elora-server-windows-x86_64.exe"
$InstallDir = Join-Path $env:ProgramData "EloraNotes Server"
$Binary = Join-Path $InstallDir "elora-server.exe"
$Runner = Join-Path $InstallDir "run-server.cmd"
$ConfigPath = Join-Path $InstallDir "config.json"
$TaskName = "EloraNotes Server"
$FirewallRule = "EloraNotes Server"
$env:ELORA_SERVER_DIR = $InstallDir

function Fail($Message) {
    Write-Host "error: $Message" -ForegroundColor Red
    exit 1
}

# Windows PowerShell turns anything a native program writes to stderr into an error record, and
# with ErrorActionPreference = Stop the script dies on it -- even when the text was headed for
# $null. "schtasks: the task does not exist" is how that shows up here, and it is not an error.
# Native calls therefore go through this helper, which keeps their messages out of PowerShell's
# error stream and hands back the exit code and the text instead.
function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$Arguments = @(),
        [string]$StandardInput
    )
    $previous = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        if ($PSBoundParameters.ContainsKey("StandardInput")) {
            $lines = @($StandardInput | & $FilePath @Arguments 2>&1 | ForEach-Object { "$_" })
        } else {
            $lines = @(& $FilePath @Arguments 2>&1 | ForEach-Object { "$_" })
        }
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }
    [pscustomobject]@{
        ExitCode = $code
        Output   = $lines
    }
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Fail "open PowerShell as Administrator and run this script again"
}

Write-Host "EloraNotes server setup" -ForegroundColor Cyan
Write-Host ""

# --- is a server already installed here? ----------------------------------------------------
# Checked before anything is asked or downloaded: an update must never look like a fresh install,
# and uninstalling has to know where the old files and the startup task are.
$TaskExists = $false
$DataExists = Test-Path $ConfigPath
$Existing = $DataExists -or (Test-Path $Binary)

$query = Invoke-Native "schtasks" @("/query", "/tn", $TaskName)
if ($query.ExitCode -eq 0) {
    $TaskExists = $true
    $Existing = $true
}

# ELORA_ACTION=update|uninstall|purge skips the question (for scripted installs).
$Action = $env:ELORA_ACTION
if ($Action -and @("update", "uninstall", "purge") -notcontains $Action) {
    Fail "ELORA_ACTION must be 'update', 'uninstall' or 'purge', not '$Action'"
}

$NotesDir = Join-Path $InstallDir "notes"
if ($Existing) {
    # A settings file can point the notes somewhere else; the default is beside the binary.
    if ($DataExists) {
        try {
            $saved = Get-Content -Raw -Path $ConfigPath | ConvertFrom-Json
            if ($saved.notes_dir) { $NotesDir = $saved.notes_dir }
        } catch {
            Write-Host "warning: could not read $ConfigPath; assuming the usual notes folder" -ForegroundColor Yellow
        }
    }

    $noteCount = 0
    if (Test-Path $NotesDir) {
        $noteCount = @(Get-ChildItem -Path $NotesDir -Filter *.md -File -ErrorAction SilentlyContinue).Count
    }

    $code = ""
    if ((Test-Path $Binary) -and $DataExists) {
        try {
            $fp = Invoke-Native $Binary @("fingerprint")
            if ($fp.ExitCode -eq 0 -and $fp.Output.Count -gt 0) {
                $code = $fp.Output[0]
            }
        } catch {
            # A binary that will not run is one reason to be running this installer again, so the
            # code is simply left out of the summary below.
        }
    }

    Write-Host "EloraNotes server is already installed on this machine:"
    if ($TaskExists) {
        Write-Host "  service:  scheduled task `"$TaskName`" (starts at boot)"
    } else {
        Write-Host "  service:  none found"
    }
    if (Test-Path $Binary) {
        Write-Host "  binary:   $Binary"
    }
    if ($DataExists) {
        Write-Host "  settings: $InstallDir"
        Write-Host "  notes:    $noteCount in $NotesDir"
    }
    if ($code) {
        Write-Host "  code:     $code (clients already trust this certificate)"
    }

    if (-not $Action) {
        Write-Host ""
        Write-Host "What would you like to do?"
        Write-Host "  1) Update it, keeping the notes and the certificate  (recommended)"
        Write-Host "  2) Uninstall it, keeping the notes on disk"
        Write-Host "  3) Uninstall it and delete the notes and certificate"
        Write-Host "  4) Cancel"
        while ($true) {
            $choice = ""
            try {
                $choice = Read-Host "Choice [1]"
            } catch {
                Write-Host "No answer could be read, so the existing install is updated."
            }
            if ([string]::IsNullOrWhiteSpace($choice)) { $choice = "1" }
            switch ($choice) {
                "1" { $Action = "update" }
                "2" { $Action = "uninstall" }
                "3" { $Action = "purge" }
                "4" { $Action = "cancel" }
                default { Write-Host "  Type 1, 2, 3 or 4." }
            }
            if ($Action) { break }
        }
    }

    if ($Action -eq "cancel") {
        Write-Host "Nothing was changed."
        exit 0
    }

    if ($Action -eq "uninstall" -or $Action -eq "purge") {
        Write-Host ""
        Write-Host "Uninstalling..."
        if ($TaskExists) {
            Invoke-Native "schtasks" @("/end", "/tn", $TaskName) | Out-Null
            Invoke-Native "schtasks" @("/delete", "/tn", $TaskName, "/f") | Out-Null
            $after = Invoke-Native "schtasks" @("/query", "/tn", $TaskName)
            if ($after.ExitCode -eq 0) {
                Fail "could not remove the scheduled task `"$TaskName`""
            }
            Write-Host "  Removed the scheduled task `"$TaskName`""
        }
        Get-Process -Name "elora-server" -ErrorAction SilentlyContinue |
            Stop-Process -Force -ErrorAction SilentlyContinue
        foreach ($path in @($Binary, $Runner)) {
            if (Test-Path $path) {
                Remove-Item -Force $path -ErrorAction SilentlyContinue
                Write-Host "  Removed $path"
            }
        }
        Get-NetFirewallRule -DisplayName $FirewallRule -ErrorAction SilentlyContinue |
            Remove-NetFirewallRule -ErrorAction SilentlyContinue

        if ($Action -eq "purge") {
            if (Test-Path $InstallDir) {
                Remove-Item -Recurse -Force $InstallDir
                Write-Host "  Deleted $InstallDir"
            }
            Write-Host ""
            Write-Host "The server and everything it held are gone."
        } else {
            Write-Host ""
            Write-Host "The server is gone; the notes are still in $NotesDir."
            Write-Host "Copy that folder somewhere safe if you want to keep them."
        }
        Write-Host "Run this installer again to set a server up from scratch."
        exit 0
    }

    Write-Host ""
    Write-Host "Updating it; the notes and the certificate stay where they are."
}

# --- what the server should be called -------------------------------------------------------
$name = Read-Host "Server name (shown in everyone's sidebar) [EloraNotes]"
if ([string]::IsNullOrWhiteSpace($name)) { $name = "EloraNotes" }

while ($true) {
    $portInput = Read-Host "Port [7331]"
    if ([string]::IsNullOrWhiteSpace($portInput)) { $portInput = "7331" }
    $port = 0
    if (-not [int]::TryParse($portInput, [ref]$port) -or $port -lt 1024 -or $port -gt 65535) {
        Write-Host "  Pick a port between 1024 and 65535."
        continue
    }
    break
}

$secure = Read-Host "Password everyone will connect with" -AsSecureString
$again = Read-Host "Repeat the password" -AsSecureString
$password = [Runtime.InteropServices.Marshal]::PtrToStringAuto(
    [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure))
$passwordAgain = [Runtime.InteropServices.Marshal]::PtrToStringAuto(
    [Runtime.InteropServices.Marshal]::SecureStringToBSTR($again))
if ([string]::IsNullOrEmpty($password)) { Fail "the password cannot be empty" }
if ($password -ne $passwordAgain) { Fail "the passwords did not match" }

# --- stop the old server before its files are replaced --------------------------------------
# Windows refuses to overwrite the executable of a running program, so the task is stopped here
# rather than after the new binary has been copied in.
if ($TaskExists) {
    Invoke-Native "schtasks" @("/end", "/tn", $TaskName) | Out-Null
}
Get-Process -Name "elora-server" -ErrorAction SilentlyContinue |
    Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

# --- the binary -----------------------------------------------------------------------------
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$localBinary = Join-Path $PSScriptRoot "elora-server.exe"
if (-not (Test-Path $localBinary)) {
    $localBinary = Join-Path $PSScriptRoot "elora-server-windows-x86_64.exe"
}
# The download goes to a temporary file first, so a failed transfer can never leave a
# half-written server in place.
$staged = Join-Path $env:TEMP "elora-server-download.exe"
if (Test-Path $localBinary) {
    Write-Host "Using $localBinary"
    Copy-Item -Force $localBinary $staged
} else {
    Write-Host "Downloading the server from the latest EloraNotes release..."
    try {
        Invoke-WebRequest -Uri $ReleaseUrl -OutFile $staged -UseBasicParsing
    } catch {
        Fail "could not download the server: $($_.Exception.Message). Put elora-server.exe next to this script instead."
    }
}
if (-not (Test-Path $staged) -or (Get-Item $staged).Length -lt 1000000) {
    Fail "the downloaded server looks incomplete; check the connection and try again"
}
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
$runs = $false
try {
    $check = Invoke-Native $Binary @("--help")
    $runs = $check.ExitCode -eq 0
} catch {
    $runs = $false
}
if (-not $runs) {
    Fail "$Binary does not run on this machine (wrong CPU architecture or a corrupt download)"
}

# --- settings + certificate -----------------------------------------------------------------
Write-Host "Writing the settings and the certificate..."
$init = Invoke-Native $Binary @("init", "--name", $name, "--port", "$port", "--password-stdin") -StandardInput $password
if ($init.ExitCode -ne 0) {
    foreach ($line in $init.Output) { Write-Host "  $line" }
    Fail "elora-server init failed"
}

# --- start at boot --------------------------------------------------------------------------
# Windows services need a program that speaks the service protocol, so this uses a scheduled
# task instead: it starts at boot as SYSTEM and keeps the server's own folder as its config.
$runnerContent = @"
@echo off
set ELORA_SERVER_DIR=$InstallDir
"$Binary" run
"@
Set-Content -Path $Runner -Value $runnerContent -Encoding ASCII

Invoke-Native "schtasks" @("/delete", "/tn", $TaskName, "/f") | Out-Null
$create = Invoke-Native "schtasks" @("/create", "/tn", $TaskName, "/tr", "`"$Runner`"", "/sc", "onstart", "/ru", "SYSTEM", "/rl", "HIGHEST", "/f")
if ($create.ExitCode -ne 0) {
    foreach ($line in $create.Output) { Write-Host "  $line" }
    Fail "could not register the startup task"
}
$start = Invoke-Native "schtasks" @("/run", "/tn", $TaskName)
if ($start.ExitCode -ne 0) {
    foreach ($line in $start.Output) { Write-Host "  $line" }
    Fail "registered the startup task, but could not start it"
}

# --- firewall -------------------------------------------------------------------------------
Get-NetFirewallRule -DisplayName $FirewallRule -ErrorAction SilentlyContinue |
    Remove-NetFirewallRule -ErrorAction SilentlyContinue
New-NetFirewallRule -DisplayName $FirewallRule -Direction Inbound -Protocol TCP `
    -LocalPort $port -Action Allow -Profile Any | Out-Null

Start-Sleep -Seconds 2

# --- what to tell everyone else -------------------------------------------------------------
$fingerprint = ""
$fp = Invoke-Native $Binary @("fingerprint")
if ($fp.ExitCode -eq 0 -and $fp.Output.Count -gt 0) { $fingerprint = $fp.Output[0] }
$addresses = Get-NetIPAddress -AddressFamily IPv4 |
    Where-Object { $_.IPAddress -notlike "127.*" -and $_.IPAddress -notlike "169.254.*" } |
    Select-Object -ExpandProperty IPAddress

Write-Host ""
Write-Host "EloraNotes server `"$name`" is running on port $port." -ForegroundColor Green
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
Write-Host "Notes live in $NotesDir."
Write-Host "Manage it with: Start-ScheduledTask -TaskName `"$TaskName`" / Stop-ScheduledTask -TaskName `"$TaskName`""
