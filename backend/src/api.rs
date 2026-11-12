//! Endpoints called by the RustDesk client. The README lists which client file each one
//! was derived from.

use crate::{db, paged, parse_json, ApiError, ApiResult, AppState, AuthUser, Page};
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};

type JsonMap = Map<String, Value>;

// ---------------------------------------------------------------- account

#[derive(Deserialize)]
pub struct LoginRequest {
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    uuid: String,
    #[serde(default, rename = "type")]
    typ: Option<String>,
}

pub async fn login(State(st): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let req: LoginRequest = parse_json(&body)?;
    if req.typ.as_deref().is_some_and(|t| t != "account") {
        return Err(ApiError::bad_request("Unsupported login type"));
    }
    let found = db::user_by_name(&st.db(), req.username.trim())?;
    let ok = match &found {
        Some((user, hash)) if user.status == 1 => {
            let (password, hash) = (req.password.clone(), hash.clone());
            tokio::task::spawn_blocking(move || db::verify_password(&password, &hash))
                .await
                .unwrap_or(false)
        }
        _ => false,
    };
    let Some((user, _)) = found.filter(|_| ok) else {
        tracing::warn!(
            "failed login for user {:?} from device {:?}",
            req.username,
            req.id
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "Wrong username or password".to_owned(),
        ));
    };
    let token = {
        let conn = st.db();
        db::link_device(&conn, &req.id, &req.uuid, user.id)?;
        db::create_token(&conn, user.id, &req.id, st.cfg.token_ttl_days)?
    };
    tracing::info!("user {} logged in from device {:?}", user.name, req.id);
    Ok(Json(json!({
        "type": "access_token",
        "access_token": token,
        "user": user.payload(),
    })))
}

pub async fn logout(State(st): State<AppState>, headers: HeaderMap) -> ApiResult<StatusCode> {
    if let Some(token) = crate::bearer(&headers) {
        db::delete_token(&st.db(), &token)?;
    }
    Ok(StatusCode::OK)
}

pub async fn current_user(auth: AuthUser) -> Json<Value> {
    Json(auth.user.payload())
}

/// No third-party (OIDC) login providers: the client then shows only the password form.
pub async fn login_options() -> Json<Value> {
    Json(json!([]))
}

pub async fn oidc_unsupported() -> ApiError {
    ApiError::not_found("OIDC login is not supported by this server")
}

// ---------------------------------------------------------------- device registry

#[derive(Deserialize)]
pub struct Heartbeat {
    #[serde(default)]
    id: String,
    #[serde(default)]
    uuid: String,
    #[serde(default)]
    conns: Vec<i64>,
}

/// src/hbbs_http/sync.rs. A `sysinfo` key in the reply makes the client upload its sysinfo.
pub async fn heartbeat(State(st): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let hb: Heartbeat = parse_json(&body)?;
    if hb.id.is_empty() {
        return Ok(Json(json!({})));
    }
    let need_sysinfo = db::heartbeat(&st.db(), &hb.id, &hb.uuid, hb.conns.len() as i64)?;
    Ok(Json(if need_sysinfo {
        json!({ "sysinfo": true })
    } else {
        json!({})
    }))
}

/// src/hbbs_http/sync.rs: the reply is plain text, `SYSINFO_UPDATED` on success.
pub async fn sysinfo(State(st): State<AppState>, body: Bytes) -> ApiResult<&'static str> {
    let info: JsonMap = parse_json(&body)?;
    let Some(id) = info
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return Ok("ID_NOT_FOUND");
    };
    db::sysinfo(&st.db(), id, &info)?;
    Ok("SYSINFO_UPDATED")
}

pub async fn sysinfo_ver() -> &'static str {
    "1"
}

/// src/ui_interface.rs `deploy_device`: this server never requires deployment.
pub async fn devices_deploy() -> Json<Value> {
    Json(json!({ "result": "NOT_ENABLED" }))
}

/// src/core_main.rs `--assign`: user/group assignment is not supported.
pub async fn devices_cli(_auth: AuthUser) -> ApiError {
    ApiError::not_found("Device assignment is not supported by this server")
}

/// src/hbbs_http/sync.rs `register_switch_grant`. Nothing consumes the grant with the
/// open-source ID server, so acknowledge it to keep the client quiet.
pub async fn switch_grant() -> Json<Value> {
    Json(json!({ "accepted": true }))
}

/// src/hbbs_http/record_upload.rs: an `error` reply stops the upload.
pub async fn record() -> ApiError {
    ApiError::not_found("Recording upload is not supported by this server")
}

// ---------------------------------------------------------------- audit

/// src/server/connection.rs: success is HTTP 200 with an empty body.
pub async fn audit_post(
    State(st): State<AppState>,
    Path(kind): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let v: JsonMap = parse_json(&body)?;
    let f: fn(&rusqlite::Connection, &JsonMap) -> rusqlite::Result<()> = match kind.as_str() {
        "conn" => db::audit_conn,
        "file" => db::audit_file,
        "alarm" => db::audit_alarm,
        _ => return Err(ApiError::not_found("Unknown audit type")),
    };
    let nonce = v.get("nonce").and_then(Value::as_str);
    db::with_nonce(&mut st.db(), nonce, |c| f(c, &v))?;
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
pub struct AuditNote {
    guid: String,
    #[serde(default)]
    note: String,
}

/// flutter/lib/common/widgets/dialog.dart: note entered at the end of a connection.
pub async fn audit_note(
    State(st): State<AppState>,
    _auth: AuthUser,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let req: AuditNote = parse_json(&body)?;
    if !db::audit_set_note(&st.db(), &req.guid, &req.note)? {
        return Err(ApiError::not_found("Audit record not found"));
    }
    Ok(StatusCode::OK)
}

// ---------------------------------------------------------------- listings (group tab)

pub async fn users(
    State(st): State<AppState>,
    auth: AuthUser,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    if !auth.user.is_admin {
        return Ok(paged(1, vec![auth.user.payload()]));
    }
    let all = db::list_users(&st.db())?;
    let (limit, offset) = page.limit_offset();
    let data = all
        .iter()
        .filter(|u| u.status == 1)
        .skip(offset as usize)
        .take(limit as usize)
        .map(db::User::payload)
        .collect();
    Ok(paged(
        all.iter().filter(|u| u.status == 1).count() as i64,
        data,
    ))
}

/// Shape of the client's `PeerPayload` (flutter/lib/common/hbbs/hbbs.dart).
pub fn peer_payload(d: &db::Device, online_timeout: i64) -> Value {
    let field = |k: &str| d.info.get(k).cloned().unwrap_or(Value::Null);
    let online = d.last_seen.is_some_and(|t| db::now() - t <= online_timeout);
    json!({
        "id": d.id,
        "info": {
            "username": d.username,
            "os": d.os,
            "device_name": d.hostname,
            "cpu": field("cpu"),
            "memory": field("memory"),
        },
        "status": 1,
        "user": d.owner,
        "user_name": d.owner,
        "device_group_name": "",
        "note": d.note,
        "version": d.version,
        "last_online": d.last_seen,
        "online": online,
        "conns": d.conns,
    })
}

pub async fn peers(
    State(st): State<AppState>,
    auth: AuthUser,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let (limit, offset) = page.limit_offset();
    let (total, devices) = db::list_devices(&st.db(), Some(&auth.user), limit, offset)?;
    let data = devices
        .iter()
        .map(|d| peer_payload(d, st.cfg.online_timeout_secs))
        .collect();
    Ok(paged(total, data))
}

pub async fn device_groups(_auth: AuthUser) -> Json<Value> {
    paged(0, vec![])
}

// ---------------------------------------------------------------- address book

pub async fn ab_settings(_auth: AuthUser) -> Json<Value> {
    Json(json!({ "max_peer_one_ab": 0 }))
}

pub async fn ab_personal(State(st): State<AppState>, auth: AuthUser) -> ApiResult<Json<Value>> {
    let guid = db::personal_ab_guid(&st.db(), auth.user.id)?;
    Ok(Json(json!({ "guid": guid })))
}

/// Only personal address books exist; sharing is not implemented.
pub async fn ab_shared_profiles(_auth: AuthUser) -> Json<Value> {
    paged(0, vec![])
}

fn check_ab(st: &AppState, auth: &AuthUser, guid: &str) -> ApiResult<()> {
    if db::ab_owned_by(&st.db(), guid, auth.user.id)? {
        Ok(())
    } else {
        Err(ApiError::not_found("Address book not found"))
    }
}

#[derive(Deserialize)]
pub struct AbPeersQuery {
    current: Option<i64>,
    #[serde(rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    ab: String,
}

pub async fn ab_peers(
    State(st): State<AppState>,
    auth: AuthUser,
    Query(q): Query<AbPeersQuery>,
) -> ApiResult<Json<Value>> {
    check_ab(&st, &auth, &q.ab)?;
    let (limit, offset) = Page {
        current: q.current,
        page_size: q.page_size,
    }
    .limit_offset();
    let (total, data) = db::ab_peers(&st.db(), &q.ab, limit, offset)?;
    Ok(paged(total, data))
}

pub async fn ab_tags(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(guid): Path<String>,
) -> ApiResult<Json<Value>> {
    check_ab(&st, &auth, &guid)?;
    Ok(Json(Value::Array(db::ab_tags(&st.db(), &guid)?)))
}

fn peer_id(m: &JsonMap) -> ApiResult<()> {
    match m.get("id").and_then(Value::as_str) {
        Some(id) if !id.is_empty() => Ok(()),
        _ => Err(ApiError::bad_request("Peer id required")),
    }
}

pub async fn ab_peer_add(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(guid): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    check_ab(&st, &auth, &guid)?;
    let peer: JsonMap = parse_json(&body)?;
    peer_id(&peer)?;
    db::ab_add_peer(&st.db(), &guid, peer)?;
    Ok(StatusCode::OK)
}

pub async fn ab_peer_update(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(guid): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    check_ab(&st, &auth, &guid)?;
    let fields: JsonMap = parse_json(&body)?;
    peer_id(&fields)?;
    if !db::ab_update_peer(&st.db(), &guid, fields)? {
        return Err(ApiError::not_found("Peer not found"));
    }
    Ok(StatusCode::OK)
}

pub async fn ab_peer_delete(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(guid): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    check_ab(&st, &auth, &guid)?;
    let ids: Vec<String> = parse_json(&body)?;
    db::ab_delete_peers(&st.db(), &guid, &ids)?;
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
pub struct TagReq {
    name: String,
    #[serde(default)]
    color: i64,
}

pub async fn ab_tag_add(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(guid): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    check_ab(&st, &auth, &guid)?;
    let t: TagReq = parse_json(&body)?;
    if t.name.is_empty() {
        return Err(ApiError::bad_request("Tag name required"));
    }
    db::ab_upsert_tag(&st.db(), &guid, &t.name, t.color)?;
    Ok(StatusCode::OK)
}

pub async fn ab_tag_update(
    st: State<AppState>,
    auth: AuthUser,
    guid: Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    ab_tag_add(st, auth, guid, body).await
}

#[derive(Deserialize)]
pub struct TagRename {
    old: String,
    new: String,
}

pub async fn ab_tag_rename(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(guid): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    check_ab(&st, &auth, &guid)?;
    let r: TagRename = parse_json(&body)?;
    if r.new.is_empty() {
        return Err(ApiError::bad_request("Tag name required"));
    }
    db::ab_rename_tag(&st.db(), &guid, &r.old, &r.new)?;
    Ok(StatusCode::OK)
}

pub async fn ab_tag_delete(
    State(st): State<AppState>,
    auth: AuthUser,
    Path(guid): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    check_ab(&st, &auth, &guid)?;
    let names: Vec<String> = parse_json(&body)?;
    db::ab_delete_tags(&st.db(), &guid, &names)?;
    Ok(StatusCode::OK)
}

/// Legacy single-blob address book (`LegacyAb` in flutter/lib/models/ab_model.dart).
/// The reply is `null` when nothing was stored yet.
pub async fn legacy_ab_get(
    State(st): State<AppState>,
    auth: AuthUser,
) -> ApiResult<impl IntoResponse> {
    Ok(Json(match db::legacy_ab_get(&st.db(), auth.user.id)? {
        Some(data) => json!({ "data": data, "licensed_devices": 0 }),
        None => Value::Null,
    }))
}

#[derive(Deserialize)]
pub struct LegacyAbBody {
    data: String,
}

pub async fn legacy_ab_set(
    State(st): State<AppState>,
    auth: AuthUser,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let b: LegacyAbBody = parse_json(&body)?;
    db::legacy_ab_set(&st.db(), auth.user.id, &b.data)?;
    Ok(StatusCode::OK)
}
