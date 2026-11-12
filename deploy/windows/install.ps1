<#
.SYNOPSIS
    Installs the RustDesk ID server (hbbs), relay server (hbbr) and the custom API
    backend (rdbackend) as Windows services that are reachable only over Tailscale.

.DESCRIPTION
    Safe to run again: every run brings the machine to the same end state and keeps
    existing data (the server key, the hbbs database and the backend database).

    What it does:
      1. Checks it runs as Administrator, that Tailscale is installed and connected,
         and finds this machine's Tailscale IPv4 address and MagicDNS name.
      2. Downloads the pinned rustdesk-server Windows release and the WinSW service
         wrapper from GitHub and checks their SHA-256 hashes.
      3. Installs hbbs and hbbr as auto-start services (via WinSW) with restart on failure.
      4. Installs rdbackend.exe as a service using its own --service mode and writes
         its config.toml bound to the Tailscale IP.
      5. Creates Windows Firewall rules that allow the server ports only from the
         Tailscale address ranges, on the Tailscale network adapter, and block them
         from everywhere else.
      6. Starts everything and prints the values the RustDesk client needs.

.PARAMETER BackendExe
    Path to rdbackend.exe. Needed on the first install. On later runs it can be left
    out to keep the already-installed copy.

.PARAMETER SkipBackend
    Install only hbbs and hbbr (for example while the backend is not built yet).

.PARAMETER Version
    rustdesk-server release tag to install. Default: 1.1.16.

.PARAMETER Sha256
    Expected SHA-256 of the release zip. Only needed for versions this script does not
    know; otherwise the pinned value is used (or, failing that, the digest GitHub
    publishes for the asset).

.PARAMETER ZipPath
    Use an already-downloaded rustdesk-server-windows-x86_64-unsigned.zip instead of
    downloading it. Its hash is still checked.

.PARAMETER WinSWPath
    Use an already-downloaded WinSW.NET461.exe instead of downloading it. Its hash is
    still checked.

.PARAMETER IncludeWebSocketPorts
    Also open 21118 and 21119 (only needed by the RustDesk *web* client).

.PARAMETER DependOnTailscale
    Make the three services hard-depend on the Tailscale service. Off by default;
    see README.md ("Start-up order") for the trade-off.

.PARAMETER NoWatchdog
    Do not install the scheduled task that restarts stopped services and re-binds
    the backend after Tailscale reconnects.

.PARAMETER AllowUnverifiedDownload
    Continue even when no SHA-256 is known for the chosen version. Not recommended.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\install.ps1 -BackendExe C:\Users\me\Downloads\rdbackend.exe
#>
[CmdletBinding()]
param(
    [string]$BackendExe,
    [switch]$SkipBackend,
    [string]$Version = '1.1.16',
    [string]$Sha256,
    [string]$ZipPath,
    [string]$WinSWPath,
    [switch]$IncludeWebSocketPorts,
    [switch]$DependOnTailscale,
    [switch]$NoWatchdog,
    [switch]$AllowUnverifiedDownload
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is very slow with the progress bar in PS 5.1
. (Join-Path $PSScriptRoot 'common.ps1')

Assert-Admin
Assert-64BitProcess

# GitHub needs TLS 1.2; older .NET defaults on Windows 10 may not offer it.
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch { }

# ---------------------------------------------------------------------------
# Pinned downloads. Hashes were computed from the files on the GitHub release
# pages; the rustdesk-server one also matches the SHA-256 digest GitHub shows
# for that asset.
# ---------------------------------------------------------------------------
$ServerAssetName = 'rustdesk-server-windows-x86_64-unsigned.zip'
$PinnedServerSha256 = @{
    '1.1.16' = 'b865a3a62fc8755b45480c508f1c4871c3338590408dda8c58c7e9c373b7adb0'
}
$WinSWVersion = 'v2.12.0'
$WinSWAsset   = 'WinSW.NET461.exe'   # needs .NET Framework 4.6.1+, built into Windows 10/11
$WinSWSha256  = 'b5066b7bbdfba1293e5d15cda3caaea88fbeab35bd5b38c41c913d492aadfc4f'
$WinSWUrl     = "https://github.com/winsw/winsw/releases/download/$WinSWVersion/$WinSWAsset"

# ---------------------------------------------------------------------------
# Options remembered from the previous run (so a plain re-run keeps them)
# ---------------------------------------------------------------------------
$prev = Read-State
if (-not $PSBoundParameters.ContainsKey('IncludeWebSocketPorts')) { $IncludeWebSocketPorts = [bool](Get-StateValue $prev 'IncludeWebSocketPorts' $false) }
if (-not $PSBoundParameters.ContainsKey('DependOnTailscale'))     { $DependOnTailscale     = [bool](Get-StateValue $prev 'DependOnTailscale' $false) }
if (-not $PSBoundParameters.ContainsKey('NoWatchdog'))            { $NoWatchdog            = [bool](Get-StateValue $prev 'NoWatchdog' $false) }
if (-not $PSBoundParameters.ContainsKey('SkipBackend'))           { $SkipBackend           = [bool](Get-StateValue $prev 'SkipBackend' $false) }

# ---------------------------------------------------------------------------
# 1. Tailscale
# ---------------------------------------------------------------------------
Write-Step 'Checking Tailscale'
try { $ts = Get-TailscaleInfo } catch { Write-Host "    $($_.Exception.Message)" -ForegroundColor Red; exit 1 }
Write-Ok "Tailscale is connected. IPv4: $($ts.Ip4)"
if ($ts.DnsName) { Write-Ok "MagicDNS name: $($ts.DnsName)" } else { Write-Warn 'No MagicDNS name found; clients will have to use the IP address.' }
if (-not $ts.MagicDnsEnabled) { Write-Warn 'MagicDNS looks disabled for this tailnet; use the IP address in clients.' }
if (-not $ts.InterfaceAlias) {
    Write-Host "    Could not find the network adapter that holds $($ts.Ip4)." -ForegroundColor Red
    exit 1
}
Write-Ok "Tailscale network adapter: '$($ts.InterfaceAlias)'"
$prevIp = Get-StateValue $prev 'TailscaleIp4' $null
if ($prevIp -and $prevIp -ne $ts.Ip4) {
    Write-Warn "The Tailscale IP changed from $prevIp to $($ts.Ip4). Clients that use the IP must be updated."
}

$unattended = Get-TailscaleUnattended
if ($unattended -eq $false) {
    Write-Warn 'Tailscale "Run unattended" is OFF. Tailscale will disconnect when you sign out of'
    Write-Warn 'Windows, and the server becomes unreachable. Turn it on: Tailscale tray icon ->'
    Write-Warn 'Preferences (or Settings) -> "Run unattended". See README.md.'
} elseif ($null -eq $unattended) {
    Write-Warn 'Could not check Tailscale "Run unattended". Make sure it is turned on (see README.md).'
} else {
    Write-Ok 'Tailscale "Run unattended" is on.'
}

# ---------------------------------------------------------------------------
# Backend input
# ---------------------------------------------------------------------------
$installedBackend = Join-Path $script:InstallDir 'rdbackend.exe'
if (-not $SkipBackend) {
    if ($BackendExe) {
        if (-not (Test-Path -LiteralPath $BackendExe -PathType Leaf)) {
            Write-Host "    -BackendExe: file not found: $BackendExe" -ForegroundColor Red; exit 1
        }
        $BackendExe = (Resolve-Path -LiteralPath $BackendExe).Path
    } elseif (-not (Test-Path -LiteralPath $installedBackend)) {
        Write-Host '    rdbackend.exe is not installed yet. Pass its location, e.g.:' -ForegroundColor Red
        Write-Host '      .\install.ps1 -BackendExe C:\path\to\rdbackend.exe'
        Write-Host '    or install only hbbs/hbbr for now with -SkipBackend.'
        exit 1
    }
}

# ---------------------------------------------------------------------------
# 2. Downloads
# ---------------------------------------------------------------------------
function Get-VerifiedFile([string]$Url, [string]$LocalPath, [string]$Expected, [string]$Label) {
    if (-not $LocalPath) {
        $LocalPath = Join-Path $script:WorkDir ([IO.Path]::GetFileName($Url))
        Write-Info "Downloading $Label from $Url"
        Invoke-WebRequest -Uri $Url -OutFile $LocalPath -UseBasicParsing
    } else {
        if (-not (Test-Path -LiteralPath $LocalPath -PathType Leaf)) { throw "File not found: $LocalPath" }
        Write-Info "Using local file for ${Label}: $LocalPath"
    }
    $actual = (Get-FileHash -LiteralPath $LocalPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($Expected) {
        if ($actual -ne $Expected.ToLowerInvariant()) {
            throw "SHA-256 mismatch for $Label.`n  expected: $Expected`n  actual:   $actual`nThe download may be corrupted or tampered with. Nothing was installed."
        }
        Write-Ok "$Label SHA-256 verified ($actual)"
    } else {
        Write-Warn "$Label is NOT verified (no known hash). SHA-256: $actual"
    }
    return $LocalPath
}

Write-Step "Getting rustdesk-server $Version and WinSW $WinSWVersion"
$script:WorkDir = Join-Path $env:TEMP ("rustdesk-server-install-" + [Guid]::NewGuid().ToString('N'))
New-DirectoryIfMissing $script:WorkDir
try {
    $expected = $Sha256
    if (-not $expected -and $PinnedServerSha256.ContainsKey($Version)) { $expected = $PinnedServerSha256[$Version] }
    if (-not $expected) {
        # Fall back to the digest GitHub publishes for release assets (newer releases have it).
        try {
            $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/rustdesk/rustdesk-server/releases/tags/$Version" -UseBasicParsing
            $asset = @($rel.assets | Where-Object { $_.name -eq $ServerAssetName }) | Select-Object -First 1
            if ($asset -and ($asset.PSObject.Properties.Name -contains 'digest') -and ([string]$asset.digest -like 'sha256:*')) {
                $expected = ([string]$asset.digest).Substring(7)
                Write-Info "Using the SHA-256 published by GitHub for $ServerAssetName"
            }
        } catch { Write-Warn "Could not ask GitHub for the asset digest: $($_.Exception.Message)" }
    }
    if (-not $expected -and -not $AllowUnverifiedDownload) {
        throw "No SHA-256 known for rustdesk-server $Version. Pass -Sha256 <hash> (shown on the GitHub release page next to the file) or, at your own risk, -AllowUnverifiedDownload."
    }
    $zip = Get-VerifiedFile "https://github.com/rustdesk/rustdesk-server/releases/download/$Version/$ServerAssetName" $ZipPath $expected 'rustdesk-server zip'
    $winsw = Get-VerifiedFile $WinSWUrl $WinSWPath $WinSWSha256 'WinSW'

    $extract = Join-Path $script:WorkDir 'extract'
    Expand-Archive -LiteralPath $zip -DestinationPath $extract -Force
    $hbbsSrc = Get-ChildItem -LiteralPath $extract -Recurse -Filter 'hbbs.exe' | Select-Object -First 1
    $hbbrSrc = Get-ChildItem -LiteralPath $extract -Recurse -Filter 'hbbr.exe' | Select-Object -First 1
    $utilsSrc = Get-ChildItem -LiteralPath $extract -Recurse -Filter 'rustdesk-utils.exe' | Select-Object -First 1
    if (-not $hbbsSrc -or -not $hbbrSrc) { throw 'hbbs.exe / hbbr.exe not found inside the release zip.' }
    Write-Ok 'Release zip contains hbbs.exe and hbbr.exe'

    # -b/--bind exists only from rustdesk-server 1.1.17. Older versions listen on all
    # interfaces and rely on the firewall rules below.
    $v = ConvertTo-VersionOrNull $Version
    $supportsBind = ($null -ne $v -and $v -ge [version]'1.1.17')

    # -----------------------------------------------------------------------
    # 3. Stop and remove the old services, copy files
    # -----------------------------------------------------------------------
    Write-Step 'Stopping existing services (if any)'
    foreach ($n in @($script:SvcBackend, $script:SvcHbbr, $script:SvcHbbs)) { Remove-ServiceSafe $n }
    Write-Ok 'Old services removed (data is kept)'

    Write-Step 'Copying files'
    foreach ($d in @($script:InstallDir, $script:DataRoot, $script:ServerDataDir, $script:BackendDir, $script:LogDir, $script:BackendLogDir)) { New-DirectoryIfMissing $d }
    Protect-DataDir $script:DataRoot
    Copy-Item -LiteralPath $hbbsSrc.FullName -Destination (Join-Path $script:InstallDir 'hbbs.exe') -Force
    Copy-Item -LiteralPath $hbbrSrc.FullName -Destination (Join-Path $script:InstallDir 'hbbr.exe') -Force
    if ($utilsSrc) { Copy-Item -LiteralPath $utilsSrc.FullName -Destination (Join-Path $script:InstallDir 'rustdesk-utils.exe') -Force }
    # WinSW finds its XML by its own file name, so each service gets its own copy.
    Copy-Item -LiteralPath $winsw -Destination (Join-Path $script:InstallDir "$($script:SvcHbbs).exe") -Force
    Copy-Item -LiteralPath $winsw -Destination (Join-Path $script:InstallDir "$($script:SvcHbbr).exe") -Force
    if (-not $SkipBackend -and $BackendExe) {
        if ($BackendExe -ne $installedBackend) { Copy-Item -LiteralPath $BackendExe -Destination $installedBackend -Force }
    }
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'watchdog.ps1') -Destination (Join-Path $script:InstallDir 'watchdog.ps1') -Force
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'common.ps1')   -Destination (Join-Path $script:InstallDir 'common.ps1') -Force
    Write-Ok "Programs in $($script:InstallDir)"
    Write-Ok "Data in $($script:DataRoot)"
} finally {
    Remove-Item -LiteralPath $script:WorkDir -Recurse -Force -ErrorAction SilentlyContinue
}

# ---------------------------------------------------------------------------
# 4. Services
# ---------------------------------------------------------------------------
function ConvertTo-XmlText([string]$s) { return [Security.SecurityElement]::Escape($s) }

function New-WinSWService([string]$Id, [string]$Display, [string]$Description, [string]$Exe, [string]$Arguments) {
    $depend = ''
    if ($DependOnTailscale) { $depend = "  <depend>$($script:TailscaleServiceName)</depend>`r`n" }
    $xml = @"
<service>
  <id>$(ConvertTo-XmlText $Id)</id>
  <name>$(ConvertTo-XmlText $Display)</name>
  <description>$(ConvertTo-XmlText $Description)</description>
  <executable>$(ConvertTo-XmlText $Exe)</executable>
  <arguments>$(ConvertTo-XmlText $Arguments)</arguments>
  <workingdirectory>$(ConvertTo-XmlText $script:ServerDataDir)</workingdirectory>
  <startmode>Automatic</startmode>
  <delayedAutoStart/>
$depend  <onfailure action="restart" delay="10 sec"/>
  <onfailure action="restart" delay="15 sec"/>
  <onfailure action="restart" delay="30 sec"/>
  <resetfailure>1 hour</resetfailure>
  <stoptimeout>15 sec</stoptimeout>
  <logpath>$(ConvertTo-XmlText $script:LogDir)</logpath>
  <log mode="roll-by-size">
    <sizeThreshold>10240</sizeThreshold>
    <keepFiles>5</keepFiles>
  </log>
  <env name="RUST_LOG" value="info"/>
</service>
"@
    $wrapper = Join-Path $script:InstallDir "$Id.exe"
    $ErrorActionPreference = 'Continue'
    Write-Utf8NoBom (Join-Path $script:InstallDir "$Id.xml") $xml
    & $wrapper install 2>&1 | ForEach-Object { Write-Info "$_" }
    if ($LASTEXITCODE -ne 0) { throw "WinSW could not install $Id (exit code $LASTEXITCODE). See $($script:LogDir)\$Id.wrapper.log" }
    Set-ServiceRecovery $Id
}

Write-Step 'Installing services'
$hbbsExe = Join-Path $script:InstallDir 'hbbs.exe'
$hbbrExe = Join-Path $script:InstallDir 'hbbr.exe'
# hbbs: -r tells clients which relay to use. hbbr: -k _ makes the relay require the
# same key as hbbs (it reads id_ed25519 from the shared working directory).
$hbbsArgs = "-r $($ts.Ip4)"
$hbbrArgs = '-k _'
if ($supportsBind) {
    $hbbsArgs = "-b $($ts.Ip4) $hbbsArgs"
    $hbbrArgs = "-b $($ts.Ip4) $hbbrArgs"
}
New-WinSWService $script:SvcHbbs 'RustDesk ID server (hbbs)' 'RustDesk rendezvous/ID server, reachable over Tailscale only.' $hbbsExe $hbbsArgs
Write-Ok "$($script:SvcHbbs) installed  ($hbbsExe $hbbsArgs)"
New-WinSWService $script:SvcHbbr 'RustDesk relay server (hbbr)' 'RustDesk relay server, reachable over Tailscale only.' $hbbrExe $hbbrArgs
Write-Ok "$($script:SvcHbbr) installed  ($hbbrExe $hbbrArgs)"

if (-not $SkipBackend) {
    # config.toml: keep a hand-edited file, only (re)write the bind line.
    $bindValue = "$($ts.Ip4):$($script:PortBackend)"
    $bindLine = "bind = '$bindValue'"
    if (Test-Path -LiteralPath $script:BackendConfig) {
        $cfg = Get-Content -LiteralPath $script:BackendConfig -Raw
        if ($cfg -match '(?m)^[ \t]*bind[ \t]*=[^\r\n]*') {
            $cfg = [regex]::Replace($cfg, '(?m)^[ \t]*bind[ \t]*=[^\r\n]*', $bindLine)
        } else {
            $cfg = $bindLine + "`r`n" + $cfg
        }
        Write-Utf8NoBom $script:BackendConfig $cfg
        Write-Ok "Updated bind in $($script:BackendConfig)"
    } else {
        # TOML literal strings ('...') need no escaping of Windows backslashes.
        $cfg = @"
# rdbackend configuration, written by deploy/windows/install.ps1.
# 'bind' is rewritten on every install.ps1 run; other lines are kept.
$bindLine
db_path = '$(Join-Path $script:BackendDir 'rdbackend.sqlite3')'
log_dir = '$($script:BackendLogDir)'
"@
        Write-Utf8NoBom $script:BackendConfig $cfg
        Write-Ok "Wrote $($script:BackendConfig)"
    }

    $binPath = "`"$installedBackend`" --service --config `"$($script:BackendConfig)`""
    New-Service -Name $script:SvcBackend -BinaryPathName $binPath -DisplayName 'RustDesk API backend (rdbackend)' `
        -Description 'RustDesk API server for this tailnet, reachable over Tailscale only.' -StartupType Automatic | Out-Null
    & sc.exe config $script:SvcBackend start= delayed-auto | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Warn "Could not set delayed start on $($script:SvcBackend) (sc.exe exit $LASTEXITCODE)" }
    if ($DependOnTailscale) {
        & sc.exe config $script:SvcBackend depend= $script:TailscaleServiceName | Out-Null
        if ($LASTEXITCODE -ne 0) { Write-Warn "Could not set the Tailscale dependency on $($script:SvcBackend) (sc.exe exit $LASTEXITCODE)" }
    }
    Set-ServiceRecovery $script:SvcBackend
    Write-Ok "$($script:SvcBackend) installed  ($binPath)"
} else {
    Write-Info 'Skipping rdbackend (-SkipBackend).'
}

# ---------------------------------------------------------------------------
# 5. Firewall
# ---------------------------------------------------------------------------
Write-Step 'Configuring Windows Firewall'
Get-NetFirewallRule -Group $script:FirewallGroup -ErrorAction SilentlyContinue | Remove-NetFirewallRule

$tcpHbbs = @($script:PortHbbsTcp)
$tcpHbbr = @($script:PortHbbrTcp)
if ($IncludeWebSocketPorts) { $tcpHbbs += $script:PortHbbsWs; $tcpHbbr += $script:PortHbbrWs }
$allTcp = @($tcpHbbs + $tcpHbbr)
if (-not $SkipBackend) { $allTcp += $script:PortBackend }
$allTcpStr = @($allTcp | ForEach-Object { [string]$_ })
$udpStr = @($script:PortHbbsUdp | ForEach-Object { [string]$_ })

function Add-AllowRule([string]$Name, [string]$Protocol, [string[]]$Ports) {
    New-NetFirewallRule -DisplayName $Name -Group $script:FirewallGroup -Direction Inbound -Action Allow `
        -Protocol $Protocol -LocalPort $Ports -RemoteAddress $script:TailnetRanges `
        -InterfaceAlias $ts.InterfaceAlias -Profile Any -Enabled True | Out-Null
    Write-Ok "Allow $Protocol $($Ports -join ',') from Tailscale only: '$Name'"
}
Add-AllowRule 'RustDesk hbbs TCP (Tailscale only)' TCP @($tcpHbbs | ForEach-Object { [string]$_ })
Add-AllowRule 'RustDesk hbbs UDP (Tailscale only)' UDP $udpStr
Add-AllowRule 'RustDesk hbbr TCP (Tailscale only)' TCP @($tcpHbbr | ForEach-Object { [string]$_ })
if (-not $SkipBackend) { Add-AllowRule 'RustDesk API backend TCP (Tailscale only)' TCP @([string]$script:PortBackend) }

# Block rules win over allow rules in Windows Firewall. These make sure no other
# (older, broader) allow rule can expose the ports on LAN/public interfaces.
New-NetFirewallRule -DisplayName 'RustDesk block non-Tailscale TCP' -Group $script:FirewallGroup -Direction Inbound -Action Block `
    -Protocol TCP -LocalPort $allTcpStr -RemoteAddress $script:NonTailnetRanges -Profile Any -Enabled True | Out-Null
New-NetFirewallRule -DisplayName 'RustDesk block non-Tailscale UDP' -Group $script:FirewallGroup -Direction Inbound -Action Block `
    -Protocol UDP -LocalPort $udpStr -RemoteAddress $script:NonTailnetRanges -Profile Any -Enabled True | Out-Null
Write-Ok "Block TCP $($allTcpStr -join ',') and UDP $($udpStr -join ',') from all non-Tailscale addresses"

foreach ($p in Get-NetFirewallProfile) {
    if (-not $p.Enabled) {
        Write-Warn "Windows Firewall is OFF for the '$($p.Name)' profile. The rules above only work when it is on."
    } elseif ([string]$p.DefaultInboundAction -eq 'Allow') {
        Write-Warn "The '$($p.Name)' firewall profile allows inbound traffic by default. Set it back to Block."
    }
}

# ---------------------------------------------------------------------------
# 6. Watchdog
# ---------------------------------------------------------------------------
Unregister-ScheduledTask -TaskName $script:WatchdogTask -Confirm:$false -ErrorAction SilentlyContinue
if (-not $NoWatchdog) {
    Write-Step 'Installing the watchdog scheduled task'
    $wd = Join-Path $script:InstallDir 'watchdog.ps1'
    $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$wd`""
    $t1 = New-ScheduledTaskTrigger -AtStartup
    $t1.Delay = 'PT3M'
    $t2 = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(5) -RepetitionInterval (New-TimeSpan -Minutes 5) -RepetitionDuration (New-TimeSpan -Days 3650)
    $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
    $settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -ExecutionTimeLimit (New-TimeSpan -Minutes 5) -MultipleInstances IgnoreNew
    Register-ScheduledTask -TaskName $script:WatchdogTask -Action $action -Trigger @($t1, $t2) -Principal $principal -Settings $settings `
        -Description 'Starts RustDesk server services that stopped and restarts rdbackend if it lost its Tailscale listener.' | Out-Null
    Write-Ok "Scheduled task '$($script:WatchdogTask)' runs every 5 minutes"
}

# ---------------------------------------------------------------------------
# Save state, start everything
# ---------------------------------------------------------------------------
$state = [ordered]@{
    InstalledAt           = (Get-Date).ToString('s')
    Version               = $Version
    TailscaleIp4          = $ts.Ip4
    TailscaleIp6          = $ts.Ip6
    TailscaleDnsName      = $ts.DnsName
    TailscaleInterface    = $ts.InterfaceAlias
    BindsToTailscaleIp    = $supportsBind
    IncludeWebSocketPorts = [bool]$IncludeWebSocketPorts
    DependOnTailscale     = [bool]$DependOnTailscale
    NoWatchdog            = [bool]$NoWatchdog
    SkipBackend           = [bool]$SkipBackend
}
Save-State $state

Write-Step 'Starting services'
function Start-ServiceSafe([string]$Name) {
    try { Start-Service -Name $Name -ErrorAction Stop }
    catch { Write-Warn "$Name did not start: $($_.Exception.Message). Run .\status.ps1 to see its log." }
}
Start-ServiceSafe $script:SvcHbbs
# Wait for hbbs to create the key pair before hbbr reads it.
$pub = Join-Path $script:ServerDataDir 'id_ed25519.pub'
for ($i = 0; $i -lt 60 -and -not (Test-Path -LiteralPath $pub); $i++) { Start-Sleep -Milliseconds 500 }
Start-ServiceSafe $script:SvcHbbr
if (-not $SkipBackend) { Start-ServiceSafe $script:SvcBackend }
Start-Sleep -Seconds 3
foreach ($n in $script:AllServices) {
    $s = Get-Service -Name $n -ErrorAction SilentlyContinue
    if (-not $s) { continue }
    if ($s.Status -eq 'Running') { Write-Ok "$n is running" } else { Write-Warn "$n is $($s.Status). Run .\status.ps1 to see its log." }
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
$key = Get-PublicKey
if (-not $key) { $key = '(not generated yet - run .\status.ps1 in a minute)' }
$hostAlt = $ts.DnsName
if (-not $hostAlt) { $hostAlt = '(no MagicDNS name)' }
$apiUrl = "http://$($ts.Ip4):$($script:PortBackend)"
$summary = @"
RustDesk client settings (Settings -> Network -> ID/Relay server)
------------------------------------------------------------------
ID server     : $($ts.Ip4)
Relay server  : $($ts.Ip4)
API server    : $apiUrl
Key           : $key

Alternative host name (MagicDNS): $hostAlt
  -> ID/Relay server: $hostAlt   API server: http://$($hostAlt):$($script:PortBackend)

Ports: hbbs TCP $($tcpHbbs -join ',') + UDP 21116, hbbr TCP $($tcpHbbr -join ','), API TCP 21114
Every client must be signed in to the same Tailscale network.
"@
Write-Utf8NoBom $script:ClientInfoFile $summary
Write-Host ''
Write-Host $summary -ForegroundColor White
Write-Host "(Saved to $($script:ClientInfoFile))"
if (-not $supportsBind) {
    Write-Host ''
    Write-Info "Note: rustdesk-server $Version cannot bind to a single address, so hbbs/hbbr listen on all"
    Write-Info 'interfaces. The firewall rules above are what keeps them Tailscale-only.'
}
