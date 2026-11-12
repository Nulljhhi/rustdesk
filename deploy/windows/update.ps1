<#
.SYNOPSIS
    Updates hbbs/hbbr to another rustdesk-server release and/or replaces rdbackend.exe.
    Makes a backup first, then runs install.ps1, which keeps all data and settings.

.PARAMETER Version
    rustdesk-server release tag, e.g. 1.1.16. Default: the version currently installed.

.PARAMETER Sha256
    SHA-256 of rustdesk-server-windows-x86_64-unsigned.zip for that version, as shown on
    the GitHub release page. Needed for versions install.ps1 has no pinned hash for,
    unless GitHub publishes a digest for the file.

.PARAMETER BackendExe
    New rdbackend.exe to install. Leave out to keep the current one.

.PARAMETER ZipPath
    Use an already-downloaded release zip.

.PARAMETER SkipBackup
    Do not run backup.ps1 first.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\update.ps1 -BackendExe C:\Users\me\Downloads\rdbackend.exe
.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\update.ps1 -Version 1.1.17 -Sha256 <hash from the release page>
#>
[CmdletBinding()]
param(
    [string]$Version,
    [string]$Sha256,
    [string]$BackendExe,
    [string]$ZipPath,
    [switch]$SkipBackup
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-Admin
Assert-64BitProcess

$state = Read-State
if (-not $state) {
    Write-Host 'Nothing is installed yet. Run install.ps1 instead.' -ForegroundColor Red
    exit 1
}
if (-not $Version) { $Version = Get-StateValue $state 'Version' '1.1.16' }
Write-Step "Updating to rustdesk-server $Version (installed: $(Get-StateValue $state 'Version' 'unknown'))"

if (-not $SkipBackup) {
    & (Join-Path $PSScriptRoot 'backup.ps1')
}

$installArgs = @{ Version = $Version }
if ($Sha256)     { $installArgs.Sha256 = $Sha256 }
if ($BackendExe) { $installArgs.BackendExe = $BackendExe }
if ($ZipPath)    { $installArgs.ZipPath = $ZipPath }
& (Join-Path $PSScriptRoot 'install.ps1') @installArgs
