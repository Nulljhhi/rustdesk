<#
.SYNOPSIS
    Removes the RustDesk tailnet server: services, firewall rules, the watchdog task
    and the program files. Data (server key, databases, logs) is kept unless
    -PurgeData is given.

.PARAMETER PurgeData
    Also delete C:\ProgramData\RustDeskServer: the server key pair, both databases,
    backups and logs. Clients configured with the old key will stop working.

.PARAMETER Force
    With -PurgeData: do not ask for confirmation.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\uninstall.ps1
.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\uninstall.ps1 -PurgeData
#>
[CmdletBinding()]
param(
    [switch]$PurgeData,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-Admin
Assert-64BitProcess

if ($PurgeData -and -not $Force) {
    Write-Host "This deletes $($script:DataRoot), including the server's private key and the databases." -ForegroundColor Yellow
    Write-Host 'Every RustDesk client would then need the new key after a reinstall.'
    $answer = Read-Host 'Type YES to continue'
    if ($answer -ne 'YES') { Write-Host 'Cancelled.'; exit 1 }
}

Write-Step 'Removing scheduled task'
Unregister-ScheduledTask -TaskName $script:WatchdogTask -Confirm:$false -ErrorAction SilentlyContinue
Write-Ok 'done'

Write-Step 'Removing services'
foreach ($n in @($script:SvcBackend, $script:SvcHbbr, $script:SvcHbbs)) {
    if (Test-ServicePresent $n) { Remove-ServiceSafe $n; Write-Ok "$n removed" } else { Write-Info "$n not installed" }
}

Write-Step 'Removing firewall rules'
$rules = Get-NetFirewallRule -Group $script:FirewallGroup -ErrorAction SilentlyContinue
if ($rules) { $rules | Remove-NetFirewallRule; Write-Ok "$(@($rules).Count) rule(s) removed" } else { Write-Info 'none found' }

Write-Step 'Removing program files'
if (Test-Path -LiteralPath $script:InstallDir) {
    Remove-Item -LiteralPath $script:InstallDir -Recurse -Force
    Write-Ok "$($script:InstallDir) removed"
} else { Write-Info 'not present' }

if ($PurgeData) {
    Write-Step 'Deleting data'
    if (Test-Path -LiteralPath $script:DataRoot) {
        Remove-Item -LiteralPath $script:DataRoot -Recurse -Force
        Write-Ok "$($script:DataRoot) deleted"
    }
} else {
    Write-Host ''
    Write-Host "Data kept in $($script:DataRoot) (key, databases, logs)." -ForegroundColor White
    Write-Host 'Running install.ps1 again will reuse it, so clients keep working with the same key.'
}
