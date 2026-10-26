# Stand-ins for the Windows-only commands install-windows.ps1 uses, so the real installer can be
# run on any machine with PowerShell. Each one records what it was asked to do under
# $env:ELORA_TEST_DIR; the installer's own code is the code under test.

$TestDir = $env:ELORA_TEST_DIR

function Add-Call($Text) { Add-Content -Path (Join-Path $TestDir "calls.log") -Value $Text }

# The download: a file from the test's "release" folder, as the installer asked for it by name.
function Invoke-WebRequest {
    param([string]$Uri, [string]$OutFile, [switch]$UseBasicParsing)
    $source = Join-Path (Join-Path $TestDir "release") (Split-Path $Uri -Leaf)
    if (-not (Test-Path $source)) { throw "404 Not Found: $Uri" }
    Copy-Item -Force $source $OutFile
    chmod +x $OutFile
}

# These take the real cmdlets' parameters, so a misspelt or invalid one in the installer is an error.
function New-ScheduledTaskAction {
    param([Parameter(Mandatory = $true)][string]$Execute, [string]$Argument, [string]$WorkingDirectory)
    [pscustomobject]$PSBoundParameters
}
function New-ScheduledTaskTrigger {
    param([switch]$AtStartup, [switch]$AtLogOn, [switch]$Once)
    [pscustomobject]$PSBoundParameters
}
function New-ScheduledTaskPrincipal {
    param(
        [string]$UserId,
        [ValidateSet("Group", "Interactive", "InteractiveOrPassword", "None", "Password", "S4U", "ServiceAccount")][string]$LogonType,
        [ValidateSet("Highest", "Limited")][string]$RunLevel
    )
    [pscustomobject]$PSBoundParameters
}
function New-ScheduledTaskSettingsSet {
    param(
        [switch]$AllowStartIfOnBatteries,
        [switch]$DontStopIfGoingOnBatteries,
        [switch]$StartWhenAvailable,
        [ValidateSet("Parallel", "Queue", "IgnoreNew")][string]$MultipleInstances,
        [TimeSpan]$ExecutionTimeLimit,
        [ValidateRange(1, 999)][int]$RestartCount,
        [TimeSpan]$RestartInterval
    )
    [pscustomobject]$PSBoundParameters
}

function Register-ScheduledTask {
    param([string]$TaskName, $Action, $Trigger, $Principal, $Settings, [string]$Description, [switch]$Force)
    [pscustomobject]@{
        TaskName = $TaskName; Action = $Action; Trigger = $Trigger
        Principal = $Principal; Settings = $Settings
    } | ConvertTo-Json -Depth 6 | Set-Content -Path (Join-Path $TestDir "task.json")
    Add-Call "Register-ScheduledTask $TaskName"
}

function Get-ScheduledTask {
    param($TaskName)
    $file = Join-Path $TestDir "task.json"
    if (Test-Path $file) { return (Get-Content -Raw $file | ConvertFrom-Json) }
}

function Unregister-ScheduledTask {
    param($TaskName, $Confirm)
    Remove-Item -Force (Join-Path $TestDir "task.json") -ErrorAction SilentlyContinue
    Add-Call "Unregister-ScheduledTask $TaskName"
}

function Stop-ScheduledTask { param($TaskName) Add-Call "Stop-ScheduledTask $TaskName" }

# Starting the task starts the real server, the way Task Scheduler would run the runner.
function Start-ScheduledTask {
    param($TaskName)
    Add-Call "Start-ScheduledTask $TaskName"
    if (-not (Test-Path (Join-Path $TestDir "task.json"))) { throw "no such task" }
    $process = Start-Process -FilePath $Binary -ArgumentList @("run") -PassThru `
        -RedirectStandardOutput (Join-Path $TestDir "server.out") `
        -RedirectStandardError (Join-Path $TestDir "server.err")
    Set-Content -Path (Join-Path $TestDir "server.pid") -Value $process.Id
}

function New-NetFirewallRule {
    param(
        [string]$DisplayName,
        [ValidateSet("Inbound", "Outbound")][string]$Direction,
        [string]$Protocol,
        [int]$LocalPort,
        [ValidateSet("Allow", "Block")][string]$Action,
        [string]$Profile,
        [string]$Program
    )
    Add-Content -Path (Join-Path $TestDir "firewall.txt") -Value "$LocalPort|$Program|$Profile"
    Add-Call "New-NetFirewallRule $LocalPort"
}
function Get-NetFirewallRule {
    param($DisplayName)
    $file = Join-Path $TestDir "firewall.txt"
    if ((Test-Path $file) -and (Get-Content $file)) { [pscustomobject]@{ DisplayName = $DisplayName } }
}
function Remove-NetFirewallRule {
    param([Parameter(ValueFromPipeline = $true)]$InputObject)
    process {
        Remove-Item -Force (Join-Path $TestDir "firewall.txt") -ErrorAction SilentlyContinue
        Add-Call "Remove-NetFirewallRule"
    }
}

function Get-NetIPAddress {
    param($AddressFamily)
    [pscustomobject]@{ IPAddress = "192.168.1.5"; InterfaceAlias = "Ethernet" }
    [pscustomobject]@{ IPAddress = "172.20.0.1"; InterfaceAlias = "vEthernet (WSL)" }
    [pscustomobject]@{ IPAddress = "127.0.0.1"; InterfaceAlias = "Loopback Pseudo-Interface 1" }
    [pscustomobject]@{ IPAddress = "169.254.9.9"; InterfaceAlias = "Ethernet 2" }
}

function Unblock-File { param($Path) Add-Call "Unblock-File $Path" }

# icacls is a program, but the installer reaches it by name, so a function of that name stands in.
function icacls {
    Add-Call "icacls $($args -join ' ')"
    $global:LASTEXITCODE = 0
}
