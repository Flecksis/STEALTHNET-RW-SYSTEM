use crate::models::{HwidDevice, Node, RemnaUser, TorrentNodeRef, TorrentReportDetails};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sn_domain::ids::RemnaUserId;
use uuid::Uuid;

pub const SIGNATURE_HEADER: &str = "x-remnawave-signature";
pub const TIMESTAMP_HEADER: &str = "x-remnawave-timestamp";
pub const MIN_SECRET_LEN: usize = 32;
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

pub mod scopes {
    pub const USER: &str = "user";
    pub const USER_HWID_DEVICES: &str = "user_hwid_devices";
    pub const NODE: &str = "node";
    pub const SERVICE: &str = "service";
    pub const ERRORS: &str = "errors";
    pub const CRM: &str = "crm";
    pub const TORRENT_BLOCKER: &str = "torrent_blocker";

    pub const ALL: [&str; 7] = [
        USER,
        USER_HWID_DEVICES,
        NODE,
        SERVICE,
        ERRORS,
        CRM,
        TORRENT_BLOCKER,
    ];
}

pub mod events {
    pub const USER_CREATED: &str = "user.created";
    pub const USER_MODIFIED: &str = "user.modified";
    pub const USER_DELETED: &str = "user.deleted";
    pub const USER_REVOKED: &str = "user.revoked";
    pub const USER_DISABLED: &str = "user.disabled";
    pub const USER_ENABLED: &str = "user.enabled";
    pub const USER_LIMITED: &str = "user.limited";
    pub const USER_EXPIRED: &str = "user.expired";
    pub const USER_TRAFFIC_RESET: &str = "user.traffic_reset";
    pub const USER_FIRST_CONNECTED: &str = "user.first_connected";
    pub const USER_BANDWIDTH_THRESHOLD: &str = "user.bandwidth_usage_threshold_reached";
    pub const USER_NOT_CONNECTED: &str = "user.not_connected";
    pub const USER_EXPIRATION: &str = "user.expiration";

    pub const HWID_ADDED: &str = "user_hwid_devices.added";
    pub const HWID_DELETED: &str = "user_hwid_devices.deleted";

    pub const NODE_CREATED: &str = "node.created";
    pub const NODE_MODIFIED: &str = "node.modified";
    pub const NODE_DISABLED: &str = "node.disabled";
    pub const NODE_ENABLED: &str = "node.enabled";
    pub const NODE_DELETED: &str = "node.deleted";
    pub const NODE_CONNECTION_LOST: &str = "node.connection_lost";
    pub const NODE_CONNECTION_RESTORED: &str = "node.connection_restored";
    pub const NODE_TRAFFIC_NOTIFY: &str = "node.traffic_notify";

    pub const SERVICE_PANEL_STARTED: &str = "service.panel_started";
    pub const SERVICE_LOGIN_FAILED: &str = "service.login_attempt_failed";
    pub const SERVICE_LOGIN_SUCCESS: &str = "service.login_attempt_success";
    pub const SERVICE_SUBPAGE_CONFIG_CHANGED: &str = "service.subpage_config_changed";
    pub const SERVICE_API_TOKEN_CREATED: &str = "service.api_token_created";
    pub const SERVICE_API_TOKEN_DELETED: &str = "service.api_token_deleted";

    pub const ERRORS_BANDWIDTH_MAX_NOTIFICATIONS: &str =
        "errors.bandwidth_usage_threshold_reached_max_notifications";

    pub const CRM_PAYMENT_IN_7_DAYS: &str = "crm.infra_billing_node_payment_in_7_days";
    pub const CRM_PAYMENT_IN_48_HOURS: &str = "crm.infra_billing_node_payment_in_48hrs";
    pub const CRM_PAYMENT_IN_24_HOURS: &str = "crm.infra_billing_node_payment_in_24hrs";
    pub const CRM_PAYMENT_DUE_TODAY: &str = "crm.infra_billing_node_payment_due_today";
    pub const CRM_PAYMENT_OVERDUE_24_HOURS: &str = "crm.infra_billing_node_payment_overdue_24hrs";
    pub const CRM_PAYMENT_OVERDUE_48_HOURS: &str = "crm.infra_billing_node_payment_overdue_48hrs";
    pub const CRM_PAYMENT_OVERDUE_7_DAYS: &str = "crm.infra_billing_node_payment_overdue_7_days";

    pub const TORRENT_BLOCKER_REPORT: &str = "torrent_blocker.report";

    pub const ALL: [&str; 38] = [
        USER_CREATED,
        USER_MODIFIED,
        USER_DELETED,
        USER_REVOKED,
        USER_DISABLED,
        USER_ENABLED,
        USER_LIMITED,
        USER_EXPIRED,
        USER_TRAFFIC_RESET,
        USER_FIRST_CONNECTED,
        USER_BANDWIDTH_THRESHOLD,
        USER_NOT_CONNECTED,
        USER_EXPIRATION,
        HWID_ADDED,
        HWID_DELETED,
        NODE_CREATED,
        NODE_MODIFIED,
        NODE_DISABLED,
        NODE_ENABLED,
        NODE_DELETED,
        NODE_CONNECTION_LOST,
        NODE_CONNECTION_RESTORED,
        NODE_TRAFFIC_NOTIFY,
        SERVICE_PANEL_STARTED,
        SERVICE_LOGIN_FAILED,
        SERVICE_LOGIN_SUCCESS,
        SERVICE_SUBPAGE_CONFIG_CHANGED,
        SERVICE_API_TOKEN_CREATED,
        SERVICE_API_TOKEN_DELETED,
        ERRORS_BANDWIDTH_MAX_NOTIFICATIONS,
        CRM_PAYMENT_IN_7_DAYS,
        CRM_PAYMENT_IN_48_HOURS,
        CRM_PAYMENT_IN_24_HOURS,
        CRM_PAYMENT_DUE_TODAY,
        CRM_PAYMENT_OVERDUE_24_HOURS,
        CRM_PAYMENT_OVERDUE_48_HOURS,
        CRM_PAYMENT_OVERDUE_7_DAYS,
        TORRENT_BLOCKER_REPORT,
    ];

    pub fn is_known(event: &str) -> bool {
        ALL.contains(&event)
    }

    pub fn scope_of(event: &str) -> Option<&'static str> {
        let prefix = event.split_once('.')?.0;
        super::scopes::ALL
            .into_iter()
            .find(|scope| *scope == prefix)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebhookError {
    #[error("подпись вебхука Remnawave не совпала")]
    BadSignature,
    #[error("тело вебхука Remnawave не разобрано: {0}")]
    Malformed(String),
    #[error("секрет вебхуков Remnawave не подходит: {0}")]
    BadSecret(&'static str),
}

pub fn validate_secret(secret: &str) -> Result<(), WebhookError> {
    if secret.len() < MIN_SECRET_LEN {
        return Err(WebhookError::BadSecret("нужно не меньше 32 символов"));
    }
    if !secret.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(WebhookError::BadSecret(
            "допустимы только латинские буквы и цифры",
        ));
    }
    Ok(())
}

fn mac(secret: &str) -> Option<Hmac<Sha256>> {
    Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()
}

pub fn sign(secret: &str, body: &[u8]) -> String {
    match mac(secret) {
        Some(mut m) => {
            m.update(body);
            hex::encode(m.finalize().into_bytes())
        }
        None => String::new(),
    }
}

pub fn verify_signature(secret: &str, body: &[u8], signature: Option<&str>) -> bool {
    if secret.is_empty() {
        return false;
    }
    let Some(expected) = signature.and_then(|s| hex::decode(s.trim()).ok()) else {
        return false;
    };
    let Some(mut m) = mac(secret) else {
        return false;
    };
    m.update(body);
    m.verify_slice(&expected).is_ok()
}

pub fn body_digest(body: &[u8]) -> [u8; 32] {
    Sha256::digest(body).into()
}

#[derive(Debug, Clone, Deserialize)]
struct Envelope {
    scope: String,
    event: String,
    timestamp: DateTime<Utc>,
    #[serde(default, deserialize_with = "crate::models::nullable")]
    data: serde_json::Value,
    #[serde(default, deserialize_with = "crate::models::nullable")]
    meta: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookMeta {
    #[serde(default, deserialize_with = "crate::models::nullable")]
    pub not_connected_after_hours: Option<f64>,
    #[serde(default, deserialize_with = "crate::models::nullable")]
    pub expiration: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HwidEventData {
    pub user: RemnaUser,
    #[serde(rename = "hwidUserDevice")]
    pub device: HwidDevice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmEventData {
    pub provider_name: String,
    pub node_name: String,
    pub next_billing_at: DateTime<Utc>,
    #[serde(default, deserialize_with = "crate::models::nullable")]
    pub login_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginAttempt {
    #[serde(default, deserialize_with = "crate::models::nullable")]
    pub username: String,
    #[serde(default, deserialize_with = "crate::models::nullable")]
    pub ip: String,
    #[serde(default, deserialize_with = "crate::models::nullable")]
    pub user_agent: String,
    #[serde(default, deserialize_with = "crate::models::nullable")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubpageConfigChange {
    pub action: String,
    pub uuid: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiTokenInfo {
    pub name: String,
    pub uuid: Uuid,
    #[serde(default, deserialize_with = "crate::models::nullable")]
    pub expire_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "crate::models::nullable")]
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceEventData {
    #[serde(default, deserialize_with = "crate::models::nullable")]
    pub login_attempt: Option<LoginAttempt>,
    #[serde(default, deserialize_with = "crate::models::nullable")]
    pub panel_version: Option<String>,
    #[serde(default, deserialize_with = "crate::models::nullable")]
    pub subpage_config: Option<SubpageConfigChange>,
    #[serde(default, deserialize_with = "crate::models::nullable")]
    pub api_token: Option<ApiTokenInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ErrorsEventData {
    #[serde(default, deserialize_with = "crate::models::nullable")]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentEventUser {
    pub id: RemnaUserId,
    #[serde(default, deserialize_with = "crate::models::nullable")]
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentBlockerEventData {
    pub node: TorrentNodeRef,
    pub user: TorrentEventUser,
    pub report: TorrentReportDetails,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RemnaEvent {
    User {
        user: Box<RemnaUser>,
        meta: WebhookMeta,
    },
    UserHwid(Box<HwidEventData>),
    Node(Box<Node>),
    Crm(CrmEventData),
    Service(ServiceEventData),
    Errors(ErrorsEventData),
    TorrentBlocker(Box<TorrentBlockerEventData>),
    Other,
    Unparsed {
        error: String,
        subject: Option<String>,
    },
}

fn redact_user(value: &mut serde_json::Value) {
    if let Some(user) = value.as_object_mut() {
        user.remove("subscriptionUrl");
        for secret in ["vlessUuid", "trojanPassword", "ssPassword"] {
            user.remove(secret);
        }
        if user.contains_key("shortUuid") {
            user.insert(
                "shortUuid".to_string(),
                serde_json::Value::String(String::new()),
            );
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedWebhook {
    pub scope: String,
    pub event: String,
    pub occurred_at: DateTime<Utc>,
    pub digest: [u8; 32],
    pub payload: RemnaEvent,
}

impl ParsedWebhook {
    pub fn subject(&self) -> Option<(&'static str, String)> {
        match &self.payload {
            RemnaEvent::User { user, .. } => Some(("user", user.id.to_string())),
            RemnaEvent::UserHwid(data) => Some(("user", data.user.id.to_string())),
            RemnaEvent::Node(node) => Some(("node", node.uuid.to_string())),
            RemnaEvent::Crm(data) => Some(("node_name", data.node_name.clone())),
            RemnaEvent::TorrentBlocker(data) => Some(("user", data.user.id.to_string())),
            RemnaEvent::Unparsed {
                subject: Some(id), ..
            } => Some((
                if self.scope == scopes::NODE {
                    "node"
                } else {
                    "user"
                },
                id.clone(),
            )),
            RemnaEvent::Service(_)
            | RemnaEvent::Errors(_)
            | RemnaEvent::Other
            | RemnaEvent::Unparsed { .. } => None,
        }
    }

    pub fn redacted_payload(&self) -> serde_json::Value {
        let data = match &self.payload {
            RemnaEvent::User { user, meta } => {
                let mut user = serde_json::to_value(user).unwrap_or_default();
                redact_user(&mut user);
                serde_json::json!({ "user": user, "meta": meta })
            }
            RemnaEvent::UserHwid(data) => {
                let mut value = serde_json::to_value(data).unwrap_or_default();
                if let Some(user) = value.get_mut("user") {
                    redact_user(user);
                }
                value
            }
            RemnaEvent::Node(node) => serde_json::to_value(node).unwrap_or_default(),
            RemnaEvent::Crm(data) => serde_json::to_value(data).unwrap_or_default(),
            RemnaEvent::Service(data) => serde_json::to_value(data).unwrap_or_default(),
            RemnaEvent::Errors(data) => serde_json::to_value(data).unwrap_or_default(),
            RemnaEvent::TorrentBlocker(data) => serde_json::to_value(data).unwrap_or_default(),
            RemnaEvent::Other => serde_json::Value::Null,
            RemnaEvent::Unparsed { error, subject } => {
                serde_json::json!({ "error": error, "subject": subject })
            }
        };
        serde_json::json!({
            "scope": self.scope,
            "event": self.event,
            "timestamp": self.occurred_at,
            "data": data,
        })
    }
}

fn subject_of(data: &serde_json::Value) -> Option<String> {
    let id = data.get("id").or_else(|| data.get("uuid"))?;
    match id {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn typed<T: serde::de::DeserializeOwned>(data: &serde_json::Value) -> Result<T, String> {
    serde_json::from_value(data.clone()).map_err(|e| e.to_string())
}

pub fn parse(body: &[u8]) -> Result<ParsedWebhook, WebhookError> {
    let envelope: Envelope =
        serde_json::from_slice(body).map_err(|e| WebhookError::Malformed(e.to_string()))?;
    let unparsed = |error: String| RemnaEvent::Unparsed {
        error,
        subject: subject_of(&envelope.data),
    };
    let nested_user = |error: String| RemnaEvent::Unparsed {
        error,
        subject: envelope.data.get("user").and_then(subject_of),
    };
    let payload = match envelope.scope.as_str() {
        scopes::USER => match typed::<RemnaUser>(&envelope.data) {
            Ok(user) => RemnaEvent::User {
                user: Box::new(user),
                meta: envelope
                    .meta
                    .as_ref()
                    .and_then(|m| serde_json::from_value(m.clone()).ok())
                    .unwrap_or_default(),
            },
            Err(e) => unparsed(e),
        },
        scopes::USER_HWID_DEVICES => match typed::<HwidEventData>(&envelope.data) {
            Ok(data) => RemnaEvent::UserHwid(Box::new(data)),
            Err(e) => nested_user(e),
        },
        scopes::NODE => match typed::<Node>(&envelope.data) {
            Ok(node) => RemnaEvent::Node(Box::new(node)),
            Err(e) => unparsed(e),
        },
        scopes::CRM => match typed::<CrmEventData>(&envelope.data) {
            Ok(data) => RemnaEvent::Crm(data),
            Err(e) => unparsed(e),
        },
        scopes::SERVICE => match typed::<ServiceEventData>(&envelope.data) {
            Ok(data) => RemnaEvent::Service(data),
            Err(e) => unparsed(e),
        },
        scopes::ERRORS => match typed::<ErrorsEventData>(&envelope.data) {
            Ok(data) => RemnaEvent::Errors(data),
            Err(e) => unparsed(e),
        },
        scopes::TORRENT_BLOCKER => match typed::<TorrentBlockerEventData>(&envelope.data) {
            Ok(data) => RemnaEvent::TorrentBlocker(Box::new(data)),
            Err(e) => nested_user(e),
        },
        _ => RemnaEvent::Other,
    };
    Ok(ParsedWebhook {
        scope: envelope.scope,
        event: envelope.event,
        occurred_at: envelope.timestamp,
        digest: body_digest(body),
        payload,
    })
}

pub fn verify_and_parse(
    secret: &str,
    body: &[u8],
    signature: Option<&str>,
) -> Result<ParsedWebhook, WebhookError> {
    if !verify_signature(secret, body, signature) {
        return Err(WebhookError::BadSignature);
    }
    parse(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";
    const BODY: &str = r#"{"scope":"service","event":"service.panel_started","timestamp":"2026-01-07T11:57:29.426Z","data":{"panelVersion":"3.4.11"}}"#;

    #[test]
    fn тестовый_вектор_подписи() {
        assert_eq!(BODY.len(), 123);
        assert_eq!(
            sign(SECRET, BODY.as_bytes()),
            "06fa972227d8d2a386eb9effedce0e077ed74d12a6f607a45461ef544cae08e0"
        );
        assert_eq!(
            hex::encode(body_digest(BODY.as_bytes())),
            "714791d97cb5021ef7cd92c107d05be8f5c6b3782f97c649acfdbd428989dc88"
        );
    }

    #[test]
    fn неверная_или_пустая_подпись_отклоняется() {
        let good = sign(SECRET, BODY.as_bytes());
        assert!(verify_signature(SECRET, BODY.as_bytes(), Some(&good)));
        assert!(verify_signature(
            SECRET,
            BODY.as_bytes(),
            Some(&good.to_uppercase())
        ));
        assert!(!verify_signature(SECRET, BODY.as_bytes(), None));
        assert!(!verify_signature(SECRET, BODY.as_bytes(), Some("zz")));
        assert!(!verify_signature(SECRET, b"{}", Some(&good)));
        assert!(!verify_signature("", BODY.as_bytes(), Some(&good)));
        assert!(matches!(
            verify_and_parse(SECRET, BODY.as_bytes(), Some("00")),
            Err(WebhookError::BadSignature)
        ));
    }

    #[test]
    fn событие_пользователя_без_секретов_в_журнале() {
        let body = serde_json::json!({
            "scope": "user", "event": "user.modified", "timestamp": "2026-11-16T14:52:00.000Z",
            "data": {
                "id": 42, "shortUuid": "abc", "username": "sn_x", "status": "ACTIVE",
                "trafficLimitBytes": 0, "trafficLimitStrategy": "NO_RESET",
                "expireAt": "2027-02-14T00:00:00.000Z", "createdAt": "2026-11-16T14:52:00.000Z",
                "updatedAt": "2026-11-16T14:52:00.000Z", "trojanPassword": "top-secret",
                "vlessUuid": "00000000-0000-0000-0000-000000000000", "ssPassword": "top-secret",
                "subscriptionUrl": "https://sub.example.com/abc", "activeInternalSquads": [],
                "userTraffic": {"usedTrafficBytes": 5, "lifetimeUsedTrafficBytes": 5}
            },
            "meta": null
        })
        .to_string();
        let parsed = parse(body.as_bytes()).unwrap();
        assert_eq!(parsed.subject(), Some(("user", "42".to_string())));
        let payload = parsed.redacted_payload();
        let stored = payload.to_string();
        assert!(!stored.contains("top-secret"));
        assert!(!stored.contains("vlessUuid"));
        assert!(!stored.contains("sub.example.com"));
        assert_eq!(payload["data"]["user"]["shortUuid"], "");
        let back: RemnaUser = serde_json::from_value(payload["data"]["user"].clone()).unwrap();
        assert_eq!(back.id, RemnaUserId(42));
        assert_eq!(back.user_traffic.used_traffic_bytes, 5);
    }

    fn user_data() -> serde_json::Value {
        serde_json::json!({
            "id": 42, "shortUuid": "abcdefgh12345678", "username": "sn_x", "status": "ACTIVE",
            "trafficLimitBytes": 0, "trafficLimitStrategy": "NO_RESET",
            "expireAt": "2027-02-14T00:00:00.000Z", "createdAt": "2026-11-16T14:52:00.000Z",
            "updatedAt": "2026-11-16T14:52:00.000Z", "trojanPassword": "top-secret",
            "vlessUuid": "00000000-0000-0000-0000-000000000000", "ssPassword": "top-secret",
            "subscriptionUrl": "https://sub.example.com/abcdefgh12345678", "activeInternalSquads": [],
            "userTraffic": {"usedTrafficBytes": 0, "lifetimeUsedTrafficBytes": 0}
        })
    }

    fn envelope(scope: &str, event: &str, data: serde_json::Value) -> Vec<u8> {
        serde_json::json!({
            "scope": scope, "event": event, "timestamp": "2026-11-16T14:52:00.000Z", "data": data
        })
        .to_string()
        .into_bytes()
    }

    fn node_data() -> serde_json::Value {
        serde_json::json!({
            "uuid": "00000000-0000-4000-8000-000000000002", "id": 2, "name": "NL-2",
            "address": "198.51.100.22", "isConnected": false, "isDisabled": false, "isConnecting": false,
            "lastStatusChange": "2026-11-16T14:38:00.000Z", "lastStatusMessage": "connect ETIMEDOUT",
            "countryCode": "NL", "usersOnline": 0
        })
    }

    #[test]
    fn событие_устройства_разбирается_без_ссылки_подписки() {
        let body = envelope(
            scopes::USER_HWID_DEVICES,
            events::HWID_ADDED,
            serde_json::json!({
                "user": user_data(),
                "hwidUserDevice": {
                    "hwid": "hw-1", "userId": 42, "platform": "iOS", "osVersion": "18.1",
                    "deviceModel": "iPhone 15 Pro", "userAgent": "Happ/3.2.1", "requestIp": "203.0.113.7",
                    "createdAt": "2026-11-16T14:52:00.000Z", "updatedAt": "2026-11-16T14:52:00.000Z"
                }
            }),
        );
        let parsed = parse(&body).unwrap();
        let RemnaEvent::UserHwid(data) = &parsed.payload else {
            panic!("ожидалось событие устройства");
        };
        assert_eq!(data.device.hwid, "hw-1");
        assert_eq!(parsed.subject(), Some(("user", "42".to_string())));
        let stored = parsed.redacted_payload().to_string();
        assert!(!stored.contains("top-secret"));
        assert!(!stored.contains("abcdefgh12345678"));
        assert!(stored.contains("iPhone 15 Pro"));
    }

    #[test]
    fn событие_ноды_разбирается() {
        let parsed = parse(&envelope(
            scopes::NODE,
            events::NODE_CONNECTION_LOST,
            node_data(),
        ))
        .unwrap();
        let RemnaEvent::Node(node) = &parsed.payload else {
            panic!("ожидалось событие ноды");
        };
        assert_eq!(node.state(), crate::models::NodeState::Down);
        assert_eq!(
            node.last_status_message.as_deref(),
            Some("connect ETIMEDOUT")
        );
        assert_eq!(
            parsed.subject(),
            Some(("node", "00000000-0000-4000-8000-000000000002".to_string()))
        );
    }

    #[test]
    fn событие_оплаты_серверов_разбирается() {
        let parsed = parse(&envelope(
            scopes::CRM,
            events::CRM_PAYMENT_IN_48_HOURS,
            serde_json::json!({
                "providerName": "Hetzner", "nodeName": "FI-1",
                "nextBillingAt": "2026-11-18T00:00:00.000Z", "loginUrl": "https://hetzner.example"
            }),
        ))
        .unwrap();
        let RemnaEvent::Crm(data) = &parsed.payload else {
            panic!("ожидалось событие оплаты");
        };
        assert_eq!(data.provider_name, "Hetzner");
        assert_eq!(parsed.subject(), Some(("node_name", "FI-1".to_string())));
    }

    #[test]
    fn служебное_событие_не_хранит_введённый_пароль() {
        let parsed = parse(&envelope(
            scopes::SERVICE,
            events::SERVICE_LOGIN_FAILED,
            serde_json::json!({
                "loginAttempt": {"username": "admin", "ip": "198.51.100.9", "userAgent": "curl/8",
                                 "description": "wrong password", "password": "hunter2-пароль"}
            }),
        ))
        .unwrap();
        let RemnaEvent::Service(data) = &parsed.payload else {
            panic!("ожидалось служебное событие");
        };
        assert_eq!(data.login_attempt.as_ref().unwrap().username, "admin");
        assert_eq!(parsed.subject(), None);
        let stored = parsed.redacted_payload().to_string();
        assert!(!stored.contains("hunter2"));
        assert!(stored.contains("198.51.100.9"));
        let started = parse(BODY.as_bytes()).unwrap();
        assert_eq!(
            started.payload,
            RemnaEvent::Service(ServiceEventData {
                panel_version: Some("3.4.11".into()),
                ..ServiceEventData::default()
            })
        );
    }

    #[test]
    fn ошибка_панели_разбирается() {
        let parsed = parse(&envelope(
            scopes::ERRORS,
            events::ERRORS_BANDWIDTH_MAX_NOTIFICATIONS,
            serde_json::json!({"description": "достигнут предел уведомлений"}),
        ))
        .unwrap();
        assert_eq!(
            parsed.payload,
            RemnaEvent::Errors(ErrorsEventData {
                description: "достигнут предел уведомлений".into()
            })
        );
        assert_eq!(parsed.subject(), None);
    }

    #[test]
    fn торрент_блокер_без_адреса_назначения_и_секретов() {
        let parsed = parse(&envelope(
            scopes::TORRENT_BLOCKER,
            events::TORRENT_BLOCKER_REPORT,
            serde_json::json!({
                "node": node_data(),
                "user": user_data(),
                "report": {
                    "actionReport": {"blocked": true, "ip": "203.0.113.7", "blockDuration": 3600,
                                     "willUnblockAt": "2026-11-16T15:52:00.000Z", "userId": "42",
                                     "processedAt": "2026-11-16T14:52:00.000Z"},
                    "xrayReport": {"destination": "tracker.example.org:6969", "network": "tcp", "ts": 1}
                }
            }),
        ))
        .unwrap();
        let RemnaEvent::TorrentBlocker(data) = &parsed.payload else {
            panic!("ожидалось событие торрент-блокера");
        };
        assert_eq!(data.user.id, RemnaUserId(42));
        assert_eq!(data.node.name, "NL-2");
        assert_eq!(data.report.action_report.ip, "203.0.113.7");
        assert_eq!(parsed.subject(), Some(("user", "42".to_string())));
        let stored = parsed.redacted_payload().to_string();
        assert!(!stored.contains("tracker.example.org"));
        assert!(!stored.contains("top-secret"));
        assert!(!stored.contains("abcdefgh12345678"));
    }

    #[test]
    fn перечень_событий_полный_и_без_повторов() {
        let mut seen = std::collections::HashSet::new();
        for event in events::ALL {
            assert!(seen.insert(event), "повтор {event}");
            assert!(events::scope_of(event).is_some(), "{event} без scope");
            assert!(events::is_known(event));
        }
        for scope in scopes::ALL {
            assert!(
                events::ALL
                    .iter()
                    .any(|e| events::scope_of(e) == Some(scope)),
                "{scope} без событий"
            );
        }
        assert_eq!(
            events::scope_of(events::HWID_DELETED),
            Some(scopes::USER_HWID_DEVICES)
        );
        assert!(!events::is_known("user.teleported"));
    }

    #[test]
    fn незнакомое_событие_не_ломает_приём() {
        let parsed = parse(&envelope(
            "future_scope",
            "future_scope.happened",
            serde_json::json!({"x": 1}),
        ))
        .unwrap();
        assert_eq!(parsed.payload, RemnaEvent::Other);
        assert_eq!(parsed.subject(), None);
        let broken = br#"{"scope":"user","event":"user.created","timestamp":"2026-11-16T14:52:00Z","data":{"id":5}}"#;
        let parsed = parse(broken).unwrap();
        assert!(matches!(parsed.payload, RemnaEvent::Unparsed { .. }));
        assert_eq!(parsed.subject(), Some(("user", "5".to_string())));
        assert!(parse(b"not json").is_err());
    }

    #[test]
    fn секрет_проверяется_как_в_панели() {
        assert!(validate_secret(SECRET).is_ok());
        assert!(validate_secret("short").is_err());
        assert!(validate_secret("0123456789abcdef0123456789abcde!").is_err());
    }
}
