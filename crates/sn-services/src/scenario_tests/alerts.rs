use super::*;
use crate::alerts::{AlertsUpdate, RouteInput};
use sn_domain::alerts::AlertKind;
use sn_domain::ids::TicketId;

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
