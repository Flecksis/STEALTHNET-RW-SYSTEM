use super::money::scene;
use super::*;
use sn_store::testing::test_store;

#[tokio::test]
async fn продажи_переходы_допы_промокоды_и_призы() {
    let Some(db) = test_store().await else {
        return;
    };
    let s = scene(db.store.clone()).await;
    let seed = &s.seed;
    let sa: i64 = seed
        .scalar(&format!(
            "SELECT id FROM subscriptions WHERE client_id = {}",
            s.a
        ))
        .await;
    seed.buy(Buy {
        client: s.a,
        subscription: Some(sa),
        kind: "change",
        at: utc(2026, 3, 20, 9),
        tariff: Some(s.pro),
        total: 50_000,
        amount: 50_000,
        ..Buy::default()
    })
    .await;
    let order: i64 = seed
        .scalar(&format!(
            "SELECT min(id) FROM orders WHERE client_id = {}",
            s.a
        ))
        .await;
    seed.exec(&format!(
        "INSERT INTO promo_codes (id, code, kind, percent) OVERRIDING SYSTEM VALUE VALUES (900, 'AUTUMN', 'percent', 17); \
         INSERT INTO promo_redemptions (promo_id, client_id, order_id, status, used_at) VALUES (900, {a}, {order}, 'used', '2026-03-02 09:00+00'); \
         INSERT INTO balance_entries (client_id, wallet, currency, delta_minor, balance_after_minor, kind, created_at) \
           VALUES ({a}, 'main', 'RUB', 3000, 3000, 'contest', '2026-03-10 10:00+00')",
        a = s.a
    ))
    .await;
    let r = ctx_report(&db.store, "tariffs", month(3, Compare::None)).await;
    assert_eq!(value(&r, "sales"), 3.0);
    let rows = table(&r, "sales");
    assert_eq!(
        rows[0],
        (
            "Про".to_string(),
            vec![Some(2.0), Some(149_000.0), Some(74_500.0)]
        )
    );
    assert_eq!(
        rows[1],
        (
            "Старт".to_string(),
            vec![Some(1.0), Some(25_000.0), Some(25_000.0)]
        )
    );
    assert_eq!(value(&r, "changes"), 1.0);
    assert_eq!(table(&r, "changes")[0].0, "Старт → Про");
    assert_eq!(value(&r, "addon_devices"), 15_000.0);
    assert_eq!(value(&r, "addon_traffic"), 0.0);
    assert_eq!(value(&r, "promocodes"), 1.0);
    assert_eq!(
        table(&r, "promocodes")[0],
        (
            "AUTUMN".to_string(),
            vec![Some(1.0), Some(5_000.0), Some(25_000.0), Some(0.0)]
        )
    );
    assert_eq!(value(&r, "prizes"), 3_000.0);
    assert_eq!(value(&r, "gifts_sold"), 0.0);
}
