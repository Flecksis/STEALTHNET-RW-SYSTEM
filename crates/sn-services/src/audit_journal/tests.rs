use super::*;
use crate::{EnvFallbacks, ServiceDeps, SystemClock};
use chrono::Duration;
use serde_json::json;
use sn_domain::access::AdminRole;
use sn_domain::client::{RegistrationSource, TelegramProfile};
use sn_modules::ModuleRegistry;
use sn_payments::ProviderRegistry;
use sn_store::testing::{TestDb, test_store};
use sn_store::{ActorKind, AuditEntry, NewAdmin, NewApiKey};
use std::sync::Arc;

async fn setup() -> Option<(TestDb, Services)> {
    let db = test_store().await?;
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: None,
        clock: Arc::new(SystemClock),
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    Some((db, services))
}

async fn admin(services: &Services, email: &str, role: AdminRole) -> i64 {
    let mut conn = services.store().conn().await.unwrap();
    conn.admins()
        .create(&NewAdmin {
            email: email.into(),
            password_hash: "x".into(),
            role,
            must_change_password: false,
        })
        .await
        .unwrap()
        .id
        .0
}

async fn client(services: &Services, telegram_id: i64, username: &str) -> i64 {
    services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: telegram_id,
                username: Some(username.into()),
                first_name: None,
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
            RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap()
        .0
        .id
        .0
}

async fn row(
    services: &Services,
    at: DateTime<Utc>,
    actor: (&str, Option<i64>),
    action: &str,
    entity: Option<(&str, i64)>,
    details: Value,
) -> i64 {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_scalar(
        "INSERT INTO audit_log (actor_kind, actor_id, action, entity_kind, entity_id, details, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(actor.0)
    .bind(actor.1)
    .bind(action)
    .bind(entity.map(|e| e.0))
    .bind(entity.map(|e| e.1))
    .bind(details)
    .bind(at)
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

fn base() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 19, 9, 0, 0).unwrap()
}

async fn all(services: &Services, query: JournalQuery) -> Vec<JournalItem> {
    let mut query = query;
    let mut items = Vec::new();
    loop {
        let page = services.audit_journal().page(&query).await.unwrap();
        items.extend(page.items);
        match page.next {
            Some(next) => query.cursor = Some(next),
            None => return items,
        }
    }
}

fn ids(items: &[JournalItem]) -> Vec<i64> {
    items.iter().map(|item| item.id).collect()
}

#[tokio::test]
async fn журнал_листается_курсором_без_повторов_и_пропусков() {
    let Some((db, services)) = setup().await else {
        return;
    };
    let mut expected = Vec::new();
    for (minutes, count) in [(0, 2), (5, 3), (9, 1), (20, 2)] {
        for _ in 0..count {
            let id = row(
                &services,
                base() + Duration::minutes(minutes),
                ("system", None),
                "subscription.ended",
                None,
                json!({}),
            )
            .await;
            expected.push((minutes, id));
        }
    }
    expected.sort_by(|a, b| b.cmp(a));
    let expected: Vec<i64> = expected.into_iter().map(|(_, id)| id).collect();
    let paged = all(
        &services,
        JournalQuery {
            limit: Some(3),
            ..JournalQuery::default()
        },
    )
    .await;
    assert_eq!(ids(&paged), expected);
    let first = services
        .audit_journal()
        .page(&JournalQuery {
            limit: Some(3),
            ..JournalQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(first.items.len(), 3);
    let cursor = parse_cursor(first.next.as_deref().unwrap()).unwrap();
    assert_eq!(cursor.id, first.items[2].id);
    assert_eq!(cursor.at, first.items[2].at);
    let rest = services
        .audit_journal()
        .page(&JournalQuery {
            cursor: first.next.clone(),
            ..JournalQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(rest.items.len(), expected.len() - 3);
    assert!(rest.next.is_none());
    assert!(
        services
            .audit_journal()
            .page(&JournalQuery {
                cursor: Some("вчера".into()),
                ..JournalQuery::default()
            })
            .await
            .is_err()
    );
    db.cleanup().await;
}

#[tokio::test]
async fn фильтры_по_участнику_разделу_действию_клиенту_периоду_и_поиску() {
    let Some((db, services)) = setup().await else {
        return;
    };
    let anna = admin(&services, "anna@studio.example", AdminRole::Admin).await;
    let help = admin(&services, "help@studio.example", AdminRole::Support).await;
    let maria = client(&services, 7_400_000_101, "maria_k").await;
    let ivan = client(&services, 7_400_000_102, "ivan_petrov").await;
    let sub: i64 = {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query_scalar(
            "INSERT INTO subscriptions (client_id, kind) VALUES ($1, 'manual') RETURNING id",
        )
        .bind(maria)
        .fetch_one(conn.conn())
        .await
        .unwrap()
    };
    let days = row(
        &services,
        base(),
        ("admin", Some(anna)),
        "subscription.add_days",
        Some(("subscription", sub)),
        json!({ "days": 7, "reason": "Компенсация за сбой 18 ноября" }),
    )
    .await;
    let block = row(
        &services,
        base() + Duration::minutes(1),
        ("admin", Some(help)),
        "client.block",
        Some(("client", ivan)),
        json!({ "reason": "Рассылка спама в поддержку" }),
    )
    .await;
    let note = row(
        &services,
        base() + Duration::minutes(2),
        ("admin", Some(anna)),
        "client.note",
        Some(("client", maria)),
        json!({ "note": "Купил годовой" }),
    )
    .await;
    let ended = row(
        &services,
        base() - Duration::days(3),
        ("system", None),
        "subscription.ended",
        Some(("subscription", sub)),
        json!({}),
    )
    .await;
    let own = row(
        &services,
        base() + Duration::minutes(3),
        ("client", Some(maria)),
        "client.password_changed",
        Some(("client", maria)),
        json!({}),
    )
    .await;
    let tariff = row(
        &services,
        base() + Duration::minutes(4),
        ("admin", Some(anna)),
        "tariff.update",
        Some(("tariff", 999_999)),
        json!({ "name": "Базовый" }),
    )
    .await;
    let mut ip = row(
        &services,
        base() + Duration::minutes(5),
        ("admin", Some(anna)),
        "backups.downloaded",
        None,
        json!({ "file": "panel-2026-11-18-0400.tar.gz" }),
    )
    .await;
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query("UPDATE audit_log SET ip = '85.143.12.40' WHERE id = $1")
            .bind(ip)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let by = |query: JournalQuery| {
        let services = &services;
        async move { ids(&all(services, query).await) }
    };
    assert_eq!(
        by(JournalQuery {
            actor: Some(format!("admin:{anna}")),
            ..JournalQuery::default()
        })
        .await,
        vec![ip, tariff, note, days]
    );
    assert_eq!(
        by(JournalQuery {
            actor: Some("system".into()),
            ..JournalQuery::default()
        })
        .await,
        vec![ended]
    );
    assert_eq!(
        by(JournalQuery {
            actor: Some("clients".into()),
            ..JournalQuery::default()
        })
        .await,
        vec![own]
    );
    assert_eq!(
        by(JournalQuery {
            area: Some("clients".into()),
            ..JournalQuery::default()
        })
        .await,
        vec![own, note, block]
    );
    assert_eq!(
        by(JournalQuery {
            action: Some("client.block".into()),
            ..JournalQuery::default()
        })
        .await,
        vec![block]
    );
    assert_eq!(
        by(JournalQuery {
            client: Some(maria),
            ..JournalQuery::default()
        })
        .await,
        vec![own, note, days, ended]
    );
    assert_eq!(
        by(JournalQuery {
            since: Some(base() - Duration::hours(1)),
            until: Some(base() + Duration::seconds(90)),
            ..JournalQuery::default()
        })
        .await,
        vec![block, days]
    );
    assert_eq!(
        by(JournalQuery {
            q: Some("спама".into()),
            ..JournalQuery::default()
        })
        .await,
        vec![block]
    );
    assert_eq!(
        by(JournalQuery {
            q: Some("блокир".into()),
            ..JournalQuery::default()
        })
        .await,
        vec![block]
    );
    assert_eq!(
        by(JournalQuery {
            q: Some("85.143.12.40".into()),
            ..JournalQuery::default()
        })
        .await,
        vec![ip]
    );
    assert_eq!(
        by(JournalQuery {
            q: Some("100%_".into()),
            ..JournalQuery::default()
        })
        .await,
        Vec::<i64>::new()
    );
    assert_eq!(
        by(JournalQuery {
            actor: Some(format!("admin:{anna}")),
            area: Some("clients".into()),
            client: Some(maria),
            ..JournalQuery::default()
        })
        .await,
        vec![note]
    );
    for bad in [
        JournalQuery {
            q: Some("аб".into()),
            ..JournalQuery::default()
        },
        JournalQuery {
            actor: Some("root".into()),
            ..JournalQuery::default()
        },
        JournalQuery {
            area: Some("everything".into()),
            ..JournalQuery::default()
        },
        JournalQuery {
            action: Some("DROP TABLE".into()),
            ..JournalQuery::default()
        },
        JournalQuery {
            since: Some(base()),
            until: Some(base()),
            ..JournalQuery::default()
        },
    ] {
        assert!(
            services.audit_journal().page(&bad).await.is_err(),
            "{bad:?}"
        );
    }
    let page = services
        .audit_journal()
        .page(&JournalQuery::default())
        .await
        .unwrap();
    let days_item = page.items.iter().find(|i| i.id == days).unwrap();
    assert_eq!(days_item.title, "Добавлено к подписке: 7\u{a0}дней");
    assert_eq!(
        days_item.actor,
        ActorView::Admin {
            id: anna,
            email: Some("anna@studio.example".into()),
            role: Some("Администратор".into()),
        }
    );
    let target = days_item.target.clone().unwrap();
    assert_eq!(target.label, "@maria_k");
    assert_eq!(target.client_id, Some(maria));
    assert_eq!(
        target.link.as_deref(),
        Some(format!("/admin/clients/{maria}").as_str())
    );
    assert!(
        days_item
            .fields
            .iter()
            .any(|f| f.label.as_deref() == Some("Причина"))
    );
    let tariff_item = page.items.iter().find(|i| i.id == tariff).unwrap();
    assert_eq!(
        tariff_item.target.as_ref().unwrap().label,
        "Тариф «Базовый»"
    );
    ip = page.items.iter().find(|i| i.id == ip).unwrap().id;
    assert_eq!(
        services
            .audit_journal()
            .entry(ip)
            .await
            .unwrap()
            .ip
            .as_deref(),
        Some("85.143.12.40")
    );
    db.cleanup().await;
}

#[tokio::test]
async fn секреты_в_подробностях_не_показываются() {
    let Some((db, services)) = setup().await else {
        return;
    };
    let owner = admin(&services, "owner@studio.example", AdminRole::Owner).await;
    let secrets = [
        "hunter2-очень-тайный",
        "7400000123:AAH4kq9Zx_wPq3eLr8TmV2nYc5uJb1oXsDf",
        "snk_4f1c2b9a8e7d6c5b4a3928170615f4e3d2c1b0a99887766",
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJvd25lciJ9.c2lnbmF0dXJlLXZhbHVlLXRlc3Q",
        "Zk3pQ9vX2mL7rT1wY8bN4cJ6hD0sF5gA",
        "s3cr3t-pa55",
        "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    ];
    let id = row(
        &services,
        base(),
        ("admin", Some(owner)),
        "settings.provider",
        None,
        json!({
            "provider": "yookassa",
            "password": secrets[0],
            "bot_token": secrets[1],
            "note": format!("ключ {} и токен {}", secrets[2], secrets[3]),
            "config": { "shop_id": "502337", "secret_key": secrets[4], "list": [secrets[4]] },
            "webhook": format!("https://bot:{}@hooks.example/notify", secrets[5]),
            "header": format!("Bearer {}", secrets[6]),
            "secrets": ["secret_key"],
            "fields": ["shop_id"],
            "password_failures": 3,
            "page_config": "c34c0808-3359-4c9a-a488-06bef541c66b",
        }),
    )
    .await;
    let item = services.audit_journal().entry(id).await.unwrap();
    let text = serde_json::to_string(&item).unwrap();
    for secret in secrets {
        assert!(!text.contains(secret), "секрет виден: {secret}\n{text}");
    }
    let field = |key: &str| {
        item.fields
            .iter()
            .find(|f| f.key == key)
            .unwrap()
            .value
            .clone()
    };
    assert_eq!(field("password"), scrub::HIDDEN);
    assert_eq!(field("bot_token"), scrub::HIDDEN);
    assert_eq!(field("secrets"), "secret_key");
    assert_eq!(field("fields"), "shop_id");
    assert_eq!(field("password_failures"), "3");
    assert_eq!(field("page_config"), "c34c0808-3359-4c9a-a488-06bef541c66b");
    assert!(field("config").contains("502337"));
    assert!(field("webhook").contains("hooks.example"));
    assert_eq!(field("header"), format!("Bearer {}", scrub::HIDDEN));
    assert_eq!(
        field("note"),
        format!("ключ {0} и токен {0}", scrub::HIDDEN)
    );
    for q in ["hunter2", "AAH4kq9Zx", "s3cr3t-pa55", "wJalrXUtn"] {
        let page = services
            .audit_journal()
            .page(&JournalQuery {
                q: Some(q.into()),
                ..JournalQuery::default()
            })
            .await
            .unwrap();
        assert!(
            page.items.is_empty() && page.next.is_none(),
            "поиск нашёл скрытое: {q}"
        );
    }
    let page = services
        .audit_journal()
        .page(&JournalQuery {
            q: Some("502337".into()),
            ..JournalQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(ids(&page.items), vec![id]);
    db.cleanup().await;
}

#[tokio::test]
async fn ключ_api_и_внешний_модуль_видны_как_участники() {
    let Some((db, services)) = setup().await else {
        return;
    };
    let owner = admin(&services, "owner@studio.example", AdminRole::Owner).await;
    let maria = client(&services, 7_400_000_201, "maria_k").await;
    let (crm, polls) = {
        let mut conn = services.store().conn().await.unwrap();
        let key = |name: &str, hash: u8| NewApiKey {
            name: name.into(),
            token_prefix: "snk_0000".into(),
            token_hash: vec![hash; 32],
            permissions: vec![Permission::ClientsWrite],
            created_by: sn_domain::ids::AdminId(owner),
            expires_at: None,
        };
        let crm = conn
            .api_keys()
            .create(&key("CRM-выгрузка", 1))
            .await
            .unwrap()
            .id;
        let polls = conn
            .api_keys()
            .create(&key("Модуль «Опросы после оплаты»", 2))
            .await
            .unwrap()
            .id;
        sqlx::query(
            "INSERT INTO ext_modules (id, manifest, api_key_id, installed_by) VALUES ('polls', $1, $2, $3)",
        )
        .bind(json!({ "name": "Опросы после оплаты" }))
        .bind(polls)
        .bind(owner)
        .execute(conn.conn())
        .await
        .unwrap();
        (crm, polls)
    };
    let write = |key: Option<i64>, action: &'static str| {
        let services = &services;
        async move {
            sn_store::with_api_key(key, async {
                let mut conn = services.store().conn().await.unwrap();
                conn.audit()
                    .record(&AuditEntry {
                        actor_kind: ActorKind::Admin,
                        actor_id: Some(owner),
                        action: action.into(),
                        entity_kind: Some("client".into()),
                        entity_id: Some(maria),
                        details: json!({}),
                        ip: None,
                    })
                    .await
                    .unwrap()
            })
            .await
        }
    };
    let by_key = write(Some(crm), "client.message").await;
    let by_module = write(Some(polls), "client.note").await;
    let by_owner = write(None, "client.block").await;
    let items = all(&services, JournalQuery::default()).await;
    let actor = |id: i64| items.iter().find(|i| i.id == id).unwrap().actor.clone();
    assert_eq!(
        actor(by_key),
        ActorView::ApiKey {
            id: crm,
            name: Some("CRM-выгрузка".into()),
            owner: Some("owner@studio.example".into()),
        }
    );
    assert_eq!(
        actor(by_module),
        ActorView::Module {
            id: "polls".into(),
            name: Some("Опросы после оплаты".into()),
        }
    );
    assert!(matches!(actor(by_owner), ActorView::Admin { id, .. } if id == owner));
    let via = |id: i64| items.iter().find(|i| i.id == id).unwrap().via_unknown;
    assert!(!via(by_key) && !via(by_module) && !via(by_owner));
    let (before, system) = {
        let mut conn = services.store().conn().await.unwrap();
        let old = |kind: &'static str, actor: Option<i64>| {
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO audit_log (actor_kind, actor_id, action, via_recorded) \
                 VALUES ($1, $2, 'client.note', false) RETURNING id",
            )
            .bind(kind)
            .bind(actor)
        };
        let before = old("admin", Some(owner))
            .fetch_one(conn.conn())
            .await
            .unwrap();
        let system = old("system", None).fetch_one(conn.conn()).await.unwrap();
        (before, system)
    };
    assert!(
        services
            .audit_journal()
            .entry(before)
            .await
            .unwrap()
            .via_unknown
    );
    assert!(
        !services
            .audit_journal()
            .entry(system)
            .await
            .unwrap()
            .via_unknown
    );
    let only = |actor: String| {
        let services = &services;
        async move {
            ids(&all(
                services,
                JournalQuery {
                    actor: Some(actor),
                    ..JournalQuery::default()
                },
            )
            .await)
        }
    };
    assert_eq!(only(format!("key:{crm}")).await, vec![by_key]);
    assert_eq!(only("module:polls".into()).await, vec![by_module]);
    assert_eq!(only(format!("admin:{owner}")).await, vec![before, by_owner]);
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query("DELETE FROM ext_modules WHERE id = 'polls'")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let items = all(&services, JournalQuery::default()).await;
    assert_eq!(
        items.iter().find(|i| i.id == by_module).unwrap().actor,
        ActorView::Module {
            id: "polls".into(),
            name: Some("Опросы после оплаты".into()),
        }
    );
    let meta = services.audit_journal().meta().await.unwrap();
    assert_eq!(
        meta.keys.iter().map(|k| k.id).collect::<Vec<_>>(),
        vec![crm]
    );
    assert_eq!(
        meta.modules,
        vec![MetaModule {
            id: "polls".into(),
            name: "Опросы после оплаты".into(),
        }]
    );
    assert!(
        meta.admins
            .iter()
            .any(|a| a.email == "owner@studio.example" && a.role == "Владелец")
    );
    assert_eq!(meta.areas.len(), Area::ALL.len());
    assert_eq!(
        meta.areas.iter().map(|a| a.actions.len()).sum::<usize>(),
        labels::ACTIONS.len()
    );
    db.cleanup().await;
}

#[test]
fn было_и_стало_собираются_из_подробностей() {
    let (changes, rest) = changes_of(
        &Names::default(),
        "settings.appearance",
        &json!({ "from": { "admin": "side", "admin_theme": "dark" }, "to": { "admin": "top", "admin_theme": "dark" } }),
    );
    assert_eq!(
        changes,
        vec![ChangeView {
            label: "Дизайн админки".into(),
            before: "Боковое меню".into(),
            after: "Верхнее меню".into()
        }]
    );
    assert!(rest.is_empty());
    let access = |on: bool| json!({ "miniapp": true, "miniapp_fullscreen": on, "menu_button": true, "site_login": true, "self_devices": true, "self_reissue": true });
    let (changes, rest) = changes_of(
        &Names::default(),
        "settings.client_access",
        &json!({ "from": access(false), "to": access(true) }),
    );
    assert_eq!(
        changes,
        vec![ChangeView {
            label: "Открывать на весь экран".into(),
            before: "нет".into(),
            after: "да".into()
        }]
    );
    assert!(rest.is_empty());
    for key in access(false).as_object().unwrap().keys() {
        assert!(
            labels::field_label_in("settings.client_access", key).is_some(),
            "{key}"
        );
    }
    let names = Names {
        providers: [("yookassa".to_string(), "ЮKassa".to_string())].into(),
        methods: [("sbp".to_string(), "СБП".to_string())].into(),
    };
    assert_eq!(text_of(&names, "provider", &json!("yookassa")), "ЮKassa");
    assert_eq!(
        text_of(&names, "methods", &json!(["sbp", "bank_card"])),
        "СБП, bank_card"
    );
    assert_eq!(
        text_of(&names, "channels", &json!(["bot", "miniapp"])),
        "бот, мини-приложение"
    );
    let (changes, _) = changes_of(
        &Names::default(),
        "admin.role_changed",
        &json!({ "from": "support", "to": "admin" }),
    );
    assert_eq!(
        changes,
        vec![ChangeView {
            label: "Роль".into(),
            before: "Поддержка".into(),
            after: "Администратор".into()
        }]
    );
    let (changes, rest) = changes_of(
        &Names::default(),
        "subscription.change_limits",
        &json!({ "hwid_limit_before": 3, "hwid_limit": 5, "reason": "просьба" }),
    );
    assert_eq!(
        changes,
        vec![ChangeView {
            label: "Устройств".into(),
            before: "3".into(),
            after: "5".into()
        }]
    );
    assert_eq!(rest.keys().collect::<Vec<_>>(), vec!["reason"]);
    let (changes, _) = changes_of(
        &Names::default(),
        "client.email_set",
        &json!({ "previous": "old@example.com", "email": "new@example.com" }),
    );
    assert_eq!(changes[0].before, "old@example.com");
    assert_eq!(changes[0].after, "new@example.com");
    let (changes, rest) = changes_of(
        &Names::default(),
        "admin.permission_changed",
        &json!({ "permission": "audit_log", "on": true }),
    );
    assert_eq!(
        changes,
        vec![ChangeView {
            label: "Журнал действий".into(),
            before: "нет".into(),
            after: "есть".into()
        }]
    );
    assert!(rest.is_empty());
    assert_eq!(
        title_for(
            "admin.permission_changed",
            &json!({ "permission": "audit_log", "on": false })
        ),
        "Снято право «Журнал действий»"
    );
    assert_eq!(
        title_for("modules.toggle", &json!({ "enabled": true })),
        "Модуль включён"
    );
    assert_eq!(
        title_for("client.note", &json!({ "cleared": true })),
        "Удалена заметка о клиенте"
    );
    let kept: Vec<String> = fields_of(
        &Names::default(),
        "client.block",
        json!({ "reason": "Спам", "changed": true, "canceled_orders": [], "note": "", "total": 0, "error": null })
            .as_object()
            .cloned()
            .unwrap(),
    )
    .into_iter()
    .map(|f| f.key)
    .collect();
    assert_eq!(kept, vec!["reason", "total"]);
    assert_eq!(
        title_for("tariff.on_sale", &json!({ "on_sale": false })),
        "Тариф снят с продажи"
    );
    assert_eq!(
        title_for("subscription.add_days", &json!({ "days": 1 })),
        "Добавлено к подписке: 1\u{a0}день"
    );
    assert_eq!(
        text_of(
            &Names::default(),
            "amount",
            &json!({ "minor": 24900, "currency": "RUB" })
        ),
        format::money(Money::new(24900, Currency::Rub))
    );
    assert_eq!(
        text_of(
            &Names::default(),
            "permissions",
            &json!(["clients_read", "money"])
        ),
        "Клиенты: просмотр, Деньги"
    );
}
