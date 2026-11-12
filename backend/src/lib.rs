//! A small API server for the RustDesk client: login, address book, device registry and audit log.

pub mod admin;
pub mod api;
pub mod config;
pub mod db;

use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    future::Future,
    sync::{Arc, Mutex, MutexGuard},
};

pub use config::Config;

#[derive(Clone)]
pub struct AppState {
    db: Arc<Mutex<Connection>>,
    pub cfg: Arc<Config>,
}

impl AppState {
    pub fn new(conn: Connection, cfg: Config) -> Self {
        Self {
            db: Arc::new(Mutex::new(conn)),
            cfg: Arc::new(cfg),
        }
    }

    /// Database statements are short; handlers take the lock, run them and release it
    /// without awaiting in between.
    pub fn db(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Error reply in the shape the client reads: `{"error": "..."}`.
#[derive(Debug)]
pub struct ApiError(pub StatusCode, pub String);

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, msg.into())
    }
    pub fn unauthorized() -> Self {
        Self(StatusCode::UNAUTHORIZED, "Unauthorized".to_owned())
    }
    pub fn forbidden() -> Self {
        Self(StatusCode::FORBIDDEN, "Admin required!".to_owned())
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self(StatusCode::NOT_FOUND, msg.into())
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        tracing::error!("database error: {e}");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database error".to_owned(),
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// A request carrying a valid `Authorization: Bearer <token>` header.
pub struct AuthUser {
    pub user: db::User,
    pub token: String,
}

pub fn bearer(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = v
        .strip_prefix("Bearer ")
        .or_else(|| v.strip_prefix("bearer "))?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_owned())
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer(&parts.headers).ok_or_else(ApiError::unauthorized)?;
        let user = db::user_by_token(&state.db(), &token)?.ok_or_else(ApiError::unauthorized)?;
        Ok(AuthUser { user, token })
    }
}

/// An authenticated administrator.
pub struct AdminUser(pub db::User);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let AuthUser { user, .. } = AuthUser::from_request_parts(parts, state).await?;
        if !user.is_admin {
            return Err(ApiError::forbidden());
        }
        Ok(AdminUser(user))
    }
}

/// The client's pagination query: `current` (1-based) and `pageSize`.
#[derive(Debug, Deserialize)]
pub struct Page {
    pub current: Option<i64>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<i64>,
}

impl Page {
    pub fn limit_offset(&self) -> (i64, i64) {
        let size = self.page_size.unwrap_or(100).clamp(1, 1000);
        let current = self.current.unwrap_or(1).max(1);
        (size, (current - 1) * size)
    }
}

/// Parses a JSON body regardless of the Content-Type the client sent.
pub fn parse_json<T: serde::de::DeserializeOwned>(body: &[u8]) -> ApiResult<T> {
    serde_json::from_slice(body).map_err(|e| ApiError::bad_request(format!("invalid json: {e}")))
}

pub fn paged(total: i64, data: Vec<Value>) -> Json<Value> {
    Json(json!({ "total": total, "data": data }))
}

async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_SECURITY_POLICY, "default-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; frame-ancestors 'none'")],
        Html(include_str!("../static/admin.html")),
    )
}

pub fn router(state: AppState) -> Router {
    use api::*;
    Router::new()
        .route("/", get(index))
        // account
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/currentUser", post(current_user))
        .route("/api/login-options", get(login_options))
        .route("/api/oidc/auth", post(oidc_unsupported))
        .route("/api/oidc/auth-query", get(oidc_unsupported))
        // device registry
        .route("/api/heartbeat", post(heartbeat))
        .route("/api/sysinfo", post(sysinfo))
        .route("/api/sysinfo_ver", post(sysinfo_ver))
        .route("/api/devices/deploy", post(devices_deploy))
        .route("/api/devices/cli", post(devices_cli))
        .route("/api/switch-grant", post(switch_grant))
        .route("/api/record", post(record))
        // audit
        .route("/api/audit/{kind}", post(audit_post))
        .route("/api/audit", put(audit_note))
        // group / accessible listings
        .route("/api/users", get(users))
        .route("/api/peers", get(peers))
        .route("/api/device-group/accessible", get(device_groups))
        // address book (current API)
        .route("/api/ab/settings", post(ab_settings))
        .route("/api/ab/personal", post(ab_personal))
        .route("/api/ab/shared/profiles", post(ab_shared_profiles))
        .route("/api/ab/peers", post(ab_peers))
        .route("/api/ab/tags/{guid}", post(ab_tags))
        .route("/api/ab/peer/add/{guid}", post(ab_peer_add))
        .route("/api/ab/peer/update/{guid}", put(ab_peer_update))
        .route("/api/ab/peer/{guid}", delete(ab_peer_delete))
        .route("/api/ab/tag/add/{guid}", post(ab_tag_add))
        .route("/api/ab/tag/rename/{guid}", put(ab_tag_rename))
        .route("/api/ab/tag/update/{guid}", put(ab_tag_update))
        .route("/api/ab/tag/{guid}", delete(ab_tag_delete))
        // address book (legacy single-blob API)
        .route("/api/ab", get(legacy_ab_get).post(legacy_ab_set))
        // web console
        .route("/api/admin/devices", get(admin::devices))
        .route("/api/admin/devices/{id}", delete(admin::delete_device))
        .route("/api/admin/audit/{kind}", get(admin::audit))
        .route(
            "/api/admin/users",
            get(admin::users).post(admin::create_user),
        )
        .route(
            "/api/admin/users/{name}",
            put(admin::update_user).delete(admin::delete_user),
        )
        .fallback(|| async { ApiError::not_found("Not found") })
        .with_state(state)
}

/// Serves until `shutdown` resolves.
pub async fn serve(
    listener: tokio::net::TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown)
        .await
}

/// Opens the database, creates the first admin if needed and serves on `cfg.bind`.
pub async fn run(
    cfg: Config,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
    if let Some(dir) = cfg.db_path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let conn = db::open(&cfg.db_path)
        .map_err(|e| format!("cannot open database {}: {e}", cfg.db_path.display()))?;
    if let Some(password) = db::ensure_admin(&conn)? {
        let msg = format!(
            "Created initial user `admin` with password: {password}  (shown once; change it in the web console)"
        );
        // Goes to the console (foreground) and to the log file.
        tracing::warn!("{msg}");
    }
    let listener = tokio::net::TcpListener::bind(&cfg.bind)
        .await
        .map_err(|e| format!("cannot bind {}: {e}", cfg.bind))?;
    tracing::info!("listening on {}", cfg.bind);
    serve(listener, AppState::new(conn, cfg), shutdown)
        .await
        .map_err(|e| e.to_string())
}
