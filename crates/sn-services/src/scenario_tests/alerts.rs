use super::*;
use crate::alerts::{AlertsUpdate, RouteInput};
use sn_domain::alerts::AlertKind;
use sn_domain::ids::TicketId;
use sn_modules::catalog::antibot::{AntibotModule, captcha};

async fn withdrawal(services: &Services, client: ClientId, minor: i64) {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query(
        "INSERT INTO referral_withdrawals (client_id, amount_minor, currency, method, details) \
         VALUES ($1, $2, 'USD', 'usdt_trc20', 'TXYZ')",
    )
    .bind(client.0)
    .bind(minor)
    .execute(conn.conn())
    .await
    .unwrap();
}

async fn ticket(services: &Services, client: ClientId, body: &str) -> TicketId {
    services
        .support()
        .open(
            client,
            &OpenTicket {
                subscription_id: None,
                channel: TicketChannel::Cabinet,
                subject: None,
                body: body.into(),
            },
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn уведомления_собираются_только_о_новом_и_с_лимитом() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let alerts = services.alerts();
    let client = telegram_client(services, 7_301).await;
    ticket(services, client, "Старое обращение").await;
    withdrawal(services, client, 1500).await;
    assert!(alerts.collect().await.unwrap().is_empty());
    assert!(alerts.collect().await.unwrap().is_empty());

    let first = ticket(services, client, "Не подключается iPhone").await;
    let got = alerts.collect().await.unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].kind, AlertKind::Support);
    assert!(
        got[0]
            .html
            .contains(&format!("Новое обращение №{}", first.0)),
        "{}",
        got[0].html
    );
    assert!(got[0].html.contains("Алексей (@morozov_a)"));
    assert!(got[0].html.contains("«Не подключается iPhone»"));
    assert_eq!(got[0].path, format!("/admin/support?ticket={}", first.0));

    services
        .support()
        .reply(
            first,
            &ReplyRequest {
                body: "<b>не</b> работает & всё".into(),
                close: false,
            },
            Actor::Client(client),
        )
        .await
        .unwrap();
    let got = alerts.collect().await.unwrap();
    assert_eq!(got.len(), 1);
    assert!(got[0].html.contains("Новое сообщение в обращении"));
    assert!(
        got[0]
            .html
            .contains("&lt;b&gt;не&lt;/b&gt; работает &amp; всё"),
        "{}",
        got[0].html
    );

    for minor in [1000, 2000, 3000, 4000, 5000] {
        withdrawal(services, client, minor).await;
    }
    let got = alerts.collect().await.unwrap();
    assert_eq!(got.len(), 4, "{got:?}");
    assert!(got.iter().all(|a| a.kind == AlertKind::Withdrawals));
    assert!(got[0].html.contains("Заявка на вывод"));
    assert!(
        got[0].html.contains("$10.00, USDT (TRC20)"),
        "{}",
        got[0].html
    );
    assert_eq!(got[3].html, "Ещё 2 заявки на вывод");
    assert!(alerts.collect().await.unwrap().is_empty());
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query(
            "INSERT INTO referral_withdrawals (client_id, amount_minor, currency, method, method_title, details) \
             VALUES ($1, 700, 'USD', 'paypal', 'PayPal <почта>', 'me@example.com')",
        )
        .bind(client.0)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let got = alerts.collect().await.unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].html.contains("$7.00, PayPal &lt;почта&gt;"),
        "{}",
        got[0].html
    );

    let admin = owner(services).await;
    alerts
        .update(
            &AlertsUpdate {
                routes: vec![RouteInput {
                    kind: AlertKind::Support,
                    on: false,
                    topic: None,
                }],
            },
            admin,
        )
        .await
        .unwrap();
    ticket(services, client, "Пока уведомления выключены").await;
    assert!(alerts.collect().await.unwrap().is_empty());
    alerts
        .update(
            &AlertsUpdate {
                routes: vec![RouteInput {
                    kind: AlertKind::Support,
                    on: true,
                    topic: Some("  Входящие  ".into()),
                }],
            },
            admin,
        )
        .await
        .unwrap();
    assert!(alerts.collect().await.unwrap().is_empty());
    let view = alerts.view().await.unwrap();
    let support = view
        .kinds
        .iter()
        .find(|k| k.kind == AlertKind::Support)
        .unwrap();
    assert_eq!(support.topic.as_deref(), Some("Входящие"));
    env.db.cleanup().await;
}

#[tokio::test]
async fn группа_подключается_одноразовым_кодом() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let alerts = services.alerts();
    let admin = owner(services).await;
    assert_eq!(code(alerts.link().await), "alerts.no_bot");
    assert_eq!(code(alerts.request_test().await), "alerts.no_group");
    set_bot(services).await;
    let link = alerts.link().await.unwrap();
    let code_part = link
        .link
        .strip_prefix("https://t.me/stealth_vpn_bot?startgroup=alerts_")
        .and_then(|rest| rest.strip_suffix("&admin=manage_topics"))
        .unwrap()
        .to_string();
    assert_eq!(code_part.len(), 24);
    assert!(
        !alerts
            .bind_group("ffffffffffffffffffffffff", -100_500, None, true)
            .await
            .unwrap()
    );
    assert!(
        alerts
            .bind_group(&code_part, -100_500, Some("Админы".into()), true)
            .await
            .unwrap()
    );
    assert!(
        !alerts
            .bind_group(&code_part, -100_777, None, true)
            .await
            .unwrap()
    );
    let view = alerts.view().await.unwrap();
    assert_eq!(view.chat_id, Some(-100_500));
    assert_eq!(view.title.as_deref(), Some("Админы"));
    assert!(view.forum);
    alerts.remember_topic("Поддержка", 77).await.unwrap();
    assert_eq!(alerts.topics().await.unwrap().get("Поддержка"), Some(&77));
    alerts.forget_topic("Поддержка").await.unwrap();
    assert!(alerts.topics().await.unwrap().is_empty());
    alerts.request_test().await.unwrap();
    assert!(alerts.view().await.unwrap().test_pending);
    assert!(alerts.take_test().await.unwrap());
    assert!(!alerts.take_test().await.unwrap());
    alerts.remember_topic("Звёзды", 5).await.unwrap();
    let view = alerts.unlink(admin).await.unwrap();
    assert_eq!(view.chat_id, None);
    assert!(alerts.topics().await.unwrap().is_empty());
    env.db.cleanup().await;
}

#[tokio::test]
async fn заказы_и_звёзды_на_разборе_попадают_в_уведомления() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let alerts = services.alerts();
    let client = telegram_client(services, 7_302).await;
    assert!(alerts.collect().await.unwrap().is_empty());
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query(
            "INSERT INTO orders (client_id, kind, status, intent, quote, currency, total_minor, review_reason) \
             VALUES ($1, 'renewal', 'review', '{}', '{}', 'RUB', 49900, 'Remnawave не ответила')",
        )
        .bind(client.0)
        .execute(conn.conn())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO bot_star_events (charge_id, event, payload, source, status, next_attempt_at, last_error) \
             VALUES ('ch-1', 'successful_payment', '{\"total_amount\": 250}', 'update', 'review', now(), 'счёт не найден')",
        )
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let got = alerts.collect().await.unwrap();
    assert_eq!(got.len(), 2, "{got:?}");
    let order = got.iter().find(|a| a.kind == AlertKind::Orders).unwrap();
    assert!(order.html.contains("ждёт ручной проверки"));
    assert!(order.html.contains("499"), "{}", order.html);
    assert!(order.html.contains("Причина: Remnawave не ответила"));
    let stars = got.iter().find(|a| a.kind == AlertKind::Stars).unwrap();
    assert!(stars.html.contains("Сумма: 250 звёзд"), "{}", stars.html);
    assert!(stars.html.contains("Причина: счёт не найден"));
    assert!(alerts.collect().await.unwrap().is_empty());
    env.db.cleanup().await;
}

async fn notice(
    services: &Services,
    provider: &str,
    event: &str,
    kind: sn_domain::payment::NoticeKind,
    client: Option<ClientId>,
    minor: Option<i64>,
    text: &str,
) {
    let mut conn = services.store().conn().await.unwrap();
    conn.payment_notices()
        .record(&sn_store::NewPaymentNotice {
            provider: provider.into(),
            event_key: event.into(),
            kind,
            payment_id: None,
            client_id: client,
            amount: minor
                .map(|m| sn_domain::money::Amount::new(m, sn_domain::money::PayCurrency::Rub)),
            text: text.into(),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn споры_и_возвраты_платёжек_приходят_отдельной_темой() {
    use sn_domain::payment::NoticeKind;
    let Some(env) = env().await else { return };
    let services = &env.services;
    let alerts = services.alerts();
    let admin = owner(services).await;
    let client = telegram_client(services, 7_310).await;
    notice(
        services,
        providers::PLATEGA,
        "old:DISPUTED",
        NoticeKind::Dispute,
        Some(client),
        Some(49_900),
        "Старый спор до подключения уведомлений",
    )
    .await;
    assert!(alerts.collect().await.unwrap().is_empty());
    notice(
        services,
        providers::PLATEGA,
        "tx-1:DISPUTED",
        NoticeKind::Dispute,
        Some(client),
        Some(49_900),
        "Platega открыла спор по платежу tx-1",
    )
    .await;
    notice(
        services,
        providers::LAVA,
        "refund:r-1",
        NoticeKind::UnmatchedRefund,
        None,
        None,
        "Lava.top сообщила о возврате <без суммы>",
    )
    .await;
    let got = alerts.collect().await.unwrap();
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got.iter().all(|a| a.kind == AlertKind::Payments));
    assert!(
        got[0].html.contains("<b>Спор по платежу</b>"),
        "{}",
        got[0].html
    );
    assert!(
        got[0].html.contains("Алексей (@morozov_a)"),
        "{}",
        got[0].html
    );
    assert!(got[0].html.contains("499"), "{}", got[0].html);
    assert!(got[0].html.contains("Platega"), "{}", got[0].html);
    assert_eq!(got[0].path, format!("/admin/clients/{}", client.0));
    assert!(got[1].html.contains("Возврат не привязан к оплате"));
    assert!(got[1].html.contains("&lt;без суммы&gt;"), "{}", got[1].html);
    assert_eq!(got[1].path, "/admin/money");
    assert!(alerts.collect().await.unwrap().is_empty());
    alerts
        .update(
            &AlertsUpdate {
                routes: vec![RouteInput {
                    kind: AlertKind::Payments,
                    on: false,
                    topic: None,
                }],
            },
            admin,
        )
        .await
        .unwrap();
    notice(
        services,
        providers::LAVA,
        "chargeback:cb-1",
        NoticeKind::Chargeback,
        None,
        Some(69_900),
        "Чарджбэк",
    )
    .await;
    assert!(alerts.collect().await.unwrap().is_empty());
    let view = alerts.view().await.unwrap();
    let payments = view
        .kinds
        .iter()
        .find(|k| k.kind == AlertKind::Payments)
        .unwrap();
    assert!(!payments.on);
    assert_eq!(payments.default_topic, "Платежи");
    env.db.cleanup().await;
}

fn security_alerts(got: &[crate::alerts::Alert]) -> Vec<crate::alerts::Alert> {
    got.iter()
        .filter(|a| a.kind == AlertKind::Security)
        .cloned()
        .collect()
}

#[tokio::test]
async fn блокировка_входа_по_коду_приходит_админам_в_telegram() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let alerts = services.alerts();
    let auth = services.auth();
    let admin = owner(services).await;
    let setup = auth.admin_totp_setup(admin).await.unwrap();
    let now = services.now();
    auth.admin_totp_enable(
        admin,
        &setup.secret,
        &totp_code(&setup.secret, now),
        PASSWORD,
    )
    .await
    .unwrap();
    assert!(alerts.collect().await.unwrap().is_empty());
    let wrong = (0..10u32)
        .map(|d| d.to_string().repeat(6))
        .find(|c| {
            [now, now + Duration::minutes(31)]
                .iter()
                .all(|at| totp::verify(&setup.secret, c, at.timestamp()).is_none())
        })
        .unwrap();
    let guess = |ip: &'static str| {
        let wrong = wrong.clone();
        async move {
            for _ in 0..rules::ADMIN_TOTP_FAILURES.limit {
                assert_eq!(
                    code(
                        services
                            .auth()
                            .admin_login("owner@example.com", PASSWORD, Some(&wrong), &meta(ip))
                            .await
                    ),
                    TOTP_INVALID
                );
            }
        }
    };
    guess("198.51.100.7").await;
    let got = security_alerts(&alerts.collect().await.unwrap());
    assert_eq!(got.len(), 1, "{got:?}");
    let html = &got[0].html;
    assert!(
        html.starts_with(
            "<b>Вход по коду из приложения закрыт</b>\nАдминистратор: owner@example.com\n"
        ),
        "{html}"
    );
    assert!(
        html.contains("5 неверных кодов за 15 минут, пароль при этом ввели верно"),
        "{html}"
    );
    assert!(html.contains("Адрес: 198.51.100.7"), "{html}");
    assert!(html.contains("смените его"), "{html}");
    assert_eq!(got[0].path, "/admin/settings/admins");
    assert!(security_alerts(&alerts.collect().await.unwrap()).is_empty());

    alerts
        .update(
            &AlertsUpdate {
                routes: vec![RouteInput {
                    kind: AlertKind::Security,
                    on: false,
                    topic: None,
                }],
            },
            admin,
        )
        .await
        .unwrap();
    env.clock.set(now + Duration::minutes(31));
    guess("198.51.100.8").await;
    assert!(security_alerts(&alerts.collect().await.unwrap()).is_empty());
    alerts
        .update(
            &AlertsUpdate {
                routes: vec![RouteInput {
                    kind: AlertKind::Security,
                    on: true,
                    topic: None,
                }],
            },
            admin,
        )
        .await
        .unwrap();
    assert!(security_alerts(&alerts.collect().await.unwrap()).is_empty());

    {
        let mut conn = services.store().conn().await.unwrap();
        for _ in 0..4 {
            sqlx::query(
                "INSERT INTO audit_log (actor_kind, action, entity_kind, entity_id, details) \
                 VALUES ('system', $1, 'admin', $2, $3)",
            )
            .bind(crate::auth::TOTP_LOCKED_AUDIT)
            .bind(9_999_i64)
            .bind(serde_json::json!({
                "failures": 20,
                "rule": "admin_totp_failures_day",
                "window_seconds": 86_400,
            }))
            .execute(conn.conn())
            .await
            .unwrap();
        }
    }
    let got = security_alerts(&alerts.collect().await.unwrap());
    assert_eq!(got.len(), 4, "{got:?}");
    assert!(
        got[0].html.contains("Администратор: администратор №9999"),
        "{}",
        got[0].html
    );
    assert!(
        got[0].html.contains("20 неверных кодов за сутки"),
        "{}",
        got[0].html
    );
    assert!(!got[0].html.contains("Адрес:"), "{}", got[0].html);
    assert_eq!(got[3].html, "Ещё 1 блокировка входа по коду");
    assert_eq!(got[3].path, "/admin/settings/admins");
    env.db.cleanup().await;
}

async fn charge_on_review(services: &Services, client: ClientId, minor: i64, reason: &str) -> i64 {
    let mut conn = services.store().conn().await.unwrap();
    let order: i64 = sqlx::query_scalar(
        "INSERT INTO orders (client_id, kind, status, intent, quote, currency, total_minor) \
         VALUES ($1, 'renewal', 'pending', '{}', '{}', 'USD', $2) RETURNING id",
    )
    .bind(client.0)
    .bind(minor)
    .fetch_one(conn.conn())
    .await
    .unwrap();
    let payment: i64 = sqlx::query_scalar(
        "INSERT INTO payments (order_id, client_id, provider, method, amount_minor, currency, expires_at) \
         VALUES ($1, $2, 'yookassa', 'autorenew', $3, 'USD', now() + interval '1 day') RETURNING id",
    )
    .bind(order)
    .bind(client.0)
    .bind(minor)
    .fetch_one(conn.conn())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO jobs (kind, dedupe_key, payload, status, last_error) \
         VALUES ('poll_payment', $1, $2, 'review', $3)",
    )
    .bind(format!("poll:{payment}"))
    .bind(serde_json::json!({ "kind": "poll_payment", "payment_id": payment }))
    .bind(reason)
    .execute(conn.conn())
    .await
    .unwrap();
    payment
}

fn orders_route(on: bool) -> AlertsUpdate {
    AlertsUpdate {
        routes: vec![RouteInput {
            kind: AlertKind::Orders,
            on,
            topic: None,
        }],
    }
}

#[tokio::test]
async fn списания_с_карты_на_проверке_приходят_в_тему_заказов_один_раз() {
    let Some(env) = env().await else { return };
    let services = &env.services;
    let admin = owner(services).await;
    let client = telegram_client(services, 7_320).await;
    charge_on_review(services, client, 299, "старое списание").await;
    assert!(services.alerts().collect().await.unwrap().is_empty());
    let fresh = charge_on_review(
        services,
        client,
        499,
        "исход автосписания так и не выяснен: проверьте платёж в кабинете кассы",
    )
    .await;
    let got = services.alerts().collect().await.unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].kind, AlertKind::Orders);
    assert!(
        got[0].html.starts_with(&format!(
            "<b>Списание с карты №{fresh} ждёт ручной проверки</b>\nКлиент: Алексей (@morozov_a)\nСумма: $4.99\n"
        )),
        "{}",
        got[0].html
    );
    assert!(
        got[0].html.ends_with(
            "Причина: исход автосписания так и не выяснен: проверьте платёж в кабинете кассы"
        ),
        "{}",
        got[0].html
    );
    assert_eq!(got[0].path, "/admin/money#todo-charges");
    assert!(services.alerts().collect().await.unwrap().is_empty());

    for minor in [100, 200, 300, 400] {
        charge_on_review(services, client, minor, "касса не отдаёт статус").await;
    }
    let got = services.alerts().collect().await.unwrap();
    assert_eq!(got.len(), 4, "{got:?}");
    assert!(got.iter().all(|a| a.kind == AlertKind::Orders));
    assert!(got[0].html.contains("Сумма: $1.00"), "{}", got[0].html);
    assert_eq!(got[3].html, "Ещё 1 списание с карты ждёт ручной проверки");
    assert_eq!(got[3].path, "/admin/money#todo-charges");

    services
        .alerts()
        .update(&orders_route(false), admin)
        .await
        .unwrap();
    charge_on_review(services, client, 700, "касса не отдаёт статус").await;
    assert!(services.alerts().collect().await.unwrap().is_empty());
    services
        .alerts()
        .update(&orders_route(true), admin)
        .await
        .unwrap();
    assert!(services.alerts().collect().await.unwrap().is_empty());
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query(
            "INSERT INTO orders (client_id, kind, status, intent, quote, currency, total_minor, review_reason) \
             VALUES ($1, 'renewal', 'review', '{}', '{}', 'USD', 299, 'Remnawave не ответила')",
        )
        .bind(client.0)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let got = services.alerts().collect().await.unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].html.contains("ждёт ручной проверки"));
    assert_eq!(got[0].path, "/admin/money#todo-orders");
    env.db.cleanup().await;
}

async fn captcha_failed(
    services: &Services,
    at: DateTime<Utc>,
    problem: &str,
    foreign: Option<&str>,
) {
    let mut conn = services.store().conn().await.unwrap();
    conn.bot_state()
        .set(
            captcha::HEALTH_KEY,
            &serde_json::json!(captcha::Health {
                at,
                provider: captcha::Provider::Turnstile,
                problem: problem.into(),
                foreign: foreign.map(str::to_string),
            }),
        )
        .await
        .unwrap();
}

async fn captcha_alerts(services: &Services) -> Vec<String> {
    services
        .alerts()
        .collect()
        .await
        .unwrap()
        .into_iter()
        .filter(|alert| alert.kind == AlertKind::Captcha)
        .map(|alert| {
            assert_eq!(alert.path, "/admin/clients/antibot");
            alert.html
        })
        .collect()
}

#[tokio::test]
async fn сбой_капчи_приходит_в_группу_не_чаще_раза_в_час() {
    let modules: Vec<Arc<dyn Module>> = vec![Arc::new(AntibotModule)];
    let Some(env) = env_with(ModuleRegistry::new(modules).unwrap()).await else {
        return;
    };
    let services = &env.services;
    let admin = owner(services).await;
    let shown = || services.alerts().kinds().contains(&AlertKind::Captcha);
    let at = |minutes: i64| start() + Duration::minutes(minutes);
    assert!(!shown());
    services
        .settings_ops()
        .toggle_module(ModuleId::Antibot, true, admin)
        .await
        .unwrap();
    assert!(!shown());
    captcha_failed(services, at(0), "сервис проверки не ответил вовремя", None).await;
    assert!(captcha_alerts(services).await.is_empty());
    services
        .settings_ops()
        .update_module_config(
            ModuleId::Antibot,
            &ModuleConfigUpdate {
                config: serde_json::json!({
                    "captcha": "turnstile",
                    "captcha_site_key": "0x4AAAAAAAsite",
                    "captcha_secret": "0x4AAAAAAAsecret-key",
                })
                .as_object()
                .cloned()
                .unwrap(),
                clear: Vec::new(),
            },
            admin,
        )
        .await
        .unwrap();
    assert!(shown());
    let first = captcha_alerts(services).await;
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(first[0].contains("<b>Капча антибота не работает</b>"));
    assert!(first[0].contains("Cloudflare Turnstile: сервис проверки не ответил вовремя"));
    assert!(first[0].contains("не чаще раза в час"));
    assert!(captcha_alerts(services).await.is_empty());
    env.clock.set(at(30));
    captcha_failed(services, at(30), "секретный ключ не подошёл", None).await;
    assert!(captcha_alerts(services).await.is_empty());
    env.clock.set(at(59));
    captcha_failed(services, at(59), "секретный ключ не подошёл", None).await;
    assert!(captcha_alerts(services).await.is_empty());
    env.clock.set(at(60));
    let second = captcha_alerts(services).await;
    assert_eq!(second.len(), 1, "{second:?}");
    assert!(second[0].contains("секретный ключ не подошёл"));
    env.clock.set(at(121));
    assert!(
        captcha_alerts(services).await.is_empty(),
        "о сбое часовой давности не напоминаем"
    );
    captcha_failed(
        services,
        at(121),
        "ответ получен на домене <evil>.test, его нет в поле «Домены»",
        Some("<evil>.test"),
    )
    .await;
    services
        .alerts()
        .update(
            &AlertsUpdate {
                routes: vec![RouteInput {
                    kind: AlertKind::Captcha,
                    on: false,
                    topic: Some("Капча".into()),
                }],
            },
            admin,
        )
        .await
        .unwrap();
    assert!(captcha_alerts(services).await.is_empty());
    services
        .alerts()
        .update(
            &AlertsUpdate {
                routes: vec![RouteInput {
                    kind: AlertKind::Captcha,
                    on: true,
                    topic: Some("Капча".into()),
                }],
            },
            admin,
        )
        .await
        .unwrap();
    let foreign = captcha_alerts(services).await;
    assert_eq!(foreign.len(), 1, "{foreign:?}");
    assert!(foreign[0].contains("<b>Капча пройдена на чужом домене</b>"));
    assert!(
        foreign[0].contains("домена &lt;evil&gt;.test нет в поле «Домены»"),
        "{}",
        foreign[0]
    );
    env.clock.set(at(300));
    captcha_failed(services, at(300), "сервис проверки недоступен", None).await;
    services
        .settings_ops()
        .toggle_module(ModuleId::Antibot, false, admin)
        .await
        .unwrap();
    assert!(!shown());
    assert!(captcha_alerts(services).await.is_empty());
    env.db.cleanup().await;
}
