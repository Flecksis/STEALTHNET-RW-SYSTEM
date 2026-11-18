use super::*;
use sn_payments::providers::cryptopay_signature;

const TOKEN: &str = "4321:TEST";

async fn post(state: &AppState, path: &str, signature: Option<String>, body: &str) -> StatusCode {
    let mut request = Request::post(path);
    if let Some(signature) = signature {
        request = request.header("crypto-pay-api-signature", signature);
    }
    router(state.clone())
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
        .status()
}

fn paid(payload: &str) -> String {
    json!({
        "update_id": 31, "update_type": "invoice_paid", "request_date": "2026-11-16T15:00:00.000Z",
        "payload": {"invoice_id": 9031, "status": "paid", "currency_type": "fiat", "fiat": "USD",
                    "amount": "5.00", "payload": payload}
    })
    .to_string()
}

#[tokio::test]
async fn старые_адреса_уведомлений_и_счета_старой_панели() {
    let Some(http) = http().await else { return };
    let s = http.services();
    {
        let mut conn = http.db.store.conn().await.unwrap();
        conn.settings()
            .set(
                &keys::provider(providers::CRYPTOPAY),
                &json!({ "enabled": true }),
                None,
            )
            .await
            .unwrap();
        sn_services::settings::write_secret(
            &mut conn,
            s.secrets(),
            &sn_domain::settings::secret_keys::provider(providers::CRYPTOPAY, "api_token"),
            Some(TOKEN),
            None,
        )
        .await
        .unwrap();
        let client = conn
            .clients()
            .create(&sn_store::NewClient {
                locale: sn_domain::client::Locale::Ru,
                registration_source: RegistrationSource::Migration,
                note: None,
            })
            .await
            .unwrap()
            .id;
        let intent = serde_json::to_value(PurchaseIntent::TopUp {
            amount: sn_domain::money::Money::new(500, Currency::Usd),
        })
        .unwrap();
        sqlx::query(
            "INSERT INTO legacy_invoices (source, old_id, provider, old_provider, order_ref, client_id, intent, \
             label, amount_minor, currency, pay_url, created_at) \
             VALUES ('stealthnet', 'cmold1', 'cryptopay', 'cryptopay', 'ord-old-1', $1, $2, 'Пополнение баланса', \
             500, 'USD', 'https://pay.example.com/old', now())",
        )
        .bind(client.0)
        .bind(intent)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    s.reload().await.unwrap();
    s.set_remna(Some(http.remna.clone()));
    let state = &http.state;

    let (status, headers, _) = raw(state, "/api/pay/ord-old-1").await;
    assert!(status.is_redirection(), "{status}");
    assert_eq!(
        headers.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("https://pay.example.com/old")
    );

    let body = paid("cmold1");
    let forged = post(state, "/api/webhooks/cryptopay", Some("00".into()), &body).await;
    assert_eq!(forged, StatusCode::UNAUTHORIZED);
    let signature = cryptopay_signature(TOKEN, body.as_bytes());
    assert_eq!(
        post(
            state,
            "/api/webhooks/cryptopay",
            Some(signature.clone()),
            &body
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        post(state, "/api/webhooks/cryptopay", Some(signature), &body).await,
        StatusCode::OK
    );
    let mut conn = http.db.store.conn().await.unwrap();
    let balance: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(amount_minor), 0)::bigint FROM client_balances WHERE currency = 'USD'",
    )
    .fetch_one(conn.conn())
    .await
    .unwrap();
    assert_eq!(balance, 500, "повтор не зачисляет второй раз");
    let marked: Vec<(String, bool, bool)> = sqlx::query_as(
        "SELECT outcome, legacy_path, legacy_invoice_id IS NOT NULL FROM webhook_inbox ORDER BY id",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap();
    assert_eq!(
        marked,
        [
            ("rejected_signature".to_string(), true, false),
            ("accepted".to_string(), true, true)
        ]
    );
    drop(conn);
    let (status, _, _) = raw(state, "/api/pay/ord-old-1").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "оплаченный счёт больше не открывается"
    );

    for path in [
        "/api/webhooks/unknownpay",
        "/api/webhooks/lava",
        "/api/webhooks/nalog",
    ] {
        assert_eq!(
            post(state, path, None, "{}").await,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    assert_eq!(
        post(state, "/api/webhooks/remna", None, "{}").await,
        StatusCode::SERVICE_UNAVAILABLE,
        "старый адрес Remnawave ведёт в обработчик Remnawave"
    );
    assert_eq!(
        post(state, "/api/webhooks/pay/cryptopay", None, &paid("cmold1")).await,
        StatusCode::UNAUTHORIZED
    );
}
