# RustDesk server on Windows, reachable only over Tailscale

These scripts set up a Windows 10/11 PC as the server for this RustDesk fork:

| Part | What it does | Port(s) |
|---|---|---|
| **hbbs** (ID server) | Lets your devices find each other and arranges connections | TCP 21115, TCP+UDP 21116 |
| **hbbr** (relay server) | Passes the connection through when devices can't connect directly | TCP 21117 |
| **rdbackend** (API server) | This fork's own backend (address book, accounts, ...) | TCP 21114 |

Everything is reachable **only from devices on your Tailscale network**. You don't
need to set up port forwarding on your router, and you shouldn't.

> **Untested on real Windows.** These scripts were written and syntax-checked
> (PowerShell parser and PSScriptAnalyzer, including its Windows PowerShell 5.1
> compatibility rules) on Linux, but they have **not been run on a Windows machine
> yet**. The first install may need small fixes. See "Things to check on the first
> run" at the end.

---

## Contents

1. [What you need](#1-what-you-need)
2. [Install](#2-install)
3. [Set up the RustDesk clients](#3-set-up-the-rustdesk-clients)
4. [Daily use: status, update, uninstall](#4-daily-use-status-update-uninstall)
5. [Backups](#5-backups)
6. [How it works](#6-how-it-works)
7. [Troubleshooting](#7-troubleshooting)
8. [Things to check on the first run](#8-things-to-check-on-the-first-run)

---

## 1. What you need

- A **64-bit Windows 10 or 11** PC that stays switched on. It also needs to stay
  awake: in *Settings -> System -> Power*, set "Sleep" to *Never* when plugged in.
- An **administrator account** on that PC.
- **Tailscale** installed on it (<https://tailscale.com/download/windows>), signed in
  to your tailnet, and showing *Connected*.
- **"Run unattended" turned on in Tailscale.** Without it, Tailscale disconnects when
  you sign out of Windows, and then nobody can reach the server. To turn it on,
  right-click the Tailscale icon in the taskbar tray and find **Run unattended**.
  Depending on your Tailscale version it is in the menu itself or under
  *Preferences* / *Settings*. (The exact menu location has not been checked
  against the current Tailscale version.)
- **`rdbackend.exe`**, the backend program built from `backend/` in this
  repository. Copy it anywhere on the PC, for example your Downloads folder.
- An internet connection during install. The script downloads from GitHub:
  - `rustdesk-server-windows-x86_64-unsigned.zip` from
    <https://github.com/rustdesk/rustdesk-server/releases> (version **1.1.16** by default)
  - `WinSW.NET461.exe` **v2.12.0** from <https://github.com/winsw/winsw/releases>,
    the small helper that runs hbbs and hbbr as Windows services.

  If the PC has no internet access, download both files on another computer and
  use `-ZipPath` and `-WinSWPath` (see below). The script checks their SHA-256
  hashes either way.

Every client device (the PCs and phones you connect from or to) must also run
Tailscale on the same tailnet.

---

## 2. Install

1. Copy the whole `deploy\windows` folder to the server PC, for example to
   `C:\rustdesk-deploy`. Keep all the files together: the scripts use each other.
2. Open **Windows PowerShell as administrator**: click Start, type `powershell`,
   right-click **Windows PowerShell** and choose **Run as administrator**.
3. Go to the folder, and tell Windows to trust the scripts you copied:

   ```powershell
   cd C:\rustdesk-deploy
   Get-ChildItem *.ps1 | Unblock-File
   ```

4. Run the installer. Point `-BackendExe` at your `rdbackend.exe`:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install.ps1 -BackendExe "$env:USERPROFILE\Downloads\rdbackend.exe"
   ```

   `-ExecutionPolicy Bypass` lets this one command run scripts. It does not change
   any setting on your PC.

5. At the end, the installer prints a block like this:

   ```text
   RustDesk client settings (Settings -> Network -> ID/Relay server)
   ------------------------------------------------------------------
   ID server     : 100.101.102.103
   Relay server  : 100.101.102.103
   API server    : http://100.101.102.103:21114
   Key           : AbCdEf...=
   ```

   Keep this. It is also saved in `C:\ProgramData\RustDeskServer\client-settings.txt`,
   and `status.ps1` shows it again any time.

You can run `install.ps1` again at any time, for example after the Tailscale IP
changed or if something looks broken. It always rebuilds the services, firewall
rules and settings, and it **keeps your key and databases**.

### Install options

| Option | Meaning |
|---|---|
| `-BackendExe <path>` | Location of `rdbackend.exe`. Required on the first install, optional later (keeps the installed copy). |
| `-SkipBackend` | Install only hbbs and hbbr, for example if the backend isn't built yet. Run `install.ps1 -BackendExe ... -SkipBackend:$false` later to add it. |
| `-Version 1.1.16` | rustdesk-server version to install. |
| `-Sha256 <hash>` | SHA-256 of the release zip, for versions the script doesn't know. GitHub shows it next to each file on the release page. |
| `-ZipPath <file>` / `-WinSWPath <file>` | Use files you already downloaded. |
| `-IncludeWebSocketPorts` | Also open 21118 and 21119. Only the RustDesk **web** client needs these. |
| `-DependOnTailscale` | Make the services hard-depend on the Tailscale service (see [Start-up order](#start-up-order)). |
| `-NoWatchdog` | Don't create the watchdog scheduled task. |
| `-AllowUnverifiedDownload` | Install even if no SHA-256 is known for the chosen version. Not recommended. |

Options you set are remembered. A plain `.\install.ps1` keeps them. To turn a
switch off again, add `:$false`, e.g. `-IncludeWebSocketPorts:$false`.

---

## 3. Set up the RustDesk clients

On every device running this RustDesk fork (and on the remote devices you want to
reach):

1. Make sure Tailscale is running and connected.
2. In RustDesk, open **Settings -> Network**, click **Unlock network settings** if
   it's shown, then **ID/Relay server**.
3. Fill in the values the installer printed:

   | Field | Value | Example |
   |---|---|---|
   | **ID server** | the server's Tailscale IP | `100.101.102.103` |
   | **Relay server** | the same IP | `100.101.102.103` |
   | **API server** | `http://` + IP + `:21114` | `http://100.101.102.103:21114` |
   | **Key** | contents of `id_ed25519.pub` | `AbCdEf...=` |

4. Click **OK**. The status line at the bottom of the RustDesk main window should
   say *Ready*.

If you build the client yourself with these defaults built in, use the same four
values.

**IP or name?** The installer also prints the server's MagicDNS name (e.g.
`winbox.tail1234.ts.net`). You can use the name instead of the IP in all three
server fields. The IP is the safer choice: it works even when a device's DNS
ignores Tailscale (some Android "Private DNS" settings do). A Tailscale IP stays
the same unless you remove the machine from the tailnet and add it again.

**Why `http://` and not `https://`?** All traffic between Tailscale devices is
already encrypted end-to-end by Tailscale (WireGuard), and the API server isn't
reachable from anywhere else.

### Optional: limit which tailnet devices can use the server

The Windows Firewall rules let in **any** device on your tailnet. If you share your
tailnet with other people, or have devices on it that shouldn't use RustDesk, you
can restrict this further in the Tailscale admin console under *Access controls*.
Allow only your own devices to reach the server on ports `21114-21117` (TCP) and
`21116` (UDP).

---

## 4. Daily use: status, update, uninstall

Always open PowerShell **as administrator** and `cd` into the `deploy\windows` folder
first.

**Check that everything works.** Shows Tailscale, the services, which ports are
open, the firewall rules, the key and the last lines of every log:

```powershell
powershell -ExecutionPolicy Bypass -File .\status.ps1
```

**Update** hbbs/hbbr to another release, or install a newer `rdbackend.exe`. Makes
a backup first, then runs the installer again:

```powershell
# new backend build only
powershell -ExecutionPolicy Bypass -File .\update.ps1 -BackendExe "$env:USERPROFILE\Downloads\rdbackend.exe"

# newer rustdesk-server release (copy the SHA-256 from the GitHub release page)
powershell -ExecutionPolicy Bypass -File .\update.ps1 -Version 1.1.17 -Sha256 0123...abcd
```

When you update rustdesk-server, also update the version and hash pinned near the
top of `install.ps1`, so a plain re-install uses the new version.

**Uninstall.** Removes the services, firewall rules, watchdog and program files, and
keeps your key and databases:

```powershell
powershell -ExecutionPolicy Bypass -File .\uninstall.ps1
```

To also delete all data (key, databases, logs, backups), add `-PurgeData`. All
clients will then need the new key after a reinstall:

```powershell
powershell -ExecutionPolicy Bypass -File .\uninstall.ps1 -PurgeData
```

**Start, stop or restart** a single service. The services are called
`rustdesk-hbbs`, `rustdesk-hbbr` and `rdbackend`:

```powershell
Restart-Service rustdesk-hbbs
```

You can also use the *Services* app: press Win+R, type `services.msc`, and look for
the entries starting with "RustDesk".

---

## 5. Backups

What matters:

| File | Why it matters |
|---|---|
| `C:\ProgramData\RustDeskServer\server\id_ed25519` and `id_ed25519.pub` | The server's key pair. If you lose it, the server makes a new one and **every client has to be given the new key**. |
| `C:\ProgramData\RustDeskServer\server\db_v2.sqlite3` | hbbs's list of known devices. It rebuilds itself as devices reconnect, so it's nice to have but not essential. |
| `C:\ProgramData\RustDeskServer\backend\rdbackend.sqlite3` and `config.toml` | The backend's data and settings. |

Make a backup (stops the services for a few seconds):

```powershell
powershell -ExecutionPolicy Bypass -File .\backup.ps1
# or straight to a USB stick / another disk:
powershell -ExecutionPolicy Bypass -File .\backup.ps1 -Destination E:\RustDeskBackups
```

This writes `rustdesk-server-backup-<date>-<time>.zip`. Copy it off the PC as well.
**The zip contains the server's private key**, so keep it somewhere private.

**Restore** (for example on a new PC):

1. Install as described above. This makes a *new* key, which you're about to replace.
2. Stop the services:
   `Stop-Service rdbackend, rustdesk-hbbr, rustdesk-hbbs`
3. Unzip the backup. Copy the contents of its `server` folder into
   `C:\ProgramData\RustDeskServer\server` and its `backend` folder into
   `C:\ProgramData\RustDeskServer\backend`, replacing the files there.
4. Run `.\install.ps1` once more. This rewrites the IP in `config.toml`, in case the
   new PC has a different Tailscale IP, and starts everything.
5. If the Tailscale IP changed, update the server address on the clients. The key
   stays the same.

The data folder can only be opened by administrators. If Explorer says you don't
have permission, click *Continue*.

---

## 6. How it works

### Files and folders

| Path | Contents |
|---|---|
| `C:\Program Files\RustDeskServer\` | `hbbs.exe`, `hbbr.exe`, `rustdesk-utils.exe`, `rdbackend.exe`, `rustdesk-hbbs.exe`/`.xml` and `rustdesk-hbbr.exe`/`.xml` (WinSW copies and their settings), `watchdog.ps1` |
| `C:\ProgramData\RustDeskServer\server\` | working folder of hbbs and hbbr: key pair, `db_v2.sqlite3` |
| `C:\ProgramData\RustDeskServer\backend\` | `config.toml`, `rdbackend.sqlite3` |
| `C:\ProgramData\RustDeskServer\logs\` | `rustdesk-hbbs.out.log`, `.err.log`, `.wrapper.log` (same for hbbr), `watchdog.log`, `rdbackend\` (backend logs) |
| `C:\ProgramData\RustDeskServer\backups\` | output of `backup.ps1` |
| `C:\ProgramData\RustDeskServer\install-state.json` | what the installer did, used by the other scripts |

`C:\ProgramData\RustDeskServer` is restricted to SYSTEM and Administrators, because
it holds the private key. By default, every local user can read folders under
`ProgramData`.

### Running hbbs and hbbr as services (why WinSW)

`hbbs.exe` and `hbbr.exe` are ordinary console programs. Windows can't run them as
services directly, so they need a small "wrapper" program. The rustdesk-server
Windows zip contains only `hbbs.exe`, `hbbr.exe`, `rustdesk-utils.exe` and
`RustDeskServer.Setup.exe`. The Setup program (which uses NSSM internally) is
marked by the RustDesk docs as *no longer maintained, not recommended*. The current
RustDesk docs suggest either **PM2** (needs Node.js, starts only when a user logs
in) or **NSSM**. The NSSM link in those docs points to a third-party fork, because
the original NSSM hasn't had a release since 2014.

These scripts use **WinSW v2.12.0**, the current stable release of a widely used
open-source service wrapper (MIT licence):

- It's a single small program with an XML settings file, and needs no installation.
  The `.NET461` build uses the .NET Framework that comes with Windows 10/11.
- It supports everything needed here: auto start, delayed start, dependencies,
  working folder, restart on failure and size-limited log files.
- Its settings are a readable text file (`C:\Program Files\RustDeskServer\rustdesk-hbbs.xml`).

The SHA-256 of `WinSW.NET461.exe` is pinned in `install.ps1`. WinSW doesn't publish
hashes for v2.12.0, so the pinned value was computed from the file downloaded from
the official GitHub release.

`rdbackend.exe` handles the Windows service protocol itself, so it's registered
directly, with no wrapper: `rdbackend.exe --service --config "C:\ProgramData\RustDeskServer\backend\config.toml"`.

All three services run as *LocalSystem*, the account the RustDesk docs' NSSM
setup also uses.

### Restart on failure

- All three services restart after 10 s, 15 s and then every 30 s if they stop
  unexpectedly. The failure count resets after an hour without problems.
- For hbbs/hbbr, WinSW reports to Windows when the program exits with an error.
  The failure flag is also set, so a service that stops itself with an error code
  (not just a crash) counts as a failure too.
- The **watchdog** scheduled task (`RustDeskServer-Watchdog`, runs as SYSTEM every
  5 minutes and 3 minutes after boot) catches what Windows' own restart doesn't:
  - it starts any of the three services that is stopped although set to
    automatic;
  - it restarts `rdbackend` if it's running but no longer listening on the
    Tailscale address.

  It writes what it did to `logs\watchdog.log`. If you stop a service on purpose and
  want it to stay stopped, set it to *Manual* in `services.msc` (the watchdog leaves
  manual services alone), or disable the task in *Task Scheduler*.

### Start-up order

The backend listens only on the Tailscale IP (`bind = '100.x.y.z:21114'` in
`config.toml`). During boot, Windows can start services before Tailscale is
connected. The IP then doesn't exist yet, and the backend can't open its port.

This is handled in three layers:

1. **Delayed start.** All three services use *Automatic (Delayed Start)*. Windows
   starts them about two minutes after the normal services, and by then Tailscale
   is usually connected.
2. **Restart on failure.** If the backend still can't bind, it exits with an error
   and Windows retries every 30 s until the IP is there. This assumes rdbackend
   exits with an error when it can't bind; see section 8.
3. **Watchdog.** It covers the remaining cases: a service left stopped, or a
   backend that lost its listener when Tailscale reconnected.

**Why no hard dependency on the Tailscale service by default?** A dependency
(`-DependOnTailscale`) only waits until the Tailscale *service* has started. It
doesn't wait for Tailscale to be *connected* and have its IP. It also has a
downside: when Tailscale is stopped or restarted (for example during a Tailscale
update), Windows first stops every service that depends on it, and doesn't start
them again afterwards. If you turn it on anyway, the watchdog brings them back
within 5 minutes.

hbbs and hbbr in version 1.1.16 listen on all addresses (see next section), so the
start-up order doesn't matter for them.

### Tailscale only: how it's enforced

- **rustdesk-server 1.1.16 can't be told to listen on one address.** The `-b/--bind`
  option exists only in the rustdesk-server source after 1.1.16 ("available since
  1.1.17" in its docs). 1.1.17 wasn't released when these scripts were written. So
  with 1.1.16, hbbs and hbbr listen on all network adapters, and **the Windows
  Firewall is what keeps them Tailscale-only**. When you install 1.1.17 or later,
  `install.ps1` passes `-b <Tailscale IP>` automatically.
- **Firewall rules** (group *RustDesk Tailnet Server* in *Windows Defender Firewall
  with Advanced Security*):
  - *Allow* rules for exactly the server ports. They only match traffic that comes
    in on the **Tailscale network adapter** from Tailscale addresses
    (`100.64.0.0/10` and `fd7a:115c:a1e0::/48`).
  - *Block* rules for the same ports from every other address. In Windows Firewall,
    block rules win over allow rules, so even an older, broader rule (for example
    one you made by hand) can't open these ports to your LAN or the internet.
  - Windows Firewall must be **on** for all profiles. The installer and
    `status.ps1` warn you if it isn't.
- Nothing needs to be forwarded on your router, and you shouldn't forward anything.

### Confirmed port list

From the rustdesk-server source (`src/rendezvous_server.rs`, `src/relay_server.rs`)
and its `docs/environment-variables.md`:

| Port | Protocol | Program | Purpose | Opened by default |
|---|---|---|---|---|
| 21114 | TCP | rdbackend | API server | yes |
| 21115 | TCP | hbbs | NAT type test (main port - 1) | yes |
| 21116 | TCP + UDP | hbbs | ID registration, hole punching, connection setup | yes |
| 21117 | TCP | hbbr | relay | yes |
| 21118 | TCP | hbbs | WebSocket (main port + 2), **web client only** | only with `-IncludeWebSocketPorts` |
| 21119 | TCP | hbbr | WebSocket (relay port + 2), **web client only** | only with `-IncludeWebSocketPorts` |

The RustDesk docs advise keeping 21118/21119 closed unless you use the web client.
hbbs/hbbr also have an admin console on 21115/21117. It's reached through
`127.0.0.1` and isn't affected by these rules.

### Key

hbbs creates the key pair (`id_ed25519` / `id_ed25519.pub`) in its working folder on
its first start. hbbr is started with `-k _`. It reads the same key from the same
folder and only relays for clients that use it. hbbs also gets
`-r <Tailscale IP>`, which tells clients where the relay is.

---

## 7. Troubleshooting

Start with `.\status.ps1`. It shows most problems directly.

**"This script must be run as Administrator."**
Open PowerShell with *Run as administrator* (see step 2 of the install).

**"...cannot be loaded because running scripts is disabled on this system" / "not digitally signed".**
Use the `powershell -ExecutionPolicy Bypass -File .\script.ps1` form shown above,
and run `Get-ChildItem *.ps1 | Unblock-File` once in the folder.

**"Tailscale is not installed / not connected."**
Open the Tailscale app, sign in, and wait until it says *Connected*. Then run the
installer again.

**Clients say "Not ready" / "Connecting to the RustDesk network...".**
- Is Tailscale connected on the *client*? Can you ping the server?
  `ping 100.101.102.103` (use your server's IP).
- Is the ID server on the client exactly the server's Tailscale IP or MagicDNS name?
- Does `status.ps1` show hbbs *Running* and listening on 21116 (TCP and UDP)?

**"Key mismatch" on the client.**
The key in the client doesn't match the server's. Copy it again from `status.ps1`
(the line "Current public key"). It must be the whole string, including any `=`
at the end.

**Connections work only via relay, or not at all through the relay.**
Check that `rustdesk-hbbr` is running and that "TCP 100.x.x.x:21117 accepts
connections" appears in `status.ps1`. The *Relay server* field on the clients can
also just be left empty: hbbs tells them the relay address.

**The backend (API server) doesn't start.**
- Look at the backend logs in `C:\ProgramData\RustDeskServer\logs\rdbackend\`
  (shown by `status.ps1`) and at `C:\ProgramData\RustDeskServer\backend\config.toml`.
- Is `bind` in `config.toml` the machine's *current* Tailscale IP? If not, run
  `.\install.ps1` again.
- Run it in the foreground to see errors directly. Stop the service first:

  ```powershell
  Stop-Service rdbackend
  & "C:\Program Files\RustDeskServer\rdbackend.exe" --config "C:\ProgramData\RustDeskServer\backend\config.toml"
  # Ctrl+C to stop, then:
  Start-Service rdbackend
  ```

**hbbs or hbbr doesn't start.**
Look at `C:\ProgramData\RustDeskServer\logs\rustdesk-hbbs.wrapper.log` (WinSW's own
log) and `rustdesk-hbbs.err.log` / `.out.log` (the program's output). The same
files exist for hbbr. Another program using the same port shows up as
"address already in use". Check with
`Get-NetTCPConnection -LocalPort 21116 -State Listen`.

**"Service ... is marked for deletion."**
Close the *Services* window and Task Manager, then run the script again. If it
still happens, reboot.

**Windows Defender or another antivirus deletes or blocks `hbbs.exe` / `hbbr.exe`.**
The rustdesk-server Windows builds are not code-signed ("unsigned" in the file
name). The installer checked the download's SHA-256. You can add an exclusion for
`C:\Program Files\RustDeskServer`.

**Everything stopped after I signed out of Windows / after a reboot until I logged in.**
Turn on Tailscale's *Run unattended* (see "What you need").

**The Tailscale IP changed.**
Run `.\install.ps1` again (it updates `config.toml`, hbbs's relay address and the
firewall rules), then update the server address on the clients. The key doesn't
change.

**I want to see the firewall rules.**
Run `wf.msc`, click *Inbound Rules*, and sort by *Group*: look for
"RustDesk Tailnet Server".

---

## 8. Things to check on the first run

The scripts haven't been run on Windows yet. These are the parts most likely to
need adjustment:

1. **Tailscale detection** (`Get-TailscaleInfo` in `common.ps1`). It reads
   `tailscale status --json` (`BackendState`, `Self.TailscaleIPs`, `Self.DNSName`,
   `CurrentTailnet.MagicDNSEnabled`). The "Run unattended" check reads the
   `ForceDaemon` field of `tailscale debug prefs`. That's a debug command, so this
   check is only a best effort, and it just warns when it can't tell.
2. **WinSW installing the services**: whether `rustdesk-hbbs.exe install` accepts
   the generated XML (`<delayedAutoStart/>`, `<depend>`, `roll-by-size` logging),
   and where it writes its log files.
3. **Firewall rules**: whether `New-NetFirewallRule` accepts the IPv6 address
   *ranges* in the block rules, and whether the allow rules tied to the Tailscale
   adapter (`-InterfaceAlias`) keep matching after Tailscale reconnects. If clients
   can't connect but `status.ps1` shows the services listening, try disabling the
   two "RustDesk block non-Tailscale" rules in `wf.msc`, to see whether the rules
   are the cause.
4. **rdbackend**: the service relies on rdbackend exiting with an error when it
   can't bind the Tailscale IP, so that Windows restarts it. The database file
   name `rdbackend.sqlite3` matches the backend's default at the time of writing.
5. **The watchdog scheduled task**: whether its triggers (at startup + 3 minutes,
   then every 5 minutes) register as expected on your Windows version.
6. **Downloads**: the pinned hash for rustdesk-server 1.1.16 matches the digest
   GitHub shows for the file. The WinSW hash was computed from the official
   release file (WinSW doesn't publish one for v2.12.0).
