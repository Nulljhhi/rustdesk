<#
.SYNOPSIS
    Backs up the server key pair, the hbbs database and the backend database into a
    zip file. The services are stopped for a few seconds so the databases are
    copied in a consistent state.

.PARAMETER Destination
    Folder for the zip file. Default: C:\ProgramData\RustDeskServer\backups.
    Copy the zip somewhere else as well (USB stick, another machine): a backup on the
    same disk does not survive a disk failure.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\backup.ps1 -Destination D:\RustDeskBackups
#>
[CmdletBinding()]
param([string]$Destination)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-Admin
Assert-64BitProcess

if (-not $Destination) { $Destination = $script:BackupDir }
New-DirectoryIfMissing $Destination

$stamp = (Get-Date).ToString('yyyyMMdd-HHmmss')
$zip = Join-Path $Destination "rustdesk-server-backup-$stamp.zip"
$staging = Join-Path $env:TEMP "rustdesk-backup-$stamp"
New-DirectoryIfMissing $staging

$wasRunning = @()
foreach ($n in $script:AllServices) {
    $s = Get-Service -Name $n -ErrorAction SilentlyContinue
    if ($s -and $s.Status -eq 'Running') { $wasRunning += $n }
}

try {
    Write-Step 'Stopping services for a consistent copy'
    foreach ($n in @($script:SvcBackend, $script:SvcHbbr, $script:SvcHbbs)) { Stop-ServiceSafe $n }

    Write-Step 'Copying data'
    foreach ($d in @($script:ServerDataDir, $script:BackendDir)) {
        if (Test-Path -LiteralPath $d) {
            Copy-Item -LiteralPath $d -Destination (Join-Path $staging (Split-Path $d -Leaf)) -Recurse -Force
            Write-Ok $d
        }
    }
    foreach ($f in @($script:StateFile, $script:ClientInfoFile)) {
        if (Test-Path -LiteralPath $f) { Copy-Item -LiteralPath $f -Destination $staging -Force }
    }
} finally {
    Write-Step 'Starting services again'
    foreach ($n in @($script:SvcHbbs, $script:SvcHbbr, $script:SvcBackend)) {
        if ($wasRunning -contains $n) {
            try { Start-Service -Name $n -ErrorAction Stop; Write-Ok "$n started" } catch { Write-Warn "$n did not start: $($_.Exception.Message)" }
        }
    }
}

Compress-Archive -Path (Join-Path $staging '*') -DestinationPath $zip -Force
Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
Write-Host ''
Write-Host "Backup written to $zip" -ForegroundColor White
Write-Host 'It contains the server PRIVATE key (server\id_ed25519). Keep it somewhere safe and private.' -ForegroundColor Yellow
