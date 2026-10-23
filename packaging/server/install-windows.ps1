<#
.SYNOPSIS
    Installs the EloraNotes collaboration server on this Windows machine.

.DESCRIPTION
    Asks for the name people see in their sidebar, the port, and the password everyone connects
    with, then installs the binary, creates the notes folder, generates the certificate, opens the
    port in Windows Firewall and registers a task that starts the server at boot.

    Run it from an Administrator PowerShell:
        powershell -ExecutionPolicy Bypass -File .\install-windows.ps1
#>

$ErrorActionPreference = "Stop"

$ReleaseUrl = "https://github.com/noxilith/EloraNotes/releases/latest/download/elora-server-windows-x86_64.exe"
$InstallDir = Join-Path $env:ProgramData "EloraNotes Server"
$Binary = Join-Path $InstallDir "elora-server.exe"
$Runner = Join-Path $InstallDir "run-server.cmd"
$TaskName = "EloraNotes Server"
$FirewallRule = "EloraNotes Server"

function Fail($Message) {
    Write-Host "error: $Message" -ForegroundColor Red
    exit 1
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Fail "open PowerShell as Administrator and run this script again"
}

Write-Host "EloraNotes server setup" -ForegroundColor Cyan
Write-Host ""

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

# --- the binary ---------------------------------------------------------------------------
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$localBinary = Join-Path $PSScriptRoot "elora-server.exe"
if (-not (Test-Path $localBinary)) {
    $localBinary = Join-Path $PSScriptRoot "elora-server-windows-x86_64.exe"
}
if (Test-Path $localBinary) {
    Write-Host "Using $localBinary"
    Copy-Item -Force $localBinary $Binary
} else {
    Write-Host "Downloading the server from the latest EloraNotes release..."
    try {
        Invoke-WebRequest -Uri $ReleaseUrl -OutFile $Binary -UseBasicParsing
    } catch {
        Fail "could not download the server: $($_.Exception.Message). Put elora-server.exe next to this script instead."
    }
}

# Stop an older install before replacing its files.
& schtasks /query /tn $TaskName *> $null
if ($LASTEXITCODE -eq 0) {
    & schtasks /end /tn $TaskName *> $null
    Start-Sleep -Seconds 2
}

# --- settings + certificate ---------------------------------------------------------------
Write-Host "Writing the settings and the certificate..."
$env:ELORA_SERVER_DIR = $InstallDir
$password | & $Binary init --name $name --port $port --password-stdin
if ($LASTEXITCODE -ne 0) { Fail "elora-server init failed" }

# --- start at boot ------------------------------------------------------------------------
# Windows services need a program that speaks the service protocol, so this uses a scheduled
# task instead: it starts at boot as SYSTEM and keeps the server's own folder as its config.
$runnerContent = @"
@echo off
set ELORA_SERVER_DIR=$InstallDir
"$Binary" run
"@
Set-Content -Path $Runner -Value $runnerContent -Encoding ASCII

& schtasks /delete /tn $TaskName /f *> $null
& schtasks /create /tn $TaskName /tr "`"$Runner`"" /sc onstart /ru SYSTEM /rl HIGHEST /f | Out-Null
if ($LASTEXITCODE -ne 0) { Fail "could not register the startup task" }
& schtasks /run /tn $TaskName | Out-Null

# --- firewall -----------------------------------------------------------------------------
Get-NetFirewallRule -DisplayName $FirewallRule -ErrorAction SilentlyContinue |
    Remove-NetFirewallRule -ErrorAction SilentlyContinue
New-NetFirewallRule -DisplayName $FirewallRule -Direction Inbound -Protocol TCP `
    -LocalPort $port -Action Allow -Profile Any | Out-Null

Start-Sleep -Seconds 2

# --- what to tell everyone else -----------------------------------------------------------
$fingerprint = (& $Binary fingerprint 2>$null | Select-Object -First 1)
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
Write-Host "Notes live in $InstallDir\notes."
Write-Host "Manage it with: Start-ScheduledTask -TaskName `"$TaskName`" / Stop-ScheduledTask -TaskName `"$TaskName`""
