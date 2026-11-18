use super::money::scene;
use super::*;
use sn_store::testing::test_store;

#[tokio::test]
async fn регистрации_пробные_и_состояние_клиентов() {
    let Some(db) = test_store().await else {
        return;
    };
    let s = scene(db.store.clone()).await;
    let seed = &s.seed;
    seed.buy(Buy {
        client: s.f,
        kind: "trial",
        at: utc(2026, 3, 3, 10),
        days: 3,
        provider: "yookassa",
        amount: 0,
        ..Buy::default()
    })
    .await;
    let sf = seed.subscription(s.f, "paid", Some(s.start)).await;
    seed.buy(Buy {
        client: s.f,
        subscription: Some(sf),
        kind: "new",
        at: utc(2026, 3, 5, 10),
        tariff: Some(s.start),
        total: 30_000,
        amount: 30_000,
        ..Buy::default()
    })
    .await;
    seed.exec(&format!(
        "UPDATE clients SET status = 'blocked', blocked_at = '2026-03-10 10:00+00' WHERE id = {b}; \
         UPDATE clients SET locale = 'en' WHERE id = {e}; \
         INSERT INTO client_balances (client_id, wallet, currency, amount_minor) VALUES ({c}, 'main', 'RUB', 20000); \
         UPDATE subscriptions SET devices_count = 3, hwid_limit = 3, first_connected_at = '2026-03-02 10:00+00', \
           online_at = '2026-04-05 10:00+00' WHERE client_id = {a}; \
         UPDATE subscriptions SET traffic_used_bytes = 95 * 1073741824::bigint, traffic_limit_bytes = 100 * 1073741824::bigint, \
           first_connected_at = '2026-02-10 10:00+00', online_at = '2026-04-15 11:00+00' WHERE client_id = {b}",
        b = s.b,
        e = s.e,
        c = s.c,
        a = s.a
    ))
    .await;
    let r = ctx_report(&db.store, "clients", month(3, Compare::Prev)).await;
    assert_eq!(value(&r, "registrations"), 4.0);
    assert_eq!(metric(&r, "registrations").prev, Some(2.0));
    assert_eq!(
        breakdown(&r, "registrations", "channel"),
        vec![("bot".to_string(), 3.0), ("miniapp".to_string(), 1.0)]
    );
    assert_eq!(
        breakdown(&r, "registrations", "source"),
        vec![("none".to_string(), 3.0), ("utm".to_string(), 1.0)]
    );
    assert_eq!(value(&r, "trials"), 1.0);
    assert_eq!(value(&r, "trial_bought"), 100.0);
    assert_eq!(value(&r, "trial_wait"), 172_800.0);
    assert_eq!(value(&r, "active_subs"), 5.0);
    assert_eq!(value(&r, "devices"), 1.0);
    assert_eq!(value(&r, "traffic"), 1.0);
    assert_eq!(value(&r, "never"), 3.0);
    assert_eq!(
        metric(&r, "never").note.as_deref(),
        Some("не заходили 7 дней: 1, 30 дней: 0")
    );
    assert_eq!(value(&r, "balances"), 20_000.0);
    assert_eq!(value(&r, "blocked"), 1.0);
    assert_eq!(metric(&r, "blocked").note.as_deref(), Some("+1 за период"));
    assert_eq!(value(&r, "english"), 1.0);
    let q = filtered(month(3, Compare::None), |q| {
        q.source = Some("utm:vk".into())
    });
    let r = ctx_report(&db.store, "clients", q).await;
    assert_eq!(value(&r, "registrations"), 1.0);
    assert_eq!(value(&r, "english"), 1.0);
}
