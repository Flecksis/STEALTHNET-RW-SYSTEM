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

const WALLET: &str = "UQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqEBI";

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
        method: None,
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

async fn gram_payment(
    services: &Services,
    client_id: sn_domain::ids::ClientId,
    tariff: &sn_domain::tariff::Tariff,
) -> sn_domain::payment::Payment {
    let CheckoutResult::Redirect { payment_id, .. } = services
        .checkout()
        .start(client_id, &gram_request(tariff))
        .await
        .unwrap()
    else {
        panic!("ожидалась ссылка на оплату");
    };
    let mut conn = services.store().conn().await.unwrap();
    conn.payments().by_public_id(payment_id).await.unwrap()
}

async fn expire(services: &Services, payment: &sn_domain::payment::Payment, ago: &str) {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query(&format!(
        "UPDATE payments SET expires_at = now() - interval '{ago}' WHERE id = $1"
    ))
    .bind(payment.id.0)
    .execute(conn.conn())
    .await
    .unwrap();
    conn.payments()
        .expire_stale(services.now(), 10)
        .await
        .unwrap();
}

#[tokio::test]
async fn ровная_проверка_находит_оплату_и_поздний_перевод_после_истечения_курса() {
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
    let alive = gram_payment(&services, client_id, &tariff).await;
    let late = gram_payment(&services, client_id, &tariff).await;
    let old = gram_payment(&services, client_id, &tariff).await;
    expire(&services, &late, "1 minute").await;
    expire(&services, &old, "2 days").await;
    let comment = |payment: &sn_domain::payment::Payment| {
        GramInvoice::parse(payment.provider_txid.as_deref().unwrap())
            .unwrap()
            .comment
    };
    let transfers: Vec<serde_json::Value> = [&alive, &late, &old]
        .iter()
        .enumerate()
        .map(|(n, payment)| {
            serde_json::json!({
                "transaction_id": {"lt": format!("{n}"), "hash": format!("dHg{n}")},
                "in_msg": {"source": "EQSender", "value": "1400000000", "message": comment(payment)}
            })
        })
        .collect();
    Mock::given(method("GET"))
        .and(path("/getTransactions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"ok": true, "result": transfers})),
        )
        .mount(&chain)
        .await;
    assert_eq!(
        services.inbox().check_chain(old.id).await.unwrap(),
        PollResult::Unsupported
    );
    let report = services.inbox().watch_chain(services.now()).await.unwrap();
    assert_eq!((report.checked, report.settled, report.failed), (2, 2, 0));
    let mut conn = db.store.conn().await.unwrap();
    for (payment, status) in [
        (&alive, PaymentStatus::Paid),
        (&late, PaymentStatus::Paid),
        (&old, PaymentStatus::Canceled),
    ] {
        assert_eq!(
            conn.payments().get(payment.id).await.unwrap().status,
            status
        );
    }
    let order = conn.orders().get(late.order_id).await.unwrap();
    assert_eq!(order.status, sn_domain::order::OrderStatus::Paid);
    drop(conn);
    assert_eq!(
        services.inbox().check_chain(late.id).await.unwrap(),
        PollResult::Settled
    );
    let again = services.inbox().watch_chain(services.now()).await.unwrap();
    assert_eq!(again.checked, 0);
    db.cleanup().await;
}

#[tokio::test]
async fn перевод_по_заменённому_счёту_находится_в_блокчейне() {
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
    let replaced = gram_payment(&services, client_id, &tariff).await;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.payments()
            .transition(
                replaced.id,
                PaymentStatus::Canceled,
                None,
                Some(REPLACED_REASON),
            )
            .await
            .unwrap();
    }
    let comment = GramInvoice::parse(replaced.provider_txid.as_deref().unwrap())
        .unwrap()
        .comment;
    Mock::given(method("GET"))
        .and(path("/getTransactions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": [{
                "transaction_id": {"lt": "7", "hash": "dHg3"},
                "in_msg": {"source": "EQSender", "value": "1400000000", "message": comment}
            }]
        })))
        .mount(&chain)
        .await;
    let report = services.inbox().watch_chain(services.now()).await.unwrap();
    assert_eq!((report.checked, report.settled), (1, 1));
    let mut conn = db.store.conn().await.unwrap();
    assert_eq!(
        conn.payments().get(replaced.id).await.unwrap().status,
        PaymentStatus::Paid
    );
    drop(conn);
    db.cleanup().await;
}
