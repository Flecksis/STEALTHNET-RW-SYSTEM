use crate::Services;
use crate::error::{ServiceError, ServiceResult};
use serde_json::Value;
use sn_domain::ids::{ClientId, PaymentId, SubscriptionId};
use sn_domain::money::{Currency, Money};
use sn_domain::order::{NewOrder, OrderKind, PurchaseIntent};
use sn_domain::payment::{NewPayment, ProviderId};
use sn_domain::pricing::Quote;
use sn_payments::{PaymentRef, ProviderStatus, WebhookOutcome, WebhookRequest};
use sn_store::Db;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyMatch {
    pub invoice: i64,
    pub payment: PaymentId,
}

fn paths(provider: &str) -> &'static [&'static str] {
    match provider {
        "yookassa" => &["/object/metadata/payment_id", "/object/id"],
        "cryptopay" => &["/payload/payload", "/payload/invoice_id"],
        "heleket" => &["/order_id", "/additional_data", "/uuid"],
        "lava" => &["/contractId", "/parentContractId", "/id"],
        "platega" => &[
            "/payload",
            "/id",
            "/transactionId",
            "/externalId",
            "/invoiceId",
            "/orderId",
        ],
        "rollypay" => &[
            "/order_id",
            "/metadata/paymentId",
            "/payment_id",
            "/data/order_id",
            "/data/metadata/paymentId",
            "/data/payment_id",
        ],
        "yoomoney" => &["/label", "/operation_id"],
        "cloudpayments" => &["/InvoiceId", "/TransactionId"],
        "overpay" => &["/merchantTransactionId", "/merchant_order_id", "/id"],
        "cispay" => &["/order_id", "/id", "/payment_id"],
        "paritypay" => &["/order_id", "/id"],
        _ => &[],
    }
}

fn body_value(request: &WebhookRequest) -> Value {
    if let Ok(value) = serde_json::from_slice::<Value>(&request.body)
        && value.is_object()
    {
        return value;
    }
    let text = String::from_utf8_lossy(&request.body);
    let map: serde_json::Map<String, Value> = text
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((form_decode(key), Value::String(form_decode(value))))
        })
        .collect();
    Value::Object(map)
}

fn form_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn references(
    provider: &str,
    outcome: &WebhookOutcome,
    request: &WebhookRequest,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    match &outcome.payment {
        Some(PaymentRef::PublicId(id)) => out.push(id.to_string()),
        Some(PaymentRef::ProviderTxid(txid)) => out.push(txid.clone()),
        _ => {}
    }
    if let Some(txid) = &outcome.provider_txid {
        out.push(txid.clone());
    }
    let body = body_value(request);
    for path in paths(provider) {
        match body.pointer(path) {
            Some(Value::String(s)) => out.push(s.clone()),
            Some(Value::Number(n)) => out.push(n.to_string()),
            _ => {}
        }
    }
    let mut unique: Vec<String> = Vec::new();
    for value in out {
        let value = value.trim().to_string();
        if !value.is_empty() && value.len() <= 128 && !unique.contains(&value) {
            unique.push(value);
        }
    }
    unique
}

pub fn wants_legacy(outcome: &WebhookOutcome) -> bool {
    matches!(
        outcome.status,
        ProviderStatus::Paid | ProviderStatus::Overpaid { .. }
    )
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    source: String,
    old_id: String,
    client_id: i64,
    intent: Value,
    amount_minor: i64,
    currency: String,
    payment_id: Option<i64>,
    order_id: Option<i64>,
    recurring: bool,
}

async fn subscription_target(
    db: &mut impl Db,
    client: i64,
    target: Option<SubscriptionId>,
) -> ServiceResult<Option<SubscriptionId>> {
    let Some(target) = target else {
        return Ok(None);
    };
    match db.subscriptions().get(target).await {
        Ok(sub) if sub.client_id == ClientId(client) && sub.ended_at.is_none() => Ok(Some(target)),
        Ok(_) => Ok(None),
        Err(sn_store::StoreError::NotFound(_)) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

async fn order_for(db: &mut impl Db, row: &Row, total: Money) -> ServiceResult<NewOrder> {
    let intent: Option<PurchaseIntent> = serde_json::from_value(row.intent.clone()).ok();
    let quote = |days: u32| Quote {
        currency: total.currency,
        lines: Vec::new(),
        subtotal: total,
        discount: Money::zero(total.currency),
        total,
        stars: None,
        days,
        extra_devices: 0,
    };
    let topup = NewOrder {
        client_id: ClientId(row.client_id),
        subscription_id: None,
        kind: OrderKind::TopUp,
        intent: PurchaseIntent::TopUp { amount: total },
        quote: quote(0),
        snapshot: None,
    };
    let Some(PurchaseIntent::Subscription {
        tariff_id,
        period_id,
        extra_devices,
        target,
    }) = intent
    else {
        return Ok(topup);
    };
    let tariff = match db.tariffs().get(tariff_id).await {
        Ok(tariff) => tariff,
        Err(sn_store::StoreError::NotFound(_)) => return Ok(topup),
        Err(err) => return Err(err.into()),
    };
    let Some(period) = tariff.period(period_id).cloned() else {
        return Ok(topup);
    };
    let mut target = subscription_target(db, row.client_id, target).await?;
    if target.is_none()
        && row.recurring
        && let Some(first) = row.order_id
    {
        let earlier = db.orders().get(sn_domain::ids::OrderId(first)).await?;
        target = subscription_target(db, row.client_id, earlier.subscription_id).await?;
        if target.is_none() {
            return Ok(topup);
        }
    }
    Ok(NewOrder {
        client_id: ClientId(row.client_id),
        subscription_id: target,
        kind: if target.is_some() {
            OrderKind::Renewal
        } else {
            OrderKind::New
        },
        intent: PurchaseIntent::Subscription {
            tariff_id,
            period_id,
            extra_devices,
            target,
        },
        quote: quote(period.days),
        snapshot: Some(tariff.snapshot(&period)),
    })
}

pub async fn settle(
    services: &Services,
    provider: &str,
    outcome: &WebhookOutcome,
    request: &WebhookRequest,
) -> ServiceResult<Option<LegacyMatch>> {
    if !wants_legacy(outcome) {
        return Ok(None);
    }
    let candidates = references(provider, outcome, request);
    if candidates.is_empty() {
        return Ok(None);
    }
    let mut tx = services.store().begin().await?;
    let row: Option<Row> = sqlx::query_as(
        "SELECT id, source, old_id, client_id, intent, amount_minor, currency, payment_id, order_id, recurring \
         FROM legacy_invoices WHERE provider = $1 \
         AND (old_id = ANY($2) OR order_ref = ANY($2) OR external_id = ANY($2)) \
         ORDER BY recurring, id LIMIT 1 FOR UPDATE",
    )
    .bind(provider)
    .bind(&candidates)
    .fetch_optional(tx.conn())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if let Some(payment) = row.payment_id.filter(|_| !row.recurring) {
        tx.commit().await?;
        return Ok(Some(LegacyMatch {
            invoice: row.id,
            payment: PaymentId(payment),
        }));
    }
    let paid_before: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM legacy_ids WHERE entity = $1 AND old_id = $2)",
    )
    .bind(format!("{}:payment", row.source))
    .bind(&row.old_id)
    .fetch_one(tx.conn())
    .await?;
    if paid_before && !row.recurring {
        tx.commit().await?;
        return Ok(None);
    }
    let currency = Currency::parse(&row.currency)
        .map_err(|_| ServiceError::Internal(format!("валюта счёта {}", row.currency)))?;
    let amount = match (row.recurring, &outcome.amount) {
        (true, Some(paid)) if paid.minor > 0 => paid.minor,
        _ => row.amount_minor,
    };
    let total = Money::new(amount, currency);
    let order = order_for(&mut tx, &row, total).await?;
    let order = tx.orders().create(&order).await?;
    let payment = tx
        .payments()
        .create(&NewPayment {
            order_id: order.id,
            client_id: ClientId(row.client_id),
            provider: ProviderId::from(provider),
            method: None,
            amount: total.into(),
            expires_at: None,
        })
        .await?;
    sqlx::query(
        "UPDATE legacy_invoices SET order_id = COALESCE(order_id, $2), \
         payment_id = CASE WHEN recurring THEN payment_id ELSE $3 END, \
         paid_at = CASE WHEN recurring THEN paid_at ELSE now() END WHERE id = $1",
    )
    .bind(row.id)
    .bind(order.id.0)
    .bind(payment.id.0)
    .execute(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(Some(LegacyMatch {
        invoice: row.id,
        payment: payment.id,
    }))
}

pub async fn precheck(
    services: &Services,
    provider: &str,
    outcome: &WebhookOutcome,
) -> ServiceResult<Option<bool>> {
    let empty = WebhookRequest {
        headers: Vec::new(),
        body: Default::default(),
        remote_ip: None,
        route_suffix: None,
        query: None,
    };
    let candidates = references(provider, outcome, &empty);
    if candidates.is_empty() {
        return Ok(None);
    }
    let mut conn = services.store().conn().await?;
    let row: Option<(i64, String)> = sqlx::query_as(
        "SELECT amount_minor, currency FROM legacy_invoices WHERE provider = $1 AND payment_id IS NULL \
         AND NOT recurring AND (old_id = ANY($2) OR order_ref = ANY($2) OR external_id = ANY($2)) \
         ORDER BY id LIMIT 1",
    )
    .bind(provider)
    .bind(&candidates)
    .fetch_optional(conn.conn())
    .await?;
    Ok(row.map(|(minor, currency)| {
        Currency::parse(&currency)
            .ok()
            .is_some_and(|c| outcome.amount == Some(Money::new(minor, c).into()))
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayLink {
    pub url: String,
}

pub async fn pay_link(services: &Services, order_ref: &str) -> ServiceResult<Option<PayLink>> {
    let mut conn = services.store().conn().await?;
    let url: Option<String> = sqlx::query_scalar(
        "SELECT pay_url FROM legacy_invoices WHERE order_ref = $1 AND payment_id IS NULL \
         AND order_id IS NULL AND pay_url IS NOT NULL ORDER BY id DESC LIMIT 1",
    )
    .bind(order_ref.trim())
    .fetch_optional(conn.conn())
    .await?
    .flatten();
    Ok(url.map(|url| PayLink { url }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use sn_payments::WebhookReply;

    fn request(body: &str) -> WebhookRequest {
        WebhookRequest {
            headers: Vec::new(),
            body: Bytes::from(body.to_string()),
            remote_ip: None,
            route_suffix: None,
            query: None,
        }
    }

    fn outcome(payment: Option<PaymentRef>, txid: Option<&str>) -> WebhookOutcome {
        WebhookOutcome {
            event_key: "k".into(),
            payment,
            provider_txid: txid.map(str::to_string),
            status: ProviderStatus::Paid,
            amount: None,
            saved_method: None,
            reply: WebhookReply::Empty { status: 200 },
            signal: None,
        }
    }

    #[test]
    fn номера_старого_счёта_берутся_из_уведомления() {
        let refs = references(
            "yookassa",
            &outcome(Some(PaymentRef::ProviderTxid("2f1".into())), Some("2f1")),
            &request(r#"{"object":{"id":"2f1","metadata":{"payment_id":"cmold1"}}}"#),
        );
        assert_eq!(refs, ["2f1", "cmold1"]);
        let refs = references(
            "yoomoney",
            &outcome(None, None),
            &request("label=cmold2&operation_id=9001&amount=100.00"),
        );
        assert_eq!(refs, ["cmold2", "9001"]);
        let refs = references(
            "lava",
            &outcome(Some(PaymentRef::ProviderTxid("new-charge".into())), None),
            &request(r#"{"contractId":"new-charge","parentContractId":"old-order"}"#),
        );
        assert_eq!(refs, ["new-charge", "old-order"]);
        assert!(references("unknown", &outcome(None, None), &request("{}")).is_empty());
        let refs = references(
            "cispay",
            &outcome(None, None),
            &request(
                r#"{"id":"0b9d77aa-1c2d-4e3f-8a4b-5c6d7e8f9012","order_id":"ord-old-7","status":"PAID"}"#,
            ),
        );
        assert_eq!(refs, ["ord-old-7", "0b9d77aa-1c2d-4e3f-8a4b-5c6d7e8f9012"]);
        let refs = references(
            "paritypay",
            &outcome(None, None),
            &request(
                r#"{"id":"9beea835-0937-4b5c-8f5a-c3a0d0e60346","order_id":"ord-old-8","status":"PAID"}"#,
            ),
        );
        assert_eq!(refs, ["ord-old-8", "9beea835-0937-4b5c-8f5a-c3a0d0e60346"]);
    }

    #[test]
    fn старые_имена_cispay_и_paritypay_ведут_к_новым_платёжкам() {
        use crate::legacy::stealthnet::old_provider;
        for (old, new) in [
            ("cispay", "cispay"),
            ("CisPay", "cispay"),
            ("cis_pay", "cispay"),
            ("paritypay", "paritypay"),
            ("ParityPay", "paritypay"),
            ("parity_pay", "paritypay"),
        ] {
            assert_eq!(old_provider(old), Some(new), "{old}");
        }
    }
}
