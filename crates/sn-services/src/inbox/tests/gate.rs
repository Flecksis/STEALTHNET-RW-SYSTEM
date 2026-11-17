use super::*;
use crate::FixedClock;
use sn_payments::providers::YooKassa;
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const REAL_ID: &str = "2a8f5d1e-000f-5000-9000-1b4f0c5e7d21";
const ATTACKER: &str = "198.51.100.66";
const KASSA_ADDRESS: &str = "77.75.153.10";

#[derive(Clone, Default)]
struct Api {
    base: String,
    found: Arc<Mutex<HashMap<String, String>>>,
    hits: Arc<Mutex<Vec<String>>>,
}

impl Api {
    async fn start() -> Api {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = Api {
            base: format!("http://{}", listener.local_addr().unwrap()),
            ..Api::default()
        };
        let served = api.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let served = served.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut chunk = [0u8; 2048];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut chunk).await {
                            Ok(read) if read > 0 => head.extend_from_slice(&chunk[..read]),
                            _ => return,
                        }
                    }
                    let path = String::from_utf8_lossy(&head)
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_string();
                    served.hits.lock().unwrap().push(path.clone());
                    let body = served.found.lock().unwrap().get(&path).cloned();
                    let (status, body) = match body {
                        Some(body) => ("200 OK", body),
                        None => ("404 Not Found", "{}".to_string()),
                    };
                    let reply = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(reply.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        api
    }

    fn serve(&self, path: &str, body: serde_json::Value) {
        self.found
            .lock()
            .unwrap()
            .insert(path.to_string(), body.to_string());
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }

    fn forged_hits(&self) -> usize {
        self.hits().iter().filter(|p| p.contains("forged-")).count()
    }
}

async fn gate_kit(api: &Api) -> Option<Kit> {
    let clock = Arc::new(FixedClock::new(moment(chrono::Duration::zero())));
    let kit = kassa_kit_with(
        Arc::new(Kassa::default()),
        clock,
        vec![Arc::new(YooKassa::with_api_base(api.base.clone()))],
    )
    .await?;
    {
        let mut conn = kit.services.store().conn().await.unwrap();
        conn.settings()
            .set(
                &keys::provider(providers::YOOKASSA),
                &serde_json::json!({ "enabled": true, "config": { "shop_id": "504367" } }),
                None,
            )
            .await
            .unwrap();
        settings::write_secret(
            &mut conn,
            kit.services.secrets(),
            &secret_keys::provider(providers::YOOKASSA, "secret_key"),
            Some("test_secret_key"),
            None,
        )
        .await
        .unwrap();
    }
    kit.services.reload().await.unwrap();
    kit.services.set_remna(Some(kit.remna.clone()));
    Some(kit)
}

fn notification(object_id: &str, ip: &str) -> InboundWebhook {
    let body = serde_json::json!({
        "type": "notification",
        "event": "payment.succeeded",
        "object": { "id": object_id, "status": "succeeded" }
    })
    .to_string();
    InboundWebhook {
        provider: providers::YOOKASSA.into(),
        request: WebhookRequest {
            headers: Vec::new(),
            body: Bytes::from(body),
            remote_ip: ip.parse().ok(),
            route_suffix: None,
            query: None,
        },
    }
}

async fn real_invoice(kit: &Kit, api: &Api, telegram_id: i64) -> Payment {
    let client_id = client(&kit.services, telegram_id).await;
    let tariff = standard(&kit.services).await;
    let invoice = pending(
        kit,
        client_id,
        &tariff,
        providers::YOOKASSA,
        Amount::new(49_900, PayCurrency::Rub),
        None,
    )
    .await;
    api.serve(
        &format!("/v3/payments/{REAL_ID}"),
        serde_json::json!({
            "id": REAL_ID,
            "status": "succeeded",
            "paid": true,
            "amount": { "value": "499.00", "currency": "RUB" },
            "metadata": { "payment_id": invoice.public_id.to_string() }
        }),
    );
    invoice
}

#[test]
fn адрес_ipv6_считается_по_сети_64() {
    let key = |ip: &str| address_key(ip.parse().ok());
    assert_eq!(key("203.0.113.7"), "203.0.113.7");
    assert_eq!(key("::ffff:203.0.113.7"), "203.0.113.7");
    assert_eq!(
        key("2001:db8:aa:bb:1:2:3:4"),
        key("2001:db8:aa:bb:ffff:ffff:ffff:ffff")
    );
    assert_ne!(key("2001:db8:aa:bb::1"), key("2001:db8:aa:bc::1"));
    assert_eq!(key("2001:db8:aa:bb::1"), "2001:db8:aa:bb::/64");
    assert_eq!(address_key(None), "unknown");
    for ip in [
        "203.0.113.7",
        "::ffff:203.0.113.7",
        "2001:db8:aa:bb::1",
        "::1",
    ] {
        assert_eq!(key(ip), crate::limits::address_key(Some(ip)), "{ip}");
    }
    assert_eq!(address_key(None), crate::limits::address_key(None));
}

#[tokio::test]
async fn поддельные_вебхуки_с_одного_адреса_почти_не_доходят_до_api_кассы() {
    let api = Api::start().await;
    let Some(kit) = gate_kit(&api).await else {
        return;
    };
    let invoice = real_invoice(&kit, &api, 7800).await;
    let inbox = kit.services.inbox();
    let mut statuses = Vec::new();
    for n in 0..25 {
        let reply = inbox
            .handle(notification(&format!("forged-{n}"), ATTACKER))
            .await
            .unwrap();
        statuses.push(reply.reply.status());
    }
    let genuine = inbox
        .handle(notification(REAL_ID, KASSA_ADDRESS))
        .await
        .unwrap();
    assert_eq!(genuine.reply.status(), 200);
    assert_eq!(payment(&kit, invoice.id).await.status, PaymentStatus::Paid);
    for n in 25..50 {
        let reply = inbox
            .handle(notification(&format!("forged-{n}"), ATTACKER))
            .await
            .unwrap();
        statuses.push(reply.reply.status());
    }
    let limit = WEBHOOK_LOOKUPS.limit as usize;
    assert_eq!(limit, 10);
    assert!(statuses[..limit].iter().all(|s| *s == 401), "{statuses:?}");
    assert!(statuses[limit..].iter().all(|s| *s == 429), "{statuses:?}");
    assert_eq!(api.forged_hits(), limit);
    let repeat = inbox
        .handle(notification(REAL_ID, KASSA_ADDRESS))
        .await
        .unwrap();
    assert_eq!(repeat.reply.status(), 200);
    let rows = inbox_rows(&kit).await;
    assert_eq!(
        rows.iter().filter(|r| r.outcome == "accepted").count(),
        1,
        "{rows:?}"
    );
    assert_eq!(
        rows.iter()
            .filter(|r| r.outcome == "rejected_signature" && r.body.is_none())
            .count(),
        limit
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn параллельные_поддельные_вебхуки_не_обходят_лимит_запросов_к_api() {
    let api = Api::start().await;
    let Some(kit) = gate_kit(&api).await else {
        return;
    };
    let mut tasks = tokio::task::JoinSet::new();
    for n in 0..50 {
        let services = kit.services.clone();
        tasks.spawn(async move {
            services
                .inbox()
                .handle(notification(&format!("forged-{n}"), ATTACKER))
                .await
                .map(|r| r.reply.status())
        });
    }
    let mut statuses = Vec::new();
    while let Some(done) = tasks.join_next().await {
        statuses.push(done.unwrap().unwrap());
    }
    assert!(api.forged_hits() <= WEBHOOK_LOOKUPS.limit as usize);
    assert_eq!(
        statuses.iter().filter(|s| **s == 401).count(),
        api.forged_hits()
    );
    assert!(
        statuses.iter().all(|s| matches!(s, 401 | 429)),
        "{statuses:?}"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn повтор_поддельного_идентификатора_с_другого_адреса_не_вызывает_запрос() {
    let api = Api::start().await;
    let Some(kit) = gate_kit(&api).await else {
        return;
    };
    let inbox = kit.services.inbox();
    for ip in [ATTACKER, "203.0.113.50", "2001:db8::50"] {
        assert_eq!(
            inbox
                .handle(notification("forged-1", ip))
                .await
                .unwrap()
                .reply
                .status(),
            401
        );
    }
    assert_eq!(api.hits(), vec!["/v3/payments/forged-1".to_string()]);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn подлинные_уведомления_с_адреса_кассы_не_расходуют_лимит() {
    let api = Api::start().await;
    let Some(kit) = gate_kit(&api).await else {
        return;
    };
    let invoice = real_invoice(&kit, &api, 7801).await;
    let inbox = kit.services.inbox();
    for _ in 0..30 {
        assert_eq!(
            inbox
                .handle(notification(REAL_ID, KASSA_ADDRESS))
                .await
                .unwrap()
                .reply
                .status(),
            200
        );
    }
    assert_eq!(payment(&kit, invoice.id).await.status, PaymentStatus::Paid);
    assert_eq!(api.hits().len(), 30);
    kit.db.cleanup().await;
}
