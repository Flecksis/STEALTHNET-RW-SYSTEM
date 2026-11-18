use super::*;
use crate::registry::ModuleRegistry;
use sn_store::ModuleStateRow;
use std::sync::Arc;

const KEY: &str = include_str!("testdata/apple_key.p8");
const GOOGLE_ID: &str = "1234-abcdef.apps.googleusercontent.com";

fn config(value: serde_json::Value) -> ModuleConfig {
    ModuleConfig::from_value(&value)
}

fn full() -> serde_json::Value {
    serde_json::json!({
        "google_client_id": GOOGLE_ID,
        "google_client_secret": "GOCSPX-secret",
        "apple_services_id": "com.example.signin",
        "apple_team_id": "abcde12345",
        "apple_key_id": "KEY1234567",
        "apple_private_key": KEY,
    })
}

#[test]
fn настройки_google_и_apple_разбираются() {
    let settings = OauthSettings::parse(&config(full())).unwrap();
    let google = settings.google.clone().unwrap();
    assert_eq!(google.client_id, GOOGLE_ID);
    assert_eq!(google.client_secret, "GOCSPX-secret");
    let apple = settings.apple.clone().unwrap();
    assert_eq!(apple.services_id, "com.example.signin");
    assert_eq!(apple.team_id, "ABCDE12345");
    assert_eq!(apple.key_id, "KEY1234567");
    assert!(apple.private_key.starts_with(PEM_BEGIN));
    assert!(settings.has(Provider::Google) && settings.has(Provider::Apple));
    let only_google = OauthSettings::parse(&config(serde_json::json!({
        "google_client_id": GOOGLE_ID,
        "google_client_secret": "GOCSPX-secret",
    })))
    .unwrap();
    assert!(only_google.has(Provider::Google) && !only_google.has(Provider::Apple));
    assert_eq!(
        OauthSettings::parse(&ModuleConfig::default()).unwrap(),
        OauthSettings::default()
    );
}

#[test]
fn ключ_p8_принимается_с_испорченными_переносами() {
    let body: String = KEY
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");
    let mangled = [
        KEY.replace('\n', "\\n"),
        KEY.replace('\n', " "),
        body.clone(),
        format!("  {body}  "),
    ];
    let expected = normalize_key(KEY).unwrap();
    for raw in mangled {
        let normalized = normalize_key(&raw).unwrap();
        assert_eq!(normalized, expected);
        assert!(check_key(&normalized).is_ok());
    }
    assert_eq!(
        normalize_key("-----BEGIN EC PRIVATE KEY-----\nAAAA\n-----END EC PRIVATE KEY-----"),
        None
    );
    assert_eq!(normalize_key("ключ"), None);
}

#[test]
fn неполные_и_чужие_настройки_отклоняются_понятно() {
    let error = |patch: serde_json::Value| {
        let mut value = full();
        for (k, v) in patch.as_object().unwrap() {
            value[k] = v.clone();
        }
        OauthSettings::parse(&config(value)).unwrap_err()
    };
    assert!(error(serde_json::json!({ "google_client_secret": "" })).contains("Client secret"));
    assert!(
        error(serde_json::json!({ "google_client_id": "my-project-123" }))
            .contains("googleusercontent")
    );
    assert!(error(serde_json::json!({ "apple_key_id": "" })).contains("Key ID"));
    assert!(error(serde_json::json!({ "apple_team_id": "TEAM" })).contains("Team ID"));
    assert!(error(serde_json::json!({ "apple_services_id": "signin" })).contains("com.example"));
    assert!(error(serde_json::json!({ "apple_private_key": "-----BEGIN PRIVATE KEY-----\nMIIBVQIBADANBgkqhkiG9w0BAQEFAASC\n-----END PRIVATE KEY-----" })).contains(".p8"));
    assert!(
        OauthModule
            .validate_config(&config(
                serde_json::json!({ "apple_team_id": "ABCDE12345" })
            ))
            .is_err()
    );
}

#[test]
fn модуль_без_ключей_предупреждает_в_каталоге() {
    let registry = ModuleRegistry::new(vec![Arc::new(OauthModule)]).unwrap();
    let row = |enabled: bool, config: serde_json::Value| ModuleStateRow {
        module: "oauth".into(),
        enabled,
        config,
        updated_at: chrono::Utc::now(),
    };
    let warning = |registry: &ModuleRegistry| {
        registry
            .views()
            .into_iter()
            .find(|v| v.info.id == ModuleId::Oauth)
            .and_then(|v| v.warning)
    };
    assert_eq!(warning(&registry), None);
    registry.apply_rows(&[row(true, serde_json::json!({}))]);
    assert_eq!(warning(&registry), Some(NOT_CONFIGURED));
    registry.apply_rows(&[row(true, full())]);
    assert_eq!(warning(&registry), None);
    let view = registry
        .views()
        .into_iter()
        .find(|v| v.info.id == ModuleId::Oauth)
        .unwrap();
    assert!(view.has_settings && view.active);
}

#[test]
fn адрес_возврата_у_каждого_провайдера_свой() {
    assert_eq!(
        Provider::Google.callback_path(),
        "/api/client/auth/oauth/google/callback"
    );
    assert_eq!(
        Provider::Apple.callback_path(),
        "/api/client/auth/oauth/apple/callback"
    );
    assert_eq!(Provider::parse("apple"), Some(Provider::Apple));
    assert_eq!(Provider::parse("github"), None);
    assert_eq!(Provider::of(IdentityKind::Google), Some(Provider::Google));
    assert_eq!(Provider::of(IdentityKind::Email), None);
}
