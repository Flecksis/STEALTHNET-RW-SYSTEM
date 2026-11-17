use super::*;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const COMMIT: &str = "38ea03e426da7fd6ec823484a8ccb608a92afdbd";
const OLD: &str = "9636b626a1b72edef737125021a44ce286963c8a";

fn digest(n: u8) -> String {
    format!("sha256:{}", format!("{n:02x}").repeat(32))
}

fn config(server: &MockServer, arch: &str) -> UpdateConfig {
    UpdateConfig {
        build: BuildInfo {
            version: "0.1.0".into(),
            label: Some("0.1.59".into()),
            commit: Some(OLD.into()),
            built_at: parse_time("2026-11-17T12:00:00Z"),
            arch: arch.into(),
        },
        dir: None,
        registry: server.uri(),
        repository: "stealthnet-app/stealthnet-rw-system".into(),
        tag: "main".into(),
        checks_allowed: true,
    }
}

fn image_config(arch: &str, labels: Value) -> Value {
    serde_json::json!({
        "architecture": arch,
        "os": "linux",
        "created": "2026-11-17T13:20:00Z",
        "config": { "Labels": labels }
    })
}

async fn mount_image(server: &MockServer, manifest_path: &str, arch: &str, labels: Value) {
    let config_digest = digest(7);
    Mock::given(method("GET"))
        .and(path(manifest_path))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("docker-content-digest", digest(3).as_str())
                .set_body_json(serde_json::json!({
                    "schemaVersion": 2,
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "config": { "digest": config_digest, "size": 100 },
                    "layers": []
                })),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/v2/stealthnet-app/stealthnet-rw-system/blobs/{config_digest}"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(image_config(arch, labels)))
        .mount(server)
        .await;
}

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

#[tokio::test]
async fn сборка_читается_из_меток_образа() {
    let server = MockServer::start().await;
    mount_image(
        &server,
        "/v2/stealthnet-app/stealthnet-rw-system/manifests/main",
        "amd64",
        serde_json::json!({
            "org.opencontainers.image.revision": COMMIT.to_uppercase(),
            "org.opencontainers.image.created": "2026-11-17T13:05:00Z",
            "org.opencontainers.image.version": "0.1.60"
        }),
    )
    .await;
    let build = fetch_latest(&http(), &config(&server, "amd64"))
        .await
        .unwrap();
    assert_eq!(build.label.as_deref(), Some("0.1.60"));
    assert_eq!(build.commit.as_deref(), Some(COMMIT));
    assert_eq!(build.built_at, parse_time("2026-11-17T13:05:00Z").unwrap());
    assert_eq!(build.digest, Some(digest(3)));
    assert!(is_newer(&config(&server, "amd64").build, &build));
}

#[tokio::test]
async fn без_меток_берётся_дата_образа_и_обновления_нет() {
    let server = MockServer::start().await;
    mount_image(
        &server,
        "/v2/stealthnet-app/stealthnet-rw-system/manifests/main",
        "amd64",
        serde_json::json!({}),
    )
    .await;
    let build = fetch_latest(&http(), &config(&server, "amd64"))
        .await
        .unwrap();
    assert_eq!(build.commit, None);
    assert_eq!(build.built_at, parse_time("2026-11-17T13:20:00Z").unwrap());
    assert!(!is_newer(&config(&server, "amd64").build, &build));
}

#[tokio::test]
async fn вход_в_реестр_по_анонимному_токену() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/token"))
        .and(query_param("service", "ghcr.io"))
        .and(query_param(
            "scope",
            "repository:stealthnet-app/stealthnet-rw-system:pull",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "token": "anon" })),
        )
        .mount(&server)
        .await;
    let challenge = format!(
        "Bearer realm=\"{}/token\",service=\"ghcr.io\",scope=\"repository:stealthnet-app/stealthnet-rw-system:pull\"",
        server.uri()
    );
    Mock::given(method("GET"))
        .and(path(
            "/v2/stealthnet-app/stealthnet-rw-system/manifests/main",
        ))
        .and(header("authorization", "Bearer anon"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "manifests": [
                { "digest": digest(9), "platform": { "architecture": "unknown", "os": "unknown" } },
                { "digest": digest(4), "platform": { "architecture": "arm64", "os": "linux" } },
                { "digest": digest(5), "platform": { "architecture": "amd64", "os": "linux" } }
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/v2/stealthnet-app/stealthnet-rw-system/manifests/main",
        ))
        .respond_with(
            ResponseTemplate::new(401).insert_header("www-authenticate", challenge.as_str()),
        )
        .mount(&server)
        .await;
    mount_image(
        &server,
        &format!(
            "/v2/stealthnet-app/stealthnet-rw-system/manifests/{}",
            digest(5)
        ),
        "amd64",
        serde_json::json!({ "org.opencontainers.image.revision": COMMIT }),
    )
    .await;
    let build = fetch_latest(&http(), &config(&server, "amd64"))
        .await
        .unwrap();
    assert_eq!(build.commit.as_deref(), Some(COMMIT));
    assert_eq!(build.digest, Some(digest(5)));
}

#[tokio::test]
async fn сборки_под_процессор_сервера_нет() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/v2/stealthnet-app/stealthnet-rw-system/manifests/main",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "schemaVersion": 2,
            "manifests": [
                { "digest": digest(5), "platform": { "architecture": "amd64", "os": "linux" } }
            ]
        })))
        .mount(&server)
        .await;
    assert_eq!(
        fetch_latest(&http(), &config(&server, "arm64")).await,
        Err(CheckFailure::NoBuild)
    );
    let single = MockServer::start().await;
    mount_image(
        &single,
        "/v2/stealthnet-app/stealthnet-rw-system/manifests/main",
        "amd64",
        serde_json::json!({ "org.opencontainers.image.revision": COMMIT }),
    )
    .await;
    assert_eq!(
        fetch_latest(&http(), &config(&single, "arm64")).await,
        Err(CheckFailure::NoBuild)
    );
}

#[tokio::test]
async fn реестр_недоступен_или_отвечает_мусором() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/v2/stealthnet-app/stealthnet-rw-system/manifests/main",
        ))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    assert!(matches!(
        fetch_latest(&http(), &config(&server, "amd64")).await,
        Err(CheckFailure::Unreachable(_))
    ));
    let junk = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/v2/stealthnet-app/stealthnet-rw-system/manifests/main",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
        .mount(&junk)
        .await;
    assert!(matches!(
        fetch_latest(&http(), &config(&junk, "amd64")).await,
        Err(CheckFailure::Invalid(_))
    ));
    let huge = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/v2/stealthnet-app/stealthnet-rw-system/manifests/main",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(MANIFEST_LIMIT + 1)))
        .mount(&huge)
        .await;
    assert!(matches!(
        fetch_latest(&http(), &config(&huge, "amd64")).await,
        Err(CheckFailure::Invalid(_))
    ));
}

#[test]
fn новизна_сборки_по_коммиту_и_дате() {
    let current = BuildInfo {
        version: "0.1.0".into(),
        label: None,
        commit: Some(OLD.into()),
        built_at: parse_time("2026-11-17T12:00:00Z"),
        arch: "amd64".into(),
    };
    let build = |commit: Option<&str>, at: &str| PublishedBuild {
        label: None,
        commit: commit.map(str::to_string),
        built_at: parse_time(at).unwrap(),
        arch: "amd64".into(),
        digest: None,
    };
    assert!(is_newer(
        &current,
        &build(Some(COMMIT), "2026-11-17T13:00:00Z")
    ));
    assert!(!is_newer(
        &current,
        &build(Some(COMMIT), "2026-11-17T11:00:00Z")
    ));
    assert!(!is_newer(
        &current,
        &build(Some(OLD), "2026-11-18T13:00:00Z")
    ));
    assert!(!is_newer(&current, &build(None, "2026-11-18T13:00:00Z")));
    let unknown = BuildInfo {
        built_at: None,
        ..current.clone()
    };
    assert!(!is_newer(
        &unknown,
        &build(Some(COMMIT), "2026-11-18T13:00:00Z")
    ));
}

#[test]
fn заголовок_входа_разбирается_с_кавычками_и_запятыми() {
    let params = parse_challenge(
        "Bearer realm=\"https://ghcr.io/token\",service=\"ghcr.io\",scope=\"repository:a/b:pull,push\"",
    );
    assert_eq!(
        params.get("realm").map(String::as_str),
        Some("https://ghcr.io/token")
    );
    assert_eq!(params.get("service").map(String::as_str), Some("ghcr.io"));
    assert_eq!(
        params.get("scope").map(String::as_str),
        Some("repository:a/b:pull,push")
    );
    assert!(parse_challenge("Basic realm=\"x\"").is_empty());
}

#[test]
fn сведения_о_сборке_из_окружения() {
    let build = BuildInfo::new(
        Some(" 38EA03E426DA7FD6EC823484A8CCB608A92AFDBD "),
        Some("2026-11-17T13:05:00Z"),
    );
    assert_eq!(build.commit.as_deref(), Some(COMMIT));
    assert!(build.built_at.is_some());
    let bad = BuildInfo::new(Some("local"), Some("вчера"));
    assert_eq!(bad.commit, None);
    assert_eq!(bad.built_at, None);
    assert_eq!(
        build.with_label(Some(" 0.1.60 ")).label.as_deref(),
        Some("0.1.60")
    );
    for wrong in ["0.1", "0.1.x", "v0.1.60", "0.1.60.1", "0..60", "<b>"] {
        assert_eq!(clean_label(wrong), None, "{wrong}");
    }
}

#[test]
fn файл_хода_очищается_от_лишнего() {
    let run: RunFile = serde_json::from_value(serde_json::json!({
        "id": "0b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e10",
        "origin": "panel",
        "state": "running",
        "from": OLD,
        "to": "не коммит",
        "error": "<script>",
        "steps": [
            { "key": "check", "state": "done", "seconds": 1 },
            { "key": "rm -rf", "state": "done" },
            { "key": "download", "state": "взлом" }
        ]
    }))
    .unwrap();
    let run = clean_run(run).unwrap();
    assert_eq!(run.from.as_deref(), Some(OLD));
    assert_eq!(run.to, None);
    assert_eq!(run.error, None);
    assert_eq!(run.steps.len(), 1);
    let broken: RunFile =
        serde_json::from_value(serde_json::json!({ "id": "a b", "state": "done" })).unwrap();
    assert!(clean_run(broken).is_none());
}
