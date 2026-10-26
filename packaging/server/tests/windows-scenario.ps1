# Runs the real installer once, in a process of its own (the installer exits the process when it
# fails), with the Windows-only commands replaced by windows-mocks.ps1.
$env:ELORA_INSTALLER_LIBRARY = "1"
. $env:ELORA_TEST_INSTALLER
. (Join-Path $PSScriptRoot "windows-mocks.ps1")
Remove-Item Env:\ELORA_INSTALLER_LIBRARY

# Windows PowerShell 5.1 pipes text to programs as ASCII; leaving that in place means the
# password test only passes if the installer really sets its own encoding.
$global:OutputEncoding = [Text.Encoding]::ASCII

function Test-IsAdministrator { $true }
Invoke-Installer
