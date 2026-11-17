use super::testkit::*;
use super::*;
use crate::SystemClock;
use crate::inbox::PollResult;
use crate::services::{EnvFallbacks, ServiceDeps, Services};
use sn_domain::payment::{PaymentStatus, providers};
use sn_domain::settings::keys;
use sn_domain::traffic::TrafficPolicy;
use sn_modules::ModuleRegistry;
use sn_payments::ProviderRegistry;
use sn_payments::providers::{Balance, Gram, GramInvoice};
use sn_remnawave::fake::FakeRemnawave;
use sn_remnawave::{InternalSquad, SquadInfo};
use sn_store::SecretBox;
use sn_store::testing::test_store;
use std::sync::Arc;
use uuid::Uuid;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WALLET: &str = "UQBvW8Z5huBkMJYdnfAEM5JqTNkuWX3diqYENkWsIL0XggGG";

struct Setup {
    db: sn_store::testing::TestDb,
    services: Services,
    chain: MockServer,
    client_id: sn_domain::ids::ClientId,
    tariff: sn_domain::tariff::Tariff,
}

async fn setup(public_url: Option<&str>) -> Option<Setup> {
    let db = test_store().await?;
    let chain = MockServer::start().await;
    let registry = ProviderRegistry::new(vec![
        Arc::new(Gram::with_endpoints(chain.uri(), chain.uri(), chain.uri())),
        Arc::new(Balance),
    ]);
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(registry),
        modules: Arc::new(ModuleRegistry::new(Vec::new()).unwrap()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: Arc::new(SystemClock),
        env: EnvFallbacks::default(),
    });
    {
        let mut conn = db.store.conn().await.unwrap();
        if let Some(url) = public_url {
            conn.settings()
                .set(keys::PUBLIC_URL, &serde_json::json!(url), None)
                .await
                .unwrap();
        }
        conn.settings()
            .set(
                &keys::provider(providers::GRAM),
                &serde_json::json!({
                    "enabled": true,
                    "config": {"wallet": WALLET, "rate": "5"}
                }),
                None,
            )
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    remna.set_squads(vec![InternalSquad {
        uuid: Uuid::from_u128(1),
        name: "Сквад 1".into(),
        view_position: 0,
        info: SquadInfo::default(),
        inbounds: Vec::new(),
    }]);
    services.set_remna(Some(remna));
    let client_id = client(&services, 5_093_118_422).await;
    let category_id = category(&services, "VPN").await;
    let tariff = tariff(
        &services,
        category_id,
        "Стандарт",
        TrafficPolicy::UNLIMITED,
        vec![squad(1)],
    )
    .await;
    Some(Setup {
        db,
        services,
        chain,
        client_id,
        tariff,
    })
}

fn gram_request(tariff: &sn_domain::tariff::Tariff) -> CheckoutRequest {
    CheckoutRequest {
        cart: cart(tariff, 90, None),
        provider: providers::GRAM.into(),
        channel: Channel::Bot,
        return_url: None,
    }
}

#[tokio::test]
async fn оплата_в_gram_проверяется_в_блокчейне_и_засчитывается() {
    let Some(Setup {
        db,
        services,
        chain,
        client_id,
        tariff,
    }) = setup(Some(PUBLIC_URL)).await
    else {
        return;
    };
    let result = services
        .checkout()
        .start(client_id, &gram_request(&tariff))
        .await
        .unwrap();
    let CheckoutResult::Redirect {
        payment_id,
        pay_url,
        ..
    } = result
    else {
        panic!("ожидалась ссылка на оплату");
    };
    assert_eq!(pay_url, format!("{PUBLIC_URL}/api/pay/{payment_id}"));
    let payment = {
        let mut conn = db.store.conn().await.unwrap();
        conn.payments().by_public_id(payment_id).await.unwrap()
    };
    assert_eq!(
        payment.pay_url,
        Some(format!("{PUBLIC_URL}/pay/{payment_id}"))
    );
    let invoice = GramInvoice::parse(payment.provider_txid.as_deref().unwrap()).unwrap();
    assert_eq!(invoice.nano, 1_400_000_000);
    assert_eq!(invoice.amount, payment.amount);
    assert_eq!(invoice.address, WALLET);
    let delay: f64 = {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query_scalar(
            "SELECT EXTRACT(EPOCH FROM run_at - now())::float8 FROM jobs WHERE kind = 'poll_payment'",
        )
        .fetch_one(conn.conn())
        .await
        .unwrap()
    };
    assert!(
        (20.0..=40.0).contains(&delay),
        "первая проверка через {delay} с"
    );

    Mock::given(method("GET"))
        .and(path("/getTransactions"))
        .and(query_param("address", WALLET))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": [{
                "transaction_id": {"lt": "1", "hash": "aGFzaA=="},
                "in_msg": {
                    "source": "EQSender",
                    "value": "1300000000",
                    "message": invoice.comment
                }
            }]
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&chain)
        .await;
    assert_eq!(
        services.inbox().poll_payment(payment.id).await.unwrap(),
        PollResult::Pending
    );
    Mock::given(method("GET"))
        .and(path("/getTransactions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": [{
                "transaction_id": {"lt": "2", "hash": "cGFpZA=="},
                "in_msg": {
                    "source": "EQSender",
                    "value": "1400000000",
                    "message": invoice.comment
                }
            }]
        })))
        .with_priority(2)
        .mount(&chain)
        .await;
    assert_eq!(
        services.inbox().poll_payment(payment.id).await.unwrap(),
        PollResult::Settled
    );
    let mut conn = db.store.conn().await.unwrap();
    let paid = conn.payments().get(payment.id).await.unwrap();
    assert_eq!(paid.status, PaymentStatus::Paid);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn без_внешнего_адреса_gram_ведёт_сразу_в_кошелёк() {
    let Some(Setup {
        db,
        services,
        client_id,
        tariff,
        ..
    }) = setup(None).await
    else {
        return;
    };
    let result = services
        .checkout()
        .start(client_id, &gram_request(&tariff))
        .await
        .unwrap();
    let CheckoutResult::Redirect { pay_url, .. } = result else {
        panic!("ожидалась ссылка на оплату");
    };
    assert!(
        pay_url.starts_with(&format!(
            "https://app.tonkeeper.com/transfer/{WALLET}?amount=1400000000&text=SN-"
        )),
        "{pay_url}"
    );
    db.cleanup().await;
}
