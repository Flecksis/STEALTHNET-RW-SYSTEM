use super::*;
use sn_payments::providers::{Balance, Gram, GramInvoice};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WALLET: &str = "UQBvW8Z5huBkMJYdnfAEM5JqTNkuWX3diqYENkWsIL0XggGG";

async fn gram_http(chain: &MockServer) -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::new(vec![
            Arc::new(Gram::with_endpoints(chain.uri(), chain.uri(), chain.uri())),
            Arc::new(Balance),
        ])),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks::default(),
    });
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(keys::PUBLIC_URL, &json!("https://panel.example.com"), None)
            .await
            .unwrap();
        conn.settings()
            .set_as(keys::BOT_USERNAME, &"stealth_vpn_bot", None)
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    services.set_remna(Some(remna.clone()));
    Some(Http {
        db,
        state: AppState::new(ServerConfig::default(), services),
        clock,
        remna,
    })
}

#[tokio::test]
async fn страница_оплаты_gram_показывает_счёт_и_итог() {
    let chain = MockServer::start().await;
    let Some(h) = gram_http(&chain).await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/getTransactions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "result": []})))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&chain)
        .await;
    let tariff = h.tariff().await;
    let client = h.client(5_093_118_422).await;
    let other = h.pending_payment(client, &tariff).await;
    let period = &tariff.periods[0];
    let q = quote(&QuoteRequest {
        currency: Currency::Usd,
        tariff: &tariff,
        period_id: period.id,
        extra_devices: 0,
        sell_extra_devices: true,
        adjustments: &[],
    })
    .unwrap();
    let payment = {
        let mut tx = h.services().store().begin().await.unwrap();
        let order = tx
            .orders()
            .create(&NewOrder {
                client_id: client,
                subscription_id: None,
                kind: OrderKind::New,
                intent: PurchaseIntent::Subscription {
                    tariff_id: tariff.id,
                    period_id: period.id,
                    extra_devices: 0,
                    target: None,
                },
                quote: q.clone(),
                snapshot: Some(tariff.snapshot(period)),
            })
            .await
            .unwrap();
        let payment = tx
            .payments()
            .create(&NewPayment {
                order_id: order.id,
                client_id: client,
                provider: ProviderId::from("gram"),
                method: None,
                amount: q.total.into(),
                expires_at: Some(start() + Duration::hours(1)),
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
        payment
    };
    let invoice = GramInvoice {
        comment: "SN-ABCDEFGH".into(),
        nano: 1_400_000_000,
        address: WALLET.into(),
        amount: payment.amount,
    };
    {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.payments()
            .attach_invoice(
                payment.id,
                &sn_store::InvoiceUpdate {
                    pay_url: Some(format!(
                        "https://panel.example.com/pay/{}",
                        payment.public_id
                    )),
                    provider_txid: Some(invoice.external_id()),
                    expires_at: Some(start() + Duration::hours(1)),
                    payload: json!({}),
                },
            )
            .await
            .unwrap();
    }
    let url = format!("/api/public/pay/{}", payment.public_id);
    let (status, _, body) = h.send(Method::GET, &url, None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "pending");
    assert_eq!(body["provider"], "gram");
    assert_eq!(body["amount"]["minor"], 699);
    assert_eq!(body["gram"]["amount"], "1.4");
    assert_eq!(body["gram"]["nano"], "1400000000");
    assert_eq!(body["gram"]["address"], WALLET);
    assert_eq!(body["gram"]["comment"], "SN-ABCDEFGH");
    assert_eq!(
        body["gram"]["ton_link"],
        format!("ton://transfer/{WALLET}?amount=1400000000&text=SN-ABCDEFGH")
    );
    assert_eq!(body["back"]["bot"], "https://t.me/stealth_vpn_bot");
    assert_eq!(body["back"]["cabinet"], "https://panel.example.com/cabinet");
    assert!(body.get("client").is_none());
    let (status, _, again) = h.send(Method::GET, &url, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["status"], "pending");
    assert_eq!(chain.received_requests().await.unwrap().len(), 1);

    Mock::given(method("GET"))
        .and(path("/getTransactions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "result": [{
                "transaction_id": {"lt": "7", "hash": "Z3JhbQ=="},
                "in_msg": {"source": "EQSender", "value": "1400000000", "message": "SN-ABCDEFGH"}
            }]
        })))
        .with_priority(2)
        .mount(&chain)
        .await;
    h.services().inbox().poll_payment(payment.id).await.unwrap();
    let (status, _, paid) = h.send(Method::GET, &url, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(paid["status"], "paid");

    let (status, _, _) = h
        .send(
            Method::GET,
            &format!("/api/public/pay/{}", other.public_id),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = h
        .send(
            Method::GET,
            &format!("/api/public/pay/{}", Uuid::from_u128(9)),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    h.db.cleanup().await;
}
