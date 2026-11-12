# rdbackend

A small, self-hosted implementation of the HTTP "API server" that the RustDesk
client talks to (default port 21114). Together with the open-source ID/relay
server (hbbs/hbbr) it gives the unchanged client:

- account login (username + password, bearer tokens),
- an address book that syncs across every device logged in to the same account,
- a device registry with online status and last-seen time (from heartbeats and sysinfo),
- a connection / file-transfer / alarm audit log,
- a built-in web console at `http://<bind>/` (devices, online status, audit log, user management).

It is meant for one person or a household reaching their own devices over a
private network such as Tailscale. Storage is a single SQLite file (bundled,
no system library needed); passwords are hashed with argon2id.

This crate is deliberately **not** a member of the root RustDesk workspace; it
builds on its own:

```sh
cd backend
cargo build --release          # target/release/rdbackend(.exe)
cargo test
```

## Running

```sh
rdbackend --config config.toml              # foreground, logs to console and log_dir
rdbackend --config C:\rdbackend\config.toml --service   # under the Windows SCM
```

`--service` only works on Windows, when the binary was started by the Service
Control Manager (register it with e.g.
`sc.exe create rdbackend binPath= "C:\rdbackend\rdbackend.exe --config C:\rdbackend\config.toml --service" start= auto`).
On other systems `--service` is an error; use systemd/launchd with the
foreground mode instead.

Copy [`config.example.toml`](config.example.toml) to `config.toml`. Keys:

| key                   | default            | meaning |
|-----------------------|--------------------|---------|
| `bind`                | `"0.0.0.0:21114"`  | listen address. Prefer the machine's Tailscale IP (`tailscale ip -4`), e.g. `"100.101.102.103:21114"`, so the API is only reachable over the tailnet. |
| `db_path`             | `"rdbackend.sqlite3"` | SQLite database |
| `log_dir`             | `"logs"`           | daily-rotated `rdbackend.log.YYYY-MM-DD` |
| `token_ttl_days`      | `30`               | lifetime of a login token |
| `online_timeout_secs` | `60`               | a device is "online" if it sent a heartbeat this recently |

Relative paths are resolved against the directory containing the config file.
Log verbosity can be changed with the `RDBACKEND_LOG` environment variable
(`RDBACKEND_LOG=debug`).

**First run:** if the database has no users, an `admin` account is created
with a random password, which is written once to the console and the log file.
Log in to the web console with it, set a new password under *Users*, and
consider deleting that log line. There is no built-in default password.

### Client settings

In the RustDesk client, *Settings > Network > ID/Relay server*:

- *ID server*: the hbbs host (e.g. its Tailscale IP or MagicDNS name).
- *API server*: `http://<rdbackend host>:21114`. If rdbackend runs on the same
  host as hbbs with the default ports this field may be left empty: the client
  then derives `http://<ID server>:21114` itself.

The client only talks to an API server that is not `*.rustdesk.com`.

## Security model

- Everything requires `Authorization: Bearer <token>` except: `POST /api/login`,
  `GET /api/login-options`, the OIDC/deploy stubs, and the endpoints the
  client's background service calls **without** a token (`/api/heartbeat`,
  `/api/sysinfo`, `/api/sysinfo_ver`, `/api/audit/{conn,file,alarm}`,
  `/api/switch-grant`, `/api/record`). Those device-side endpoints are
  unauthenticated in the client itself (`post_request(url, body, "")`), so any
  host that can reach the port can register a device or add audit rows. This
  is why binding to the Tailscale IP is recommended.
- `/api/admin/*` (the web console API) additionally requires an admin account.
- Tokens are 256-bit random values; only their SHA-256 is stored. Logout,
  password change and disabling a user revoke tokens.
- All SQL uses bound parameters. Failed logins are logged and delayed by 500 ms.
- The console renders all values with `textContent` (device names come from
  unauthenticated sysinfo posts) and is served with a restrictive CSP.
- No TLS: run it on a private network (Tailscale encrypts the traffic), or
  put a reverse proxy in front.

## Implemented endpoints

Contracts were taken from the client source in this repository. Paths are
relative to the API server URL. "Empty 200" means HTTP 200 with an empty body,
which is what the client treats as success for write actions
(`_jsonDecodeActionResp` in `ab_model.dart`, `post_audit_async` in `connection.rs`).
Errors are always `{"error": "..."}` with a 4xx/5xx status. List endpoints take
`?current=<1-based page>&pageSize=<n>` and answer `{"total": n, "data": [...]}`.

### Account

| Method / path | Auth | Behaviour | Derived from |
|---|---|---|---|
| `POST /api/login` | - | `{username,password,id,uuid,type:"account",...}` -> `{"type":"access_token","access_token","user":{name,display_name,email,note,avatar,status,is_admin}}`; wrong credentials -> 401 `{"error"}`. Links the logging-in device id to the account. Other login types (SMS/email/2FA) -> 400. | `flutter/lib/models/user_model.dart` (`login`), `flutter/lib/common/hbbs/hbbs.dart` (`LoginRequest`, `LoginResponse`, `UserPayload`), `flutter/lib/common/widgets/login.dart` |
| `POST /api/logout` | token | revokes the token, empty 200 | `user_model.dart` (`logOut`) |
| `POST /api/currentUser` | token | `UserPayload`; 401 makes the client log out | `user_model.dart` (`refreshCurrentUser`) |
| `GET /api/login-options` | - | `[]` (no third-party login, the client shows only the password form) | `user_model.dart` (`queryOidcLoginOptions`), `src/hbbs_http/account.rs`, `src/hbbs_http/record_upload.rs` (TLS probe) |
| `POST /api/oidc/auth`, `GET /api/oidc/auth-query` | - | stub: 404 `{"error":"OIDC login is not supported..."}` | `src/hbbs_http/account.rs` |

### Address book

Personal ("new") address-book API; one personal book per account, shared
books are not implemented.

| Method / path | Auth | Behaviour | Derived from |
|---|---|---|---|
| `POST /api/ab/settings` | token | `{"max_peer_one_ab":0}` (0 = unlimited) | `flutter/lib/models/ab_model.dart` (`_getAbSettings`) |
| `POST /api/ab/personal` | token | `{"guid"}` of the caller's personal book (created on demand) | `ab_model.dart` (`_getPersonalAbGuid`) |
| `POST /api/ab/shared/profiles` | token | `{"total":0,"data":[]}` | `ab_model.dart` (`_getSharedAbProfiles`) |
| `POST /api/ab/peers?current&pageSize&ab=<guid>` | token | paged peers, each the JSON the client stored (`id, hash, username, hostname, platform, alias, tags, note, ...`) | `ab_model.dart` (`Ab._fetchPeers`), `flutter/lib/models/peer_model.dart` (`Peer.fromJson`) |
| `POST /api/ab/tags/{guid}` | token | `[{"name","color"}]` | `ab_model.dart` (`Ab._fetchTags`), `hbbs.dart` (`AbTag`) |
| `POST /api/ab/peer/add/{guid}` | token | body = peer JSON; upsert; empty 200 | `ab_model.dart` (`Ab.addPeers`) |
| `PUT /api/ab/peer/update/{guid}` | token | body = `{id, <fields>}` merged into the stored peer (alias, tags, note, hash, password, username, hostname, platform); 404 if unknown | `ab_model.dart` (`changeTagForPeers`, `changeAlias`, `changeNote`, `_setPassword`, `syncFromRecent`) |
| `DELETE /api/ab/peer/{guid}` | token | body = `["id", ...]`; empty 200 | `ab_model.dart` (`Ab.deletePeers`) |
| `POST /api/ab/tag/add/{guid}` | token | `{name,color}`; empty 200 | `ab_model.dart` (`Ab.addTags`) |
| `PUT /api/ab/tag/rename/{guid}` | token | `{old,new}`; also renames the tag on peers | `ab_model.dart` (`Ab.renameTag`) |
| `PUT /api/ab/tag/update/{guid}` | token | `{name,color}` | `ab_model.dart` (`Ab.setTagColor`) |
| `DELETE /api/ab/tag/{guid}` | token | body = `["tag", ...]`; also removes the tags from peers | `ab_model.dart` (`Ab.deleteTag`) |
| `GET /api/ab` | token | legacy book: `{"data":"<json string>","licensed_devices":0}` or `null` when empty | `ab_model.dart` (`LegacyAb.pullAbImpl`) |
| `POST /api/ab` | token | legacy book: `{"data":"<json string>"}`; empty 200 | `ab_model.dart` (`LegacyAb.pushAb`) |

Only the caller's own books are accessible; another account's guid gives 404.

### Devices, users, groups (the client's "Accessible devices" tab)

| Method / path | Auth | Behaviour | Derived from |
|---|---|---|---|
| `POST /api/heartbeat` | - (client sends none) | `{id,uuid,ver,conns?,modified_at}` -> updates last-seen and active session count; replies `{"sysinfo":true}` while the device's sysinfo is unknown (forces an upload), else `{}` | `src/hbbs_http/sync.rs` |
| `POST /api/sysinfo` | - | sysinfo JSON (`id, uuid, version, hostname, username, os, cpu, memory, preset-*`) -> plain text `SYSINFO_UPDATED`; `ID_NOT_FOUND` without an id | `src/hbbs_http/sync.rs`, `src/common.rs` (`get_sysinfo`) |
| `POST /api/sysinfo_ver` | - | plain text `1` (the client only calls this for public servers) | `src/hbbs_http/sync.rs` |
| `GET /api/peers?current&pageSize&accessible&status` | token | paged `PeerPayload`s: `{id, info:{username,os,device_name,cpu,memory}, status:1, user, user_name, device_group_name:"", note, version, last_online, online, conns}`. Admins see all devices, other accounts only devices they logged in on. | `flutter/lib/models/group_model.dart` (`_getPeers`), `hbbs.dart` (`PeerPayload`) |
| `GET /api/users?current&pageSize&accessible&status` | token | paged `UserPayload`s (admins: all enabled users; others: themselves) | `group_model.dart` (`_getUsers`) |
| `GET /api/device-group/accessible` | token | `{"total":0,"data":[]}` | `group_model.dart` (`_getDeviceGroups`) |
| `POST /api/devices/deploy` | - | stub: `{"result":"NOT_ENABLED"}` (`rustdesk --deploy` prints "Server does not require deployment") | `src/ui_interface.rs` (`deploy_device`), `src/core_main.rs` |
| `POST /api/devices/cli` | token | stub: 404 `{"error"}` (`rustdesk --assign` prints it) | `src/core_main.rs` |
| `POST /api/switch-grant` | - | stub: `{"accepted":true}` (nothing consumes the grant with open-source hbbs) | `src/hbbs_http/sync.rs` (`register_switch_grant`) |
| `POST /api/record` | - | stub: 404 `{"error"}`, which stops the recording upload | `src/hbbs_http/record_upload.rs` |

Online status is computed from the last heartbeat (`online_timeout_secs`). The
client itself shows the green dot from the ID server, not from this API.

### Audit

| Method / path | Auth | Behaviour | Derived from |
|---|---|---|---|
| `POST /api/audit/conn` | - | Connection events from the controlled device: `{action:"new",ip}` opens a record, `{peer:[id,name],type,primary_auth?,two_factor?,session_id}` fills in who connected and how, `{action:"close"}` closes it; all carry `id, uuid, conn_id, session_id, nonce`. `{id, session_id, note}` from the controlling side attaches a note. Empty 200. | `src/server/connection.rs` (`on_open`, `post_conn_audit`, authorisation), `src/ui_session_interface.rs` (`send_note`) |
| `POST /api/audit/file` | - | `{id,uuid,peer_id,conn_id,type,path,is_file,info,nonce}`; empty 200 | `src/server/connection.rs` (`post_file_audit`) |
| `POST /api/audit/alarm` | - | `{id,uuid,typ,info,conn_id,nonce,conn_audit_ref?}`; empty 200 | `src/server/connection.rs` (`post_alarm_audit`) |
| `PUT /api/audit` | token | `{guid,note}` updates the note of a connection record | `flutter/lib/common/widgets/dialog.dart` (`updateAuditNoteByGuid`) |

`nonce` de-duplicates the client's retries (remembered for an hour); the nonce
and the record are committed in one transaction, so a failed write can be retried.

### Web console (not used by the client)

| Method / path | Auth | Behaviour |
|---|---|---|
| `GET /` | - | the console page (static HTML + JS, embedded in the binary) |
| `GET /api/admin/devices` | admin | paged devices (same shape as `/api/peers`) |
| `DELETE /api/admin/devices/{id}` | admin | remove a device from the registry |
| `GET /api/admin/audit/{conn,file,alarm}` | admin | paged audit records, newest first |
| `GET /api/admin/users` | admin | all users |
| `POST /api/admin/users` | admin | `{name,password,is_admin?}` (password >= 8 chars) |
| `PUT /api/admin/users/{name}` | admin | `{password?,is_admin?,status?}` (status 1 enabled, 0 disabled); the last active admin cannot be demoted or disabled |
| `DELETE /api/admin/users/{name}` | admin | delete a user and their address book |

## Not implemented (client degrades gracefully)

- Shared address books, device groups, strategies (heartbeat never returns
  `strategy`/`disconnect`), OIDC / SMS / email / TOTP login, device deployment
  and CLI assignment, session recording upload.
- The connection note dialog (`PUT /api/audit`) needs an audit guid that the
  open-source ID server never hands to the client, so in practice notes arrive
  through `send_note` instead.
- No audit-log retention: rows accumulate in SQLite (tiny at personal scale).
