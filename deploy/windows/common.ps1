# Shared settings and helpers for the RustDesk tailnet server scripts.
# This file is dot-sourced by install.ps1, uninstall.ps1, status.ps1,
# update.ps1 and backup.ps1. It is not meant to be run on its own.
#
# Compatible with Windows PowerShell 5.1 (the version that ships with
# Windows 10/11). Keep this file ASCII-only: PowerShell 5.1 reads files
# without a byte-order mark as ANSI.

Set-StrictMode -Version 2.0

# ---------------------------------------------------------------------------
# Layout
# ---------------------------------------------------------------------------
$script:InstallDir     = Join-Path $env:ProgramFiles 'RustDeskServer'
$script:DataRoot       = Join-Path $env:ProgramData  'RustDeskServer'
# Working directory of hbbs and hbbr: id_ed25519, id_ed25519.pub, db_v2.sqlite3
$script:ServerDataDir  = Join-Path $script:DataRoot 'server'
# config.toml and the backend database
$script:BackendDir     = Join-Path $script:DataRoot 'backend'
$script:LogDir         = Join-Path $script:DataRoot 'logs'
$script:BackendLogDir  = Join-Path $script:LogDir 'rdbackend'
$script:BackupDir      = Join-Path $script:DataRoot 'backups'
$script:StateFile      = Join-Path $script:DataRoot 'install-state.json'
$script:ClientInfoFile = Join-Path $script:DataRoot 'client-settings.txt'
$script:BackendConfig  = Join-Path $script:BackendDir 'config.toml'

# ---------------------------------------------------------------------------
# Names
# ---------------------------------------------------------------------------
$script:SvcHbbs      = 'rustdesk-hbbs'
$script:SvcHbbr      = 'rustdesk-hbbr'
$script:SvcBackend   = 'rdbackend'
$script:AllServices  = @($script:SvcHbbs, $script:SvcHbbr, $script:SvcBackend)
$script:FirewallGroup = 'RustDesk Tailnet Server'
$script:WatchdogTask  = 'RustDeskServer-Watchdog'
$script:TailscaleServiceName = 'Tailscale'

# ---------------------------------------------------------------------------
# Ports (from rustdesk-server source: src/rendezvous_server.rs, src/relay_server.rs)
#   hbbs: 21116 TCP+UDP (main), 21115 TCP (= 21116-1, NAT type test),
#         21118 TCP (= 21116+2, WebSocket, only needed by the web client)
#   hbbr: 21117 TCP (relay), 21119 TCP (= 21117+2, WebSocket, web client only)
#   rdbackend: 21114 TCP (API server)
# ---------------------------------------------------------------------------
$script:PortHbbsTcp      = @(21115, 21116)
$script:PortHbbsUdp      = @(21116)
$script:PortHbbsWs       = 21118
$script:PortHbbrTcp      = @(21117)
$script:PortHbbrWs       = 21119
$script:PortBackend      = 21114

# Tailscale address ranges: IPv4 CGNAT range and Tailscale's IPv6 ULA prefix.
$script:TailnetRanges = @('100.64.0.0/10', 'fd7a:115c:a1e0::/48')
# Everything else, written as ranges, for the "block non-Tailscale" rules.
$script:NonTailnetRanges = @(
    '0.0.0.0-100.63.255.255',
    '100.128.0.0-255.255.255.255',
    '::-fd7a:115c:a1df:ffff:ffff:ffff:ffff:ffff',
    'fd7a:115c:a1e1::-ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff'
)

# ---------------------------------------------------------------------------
# Output helpers
# ---------------------------------------------------------------------------
function Write-Step([string]$Message) { Write-Host "==> $Message" -ForegroundColor Cyan }
function Write-Ok([string]$Message)   { Write-Host "    OK  $Message" -ForegroundColor Green }
function Write-Warn([string]$Message) { Write-Host "    !!  $Message" -ForegroundColor Yellow }
function Write-Info([string]$Message) { Write-Host "        $Message" }

function Assert-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($id)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        Write-Host ''
        Write-Host 'This script must be run as Administrator.' -ForegroundColor Red
        Write-Host 'Right-click "Windows PowerShell" in the Start menu, choose "Run as administrator",'
        Write-Host 'then run the command again.'
        exit 1
    }
}

function Assert-64BitProcess {
    if (-not [Environment]::Is64BitProcess) {
        Write-Host 'Please use the 64-bit "Windows PowerShell", not "Windows PowerShell (x86)".' -ForegroundColor Red
        exit 1
    }
}

# Write a UTF-8 file without a byte-order mark (TOML and XML parsers are happy with that,
# and PowerShell 5.1's Out-File -Encoding utf8 would add a BOM).
function Write-Utf8NoBom([string]$Path, [string]$Content) {
    $enc = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Content, $enc)
}

function New-DirectoryIfMissing([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) {
        New-Item -ItemType Directory -Path $Path -Force | Out-Null
    }
}

# Data folders hold the server's private key. Only SYSTEM and Administrators get access
# (by default, ProgramData sub-folders are readable by every local user).
# Well-known SIDs are used so this also works on non-English Windows.
function Protect-DataDir([string]$Path) {
    $ErrorActionPreference = 'Continue'
    & icacls.exe $Path /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Warn "icacls failed on $Path (exit $LASTEXITCODE); permissions left unchanged." }
}

# ---------------------------------------------------------------------------
# State file (remembers what install.ps1 did, so the other scripts can find it)
# ---------------------------------------------------------------------------
function Read-State {
    if (Test-Path -LiteralPath $script:StateFile) {
        try { return (Get-Content -LiteralPath $script:StateFile -Raw | ConvertFrom-Json) } catch { return $null }
    }
    return $null
}

function Save-State($State) {
    New-DirectoryIfMissing $script:DataRoot
    Write-Utf8NoBom $script:StateFile ($State | ConvertTo-Json -Depth 5)
}

function Get-StateValue($State, [string]$Name, $Default) {
    if ($null -ne $State -and ($State.PSObject.Properties.Name -contains $Name)) { return $State.$Name }
    return $Default
}

# ---------------------------------------------------------------------------
# Tailscale
# ---------------------------------------------------------------------------
function Get-TailscaleExe {
    $candidates = @(
        (Join-Path $env:ProgramFiles 'Tailscale\tailscale.exe'),
        (Join-Path ${env:ProgramFiles(x86)} 'Tailscale\tailscale.exe')
    )
    foreach ($c in $candidates) { if ($c -and (Test-Path -LiteralPath $c)) { return $c } }
    $cmd = Get-Command tailscale.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

# Returns an object with Ip4, Ip6, DnsName, HostName, InterfaceAlias, BackendState,
# or throws with an explanation a non-developer can act on.
function Get-TailscaleInfo {
    # Native commands that write to stderr must not trip $ErrorActionPreference = 'Stop' (PS 5.1).
    $ErrorActionPreference = 'Continue'
    $svc = Get-Service -Name $script:TailscaleServiceName -ErrorAction SilentlyContinue
    $exe = Get-TailscaleExe
    if (-not $svc -or -not $exe) {
        throw 'Tailscale is not installed. Install it from https://tailscale.com/download/windows, sign in, then run this script again.'
    }
    if ($svc.Status -ne 'Running') {
        throw "The Tailscale Windows service is '$($svc.Status)'. Start Tailscale (or run: Start-Service Tailscale) and sign in, then try again."
    }
    $raw = & $exe status --json 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $raw) {
        throw "'tailscale status --json' failed. Open the Tailscale app and make sure it is connected."
    }
    $st = ($raw | Out-String) | ConvertFrom-Json
    if ($st.BackendState -ne 'Running') {
        throw "Tailscale is not connected (state: $($st.BackendState)). Open the Tailscale app, sign in and connect, then try again."
    }
    $ip4 = $null; $ip6 = $null
    foreach ($ip in @($st.Self.TailscaleIPs)) {
        if ($ip -match '^\d+\.\d+\.\d+\.\d+$') { if (-not $ip4) { $ip4 = $ip } }
        elseif ($ip -like 'fd7a:*') { if (-not $ip6) { $ip6 = $ip } }
    }
    if (-not $ip4) { throw 'Could not find this machine''s Tailscale IPv4 address in "tailscale status".' }

    $dns = $null
    if ($st.Self.PSObject.Properties.Name -contains 'DNSName' -and $st.Self.DNSName) {
        $dns = ([string]$st.Self.DNSName).TrimEnd('.')
    }
    $magicDns = $false
    if ($st.PSObject.Properties.Name -contains 'CurrentTailnet' -and $st.CurrentTailnet -and
        ($st.CurrentTailnet.PSObject.Properties.Name -contains 'MagicDNSEnabled')) {
        $magicDns = [bool]$st.CurrentTailnet.MagicDNSEnabled
    }

    $alias = $null
    $addr = Get-NetIPAddress -IPAddress $ip4 -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($addr) { $alias = $addr.InterfaceAlias }

    return [pscustomobject]@{
        Ip4            = $ip4
        Ip6            = $ip6
        DnsName        = $dns
        MagicDnsEnabled = $magicDns
        HostName       = $st.Self.HostName
        InterfaceAlias = $alias
        BackendState   = $st.BackendState
        Exe            = $exe
    }
}

# Best effort: "Run unattended" keeps Tailscale connected when nobody is logged in.
# Returns $true / $false, or $null if it could not be determined.
function Get-TailscaleUnattended {
    $ErrorActionPreference = 'Continue'
    $exe = Get-TailscaleExe
    if (-not $exe) { return $null }
    try {
        $raw = & $exe debug prefs 2>$null
        if ($LASTEXITCODE -ne 0 -or -not $raw) { return $null }
        $p = ($raw | Out-String) | ConvertFrom-Json
        if ($p.PSObject.Properties.Name -contains 'ForceDaemon') { return [bool]$p.ForceDaemon }
    } catch { }
    return $null
}

# ---------------------------------------------------------------------------
# Services
# ---------------------------------------------------------------------------
function Test-ServicePresent([string]$Name) {
    return [bool](Get-Service -Name $Name -ErrorAction SilentlyContinue)
}

function Stop-ServiceSafe([string]$Name) {
    $s = Get-Service -Name $Name -ErrorAction SilentlyContinue
    if ($s -and $s.Status -ne 'Stopped') {
        Write-Info "Stopping $Name ..."
        try { Stop-Service -Name $Name -Force -ErrorAction Stop } catch { Write-Warn "Stop-Service $Name failed: $($_.Exception.Message)" }
        try { $s.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30)) } catch { Write-Warn "$Name did not stop within 30 seconds." }
    }
}

function Remove-ServiceSafe([string]$Name) {
    if (-not (Test-ServicePresent $Name)) { return }
    Stop-ServiceSafe $Name
    $ErrorActionPreference = 'Continue'
    & sc.exe delete $Name | Out-Null
    # The SCM only really deletes the service once nothing holds a handle to it
    # (e.g. an open "Services" window). Wait a little for that.
    for ($i = 0; $i -lt 20; $i++) {
        if (-not (Test-ServicePresent $Name)) { return }
        Start-Sleep -Milliseconds 500
    }
    if (Test-ServicePresent $Name) {
        throw "Service '$Name' is marked for deletion but still exists. Close the 'Services' window (services.msc) and Task Manager, then run the script again (or reboot)."
    }
}

# Restart-on-failure for any service. reset= is in seconds, delays in milliseconds;
# the last action repeats for every further failure. failureflag 1 makes the actions
# also apply when the service stops by itself with an error exit code (not only on crashes).
function Set-ServiceRecovery([string]$Name) {
    $ErrorActionPreference = 'Continue'
    & sc.exe failure $Name reset= 3600 actions= restart/10000/restart/15000/restart/30000 | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Warn "sc.exe failure $Name returned $LASTEXITCODE" }
    & sc.exe failureflag $Name 1 | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Warn "sc.exe failureflag $Name returned $LASTEXITCODE" }
}

# ---------------------------------------------------------------------------
# Misc
# ---------------------------------------------------------------------------
function Get-PublicKey {
    $pub = Join-Path $script:ServerDataDir 'id_ed25519.pub'
    if (Test-Path -LiteralPath $pub) { return (Get-Content -LiteralPath $pub -Raw).Trim() }
    return $null
}

# "1.1.16" -> [version]1.1.16 ; "1.1.11-1" -> [version]1.1.11 ; garbage -> $null
function ConvertTo-VersionOrNull([string]$Text) {
    $core = ($Text -replace '^[vV]', '') -replace '-.*$', ''
    $v = $null
    if ([version]::TryParse($core, [ref]$v)) { return $v }
    return $null
}
