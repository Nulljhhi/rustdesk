<#
.SYNOPSIS
    Shows the state of the RustDesk tailnet server: Tailscale, services, listening
    ports, firewall rules, the public key, client settings and recent log lines.

.PARAMETER Lines
    How many recent log lines to show per log file. Default 15.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\status.ps1
#>
[CmdletBinding()]
param([int]$Lines = 15)

$ErrorActionPreference = 'Continue'
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-Admin   # reading the data folder and logs needs it (the folder is locked down)

$state = Read-State
if (-not $state) { Write-Warn "No install state found at $($script:StateFile). Has install.ps1 been run?" }
$ip = Get-StateValue $state 'TailscaleIp4' $null

# --- Tailscale ---------------------------------------------------------------
Write-Step 'Tailscale'
try {
    $ts = Get-TailscaleInfo
    Write-Ok "connected, IPv4 $($ts.Ip4), name $($ts.DnsName), adapter '$($ts.InterfaceAlias)'"
    if ($ip -and $ts.Ip4 -ne $ip) { Write-Warn "Installed for $ip but Tailscale now has $($ts.Ip4). Re-run install.ps1." }
    $u = Get-TailscaleUnattended
    if ($u -eq $false) { Write-Warn '"Run unattended" is OFF (server goes offline when you sign out of Windows).' }
    elseif ($u -eq $true) { Write-Ok '"Run unattended" is on' }
} catch { Write-Warn $_.Exception.Message }

# --- Services ----------------------------------------------------------------
Write-Step 'Services'
foreach ($name in $script:AllServices) {
    $svc = Get-CimInstance -ClassName Win32_Service -Filter "Name='$name'" -ErrorAction SilentlyContinue
    if (-not $svc) { Write-Info ("{0,-15} not installed" -f $name); continue }
    $start = $svc.StartMode
    if ($svc.PSObject.Properties.Name -contains 'DelayedAutoStart' -and $svc.DelayedAutoStart) { $start = 'Auto (delayed)' }
    $line = "{0,-15} {1,-8} start: {2,-15} pid: {3}" -f $name, $svc.State, $start, $svc.ProcessId
    if ($svc.State -eq 'Running') { Write-Ok $line } else { Write-Warn $line }
}

# --- Ports -------------------------------------------------------------------
Write-Step 'Listening ports'
$tcpPorts = @($script:PortBackend) + $script:PortHbbsTcp + $script:PortHbbrTcp + @($script:PortHbbsWs, $script:PortHbbrWs)
$tcp = Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $tcpPorts -contains $_.LocalPort }
$udp = Get-NetUDPEndpoint -ErrorAction SilentlyContinue | Where-Object { $script:PortHbbsUdp -contains $_.LocalPort }
$procNames = @{}
foreach ($p in Get-Process -ErrorAction SilentlyContinue) { $procNames[[int]$p.Id] = $p.ProcessName }
foreach ($c in ($tcp | Sort-Object LocalPort)) {
    $pn = $procNames[[int]$c.OwningProcess]
    Write-Info ("TCP {0,-40} {1}" -f ("$($c.LocalAddress):$($c.LocalPort)"), $pn)
}
foreach ($c in ($udp | Sort-Object LocalPort)) {
    $pn = $procNames[[int]$c.OwningProcess]
    Write-Info ("UDP {0,-40} {1}" -f ("$($c.LocalAddress):$($c.LocalPort)"), $pn)
}
if (-not $tcp -and -not $udp) { Write-Warn 'Nothing is listening on the RustDesk ports.' }
if ($tcp | Where-Object { $_.LocalAddress -eq '0.0.0.0' -or $_.LocalAddress -eq '::' }) {
    Write-Info '(0.0.0.0 / :: means "all interfaces"; the firewall rules restrict access to Tailscale.)'
}

# Quick reachability test from this machine to its own Tailscale address.
if ($ip) {
    foreach ($port in (@($script:PortHbbsTcp) + $script:PortHbbrTcp + @($script:PortBackend))) {
        $client = New-Object System.Net.Sockets.TcpClient
        try {
            $ar = $client.BeginConnect($ip, $port, $null, $null)
            $ok = $ar.AsyncWaitHandle.WaitOne(1500) -and $client.Connected
            if ($ok) { Write-Ok "TCP ${ip}:$port accepts connections" } else { Write-Warn "TCP ${ip}:$port does not answer" }
        } catch { Write-Warn "TCP ${ip}:$port : $($_.Exception.Message)" } finally { $client.Close() }
    }
}

# --- Firewall ----------------------------------------------------------------
Write-Step 'Firewall rules'
$rules = Get-NetFirewallRule -Group $script:FirewallGroup -ErrorAction SilentlyContinue
if (-not $rules) { Write-Warn "No rules in group '$($script:FirewallGroup)'." }
foreach ($r in $rules) {
    $pf = $r | Get-NetFirewallPortFilter
    $af = $r | Get-NetFirewallAddressFilter
    $remote = ($af.RemoteAddress -join ', ')
    if ($remote.Length -gt 40) { $remote = $remote.Substring(0, 40) + '...' }
    Write-Info ("{0,-6} {1,-4} {2,-22} from {3}  [{4}]" -f $r.Action, $pf.Protocol, ($pf.LocalPort -join ','), $remote, $r.DisplayName)
}
foreach ($p in Get-NetFirewallProfile) { if (-not $p.Enabled) { Write-Warn "Firewall profile '$($p.Name)' is OFF." } }

# --- Key and client settings -------------------------------------------------
Write-Step 'Client settings'
if (Test-Path -LiteralPath $script:ClientInfoFile) {
    Get-Content -LiteralPath $script:ClientInfoFile | ForEach-Object { Write-Info $_ }
}
$key = Get-PublicKey
if ($key) { Write-Ok "Current public key: $key" } else { Write-Warn "No id_ed25519.pub in $($script:ServerDataDir) yet." }

# --- Logs --------------------------------------------------------------------
Write-Step "Recent log lines (last $Lines per file)"
function Show-Tail([string]$Path) {
    if (Test-Path -LiteralPath $Path) {
        Write-Host "  --- $Path" -ForegroundColor DarkGray
        Get-Content -LiteralPath $Path -Tail $Lines -ErrorAction SilentlyContinue | ForEach-Object { Write-Info $_ }
    }
}
foreach ($id in @($script:SvcHbbs, $script:SvcHbbr)) {
    foreach ($kind in @('out', 'err', 'wrapper')) { Show-Tail (Join-Path $script:LogDir "$id.$kind.log") }
}
$latestBackendLog = Get-ChildItem -LiteralPath $script:BackendLogDir -File -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($latestBackendLog) { Show-Tail $latestBackendLog.FullName } else { Write-Info "No rdbackend log files in $($script:BackendLogDir)" }
Show-Tail (Join-Path $script:LogDir 'watchdog.log')

# Service-manager events (crashes, failed starts) from the last day.
$events = Get-WinEvent -FilterHashtable @{ LogName = 'System'; ProviderName = 'Service Control Manager'; StartTime = (Get-Date).AddDays(-1) } -ErrorAction SilentlyContinue |
    Where-Object { $_.Message -match 'rustdesk|rdbackend|RustDesk' } | Select-Object -First 10
if ($events) {
    Write-Host '  --- Service Control Manager events (last 24h)' -ForegroundColor DarkGray
    foreach ($e in $events) { Write-Info ("{0}  {1}" -f $e.TimeCreated.ToString('s'), ($e.Message -replace '\s+', ' ')) }
}
