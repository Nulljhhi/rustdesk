# Building with self-hosted server defaults

This fork can bake the address of your own ID/relay server (hbbs/hbbr) and API
server into the client at build time, so a fresh install needs no manual
network configuration.

The values are read with `option_env!` when the Rust library is compiled
(`src/common.rs`, `BUILD_TIME_SERVER_SETTINGS`). They are used as *defaults*:
anything entered in Settings > Network > ID/Relay server still takes
precedence, and clearing a field there falls back to the built-in value.

| Variable                | Setting it provides | Example                                  |
| ----------------------- | ------------------- | ---------------------------------------- |
| `SELFHOST_ID_SERVER`    | ID server           | `winbox.tail1234.ts.net` (port 21116 is implied) |
| `SELFHOST_RELAY_SERVER` | Relay server        | `winbox.tail1234.ts.net:21117`           |
| `SELFHOST_API_SERVER`   | API server          | `http://winbox.tail1234.ts.net:21114`    |
| `SELFHOST_KEY`          | Server public key   | contents of `id_ed25519.pub` next to hbbs |

All variables are optional; unset or empty ones are ignored. Without
`SELFHOST_ID_SERVER` the client behaves like upstream (public servers,
update check). If relay or API server are left out they are derived from the
ID server as upstream does (relay = the one hbbs announces, else the ID host
on port 21117; API = the ID host on port 21114 over http).

When `SELFHOST_ID_SERVER` is set, the build additionally:

- never falls back to the public RustDesk rendezvous servers (an ID of the
  form `<id>@public` is refused);
- skips the software update check against `api.rustdesk.com`.

Independent of these variables, "Enable direct IP access"
(Settings > Security) is on by default in this fork. It listens on TCP 21118
on all interfaces, so a device can be reached as `100.x.y.z`,
`pc.tail1234.ts.net` or `pc.tail1234.ts.net:21118` when the ID server is down.
A one-word MagicDNS short name (`pc`) can not be told apart from an ID; use the
full name or the IP address.

## Example

Linux/macOS shell:

```sh
export SELFHOST_ID_SERVER=winbox.tail1234.ts.net
export SELFHOST_RELAY_SERVER=winbox.tail1234.ts.net:21117
export SELFHOST_API_SERVER=http://winbox.tail1234.ts.net:21114
export SELFHOST_KEY='<contents of id_ed25519.pub>'
python3 build.py --flutter
```

Windows PowerShell:

```powershell
$env:SELFHOST_ID_SERVER = "winbox.tail1234.ts.net"
$env:SELFHOST_RELAY_SERVER = "winbox.tail1234.ts.net:21117"
$env:SELFHOST_API_SERVER = "http://winbox.tail1234.ts.net:21114"
$env:SELFHOST_KEY = "<contents of id_ed25519.pub>"
python build.py --flutter
```

`build.py` runs `cargo`, which inherits the variables; cargo rebuilds the
library when one of them changes. For CI, set them as (secret) environment
variables of the build job.

Note: use `http://` for the API server on port 21114. An `https://...:21114`
URL has its port stripped by the client unless the built-in option
`allow-https-21114` is set.
