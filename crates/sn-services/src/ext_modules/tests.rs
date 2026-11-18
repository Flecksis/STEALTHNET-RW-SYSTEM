use super::delivery::{MAX_ATTEMPTS, Sent, outcome, retry_delay};
use super::manifest::{FieldKind, parse};
use super::*;
use chrono::TimeZone;

pub const HELLO: &str = r#"{
  "format": 1,
  "id": "hello",
  "name": "Привет",
  "version": "1.0.0",
  "author": "Студия Пример",
  "description": "Здоровается с новыми клиентами и благодарит за оплату",
  "site": "https://example.com/hello",
  "webhook": "https://hello.example.com/stealthnet/events",
  "events": ["client_registered", "payment_paid"],
  "permissions": ["clients_write", "clients_read", "money"],
  "page": { "url": "https://hello.example.com/admin", "title": "Привет: журнал" },
  "settings": [
    { "key": "greeting", "type": "text", "label": "Текст приветствия", "large": true, "default": "Спасибо, что с нами!" },
    { "key": "thank_after_payment", "type": "flag", "label": "Благодарить за оплату", "default": true },
    { "key": "crm_token", "type": "secret", "label": "Токен CRM", "hint": "Из личного кабинета CRM" },
    { "key": "limit", "type": "number", "label": "Сколько сообщений в час", "default": 20 },
    { "key": "tone", "type": "select", "label": "Тон", "options": ["formal", "friendly"], "labels": ["На вы", "На ты"], "default": "friendly", "required": true }
  ]
}"#;

fn messages(source: &str) -> Vec<(String, String)> {
    parse(source)
        .expect_err("ожидались ошибки")
        .into_iter()
        .map(|p| (p.path, p.message))
        .collect()
}

fn has(problems: &[(String, String)], path: &str, part: &str) -> bool {
    problems.iter().any(|(p, m)| p == path && m.contains(part))
}

#[test]
fn описание_модуля_разбирается_полностью() {
    let manifest = parse(HELLO).unwrap();
    assert_eq!(manifest.id, "hello");
    assert_eq!(manifest.name, "Привет");
    assert_eq!(manifest.author, "Студия Пример");
    assert_eq!(
        manifest.webhook.as_deref(),
        Some("https://hello.example.com/stealthnet/events")
    );
    assert_eq!(manifest.events, ["client_registered", "payment_paid"]);
    assert_eq!(
        manifest.permissions,
        [
            Permission::ClientsRead,
            Permission::ClientsWrite,
            Permission::Money
        ]
    );
    let page = manifest.page.as_ref().unwrap();
    assert_eq!(page.title, "Привет: журнал");
    assert_eq!(page.permission, Permission::Modules);
    assert_eq!(manifest.settings.len(), 5);
    let tone = &manifest.settings[4];
    assert_eq!(
        tone.kind,
        FieldKind::Select {
            options: vec!["formal".into(), "friendly".into()]
        }
    );
    assert_eq!(tone.default.as_deref(), Some("friendly"));
    assert!(tone.required);
    assert_eq!(manifest.settings[1].default.as_deref(), Some("true"));
    assert_eq!(manifest.settings[3].default.as_deref(), Some("20"));
    assert!(manifest.settings[2].is_secret());
    let json = serde_json::to_value(&manifest.settings[4]).unwrap();
    assert_eq!(json["kind"]["type"], "select");
    assert_eq!(json["kind"]["options"][1], "friendly");
    assert_eq!(json["labels"][0], "На вы");
    let stored: Manifest =
        serde_json::from_value(serde_json::to_value(&manifest).unwrap()).unwrap();
    assert_eq!(stored, manifest);
}

#[test]
fn не_json_и_не_тот_формат_объясняются_по_русски() {
    let problems = messages("{\n  \"id\": \"hello\",\n  oops\n}");
    assert!(has(&problems, "", "строке 3"), "{problems:?}");
    assert!(has(&messages("[1, 2]"), "", "фигурных скобках"));
    assert!(has(&messages("   "), "", "пустой"));
    assert!(has(
        &messages(r#"{"format": 2, "id": "x"}"#),
        "format",
        "только 1"
    ));
    assert!(has(
        &messages(r#"{"id": "hello"}"#),
        "format",
        "\"format\": 1"
    ));
    let huge = format!("{{\"format\": 1, \"pad\": \"{}\"}}", "a".repeat(70 * 1024));
    assert!(has(&messages(&huge), "", "64 КБ"));
}

#[test]
fn ошибки_описания_собраны_все_сразу() {
    let problems = messages(
        r#"{
      "format": 1,
      "id": "Promo!",
      "name": "",
      "version": "1",
      "permisions": ["money"],
      "events": ["payment_paid", "payment_done", 5],
      "permissions": ["clients_read", "admins", "superuser"],
      "webhook": "http://hello.example.com/events",
      "page": { "url": "https://192.168.1.10/admin", "color": "red" },
      "settings": "нет"
    }"#,
    );
    assert!(
        has(&problems, "permisions", "неизвестное поле"),
        "{problems:?}"
    );
    assert!(has(&problems, "id", "латинские буквы"));
    assert!(has(&problems, "name", "пустым"));
    assert!(has(&problems, "author", "обязательно"));
    assert!(has(&problems, "events[0]", "«Деньги»"));
    assert!(has(
        &problems,
        "events[1]",
        "неизвестное событие «payment_done»"
    ));
    assert!(has(&problems, "events[2]", "строка"));
    assert!(has(
        &problems,
        "permissions[1]",
        "«Администраторы» модулю дать нельзя"
    ));
    assert!(has(
        &problems,
        "permissions[2]",
        "неизвестное право «superuser»"
    ));
    assert!(has(
        &problems,
        "webhook",
        "http:// можно только для этого же сервера"
    ));
    assert!(has(&problems, "page.url", "внутренней сети"));
    assert!(has(&problems, "page.color", "неизвестное поле"));
    assert!(has(&problems, "settings", "список"));
}

#[test]
fn имя_модуля_не_совпадает_со_встроенными() {
    let base = |id: &str| HELLO.replace("\"id\": \"hello\"", &format!("\"id\": \"{id}\""));
    assert!(has(&messages(&base("promo")), "id", "встроенного модуля"));
    assert!(has(&messages(&base("me")), "id", "занято панелью"));
    assert!(has(&messages(&base("x")), "id", "от 2 до 40"));
    assert!(has(&messages(&base("-crm")), "id", "первая — буква"));
    assert!(parse(&base("crm-sync2")).is_ok());
}

#[test]
fn события_без_адреса_не_принимаются() {
    let source = HELLO.replace(
        "\"webhook\": \"https://hello.example.com/stealthnet/events\",",
        "",
    );
    assert!(has(&messages(&source), "webhook", "укажите адрес"));
    let quiet = source.replace("\"events\": [\"client_registered\", \"payment_paid\"],", "");
    let manifest = parse(&quiet).unwrap();
    assert!(manifest.events.is_empty());
    assert!(manifest.webhook.is_none());
}

#[test]
fn поля_настроек_проверяются() {
    let source = r#"{
      "format": 1, "id": "crm", "name": "CRM", "version": "1", "author": "Я",
      "settings": [
        { "key": "Token", "type": "secret", "label": "Ключ", "default": "x" },
        { "key": "mode", "type": "select", "label": "Режим", "options": [], "default": "a" },
        { "key": "mode", "type": "text", "label": "Дубль" },
        { "key": "level", "type": "number", "label": "Уровень", "default": "много" },
        { "key": "kind", "type": "color", "label": "Цвет" },
        { "key": "flag", "type": "flag", "label": "Флаг", "large": true },
        { "key": "pick", "type": "select", "label": "Выбор", "options": ["a", "b"], "labels": ["А"] },
        { "type": "text" }
      ]
    }"#;
    let problems = messages(source);
    assert!(
        has(&problems, "settings[0].key", "нижнем регистре"),
        "{problems:?}"
    );
    assert!(has(&problems, "settings[0].default", "секретного поля"));
    assert!(has(&problems, "settings[1].options", "от 1 до 50"));
    assert!(has(&problems, "settings[1].default", "из options"));
    assert!(has(&problems, "settings[2].key", "уже есть"));
    assert!(has(&problems, "settings[3].default", "число"));
    assert!(has(
        &problems,
        "settings[4].type",
        "неизвестный тип «color»"
    ));
    assert!(has(&problems, "settings[5].large", "только у type: text"));
    assert!(has(&problems, "settings[6].labels", "столько же"));
    assert!(has(&problems, "settings[7].key", "обязательно"));
    assert!(has(&problems, "settings[7].label", "обязательно"));
}

#[test]
fn повторы_с_паузами_и_пределом() {
    let pauses: Vec<i64> = (1..MAX_ATTEMPTS)
        .map(|n| retry_delay(n).unwrap().num_seconds())
        .collect();
    assert_eq!(pauses, [30, 120, 600, 1_800, 3_600, 10_800, 21_600]);
    assert!(retry_delay(MAX_ATTEMPTS).is_none());
    assert!(retry_delay(0).is_none());
    let now = Utc.with_ymd_and_hms(2026, 11, 18, 12, 0, 0).unwrap();
    let bad = Sent {
        http_status: Some(500),
        duration_ms: 12,
        error: Some("модуль ответил кодом 500".into()),
        response: None,
    };
    let first = outcome(1, bad.clone(), now, true);
    assert_eq!(first.status, "pending");
    assert_eq!(first.next_attempt_at, Some(now + Duration::seconds(30)));
    let last = outcome(MAX_ATTEMPTS, bad.clone(), now, true);
    assert_eq!(last.status, "failed");
    assert_eq!(last.next_attempt_at, None);
    assert_eq!(outcome(1, bad, now, false).status, "failed");
    let good = Sent {
        http_status: Some(204),
        duration_ms: 3,
        error: None,
        response: None,
    };
    assert_eq!(outcome(3, good, now, true).status, "done");
}

#[test]
fn тело_события_без_служебного_типа_внутри_данных() {
    let delivery = store::Claimed {
        id: 9,
        event_id: Some(41),
        kind: "payment_paid".into(),
        payload: serde_json::json!({ "type": "payment_paid", "client_id": 3, "amount": { "minor": 699, "currency": "RUB" } }),
        occurred_at: Utc.with_ymd_and_hms(2026, 11, 18, 12, 0, 0).unwrap(),
        attempts: 0,
    };
    let body: Value = serde_json::from_slice(&delivery::body("hello", &delivery)).unwrap();
    assert_eq!(body["id"], "evt_41");
    assert_eq!(body["type"], "payment_paid");
    assert_eq!(body["module"], "hello");
    assert_eq!(body["data"]["client_id"], 3);
    assert_eq!(body["data"]["amount"]["minor"], 699);
    assert!(body["data"].get("type").is_none());
    let service = store::Claimed {
        event_id: None,
        kind: "ping".into(),
        payload: serde_json::json!({}),
        ..delivery
    };
    assert_eq!(delivery::message_id(&service), "msg_9");
}
