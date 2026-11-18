use super::money::scene;
use super::*;
use sn_store::testing::test_store;

#[tokio::test]
async fn счета_оплаты_брошенные_и_ошибки() {
    let Some(db) = test_store().await else {
        return;
    };
    let s = scene(db.store.clone()).await;
    let seed = &s.seed;
    let order: i64 = seed
        .scalar(&format!(
            "SELECT min(id) FROM orders WHERE client_id = {}",
            s.a
        ))
        .await;
    seed.exec(&format!(
        "INSERT INTO payments (order_id, client_id, provider, pay_method, amount_minor, currency, status, created_at, failure_reason) \
           VALUES ({order}, {a}, 'yookassa', 'sbp', 25000, 'RUB', 'failed', '2026-03-07 10:00+00', 'Карта отклонена'); \
         INSERT INTO payments (order_id, client_id, provider, pay_method, amount_minor, currency, status, created_at, expires_at) \
           VALUES ({order}, {a}, 'yookassa', 'sbp', 25000, 'RUB', 'pending', '2026-03-07 11:00+00', '2026-03-08 11:00+00'); \
         INSERT INTO payments (order_id, client_id, provider, amount_minor, currency, status, created_at) \
           VALUES ({order}, {a}, 'yookassa', 25000, 'RUB', 'canceled', '2026-03-09 11:00+00')",
        a = s.a
    ))
    .await;
    let r = ctx_report(&db.store, "payments", month(3, Compare::None)).await;
    assert_eq!(value(&r, "invoices"), 8.0);
    assert_eq!(value(&r, "paid_share"), 62.5);
    assert_eq!(value(&r, "abandoned"), 2.0);
    assert_eq!(value(&r, "failed"), 1.0);
    assert_eq!(value(&r, "time_to_pay"), 180.0);
    let rows = table(&r, "invoices");
    let yk = rows.iter().find(|r| r.0 == "ЮKassa").unwrap();
    assert_eq!(yk.1[0], Some(75_000.0));
    assert_eq!(yk.1[2], Some(5.0));
    assert_eq!(yk.1[3], Some(2.0));
    assert_eq!(yk.1[5], Some(2.0));
    assert_eq!(yk.1[6], Some(1.0));
    let errors = table(&r, "failed");
    assert_eq!(errors[0], ("Карта отклонена".to_string(), vec![Some(1.0)]));
    let q = filtered(month(3, Compare::None), |q| {
        q.provider = Some("platega".into())
    });
    let r = ctx_report(&db.store, "payments", q).await;
    assert_eq!(value(&r, "invoices"), 1.0);
    assert_eq!(value(&r, "provider_refunds"), 10_000.0);
}
