use super::*;
use crate::security::{CspMode, REPORT_PATH};

async fn fetch_with(
    state: &AppState,
    method: Method,
    path: &str,
    headers: &[(&str, &str)],
    body: Body,
) -> (StatusCode, HeaderMap) {
    let mut request = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router(state.clone())
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    (response.status(), response.headers().clone())
}

fn csp(headers: &HeaderMap) -> &str {
    headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap()
}

fn report_only(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::CONTENT_SECURITY_POLICY_REPORT_ONLY)
        .map(|value| value.to_str().unwrap())
}

fn with_config(state: &mut AppState, dir: &std::path::Path, csp: CspMode, hsts_max_age: u64) {
    state.config = Arc::new(ServerConfig {
        web_dir: Some(dir.to_path_buf()),
        csp,
        hsts_max_age,
        ..ServerConfig::default()
    });
}

#[tokio::test]
async fn у_страниц_своя_политика_csp_админка_строго_остальное_в_отчётах() {
    let (state, dir) = web_state().await;
    let (status, admin) =
        fetch_with(&state, Method::GET, "/admin/clients", &[], Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    let policy = csp(&admin);
    assert!(
        policy.starts_with("default-src 'self'; script-src 'self';"),
        "{policy}"
    );
    assert!(
        policy.contains("style-src 'self' 'unsafe-inline'"),
        "{policy}"
    );
    assert!(policy.contains("frame-src 'self' https:;"), "{policy}");
    assert!(policy.contains("frame-ancestors 'none'"), "{policy}");
    assert!(
        policy.ends_with("report-uri /api/public/csp-report"),
        "{policy}"
    );
    assert_eq!(report_only(&admin), None);
    let (_, preview) = fetch_with(
        &state,
        Method::GET,
        "/admin/landing-preview",
        &[],
        Body::empty(),
    )
    .await;
    assert!(csp(&preview).contains("frame-ancestors 'self'"));
    for path in ["/", "/cabinet", "/cabinet/pay", "/pay/42", "/open-app"] {
        let (status, site) = fetch_with(&state, Method::GET, path, &[], Body::empty()).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(
            csp(&site),
            "object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
            "{path}"
        );
        let report = report_only(&site).unwrap();
        assert!(
            report.contains("script-src 'self'; style-src 'self';"),
            "{path}: {report}"
        );
        assert!(report.contains("frame-src 'none'"), "{path}: {report}");
        assert!(
            report.ends_with("report-uri /api/public/csp-report"),
            "{path}: {report}"
        );
    }
    let (_, mini) = fetch_with(&state, Method::GET, "/app/plans", &[], Body::empty()).await;
    assert_eq!(csp(&mini), "object-src 'none'; base-uri 'none'");
    let report = report_only(&mini).unwrap();
    assert!(
        report.contains("script-src 'self' https://telegram.org;"),
        "{report}"
    );
    assert!(!report.contains("frame-ancestors"), "{report}");
    assert!(!mini.contains_key(header::X_FRAME_OPTIONS));
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn политику_можно_включить_везде_или_оставить_только_отчёты() {
    let (mut state, dir) = web_state().await;
    with_config(&mut state, &dir, CspMode::All, 31_536_000);
    let (_, site) = fetch_with(&state, Method::GET, "/cabinet", &[], Body::empty()).await;
    assert!(csp(&site).starts_with("default-src 'self'; script-src 'self';"));
    assert_eq!(report_only(&site), None);
    let (_, jump) = fetch_with(&state, Method::GET, "/open-app", &[], Body::empty()).await;
    assert_eq!(csp(&jump), csp(&site));
    assert_eq!(report_only(&jump), None);
    let (_, mini) = fetch_with(&state, Method::GET, "/app", &[], Body::empty()).await;
    assert!(csp(&mini).contains("https://telegram.org"));
    with_config(&mut state, &dir, CspMode::Report, 31_536_000);
    let (_, admin) = fetch_with(&state, Method::GET, "/admin", &[], Body::empty()).await;
    assert_eq!(
        csp(&admin),
        "object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
    );
    assert!(report_only(&admin).unwrap().contains("'unsafe-inline'"));
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn модули_на_этом_же_сервере_открываются_во_фрейме_админки_только_с_переменной() {
    let (state, dir) = web_state().await;
    let store = Store::connect_lazy(
        "postgres://sn:sn@127.0.0.1:1/sn_test",
        StoreOptions {
            acquire_timeout: std::time::Duration::from_millis(300),
            ..StoreOptions::default()
        },
    )
    .unwrap();
    let services = Services::new(ServiceDeps {
        store,
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: None,
        clock: Arc::new(SystemClock),
        env: EnvFallbacks {
            ext_modules_loopback: true,
            ..EnvFallbacks::default()
        },
    });
    let (_, before) = fetch_with(&state, Method::GET, "/admin/m/ext/7", &[], Body::empty()).await;
    assert!(!csp(&before).contains("localhost"));
    let loopback = AppState {
        config: state.config.clone(),
        services,
    };
    let (_, after) = fetch_with(&loopback, Method::GET, "/admin/m/ext/7", &[], Body::empty()).await;
    assert!(csp(&after).contains(
        "frame-src 'self' https: http://localhost:* http://127.0.0.1:* http://*.localhost:*;"
    ));
    let (_, site) = fetch_with(&loopback, Method::GET, "/", &[], Body::empty()).await;
    assert!(!report_only(&site).unwrap().contains("localhost"));
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn hsts_ставится_только_на_ответы_по_https() {
    let (mut state, dir) = web_state().await;
    let (_, plain) = fetch_with(&state, Method::GET, "/", &[], Body::empty()).await;
    assert!(!plain.contains_key(header::STRICT_TRANSPORT_SECURITY));
    let (_, http) = fetch_with(
        &state,
        Method::GET,
        "/",
        &[("x-forwarded-proto", "http")],
        Body::empty(),
    )
    .await;
    assert!(!http.contains_key(header::STRICT_TRANSPORT_SECURITY));
    for path in ["/", "/admin", "/app", "/api/nothing-here"] {
        let (_, secure) = fetch_with(
            &state,
            Method::GET,
            path,
            &[("x-forwarded-proto", "https")],
            Body::empty(),
        )
        .await;
        assert_eq!(
            secure[header::STRICT_TRANSPORT_SECURITY],
            "max-age=31536000",
            "{path}"
        );
    }
    let (_, forwarded) = fetch_with(
        &state,
        Method::GET,
        "/cabinet",
        &[("forwarded", "for=203.0.113.7;proto=https")],
        Body::empty(),
    )
    .await;
    assert_eq!(
        forwarded[header::STRICT_TRANSPORT_SECURITY],
        "max-age=31536000"
    );
    with_config(&mut state, &dir, CspMode::Admin, 0);
    let (_, off) = fetch_with(
        &state,
        Method::GET,
        "/",
        &[("x-forwarded-proto", "https")],
        Body::empty(),
    )
    .await;
    assert_eq!(off[header::STRICT_TRANSPORT_SECURITY], "max-age=0");
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn отчёты_о_нарушениях_принимаются_молча_и_с_пределом_размера() {
    let state = offline_state();
    let report = json!({
        "csp-report": {
            "document-uri": "https://vpn.example/cabinet?token=SECRET",
            "effective-directive": "script-src-elem",
            "blocked-uri": "https://static.example-cdn.com/beacon.min.js",
            "disposition": "report"
        }
    });
    let (status, _) = fetch_with(
        &state,
        Method::POST,
        REPORT_PATH,
        &[("content-type", "application/csp-report")],
        Body::from(report.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = fetch_with(
        &state,
        Method::POST,
        REPORT_PATH,
        &[("content-type", "text/plain")],
        Body::from("не отчёт"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = fetch_with(
        &state,
        Method::POST,
        REPORT_PATH,
        &[("content-type", "application/csp-report")],
        Body::from(vec![b' '; 20 * 1024]),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, _) = fetch_with(&state, Method::GET, REPORT_PATH, &[], Body::empty()).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}
