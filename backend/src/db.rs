//! SQLite storage. Every statement uses bound parameters.

use rand::{distributions::Alphanumeric, Rng, RngCore};
use rusqlite::{params, Connection, OptionalExtension, Result};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '',
    email TEXT NOT NULL DEFAULT '',
    note TEXT NOT NULL DEFAULT '',
    is_admin INTEGER NOT NULL DEFAULT 0,
    status INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS tokens (
    token_hash TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    last_used INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS address_books (
    guid TEXT PRIMARY KEY,
    owner_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    personal INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS ab_peers (
    ab_guid TEXT NOT NULL REFERENCES address_books(guid) ON DELETE CASCADE,
    peer_id TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (ab_guid, peer_id)
);
CREATE TABLE IF NOT EXISTS ab_tags (
    ab_guid TEXT NOT NULL REFERENCES address_books(guid) ON DELETE CASCADE,
    name TEXT NOT NULL,
    color INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (ab_guid, name)
);
CREATE TABLE IF NOT EXISTS legacy_ab (
    user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS devices (
    id TEXT PRIMARY KEY,
    uuid TEXT NOT NULL DEFAULT '',
    hostname TEXT NOT NULL DEFAULT '',
    username TEXT NOT NULL DEFAULT '',
    os TEXT NOT NULL DEFAULT '',
    version TEXT NOT NULL DEFAULT '',
    info TEXT NOT NULL DEFAULT '{}',
    user_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
    note TEXT NOT NULL DEFAULT '',
    conns INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    last_seen INTEGER,
    sysinfo_at INTEGER
);
CREATE TABLE IF NOT EXISTS audit_nonce (
    nonce TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS audit_conn (
    id INTEGER PRIMARY KEY,
    guid TEXT NOT NULL UNIQUE,
    device_id TEXT NOT NULL,
    conn_id INTEGER NOT NULL DEFAULT 0,
    session_id TEXT NOT NULL DEFAULT '',
    ip TEXT NOT NULL DEFAULT '',
    peer_id TEXT NOT NULL DEFAULT '',
    peer_name TEXT NOT NULL DEFAULT '',
    conn_type INTEGER,
    primary_auth INTEGER,
    two_factor INTEGER,
    note TEXT NOT NULL DEFAULT '',
    started_at INTEGER NOT NULL,
    authed_at INTEGER,
    ended_at INTEGER
);
CREATE INDEX IF NOT EXISTS audit_conn_dev ON audit_conn(device_id, conn_id);
CREATE TABLE IF NOT EXISTS audit_file (
    id INTEGER PRIMARY KEY,
    device_id TEXT NOT NULL,
    peer_id TEXT NOT NULL DEFAULT '',
    conn_id INTEGER NOT NULL DEFAULT 0,
    typ INTEGER NOT NULL DEFAULT 0,
    path TEXT NOT NULL DEFAULT '',
    is_file INTEGER NOT NULL DEFAULT 0,
    info TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS audit_alarm (
    id INTEGER PRIMARY KEY,
    device_id TEXT NOT NULL,
    typ INTEGER NOT NULL DEFAULT 0,
    conn_id INTEGER NOT NULL DEFAULT 0,
    info TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);
"#;

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    init(&conn)?;
    Ok(conn)
}

pub fn init(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.execute_batch(SCHEMA)
}

fn random_string(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ---------------------------------------------------------------- passwords

pub fn hash_password(password: &str) -> std::result::Result<String, String> {
    use argon2::{
        password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
        Argon2,
    };
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    use argon2::{password_hash::PasswordHash, Argon2, PasswordVerifier};
    PasswordHash::new(hash)
        .map(|h| {
            Argon2::default()
                .verify_password(password.as_bytes(), &h)
                .is_ok()
        })
        .unwrap_or(false)
}

// ---------------------------------------------------------------- users

#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub display_name: String,
    pub email: String,
    pub note: String,
    pub is_admin: bool,
    pub status: i64,
}

impl User {
    /// Shape of the client's `UserPayload` (flutter/lib/common/hbbs/hbbs.dart).
    pub fn payload(&self) -> Value {
        json!({
            "name": self.name,
            "display_name": self.display_name,
            "email": self.email,
            "note": self.note,
            "avatar": "",
            "status": self.status,
            "is_admin": self.is_admin,
        })
    }
}

const USER_COLS: &str = "id, name, display_name, email, note, is_admin, status";

fn row_user(r: &rusqlite::Row) -> Result<User> {
    Ok(User {
        id: r.get(0)?,
        name: r.get(1)?,
        display_name: r.get(2)?,
        email: r.get(3)?,
        note: r.get(4)?,
        is_admin: r.get(5)?,
        status: r.get(6)?,
    })
}

/// Creates `admin` with a random password when there are no users yet.
/// Returns the generated password so the caller can print it once.
pub fn ensure_admin(conn: &Connection) -> std::result::Result<Option<String>, String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if count > 0 {
        return Ok(None);
    }
    let password = random_string(20);
    create_user(conn, "admin", &hash_password(&password)?, true).map_err(|e| e.to_string())?;
    Ok(Some(password))
}

pub fn create_user(conn: &Connection, name: &str, password_hash: &str, admin: bool) -> Result<i64> {
    conn.execute(
        "INSERT INTO users (name, password_hash, is_admin, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![name, password_hash, admin, now()],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn user_by_name(conn: &Connection, name: &str) -> Result<Option<(User, String)>> {
    conn.query_row(
        &format!("SELECT {USER_COLS}, password_hash FROM users WHERE name = ?1"),
        [name],
        |r| Ok((row_user(r)?, r.get(7)?)),
    )
    .optional()
}

pub fn list_users(conn: &Connection) -> Result<Vec<User>> {
    let mut st = conn.prepare(&format!("SELECT {USER_COLS} FROM users ORDER BY name"))?;
    let rows = st.query_map([], row_user)?;
    rows.collect()
}

pub fn count_active_admins(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM users WHERE is_admin = 1 AND status = 1",
        [],
        |r| r.get(0),
    )
}

pub fn set_user_password(conn: &Connection, user_id: i64, password_hash: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET password_hash = ?1 WHERE id = ?2",
        params![password_hash, user_id],
    )?;
    // Changing a password signs the user out everywhere.
    conn.execute("DELETE FROM tokens WHERE user_id = ?1", [user_id])?;
    Ok(())
}

pub fn set_user_flags(conn: &Connection, user_id: i64, is_admin: bool, status: i64) -> Result<()> {
    conn.execute(
        "UPDATE users SET is_admin = ?1, status = ?2 WHERE id = ?3",
        params![is_admin, status, user_id],
    )?;
    if status != 1 {
        conn.execute("DELETE FROM tokens WHERE user_id = ?1", [user_id])?;
    }
    Ok(())
}

pub fn delete_user(conn: &Connection, user_id: i64) -> Result<()> {
    conn.execute("DELETE FROM users WHERE id = ?1", [user_id])?;
    Ok(())
}

// ---------------------------------------------------------------- tokens

pub fn create_token(
    conn: &Connection,
    user_id: i64,
    device_id: &str,
    ttl_days: i64,
) -> Result<String> {
    let token = random_hex(32);
    let t = now();
    conn.execute(
        "INSERT INTO tokens (token_hash, user_id, device_id, created_at, expires_at, last_used)
         VALUES (?1, ?2, ?3, ?4, ?5, ?4)",
        params![
            sha256_hex(&token),
            user_id,
            device_id,
            t,
            t + ttl_days * 86400
        ],
    )?;
    conn.execute("DELETE FROM tokens WHERE expires_at < ?1", [t])?;
    Ok(token)
}

pub fn user_by_token(conn: &Connection, token: &str) -> Result<Option<User>> {
    let hash = sha256_hex(token);
    let t = now();
    let user = conn
        .query_row(
            "SELECT u.id, u.name, u.display_name, u.email, u.note, u.is_admin, u.status
             FROM tokens t JOIN users u ON u.id = t.user_id
             WHERE t.token_hash = ?1 AND t.expires_at > ?2 AND u.status = 1",
            params![hash, t],
            row_user,
        )
        .optional()?;
    if user.is_some() {
        conn.execute(
            "UPDATE tokens SET last_used = ?1 WHERE token_hash = ?2",
            params![t, hash],
        )?;
    }
    Ok(user)
}

pub fn delete_token(conn: &Connection, token: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM tokens WHERE token_hash = ?1",
        [sha256_hex(token)],
    )?;
    Ok(())
}

// ---------------------------------------------------------------- address book

pub fn personal_ab_guid(conn: &Connection, user_id: i64) -> Result<String> {
    if let Some(guid) = conn
        .query_row(
            "SELECT guid FROM address_books WHERE owner_id = ?1 AND personal = 1",
            [user_id],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(guid);
    }
    let guid = random_hex(16);
    conn.execute(
        "INSERT INTO address_books (guid, owner_id, name, personal) VALUES (?1, ?2, 'My address book', 1)",
        params![guid, user_id],
    )?;
    Ok(guid)
}

pub fn ab_owned_by(conn: &Connection, guid: &str, user_id: i64) -> Result<bool> {
    conn.query_row(
        "SELECT COUNT(*) FROM address_books WHERE guid = ?1 AND owner_id = ?2",
        params![guid, user_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|c| c > 0)
}

pub fn ab_peers(
    conn: &Connection,
    guid: &str,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<Value>)> {
    let total = conn.query_row(
        "SELECT COUNT(*) FROM ab_peers WHERE ab_guid = ?1",
        [guid],
        |r| r.get(0),
    )?;
    let mut st = conn.prepare(
        "SELECT data FROM ab_peers WHERE ab_guid = ?1 ORDER BY peer_id LIMIT ?2 OFFSET ?3",
    )?;
    let rows = st.query_map(params![guid, limit, offset], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for data in rows {
        out.push(serde_json::from_str(&data?).unwrap_or(Value::Null));
    }
    Ok((total, out))
}

fn ab_peer(conn: &Connection, guid: &str, peer_id: &str) -> Result<Option<Map<String, Value>>> {
    let data: Option<String> = conn
        .query_row(
            "SELECT data FROM ab_peers WHERE ab_guid = ?1 AND peer_id = ?2",
            params![guid, peer_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(data.and_then(|d| serde_json::from_str(&d).ok()))
}

fn save_ab_peer(conn: &Connection, guid: &str, peer: &Map<String, Value>) -> Result<()> {
    let id = peer.get("id").and_then(Value::as_str).unwrap_or_default();
    conn.execute(
        "INSERT INTO ab_peers (ab_guid, peer_id, data) VALUES (?1, ?2, ?3)
         ON CONFLICT(ab_guid, peer_id) DO UPDATE SET data = excluded.data",
        params![guid, id, Value::Object(peer.clone()).to_string()],
    )?;
    Ok(())
}

pub fn ab_add_peer(conn: &Connection, guid: &str, peer: Map<String, Value>) -> Result<()> {
    save_ab_peer(conn, guid, &peer)
}

/// Merges the given fields into an existing peer. Returns false if the peer is unknown.
pub fn ab_update_peer(conn: &Connection, guid: &str, fields: Map<String, Value>) -> Result<bool> {
    let id = fields.get("id").and_then(Value::as_str).unwrap_or_default();
    let Some(mut peer) = ab_peer(conn, guid, id)? else {
        return Ok(false);
    };
    for (k, v) in fields {
        peer.insert(k, v);
    }
    save_ab_peer(conn, guid, &peer)?;
    Ok(true)
}

pub fn ab_delete_peers(conn: &Connection, guid: &str, ids: &[String]) -> Result<()> {
    for id in ids {
        conn.execute(
            "DELETE FROM ab_peers WHERE ab_guid = ?1 AND peer_id = ?2",
            params![guid, id],
        )?;
    }
    Ok(())
}

pub fn ab_tags(conn: &Connection, guid: &str) -> Result<Vec<Value>> {
    let mut st =
        conn.prepare("SELECT name, color FROM ab_tags WHERE ab_guid = ?1 ORDER BY rowid")?;
    let rows = st.query_map([guid], |r| {
        Ok(json!({ "name": r.get::<_, String>(0)?, "color": r.get::<_, i64>(1)? }))
    })?;
    rows.collect()
}

pub fn ab_upsert_tag(conn: &Connection, guid: &str, name: &str, color: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO ab_tags (ab_guid, name, color) VALUES (?1, ?2, ?3)
         ON CONFLICT(ab_guid, name) DO UPDATE SET color = excluded.color",
        params![guid, name, color],
    )?;
    Ok(())
}

/// Applies `f` to every peer's `tags` array in the address book.
fn rewrite_peer_tags(
    conn: &Connection,
    guid: &str,
    f: impl Fn(&str) -> Option<String>,
) -> Result<()> {
    let (_, peers) = ab_peers(conn, guid, i64::MAX, 0)?;
    for peer in peers {
        let Value::Object(mut peer) = peer else {
            continue;
        };
        let Some(Value::Array(tags)) = peer.get("tags") else {
            continue;
        };
        let new: Vec<Value> = tags
            .iter()
            .filter_map(|t| t.as_str().and_then(&f).map(Value::String))
            .collect();
        if &new != tags {
            peer.insert("tags".to_owned(), Value::Array(new));
            save_ab_peer(conn, guid, &peer)?;
        }
    }
    Ok(())
}

pub fn ab_rename_tag(conn: &Connection, guid: &str, old: &str, new: &str) -> Result<()> {
    conn.execute(
        "UPDATE ab_tags SET name = ?1 WHERE ab_guid = ?2 AND name = ?3",
        params![new, guid, old],
    )?;
    rewrite_peer_tags(conn, guid, |t| {
        Some(if t == old { new } else { t }.to_owned())
    })
}

pub fn ab_delete_tags(conn: &Connection, guid: &str, names: &[String]) -> Result<()> {
    for name in names {
        conn.execute(
            "DELETE FROM ab_tags WHERE ab_guid = ?1 AND name = ?2",
            params![guid, name],
        )?;
    }
    rewrite_peer_tags(conn, guid, |t| {
        (!names.iter().any(|n| n == t)).then(|| t.to_owned())
    })
}

pub fn legacy_ab_get(conn: &Connection, user_id: i64) -> Result<Option<String>> {
    conn.query_row(
        "SELECT data FROM legacy_ab WHERE user_id = ?1",
        [user_id],
        |r| r.get(0),
    )
    .optional()
}

pub fn legacy_ab_set(conn: &Connection, user_id: i64, data: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO legacy_ab (user_id, data) VALUES (?1, ?2)
         ON CONFLICT(user_id) DO UPDATE SET data = excluded.data",
        params![user_id, data],
    )?;
    Ok(())
}

// ---------------------------------------------------------------- devices

/// Records a heartbeat. Returns true when the server still needs the device's sysinfo.
pub fn heartbeat(conn: &Connection, id: &str, uuid: &str, conns: i64) -> Result<bool> {
    let t = now();
    conn.execute(
        "INSERT INTO devices (id, uuid, conns, created_at, last_seen) VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(id) DO UPDATE SET last_seen = ?4, conns = ?3,
           uuid = CASE WHEN ?2 = '' THEN uuid ELSE ?2 END",
        params![id, uuid, conns, t],
    )?;
    conn.query_row(
        "SELECT sysinfo_at IS NULL FROM devices WHERE id = ?1",
        [id],
        |r| r.get(0),
    )
}

pub fn sysinfo(conn: &Connection, id: &str, info: &Map<String, Value>) -> Result<()> {
    let s = |k: &str| {
        info.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let t = now();
    conn.execute(
        "INSERT INTO devices (id, uuid, hostname, username, os, version, info, created_at, last_seen, sysinfo_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?8)
         ON CONFLICT(id) DO UPDATE SET uuid = ?2, hostname = ?3, username = ?4, os = ?5,
           version = ?6, info = ?7, last_seen = ?8, sysinfo_at = ?8",
        params![
            id,
            s("uuid"),
            s("hostname"),
            s("username"),
            s("os"),
            s("version"),
            Value::Object(info.clone()).to_string(),
            t
        ],
    )?;
    Ok(())
}

/// Associates a device with the user who logged in on it.
pub fn link_device(conn: &Connection, id: &str, uuid: &str, user_id: i64) -> Result<()> {
    if id.is_empty() {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO devices (id, uuid, user_id, created_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET user_id = ?3",
        params![id, uuid, user_id, now()],
    )?;
    Ok(())
}

#[derive(Debug)]
pub struct Device {
    pub id: String,
    pub hostname: String,
    pub username: String,
    pub os: String,
    pub version: String,
    pub info: Value,
    pub owner: String,
    pub note: String,
    pub conns: i64,
    pub last_seen: Option<i64>,
}

/// Devices visible to `user`: all for admins, otherwise those linked to the user.
pub fn list_devices(
    conn: &Connection,
    user: Option<&User>,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<Device>)> {
    let filter_user = user.filter(|u| !u.is_admin).map(|u| u.id);
    let total = conn.query_row(
        "SELECT COUNT(*) FROM devices WHERE ?1 IS NULL OR user_id = ?1",
        [filter_user],
        |r| r.get(0),
    )?;
    let mut st = conn.prepare(
        "SELECT d.id, d.hostname, d.username, d.os, d.version, d.info, COALESCE(u.name, ''),
                d.note, d.conns, d.last_seen
         FROM devices d LEFT JOIN users u ON u.id = d.user_id
         WHERE ?1 IS NULL OR d.user_id = ?1
         ORDER BY d.id LIMIT ?2 OFFSET ?3",
    )?;
    let rows = st.query_map(params![filter_user, limit, offset], |r| {
        Ok(Device {
            id: r.get(0)?,
            hostname: r.get(1)?,
            username: r.get(2)?,
            os: r.get(3)?,
            version: r.get(4)?,
            info: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or(Value::Null),
            owner: r.get(6)?,
            note: r.get(7)?,
            conns: r.get(8)?,
            last_seen: r.get(9)?,
        })
    })?;
    Ok((total, rows.collect::<Result<_>>()?))
}

pub fn delete_device(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn.execute("DELETE FROM devices WHERE id = ?1", [id])? > 0)
}

// ---------------------------------------------------------------- audit

/// Runs `f` unless `nonce` was already stored; the nonce and the record commit together,
/// so a failed write lets the client's retry store the record.
pub fn with_nonce(
    conn: &mut Connection,
    nonce: Option<&str>,
    f: impl FnOnce(&Connection) -> Result<()>,
) -> Result<()> {
    let tx = conn.transaction()?;
    if let Some(nonce) = nonce.filter(|n| !n.is_empty()) {
        let t = now();
        tx.execute("DELETE FROM audit_nonce WHERE created_at < ?1", [t - 3600])?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO audit_nonce (nonce, created_at) VALUES (?1, ?2)",
            params![nonce, t],
        )?;
        if inserted == 0 {
            return Ok(());
        }
    }
    f(&tx)?;
    tx.commit()
}

fn latest_open_conn(conn: &Connection, device_id: &str, conn_id: i64) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM audit_conn WHERE device_id = ?1 AND conn_id = ?2 AND ended_at IS NULL
         ORDER BY id DESC LIMIT 1",
        params![device_id, conn_id],
        |r| r.get(0),
    )
    .optional()
}

fn new_conn_row(conn: &Connection, device_id: &str, conn_id: i64, ip: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO audit_conn (guid, device_id, conn_id, ip, started_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![random_hex(16), device_id, conn_id, ip, now()],
    )?;
    Ok(conn.last_insert_rowid())
}

fn json_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Handles one `/api/audit/conn` post (src/server/connection.rs `post_conn_audit`,
/// src/ui_session_interface.rs `send_note`).
pub fn audit_conn(conn: &Connection, v: &Map<String, Value>) -> Result<()> {
    let device_id = v.get("id").map(json_str).unwrap_or_default();
    let conn_id = v.get("conn_id").and_then(Value::as_i64).unwrap_or(0);
    let session_id = v.get("session_id").map(json_str).unwrap_or_default();
    let t = now();
    if let Some(note) = v.get("note").and_then(Value::as_str) {
        // Sent by the controlling side: `id` is the controlled device.
        conn.execute(
            "UPDATE audit_conn SET note = ?1 WHERE id = (
               SELECT id FROM audit_conn WHERE device_id = ?2 AND session_id = ?3
               ORDER BY id DESC LIMIT 1)",
            params![note, device_id, session_id],
        )?;
        return Ok(());
    }
    match v.get("action").and_then(Value::as_str) {
        Some("new") => {
            let ip = v.get("ip").map(json_str).unwrap_or_default();
            new_conn_row(conn, &device_id, conn_id, &ip)?;
        }
        Some("close") => {
            let row = match latest_open_conn(conn, &device_id, conn_id)? {
                Some(row) => row,
                None => new_conn_row(conn, &device_id, conn_id, "")?,
            };
            conn.execute(
                "UPDATE audit_conn SET ended_at = ?1 WHERE id = ?2",
                params![t, row],
            )?;
        }
        _ => {
            if let Some(peer) = v.get("peer").and_then(Value::as_array) {
                let row = match latest_open_conn(conn, &device_id, conn_id)? {
                    Some(row) => row,
                    None => new_conn_row(conn, &device_id, conn_id, "")?,
                };
                conn.execute(
                    "UPDATE audit_conn SET peer_id = ?1, peer_name = ?2, conn_type = ?3,
                       primary_auth = ?4, two_factor = ?5, session_id = ?6, authed_at = ?7
                     WHERE id = ?8",
                    params![
                        peer.first().map(json_str).unwrap_or_default(),
                        peer.get(1).map(json_str).unwrap_or_default(),
                        v.get("type").and_then(Value::as_i64),
                        v.get("primary_auth").and_then(Value::as_i64),
                        v.get("two_factor").and_then(Value::as_i64),
                        session_id,
                        t,
                        row
                    ],
                )?;
            }
        }
    }
    Ok(())
}

pub fn audit_file(conn: &Connection, v: &Map<String, Value>) -> Result<()> {
    conn.execute(
        "INSERT INTO audit_file (device_id, peer_id, conn_id, typ, path, is_file, info, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            v.get("id").map(json_str).unwrap_or_default(),
            v.get("peer_id").map(json_str).unwrap_or_default(),
            v.get("conn_id").and_then(Value::as_i64).unwrap_or(0),
            v.get("type").and_then(Value::as_i64).unwrap_or(0),
            v.get("path").map(json_str).unwrap_or_default(),
            v.get("is_file").and_then(Value::as_bool).unwrap_or(false),
            v.get("info").map(json_str).unwrap_or_default(),
            now()
        ],
    )?;
    Ok(())
}

pub fn audit_alarm(conn: &Connection, v: &Map<String, Value>) -> Result<()> {
    conn.execute(
        "INSERT INTO audit_alarm (device_id, typ, conn_id, info, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            v.get("id").map(json_str).unwrap_or_default(),
            v.get("typ").and_then(Value::as_i64).unwrap_or(0),
            v.get("conn_id").and_then(Value::as_i64).unwrap_or(0),
            v.get("info").map(json_str).unwrap_or_default(),
            now()
        ],
    )?;
    Ok(())
}

pub fn audit_set_note(conn: &Connection, guid: &str, note: &str) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE audit_conn SET note = ?1 WHERE guid = ?2",
        params![note, guid],
    )? > 0)
}

/// Generic newest-first listing used by the admin page.
pub fn list_audit(
    conn: &Connection,
    kind: &str,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<Value>)> {
    let (table, cols): (&str, &[&str]) = match kind {
        "conn" => (
            "audit_conn",
            &[
                "id",
                "guid",
                "device_id",
                "conn_id",
                "session_id",
                "ip",
                "peer_id",
                "peer_name",
                "conn_type",
                "primary_auth",
                "two_factor",
                "note",
                "started_at",
                "authed_at",
                "ended_at",
            ],
        ),
        "file" => (
            "audit_file",
            &[
                "id",
                "device_id",
                "peer_id",
                "conn_id",
                "typ",
                "path",
                "is_file",
                "info",
                "created_at",
            ],
        ),
        "alarm" => (
            "audit_alarm",
            &["id", "device_id", "typ", "conn_id", "info", "created_at"],
        ),
        _ => return Ok((0, Vec::new())),
    };
    // Table and column names come from the fixed lists above, never from input.
    let total = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?;
    let mut st = conn.prepare(&format!(
        "SELECT a.{}, COALESCE(d.hostname, '') FROM {table} a LEFT JOIN devices d ON d.id = a.device_id
         ORDER BY a.id DESC LIMIT ?1 OFFSET ?2",
        cols.join(", a.")
    ))?;
    let n = cols.len();
    let rows = st.query_map(params![limit, offset], |r| {
        let mut m = Map::new();
        for (i, c) in cols.iter().enumerate() {
            let v = match r.get_ref(i)? {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(x) => json!(x),
                rusqlite::types::ValueRef::Real(x) => json!(x),
                rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                rusqlite::types::ValueRef::Blob(_) => Value::Null,
            };
            m.insert((*c).to_owned(), v);
        }
        m.insert("hostname".to_owned(), json!(r.get::<_, String>(n)?));
        Ok(Value::Object(m))
    })?;
    Ok((total, rows.collect::<Result<_>>()?))
}
