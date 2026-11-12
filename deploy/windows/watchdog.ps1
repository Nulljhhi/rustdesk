<#
    Watchdog for the RustDesk tailnet server. install.ps1 copies this file to
    C:\Program Files\RustDeskServer and runs it as SYSTEM every 5 minutes
    (scheduled task "RustDeskServer-Watchdog"). You do not run it by hand.

    It covers the cases the Windows service manager's restart-on-failure does not:
      * a service that is stopped but should run (for example because a service it
        depends on was stopped and started again);
      * rdbackend still running but no longer listening on the Tailscale address
        (for example after the Tailscale adapter went away and came back).
#>
$ErrorActionPreference = 'Continue'
. (Join-Path $PSScriptRoot 'common.ps1')

$log = Join-Path $script:LogDir 'watchdog.log'
function Add-WatchdogLog([string]$Message) {
    try {
        $line = '{0}  {1}' -f (Get-Date).ToString('s'), $Message
        Add-Content -LiteralPath $log -Value $line
        # keep the file small
        if ((Get-Item -LiteralPath $log).Length -gt 1MB) {
            $tail = Get-Content -LiteralPath $log -Tail 2000
            Set-Content -LiteralPath $log -Value $tail
        }
    } catch { }
}

$state = Read-State
if (-not $state) { exit 0 }
$ip = Get-StateValue $state 'TailscaleIp4' $null
$tsUp = $false
if ($ip) { $tsUp = [bool](Get-NetIPAddress -IPAddress $ip -ErrorAction SilentlyContinue) }

foreach ($name in $script:AllServices) {
    $svc = Get-CimInstance -ClassName Win32_Service -Filter "Name='$name'" -ErrorAction SilentlyContinue
    if (-not $svc) { continue }
    if ($svc.StartMode -ne 'Auto') { continue }   # someone set it to Manual/Disabled on purpose

    if ($svc.State -eq 'Stopped') {
        if ($name -eq $script:SvcBackend -and -not $tsUp) { continue }   # it would only fail to bind
        Add-WatchdogLog "$name was stopped; starting it"
        Start-Service -Name $name -ErrorAction SilentlyContinue
        continue
    }

    if ($name -eq $script:SvcBackend -and $svc.State -eq 'Running' -and $tsUp) {
        # Give a freshly started process a minute before judging it.
        $proc = Get-Process -Id $svc.ProcessId -ErrorAction SilentlyContinue
        if ($proc -and ((Get-Date) - $proc.StartTime).TotalSeconds -lt 60) { continue }
        $listening = Get-NetTCPConnection -State Listen -LocalPort $script:PortBackend -ErrorAction SilentlyContinue |
            Where-Object { $_.LocalAddress -eq $ip -or $_.LocalAddress -eq '0.0.0.0' -or $_.LocalAddress -eq '::' }
        if (-not $listening) {
            Add-WatchdogLog "$name is running but not listening on ${ip}:$($script:PortBackend); restarting it"
            Restart-Service -Name $name -Force -ErrorAction SilentlyContinue
        }
    }
}

if ($ip -and -not $tsUp) {
    # Could be a temporary disconnect, or the Tailscale IP changed (then re-run install.ps1).
    Add-WatchdogLog "Tailscale address $ip is not present on this machine"
}
