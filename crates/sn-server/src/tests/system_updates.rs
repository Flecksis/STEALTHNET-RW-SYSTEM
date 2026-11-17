use super::*;
use sn_services::updates::{BuildInfo, UpdateConfig};
use std::path::{Path, PathBuf};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const OLD: &str = "9636b626a1b72edef737125021a44ce286963c8a";
const NEW: &str = "38ea03e426da7fd6ec823484a8ccb608a92afdbd";

struct Folder(PathBuf);

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn folder(helper: Option<Value>) -> Folder {
    let dir = std::env::temp_dir().join(format!("sn-updater-{}", Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("request")).unwrap();
    std::fs::create_dir_all(dir.join("status")).unwrap();
    if let Some(helper) = helper {
        std::fs::write(dir.join("status/helper.json"), helper.to_string()).unwrap();
    }
    Folder(dir)
}

async fn registry(commit: &str, created: &str) -> MockServer {
    let server = MockServer::start().await;
    let config = format!("sha256:{}", "ab".repeat(32));
    Mock::given(method("GET"))
        .and(path(
            "/v2/stealthnet-app/stealthnet-rw-system/manifests/main",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "schemaVersion": 2,
            "config": { "digest": config },
            "layers": []
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/v2/stealthnet-app/stealthnet-rw-system/blobs/{config}"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "architecture": sn_services::updates::host_arch(),
            "os": "linux",
            "created": created,
            "config": { "Labels": {
                "org.opencontainers.image.revision": commit,
                "org.opencontainers.image.created": created
            } }
        })))
        .mount(&server)
        .await;
    server
}

fn configure(http: &Http, dir: &Path, server: &MockServer) {
    let mut config = UpdateConfig::new(BuildInfo::new(Some(OLD), Some("2026-11-16T10:00:00Z")));
    config.dir = Some(dir.to_path_buf());
    config.registry = server.uri();
    http.services().set_update_config(Some(config));
}

async fn audit_count(http: &Http, action: &str) -> usize {
    let mut conn = http.services().store().conn().await.unwrap();
    conn.audit()
        .recent_by_action(action, 50)
        .await
        .unwrap()
        .len()
}

fn request_file(dir: &Path) -> PathBuf {
    dir.join("request/request.json")
}

#[tokio::test]
async fn обновление_из_админки_заявка_пароль_отмена_и_ход() {
    let Some(http) = http().await else { return };
    let owner = http.owner().await;
    let admin = http.admin("admin@example.com", AdminRole::Admin).await;
    let support = http.admin("support@example.com", AdminRole::Support).await;

    let (status, body) = http.get("/api/admin/system", &owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["helper"]["installed"], false);
    assert_eq!(body["can_update"], false);
    assert_eq!(body["current"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["latest"], Value::Null);
    let (status, _) = http.get("/api/admin/system", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let dir = folder(Some(json!({ "schema": 1, "panel_updates": true })));
    let server = registry(NEW, "2026-11-17T13:05:00Z").await;
    configure(&http, &dir.0, &server);

    let (status, _) = http
        .write(Method::POST, "/api/admin/system/check", &admin, json!({}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = http
        .write(Method::POST, "/api/admin/system/check", &owner, json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["check"]["state"], "ok");
    assert_eq!(body["latest"]["commit"], NEW);
    assert_eq!(body["current"]["commit"], OLD);
    assert_eq!(body["helper"]["installed"], true);
    assert_eq!(body["can_update"], true);
    let (status, body) = http
        .write(Method::POST, "/api/admin/system/check", &owner, json!({}))
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "system.check_rate_limited");

    let (status, body) = http.get("/api/admin/system", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["latest"], Value::Null);
    assert_eq!(body["can_update"], false);
    let (_, summary) = http.get("/api/admin/system/summary", &support).await;
    assert_eq!(summary["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(summary["commit"], OLD);
    assert_eq!(summary["built_at"], "2026-11-16T10:00:00Z");
    assert_eq!(summary["update"], Value::Null);
    let (_, summary) = http.get("/api/admin/system/summary", &owner).await;
    assert_eq!(summary["update"]["commit"], NEW);

    let update = |password: &str, target: &str| json!({ "target": target, "password": password });
    let (status, _) = http
        .write(
            Method::POST,
            "/api/admin/system/update",
            &admin,
            update(PASSWORD, NEW),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = http
        .write(
            Method::POST,
            "/api/admin/system/update",
            &owner,
            update("не тот", NEW),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.wrong_password");
    assert_eq!(audit_count(&http, "system.update_denied").await, 2);
    let (status, body) = http
        .write(
            Method::POST,
            "/api/admin/system/update",
            &owner,
            update(PASSWORD, OLD),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "system.update_stale");
    assert!(!request_file(&dir.0).exists());

    let (status, body) = http
        .write(
            Method::POST,
            "/api/admin/system/update",
            &owner,
            update(PASSWORD, NEW),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    let line = std::fs::read_to_string(request_file(&dir.0)).unwrap();
    assert_eq!(
        line,
        format!(
            "{{\"id\":\"{id}\",\"action\":\"update\",\"target\":\"{NEW}\",\"requested_at\":\"2026-11-16T14:52:00Z\",\"admin_id\":1}}\n"
        )
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(request_file(&dir.0))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert_eq!(audit_count(&http, "system.update_requested").await, 1);
    let (_, body) = http.get("/api/admin/system", &owner).await;
    assert_eq!(body["update"]["state"], "requested");
    assert_eq!(body["update"]["to"], NEW);
    assert_eq!(body["can_update"], false);
    let (status, body) = http
        .write(
            Method::POST,
            "/api/admin/system/update",
            &owner,
            update(PASSWORD, NEW),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "system.update_in_progress");

    let (status, _, _) = http
        .send(
            Method::DELETE,
            "/api/admin/system/update",
            Some(&owner),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!request_file(&dir.0).exists());
    let (_, body) = http.get("/api/admin/system", &owner).await;
    assert_eq!(body["update"], Value::Null);
    assert_eq!(body["can_update"], true);
    let (status, _, body) = http
        .send(
            Method::DELETE,
            "/api/admin/system/update",
            Some(&owner),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "system.update_started");

    let (status, body) = http
        .write(
            Method::POST,
            "/api/admin/system/update",
            &owner,
            update(PASSWORD, NEW),
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    http.clock.set(start() + Duration::seconds(31));
    let (status, body) = http
        .write(
            Method::POST,
            "/api/admin/system/update",
            &owner,
            update(PASSWORD, NEW),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    std::fs::remove_file(request_file(&dir.0)).unwrap();
    let running = json!({
        "schema": 1, "id": id, "origin": "panel", "admin_id": 1,
        "from": OLD, "to": NEW, "state": "running",
        "steps": [
            { "key": "check", "state": "done", "seconds": 1 },
            { "key": "download", "state": "running" },
            { "key": "backup", "state": "pending" },
            { "key": "migrate", "state": "pending" },
            { "key": "start", "state": "pending" },
            { "key": "verify", "state": "pending" }
        ],
        "started_at": "2026-11-16T14:52:33Z", "finished_at": null, "error": null
    });
    std::fs::write(dir.0.join("status/update.json"), running.to_string()).unwrap();
    let (_, body) = http.get("/api/admin/system", &owner).await;
    assert_eq!(body["update"]["state"], "running");
    assert_eq!(body["update"]["by"], "owner@example.com");
    assert_eq!(body["update"]["steps"][1]["state"], "running");
    assert_eq!(body["update"]["steps"].as_array().unwrap().len(), 6);

    let mut done = running.clone();
    done["state"] = json!("done");
    done["finished_at"] = json!("2026-11-16T14:53:21Z");
    done["downtime"] = json!(31);
    done["backup"] = json!("2026-11-16_1452_9636b62");
    std::fs::write(dir.0.join("status/update.json"), done.to_string()).unwrap();
    let mut line = done.clone();
    line["seconds"] = json!(48);
    let history = format!(
        "{}\n{}\n",
        json!({ "id": "earlier", "origin": "command", "state": "rolled_back", "error": "migrate_failed", "from": OLD, "to": NEW, "started_at": "2026-11-15T10:00:00Z" }),
        line
    );
    std::fs::write(dir.0.join("status/history.jsonl"), history).unwrap();
    let (_, body) = http.get("/api/admin/system", &owner).await;
    assert_eq!(body["update"]["state"], "done");
    assert_eq!(body["update"]["downtime"], 31);
    assert_eq!(body["history"][0]["result"], "done");
    assert_eq!(body["history"][0]["by"], "owner@example.com");
    assert_eq!(body["history"][0]["seconds"], 48);
    assert_eq!(body["history"][1]["result"], "rolled_back");
    assert_eq!(body["history"][1]["by"], Value::Null);
    http.get("/api/admin/system", &owner).await;
    assert_eq!(audit_count(&http, "system.update_finished").await, 1);
    let (_, body) = http.get("/api/admin/system", &support).await;
    assert_eq!(body["error"]["code"], "auth.forbidden");

    http.clock.set(start() + Duration::seconds(70));
    let (status, body) = http
        .write(
            Method::POST,
            "/api/admin/system/update",
            &owner,
            update(PASSWORD, NEW),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    http.clock.set(start() + Duration::seconds(102));
    let (_, body) = http.get("/api/admin/system", &owner).await;
    assert_eq!(body["update"]["state"], "failed", "{body}");
    assert_eq!(body["update"]["error"], "helper_silent");
    assert!(!request_file(&dir.0).exists());
    assert_eq!(audit_count(&http, "system.update_canceled").await, 2);

    let (status, body) = http
        .write(
            Method::PUT,
            "/api/admin/system/settings",
            &owner,
            json!({ "check_enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["check"]["enabled"], false);
    let (status, _) = http
        .write(
            Method::PUT,
            "/api/admin/system/settings",
            &admin,
            json!({ "check_enabled": true }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    http.db.cleanup().await;
}

#[tokio::test]
async fn обновление_без_помощника_с_отказавшей_версией_и_кодом_входа() {
    let Some(http) = http().await else { return };
    let owner = http.owner().await;
    let dir = folder(Some(json!({ "schema": 1, "panel_updates": false })));
    let server = registry(NEW, "2026-11-17T13:05:00Z").await;
    configure(&http, &dir.0, &server);
    let (_, body) = http
        .write(Method::POST, "/api/admin/system/check", &owner, json!({}))
        .await;
    assert_eq!(body["latest"]["commit"], NEW);
    assert_eq!(body["helper"]["panel_updates"], false);
    assert_eq!(body["can_update"], false);
    let request = json!({ "target": NEW, "password": PASSWORD });
    let (status, body) = http
        .write(
            Method::POST,
            "/api/admin/system/update",
            &owner,
            request.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "system.helper_missing");

    std::fs::write(
        dir.0.join("status/helper.json"),
        json!({ "schema": 1, "panel_updates": true, "blocked": NEW }).to_string(),
    )
    .unwrap();
    let (_, body) = http.get("/api/admin/system", &owner).await;
    assert_eq!(body["helper"]["blocked"], NEW);
    assert_eq!(body["can_update"], false);
    let (status, body) = http
        .write(
            Method::POST,
            "/api/admin/system/update",
            &owner,
            request.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "system.update_stale");

    std::fs::write(
        dir.0.join("status/helper.json"),
        json!({ "schema": 1, "panel_updates": true }).to_string(),
    )
    .unwrap();
    let (_, setup) = http
        .write(Method::POST, "/api/admin/auth/2fa/setup", &owner, json!({}))
        .await;
    let secret = setup["secret"].as_str().unwrap().to_string();
    let (status, body) = http
        .write(
            Method::POST,
            "/api/admin/auth/2fa/enable",
            &owner,
            json!({ "secret": secret, "code": totp_code(&secret, start()), "password": PASSWORD }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = http.get("/api/admin/system", &owner).await;
    assert_eq!(body["totp_required"], true);
    let (status, body) = http
        .write(
            Method::POST,
            "/api/admin/system/update",
            &owner,
            request.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.totp_required");
    http.clock.set(start() + Duration::seconds(30));
    let (status, body) = http
        .write(
            Method::POST,
            "/api/admin/system/update",
            &owner,
            json!({ "target": NEW, "password": PASSWORD, "code": totp_code(&secret, start() + Duration::seconds(30)) }),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(request_file(&dir.0).exists());

    let mut old = UpdateConfig::new(BuildInfo::new(Some(NEW), Some("2026-11-18T10:00:00Z")));
    old.dir = Some(dir.0.clone());
    old.registry = server.uri();
    http.services().set_update_config(Some(old));
    let (_, body) = http.get("/api/admin/system", &owner).await;
    assert_eq!(body["latest"], Value::Null);
    http.db.cleanup().await;
}
