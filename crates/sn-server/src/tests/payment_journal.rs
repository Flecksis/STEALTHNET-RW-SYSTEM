use super::*;
use sn_domain::payment::PaymentStatus;
use sn_payments::{PaymentRef, ProviderStatus, WebhookOutcome, WebhookReply};
use sn_store::{InboxOutcome, NewInboundWebhook};

async fn failed_webhook(h: &Http, payment: &sn_domain::payment::Payment) -> i64 {
    let outcome = WebhookOutcome {
        event_key: "invoice_paid:77".into(),
        payment: Some(PaymentRef::Payment(payment.id)),
        provider_txid: Some("77".into()),
        status: ProviderStatus::Paid,
        amount: Some(payment.amount),
        saved_method: None,
        reply: WebhookReply::ok(),
        signal: None,
    };
    let mut conn = h.services().store().conn().await.unwrap();
    let record = conn
        .webhooks()
        .record(&NewInboundWebhook {
            source: providers::CRYPTOPAY.into(),
            dedupe_key: "invoice_paid:77".into(),
            remote_ip: Some("198.51.100.7".into()),
            headers: json!({ "content-type": "application/json" }),
            body: Some(
                br#"{"update_type":"invoice_paid","payload":{"invoice_id":77,"comment":"for mom","hash":"IVabcdef"}}"#
                    .to_vec(),
            ),
            parsed: Some(serde_json::to_value(&outcome).unwrap()),
            event: None,
        })
        .await
        .unwrap();
    conn.webhooks()
        .finish(
            record.id,
            InboxOutcome::Error,
            None,
            Some("база данных недоступна"),
            None,
        )
        .await
        .unwrap();
    record.id
}

#[tokio::test]
async fn журнал_уведомлений_через_http_только_с_правом_на_деньги() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let client = h.client(5_093_118_422).await;
    let tariff = h.tariff().await;
    let payment = h.pending_payment(client, &tariff).await;
    let id = failed_webhook(&h, &payment).await;
    let card = format!("/api/admin/webhooks/{id}");
    let retry = format!("/api/admin/webhooks/{id}/retry");

    for path in ["/api/admin/webhooks", card.as_str()] {
        let (status, _) = h.get(path, &support).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    let (status, _) = h.write(Method::POST, &retry, &support, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, page) = h
        .get(
            "/api/admin/webhooks?outcome=failed&provider=cryptopay&q=77",
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Ошибка обработки");
    assert_eq!(items[0]["note"], "база данных недоступна");
    assert_eq!(items[0]["event"], "Оплата");
    assert_eq!(items[0]["provider_title"], "Crypto Pay");
    assert_eq!(items[0]["retry"]["available"], true);
    assert_eq!(page["retention"]["days"], 30);
    assert_eq!(page["retention"]["rejected_days"], 3);
    let (status, page) = h.get("/api/admin/webhooks?outcome=credited", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page["items"].as_array().unwrap().is_empty());
    let (status, _) = h.get("/api/admin/webhooks?outcome=lost", &owner).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, page) = h
        .get("/api/admin/webhooks?from=2999-01-01T00:00:00Z", &owner)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(page["items"].as_array().unwrap().is_empty());

    let (status, body) = h.get(&card, &owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["body"]["kind"], "json");
    let text = body["body"]["text"].as_str().unwrap();
    assert!(
        !text.contains("for mom") && !text.contains("IVabcdef"),
        "{text}"
    );
    assert!(text.contains("\"invoice_id\": 77"));
    assert_eq!(body["raw_event"], "invoice_paid");
    assert_eq!(body["remote_ip"], "198.51.100.7");
    assert_eq!(body["headers"][0]["label"], "content-type");
    let (status, _) = h.get("/api/admin/webhooks/987654", &owner).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, dashboard) = h.get("/api/admin/dashboard", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(dashboard["webhooks"]["failed"], 1);
    assert_eq!(dashboard["webhooks"]["providers"][0], "Crypto Pay");

    let (status, body) = h.write(Method::POST, &retry, &owner, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "accepted");
    assert_eq!(body["title"], "Засчитано");
    assert_eq!(body["payment"]["id"], payment.id.0);
    let mut conn = h.services().store().conn().await.unwrap();
    assert_eq!(
        conn.payments().get(payment.id).await.unwrap().status,
        PaymentStatus::Paid
    );
    drop(conn);
    let (status, body) = h.write(Method::POST, &retry, &owner, json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "webhook.retry_unavailable");
    let (_, dashboard) = h.get("/api/admin/dashboard", &owner).await;
    assert_eq!(dashboard["webhooks"]["failed"], 0);
    let (_, payments) = h
        .get(&format!("/api/admin/payments?id={}", payment.id.0), &owner)
        .await;
    assert_eq!(payments["items"].as_array().unwrap().len(), 1);
    assert_eq!(payments["items"][0]["id"], payment.id.0);
    h.db.cleanup().await;
}
