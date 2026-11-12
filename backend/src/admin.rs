//! JSON API behind the built-in web console. Admin only.

use crate::{
    api::peer_payload, db, paged, parse_json, AdminUser, ApiError, ApiResult, AppState, Page,
};
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub async fn devices(
    State(st): State<AppState>,
    AdminUser(admin): AdminUser,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let (limit, offset) = page.limit_offset();
    let (total, devices) = db::list_devices(&st.db(), Some(&admin), limit, offset)?;
    let data = devices
        .iter()
        .map(|d| peer_payload(d, st.cfg.online_timeout_secs))
        .collect();
    Ok(paged(total, data))
}

pub async fn delete_device(
    State(st): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    if !db::delete_device(&st.db(), &id)? {
        return Err(ApiError::not_found("Device not found"));
    }
    Ok(StatusCode::OK)
}

pub async fn audit(
    State(st): State<AppState>,
    _admin: AdminUser,
    Path(kind): Path<String>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    if !matches!(kind.as_str(), "conn" | "file" | "alarm") {
        return Err(ApiError::not_found("Unknown audit type"));
    }
    let (limit, offset) = page.limit_offset();
    let (total, data) = db::list_audit(&st.db(), &kind, limit, offset)?;
    Ok(paged(total, data))
}

pub async fn users(State(st): State<AppState>, _admin: AdminUser) -> ApiResult<Json<Value>> {
    let users = db::list_users(&st.db())?;
    let data: Vec<Value> = users.iter().map(db::User::payload).collect();
    Ok(paged(data.len() as i64, data))
}

#[derive(Deserialize)]
pub struct NewUser {
    name: String,
    password: String,
    #[serde(default)]
    is_admin: bool,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-@".contains(c))
}

fn check_password(p: &str) -> ApiResult<()> {
    if p.chars().count() < 8 {
        return Err(ApiError::bad_request(
            "Password must be at least 8 characters",
        ));
    }
    Ok(())
}

async fn hash(password: String) -> ApiResult<String> {
    tokio::task::spawn_blocking(move || db::hash_password(&password))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e))
}

pub async fn create_user(
    State(st): State<AppState>,
    AdminUser(admin): AdminUser,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let req: NewUser = parse_json(&body)?;
    if !valid_name(&req.name) {
        return Err(ApiError::bad_request(
            "Invalid user name (letters, digits, . _ - @)",
        ));
    }
    check_password(&req.password)?;
    let h = hash(req.password).await?;
    let conn = st.db();
    if db::user_by_name(&conn, &req.name)?.is_some() {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "User already exists".to_owned(),
        ));
    }
    db::create_user(&conn, &req.name, &h, req.is_admin)?;
    tracing::info!("admin {} created user {}", admin.name, req.name);
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub struct UpdateUser {
    password: Option<String>,
    is_admin: Option<bool>,
    /// 1 = enabled, 0 = disabled (as in the client's `UserPayload.status`).
    status: Option<i64>,
}

pub async fn update_user(
    State(st): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(name): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let req: UpdateUser = parse_json(&body)?;
    let new_hash = match req.password {
        Some(p) => {
            check_password(&p)?;
            Some(hash(p).await?)
        }
        None => None,
    };
    let conn = st.db();
    let (user, _) =
        db::user_by_name(&conn, &name)?.ok_or_else(|| ApiError::not_found("User not found"))?;
    let is_admin = req.is_admin.unwrap_or(user.is_admin);
    let status = match req.status.unwrap_or(user.status) {
        0 => 0,
        _ => 1,
    };
    let loses_admin = user.is_admin && user.status == 1 && (!is_admin || status != 1);
    if loses_admin && db::count_active_admins(&conn)? <= 1 {
        return Err(ApiError::bad_request("Cannot remove the last active admin"));
    }
    db::set_user_flags(&conn, user.id, is_admin, status)?;
    if let Some(h) = new_hash {
        db::set_user_password(&conn, user.id, &h)?;
    }
    tracing::info!("admin {} updated user {}", admin.name, user.name);
    Ok(Json(json!({})))
}

pub async fn delete_user(
    State(st): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    let conn = st.db();
    let (user, _) =
        db::user_by_name(&conn, &name)?.ok_or_else(|| ApiError::not_found("User not found"))?;
    if user.id == admin.id {
        return Err(ApiError::bad_request("You cannot delete yourself"));
    }
    if user.is_admin && user.status == 1 && db::count_active_admins(&conn)? <= 1 {
        return Err(ApiError::bad_request("Cannot remove the last active admin"));
    }
    db::delete_user(&conn, user.id)?;
    tracing::info!("admin {} deleted user {}", admin.name, user.name);
    Ok(Json(json!({})))
}
