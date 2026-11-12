//! Starts the server on an ephemeral port and drives it the way the RustDesk client does.

use rdbackend::{db, AppState, Config};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};

struct Server {
    base: String,
    admin_password: String,
    http: Client,
    _dir: tempfile::TempDir,
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        db_path: dir.path().join("test.sqlite3"),
        log_dir: dir.path().join("logs"),
        ..Config::default()
    };
    let conn = db::open(&cfg.db_path).unwrap();
    let admin_password = db::ensure_admin(&conn)
        .unwrap()
        .expect("first run creates admin");
    assert!(
        db::ensure_admin(&conn).unwrap().is_none(),
        "admin is created only once"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(rdbackend::serve(
        listener,
        AppState::new(conn, cfg),
        std::future::pending(),
    ));
    Server {
        base,
        admin_password,
        http: Client::new(),
        _dir: dir,
    }
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    async fn post(&self, path: &str, token: Option<&str>, body: Value) -> (StatusCode, String) {
        self.send(reqwest::Method::POST, path, token, Some(body))
            .await
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, String) {
        let mut req = self.http.request(method, self.url(path));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        if let Some(b) = body {
            req = req
                .header("Content-Type", "application/json")
                .body(b.to_string());
        }
        let resp = req.send().await.unwrap();
        (resp.status(), resp.text().await.unwrap())
    }

    async fn json(
        &self,
        method: reqwest::Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Value {
        let (status, text) = self.send(method, path, token, body).await;
        assert_eq!(status, StatusCode::OK, "{path}: {text}");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}: {text}"))
    }

    async fn login(&self, user: &str, password: &str, device: &str) -> String {
        let body = json!({
            "username": user, "password": password, "id": device, "uuid": "dXVpZA==",
            "autoLogin": true, "type": "account", "deviceInfo": {"os": "linux", "type": "client", "name": "test"}
        });
        let (status, text) = self.post("/api/login", None, body).await;
        assert_eq!(status, StatusCode::OK, "{text}");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["type"], "access_token");
        v["access_token"].as_str().unwrap().to_owned()
    }
}

use reqwest::Method as M;

#[tokio::test]
async fn login_current_user_logout() {
    let s = start().await;

    let (status, text) = s
        .post(
            "/api/login",
            None,
            json!({"username": "admin", "password": "wrong", "type": "account"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(serde_json::from_str::<Value>(&text).unwrap()["error"].is_string());

    let opts = s.json(M::GET, "/api/login-options", None, None).await;
    assert_eq!(opts, json!([]));

    let token = s.login("admin", &s.admin_password, "111111111").await;
    let me = s
        .json(
            M::POST,
            "/api/currentUser",
            Some(&token),
            Some(json!({"id": "111111111", "uuid": "x"})),
        )
        .await;
    assert_eq!(me["name"], "admin");
    assert_eq!(me["is_admin"], true);
    assert_eq!(me["status"], 1);

    let (status, _) = s.post("/api/currentUser", None, json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = s
        .post("/api/currentUser", Some("not-a-token"), json!({}))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = s
        .post("/api/logout", Some(&token), json!({"id": "111111111"}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = s.post("/api/currentUser", Some(&token), json!({})).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "token is revoked by logout"
    );

    let (_, page) = s.send(M::GET, "/", None, None).await;
    assert!(page.contains("rdbackend console"));
}

#[tokio::test]
async fn address_book_round_trip_across_devices() {
    let s = start().await;
    let t1 = s.login("admin", &s.admin_password, "111111111").await;

    // Personal-address-book mode, as probed by flutter/lib/models/ab_model.dart.
    let settings = s.json(M::POST, "/api/ab/settings", Some(&t1), None).await;
    assert_eq!(settings["max_peer_one_ab"], 0);
    let guid = s.json(M::POST, "/api/ab/personal", Some(&t1), None).await["guid"]
        .as_str()
        .unwrap()
        .to_owned();
    let shared = s
        .json(
            M::POST,
            "/api/ab/shared/profiles?current=1&pageSize=100",
            Some(&t1),
            None,
        )
        .await;
    assert_eq!(shared, json!({"total": 0, "data": []}));

    let empty = |r: (StatusCode, String)| assert_eq!(r, (StatusCode::OK, String::new()));
    empty(
        s.post(
            &format!("/api/ab/tag/add/{guid}"),
            Some(&t1),
            json!({"name": "home", "color": 4280391411u32}),
        )
        .await,
    );
    empty(
        s.post(
            &format!("/api/ab/tag/add/{guid}"),
            Some(&t1),
            json!({"name": "work", "color": 1}),
        )
        .await,
    );
    empty(s.post(&format!("/api/ab/peer/add/{guid}"), Some(&t1),
        json!({"id": "222222222", "hash": "h", "username": "bob", "hostname": "nas", "platform": "Linux", "alias": "", "tags": ["home", "work"]})).await);
    empty(
        s.post(
            &format!("/api/ab/peer/add/{guid}"),
            Some(&t1),
            json!({"id": "333333333", "tags": []}),
        )
        .await,
    );
    empty(
        s.send(
            M::PUT,
            &format!("/api/ab/peer/update/{guid}"),
            Some(&t1),
            Some(json!({"id": "222222222", "alias": "My NAS"})),
        )
        .await,
    );
    empty(
        s.send(
            M::PUT,
            &format!("/api/ab/tag/rename/{guid}"),
            Some(&t1),
            Some(json!({"old": "home", "new": "house"})),
        )
        .await,
    );
    empty(
        s.send(
            M::PUT,
            &format!("/api/ab/tag/update/{guid}"),
            Some(&t1),
            Some(json!({"name": "house", "color": 7})),
        )
        .await,
    );
    empty(
        s.send(
            M::DELETE,
            &format!("/api/ab/tag/{guid}"),
            Some(&t1),
            Some(json!(["work"])),
        )
        .await,
    );
    empty(
        s.send(
            M::DELETE,
            &format!("/api/ab/peer/{guid}"),
            Some(&t1),
            Some(json!(["333333333"])),
        )
        .await,
    );

    let (status, text) = s
        .send(
            M::PUT,
            &format!("/api/ab/peer/update/{guid}"),
            Some(&t1),
            Some(json!({"id": "999", "alias": "x"})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(text.contains("error"));

    // A second device logged in as the same user sees the same book.
    let t2 = s.login("admin", &s.admin_password, "444444444").await;
    let guid2 = s.json(M::POST, "/api/ab/personal", Some(&t2), None).await["guid"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(guid, guid2);
    let peers = s
        .json(
            M::POST,
            &format!("/api/ab/peers?current=1&pageSize=100&ab={guid}"),
            Some(&t2),
            None,
        )
        .await;
    assert_eq!(peers["total"], 1);
    let p = &peers["data"][0];
    assert_eq!(p["id"], "222222222");
    assert_eq!(p["alias"], "My NAS");
    assert_eq!(p["hostname"], "nas");
    assert_eq!(p["tags"], json!(["house"]));
    let tags = s
        .json(M::POST, &format!("/api/ab/tags/{guid}"), Some(&t2), None)
        .await;
    assert_eq!(tags, json!([{"name": "house", "color": 7}]));

    // Pagination as the client pages it.
    for i in 0..3 {
        empty(
            s.post(
                &format!("/api/ab/peer/add/{guid}"),
                Some(&t2),
                json!({"id": format!("50000000{i}")}),
            )
            .await,
        );
    }
    let page2 = s
        .json(
            M::POST,
            &format!("/api/ab/peers?current=2&pageSize=3&ab={guid}"),
            Some(&t2),
            None,
        )
        .await;
    assert_eq!(page2["total"], 4);
    assert_eq!(page2["data"].as_array().unwrap().len(), 1);

    // Another user cannot read this book.
    s.json(
        M::POST,
        "/api/admin/users",
        Some(&t1),
        Some(json!({"name": "eve", "password": "password123"})),
    )
    .await;
    let te = s.login("eve", "password123", "555555555").await;
    let (status, _) = s
        .post(&format!("/api/ab/peers?ab={guid}"), Some(&te), json!({}))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = s.send(M::GET, "/api/admin/devices", Some(&te), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Everything except login and the device-side endpoints needs a token.
    for path in [
        "/api/ab/personal",
        "/api/ab/settings",
        "/api/ab/peers",
        "/api/users",
        "/api/peers",
    ] {
        let m = if path.starts_with("/api/ab") {
            M::POST
        } else {
            M::GET
        };
        let (status, _) = s.send(m, path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }

    // Legacy single-blob address book.
    let (_, null) = s.send(M::GET, "/api/ab", Some(&te), None).await;
    assert_eq!(null, "null");
    let blob = json!({"tags": ["a"], "peers": [{"id": "1"}], "tag_colors": "{}"}).to_string();
    empty(s.post("/api/ab", Some(&te), json!({"data": blob})).await);
    let legacy = s.json(M::GET, "/api/ab", Some(&te), None).await;
    assert_eq!(legacy["data"], blob);
}

#[tokio::test]
async fn heartbeat_marks_device_online() {
    let s = start().await;

    let hb = json!({"id": "123456789", "uuid": "dXVpZA==", "ver": 1004000, "modified_at": 0});
    let r = s
        .json(M::POST, "/api/heartbeat", None, Some(hb.clone()))
        .await;
    assert_eq!(
        r,
        json!({"sysinfo": true}),
        "unknown device is asked for sysinfo"
    );

    let info = json!({
        "id": "123456789", "uuid": "dXVpZA==", "version": "1.4.0", "hostname": "desk",
        "username": "alice", "os": "windows / Windows 11 Pro - 10.0", "cpu": "x, 8/4 cores", "memory": "16GB"
    });
    let (status, text) = s.post("/api/sysinfo", None, info).await;
    assert_eq!((status, text.as_str()), (StatusCode::OK, "SYSINFO_UPDATED"));
    let (_, text) = s.post("/api/sysinfo", None, json!({"hostname": "x"})).await;
    assert_eq!(text, "ID_NOT_FOUND");

    let r = s
        .json(
            M::POST,
            "/api/heartbeat",
            None,
            Some(json!({"id": "123456789", "uuid": "dXVpZA==", "conns": [3]})),
        )
        .await;
    assert_eq!(r, json!({}));

    let token = s.login("admin", &s.admin_password, "987654321").await;
    let peers = s
        .json(
            M::GET,
            "/api/peers?current=1&pageSize=100&accessible=&status=1",
            Some(&token),
            None,
        )
        .await;
    let dev = peers["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "123456789")
        .expect("device listed")
        .clone();
    assert_eq!(dev["online"], true);
    assert_eq!(dev["status"], 1);
    assert_eq!(dev["info"]["device_name"], "desk");
    assert_eq!(dev["info"]["username"], "alice");
    assert_eq!(dev["info"]["os"], "windows / Windows 11 Pro - 10.0");
    assert_eq!(dev["conns"], 1);

    // The device the admin logged in on is linked to the account but never sent a heartbeat.
    let own = peers["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "987654321")
        .unwrap()
        .clone();
    assert_eq!(own["online"], false);
    assert_eq!(own["user_name"], "admin");

    let users = s
        .json(
            M::GET,
            "/api/users?current=1&pageSize=100&accessible=&status=1",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(users["total"], 1);
    assert_eq!(users["data"][0]["name"], "admin");
    let groups = s
        .json(
            M::GET,
            "/api/device-group/accessible?current=1&pageSize=100",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(groups, json!({"total": 0, "data": []}));

    let admin = s
        .json(M::GET, "/api/admin/devices", Some(&token), None)
        .await;
    assert_eq!(admin["total"], 2);
}

#[tokio::test]
async fn audit_log_records_connections() {
    let s = start().await;
    let dev = |extra: Value| {
        let mut v = json!({"id": "123456789", "uuid": "dXVpZA==", "conn_id": 7, "session_id": 0});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v
    };
    let ok = |r: (StatusCode, String)| assert_eq!(r, (StatusCode::OK, String::new()));

    ok(s.post(
        "/api/audit/conn",
        None,
        dev(json!({"action": "new", "ip": "100.64.0.2", "nonce": "n1"})),
    )
    .await);
    // A retried post with the same nonce is stored once.
    ok(s.post(
        "/api/audit/conn",
        None,
        dev(json!({"action": "new", "ip": "100.64.0.2", "nonce": "n1"})),
    )
    .await);
    ok(s.post(
        "/api/audit/conn",
        None,
        dev(json!({
            "peer": ["222222222", "laptop"], "type": 0, "primary_auth": 3, "two_factor": 1,
            "session_id": 5555, "nonce": "n2"
        })),
    )
    .await);
    // Note from the controlling side (src/ui_session_interface.rs send_note).
    ok(s.post(
        "/api/audit/conn",
        None,
        json!({"id": "123456789", "session_id": 5555, "note": "fixed printer"}),
    )
    .await);
    ok(s.post(
        "/api/audit/conn",
        None,
        dev(json!({"action": "close", "nonce": "n3"})),
    )
    .await);
    ok(s.post(
        "/api/audit/file",
        None,
        json!({
            "id": "123456789", "uuid": "x", "peer_id": "222222222", "conn_id": 7, "type": 1,
            "path": "C:\\tmp", "is_file": false, "info": "{\"num\":1}", "nonce": "f1"
        }),
    )
    .await);
    ok(s.post("/api/audit/alarm", None, json!({"id": "123456789", "uuid": "x", "typ": 0, "info": "{\"ip\":\"1.2.3.4\"}", "conn_id": 0, "nonce": "a1"})).await);
    let (status, _) = s.post("/api/audit/bogus", None, json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = s.send(M::GET, "/api/admin/audit/conn", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let token = s.login("admin", &s.admin_password, "987654321").await;
    let conns = s
        .json(
            M::GET,
            "/api/admin/audit/conn?current=1&pageSize=50",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(conns["total"], 1);
    let c = &conns["data"][0];
    assert_eq!(c["device_id"], "123456789");
    assert_eq!(c["ip"], "100.64.0.2");
    assert_eq!(c["peer_id"], "222222222");
    assert_eq!(c["peer_name"], "laptop");
    assert_eq!(c["conn_type"], 0);
    assert_eq!(c["primary_auth"], 3);
    assert_eq!(c["note"], "fixed printer");
    assert!(c["ended_at"].is_i64());

    // Note update by guid (flutter/lib/common/widgets/dialog.dart).
    let guid = c["guid"].as_str().unwrap();
    ok(s.send(
        M::PUT,
        "/api/audit",
        Some(&token),
        Some(json!({"guid": guid, "note": "edited"})),
    )
    .await);
    let conns = s
        .json(M::GET, "/api/admin/audit/conn", Some(&token), None)
        .await;
    assert_eq!(conns["data"][0]["note"], "edited");

    let files = s
        .json(M::GET, "/api/admin/audit/file", Some(&token), None)
        .await;
    assert_eq!(files["total"], 1);
    assert_eq!(files["data"][0]["path"], "C:\\tmp");
    let alarms = s
        .json(M::GET, "/api/admin/audit/alarm", Some(&token), None)
        .await;
    assert_eq!(alarms["total"], 1);
    assert_eq!(alarms["data"][0]["typ"], 0);
}

#[tokio::test]
async fn user_management_guards_last_admin() {
    let s = start().await;
    let token = s.login("admin", &s.admin_password, "1").await;
    let (status, _) = s
        .send(
            M::PUT,
            "/api/admin/users/admin",
            Some(&token),
            Some(json!({"is_admin": false})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = s
        .send(
            M::POST,
            "/api/admin/users",
            Some(&token),
            Some(json!({"name": "bob", "password": "short"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    s.json(
        M::POST,
        "/api/admin/users",
        Some(&token),
        Some(json!({"name": "bob", "password": "longenough"})),
    )
    .await;
    let tb = s.login("bob", "longenough", "2").await;
    s.json(
        M::PUT,
        "/api/admin/users/bob",
        Some(&token),
        Some(json!({"status": 0})),
    )
    .await;
    let (status, _) = s.post("/api/currentUser", Some(&tb), json!({})).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "disabling a user revokes their tokens"
    );
    let (status, _) = s
        .post(
            "/api/login",
            None,
            json!({"username": "bob", "password": "longenough"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    s.json(
        M::PUT,
        "/api/admin/users/admin",
        Some(&token),
        Some(json!({"password": "new-admin-pass"})),
    )
    .await;
    s.login("admin", "new-admin-pass", "1").await;
}

#[test]
fn config_resolves_relative_paths_against_config_dir() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "bind = \"100.64.0.1:21114\"\ndb_path = \"data/rd.sqlite3\"\n",
    )
    .unwrap();
    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.bind, "100.64.0.1:21114");
    let base = dir.path().canonicalize().unwrap();
    assert_eq!(cfg.db_path, base.join("data/rd.sqlite3"));
    assert_eq!(cfg.log_dir, base.join("logs"));
    std::fs::write(&path, "bnd = \"x\"\n").unwrap();
    assert!(Config::load(&path).is_err(), "unknown keys are rejected");
}
