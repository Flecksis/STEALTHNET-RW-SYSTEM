use super::*;
use crate::admin::MarkPaidRequest;
use crate::clock::FixedClock;
use crate::services::{EnvFallbacks, ServiceDeps};
use chrono::TimeZone;
use sn_domain::client::{RegistrationSource, TelegramProfile};
use sn_domain::money::{Currency, Money};
use sn_domain::order::{NewOrder, OrderKind, PurchaseIntent};
use sn_domain::payment::{NewPayment, Payment, ProviderId};
use sn_domain::pricing::Quote;
use sn_modules::ModuleRegistry;
use sn_payments::ProviderRegistry;
use sn_store::SecretBox;
use sn_store::testing::{TestDb, test_store};
use std::sync::Arc;

const TOKEN: &str = "123456:TEST-secret-token";
const PASSWORD: &str = "очень-надёжный-пароль";
const UNREACHABLE: &str = "http://127.0.0.1:9";

fn moment() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 17, 9, 30, 0).unwrap()
}

async fn setup_with(bot_token: Option<&str>) -> Option<(TestDb, Services, AdminId)> {
    let db = test_store().await?;
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: Arc::new(FixedClock::new(moment())),
        env: EnvFallbacks {
            bot_token: bot_token.map(str::to_string),
            ..EnvFallbacks::default()
        },
    });
    services.reload().await.unwrap();
    let (admin, _) = services
        .auth()
        .upsert_owner("owner@example.com", PASSWORD)
        .await
        .unwrap();
    Some((db, services, admin))
}

async fn setup() -> Option<(TestDb, Services, AdminId)> {
    setup_with(Some(TOKEN)).await
}

async fn stars_invoice(services: &Services, telegram_id: i64, stars: i64) -> Payment {
    let (client, _) = services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: telegram_id,
                username: None,
                first_name: Some("Алексей".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
            RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap();
    let total = Money::new(699, Currency::Usd);
    let mut tx = services.store().begin().await.unwrap();
    let order = tx
        .orders()
        .create(&NewOrder {
            client_id: client.id,
            subscription_id: None,
            kind: OrderKind::Module,
            intent: PurchaseIntent::Module {
                module: sn_domain::module::ModuleId::Options,
                kind: "тест".into(),
                payload: serde_json::json!({}),
            },
            quote: Quote {
                currency: Currency::Usd,
                lines: Vec::new(),
                subtotal: total,
                discount: Money::zero(Currency::Usd),
                total,
                stars: None,
                days: 0,
                extra_devices: 0,
            },
            snapshot: None,
        })
        .await
        .unwrap();
    let payment = tx
        .payments()
        .create(&NewPayment {
            order_id: order.id,
            client_id: client.id,
            provider: ProviderId::from(providers::STARS),
            method: None,
            amount: Amount::new(stars, PayCurrency::Xtr),
            expires_at: None,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    payment
}

fn payload(event: &str, charge: &str, payer: i64, stars: i64, invoice: &str) -> Value {
    serde_json::json!({
        "event": event,
        "invoice_payload": invoice,
        "total_amount": stars,
        "currency": "XTR",
        "telegram_payment_charge_id": charge,
        "telegram_user_id": payer,
    })
}

struct Seed<'s> {
    charge: &'s str,
    event: &'s str,
    payload: Value,
    status: &'s str,
    reason: Option<&'s str>,
    payment: Option<PaymentId>,
}

async fn seed(services: &Services, seed: Seed<'_>) -> i64 {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_scalar(
        "INSERT INTO bot_star_events \
         (charge_id, event, payload, source, status, attempts, next_attempt_at, last_error, payment_id) \
         VALUES ($1, $2, $3, 'update', $4, 1, $5, $6, $7) RETURNING id",
    )
    .bind(seed.charge)
    .bind(seed.event)
    .bind(seed.payload)
    .bind(seed.status)
    .bind(services.now())
    .bind(seed.reason)
    .bind(seed.payment.map(|p| p.0))
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

async fn underpaid(services: &Services, payment: &Payment, charge: &str, payer: i64) -> i64 {
    seed(
        services,
        Seed {
            charge,
            event: SUCCESSFUL_PAYMENT,
            payload: payload(
                SUCCESSFUL_PAYMENT,
                charge,
                payer,
                payment.amount.minor - 1,
                &payment.public_id.to_string(),
            ),
            status: STATUS_REVIEW,
            reason: Some("сумма не сошлась: Underpaid"),
            payment: Some(payment.id),
        },
    )
    .await
}

#[derive(sqlx::FromRow)]
struct Stored {
    status: String,
    last_error: Option<String>,
    resolved_at: Option<DateTime<Utc>>,
    resolved_by: Option<i64>,
    resolution_note: Option<String>,
}

async fn stored(services: &Services, id: i64) -> Stored {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_as(
        "SELECT status, last_error, resolved_at, resolved_by, resolution_note \
         FROM bot_star_events WHERE id = $1",
    )
    .bind(id)
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

async fn audit_of(services: &Services, action: &str) -> Vec<sn_store::AuditRecord> {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_as(
        "SELECT id, actor_kind, actor_id, action, entity_kind, entity_id, details, created_at \
         FROM audit_log WHERE action = $1 ORDER BY id",
    )
    .bind(action)
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

async fn resolutions(services: &Services) -> i64 {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action LIKE 'payment.stars_review_%'")
        .fetch_one(conn.conn())
        .await
        .unwrap()
}

#[tokio::test]
async fn список_показывает_события_на_ручной_проверке_с_платежом_и_заказом() {
    let Some((db, services, _admin)) = setup().await else {
        return;
    };
    let payment = stars_invoice(&services, 801, 350).await;
    let review = underpaid(&services, &payment, "stx-801", 801).await;
    for (charge, status) in [
        ("stx-pending", "pending"),
        ("stx-done", "done"),
        ("stx-skipped", "skipped"),
    ] {
        seed(
            &services,
            Seed {
                charge,
                event: SUCCESSFUL_PAYMENT,
                payload: payload(SUCCESSFUL_PAYMENT, charge, 802, 350, "чужой счёт"),
                status,
                reason: None,
                payment: None,
            },
        )
        .await;
    }
    let broken = seed(
        &services,
        Seed {
            charge: "stx-broken",
            event: SUCCESSFUL_PAYMENT,
            payload: serde_json::json!("мусор"),
            status: STATUS_REVIEW,
            reason: Some("invalid type: string"),
            payment: None,
        },
    )
    .await;
    let stranger = seed(
        &services,
        Seed {
            charge: "stx-803",
            event: REFUNDED_PAYMENT,
            payload: payload(REFUNDED_PAYMENT, "stx-803", 803, 150, "не uuid"),
            status: STATUS_REVIEW,
            reason: Some("валюта возврата не совпала с валютой платежа"),
            payment: None,
        },
    )
    .await;
    let review_ops = StarsReview::new(&services);
    let page = review_ops.list(PageRequest::default()).await.unwrap();
    assert_eq!(page.next, None);
    assert_eq!(page.total, 3);
    assert_eq!(review_ops.count().await.unwrap(), 3);
    assert_eq!(
        page.items.iter().map(|i| i.id).collect::<Vec<_>>(),
        [stranger, broken, review]
    );
    let item = &page.items[2];
    assert_eq!(item.charge_id, "stx-801");
    assert_eq!(item.event, StarsPaymentEvent::SuccessfulPayment);
    assert_eq!(item.stars, Some(Stars(349)));
    assert_eq!(item.telegram_user_id, Some(801));
    assert_eq!(item.payer_client_id, Some(payment.client_id));
    assert_eq!(item.payer_name.as_deref(), Some("Алексей"));
    assert_eq!(
        item.invoice_payload.as_deref(),
        Some(payment.public_id.to_string().as_str())
    );
    assert_eq!(
        item.payment,
        Some(StarsReviewPayment {
            id: payment.id,
            client_id: payment.client_id,
            status: PaymentStatus::Pending,
            amount: Amount::new(350, PayCurrency::Xtr),
            provider_txid: None,
        })
    );
    assert_eq!(
        item.order,
        Some(StarsReviewOrder {
            id: payment.order_id,
            status: OrderStatus::Pending,
        })
    );
    assert_eq!(item.reason.as_deref(), Some("сумма не сошлась: Underpaid"));
    assert_eq!(item.source, "update");
    assert_eq!(item.attempts, 1);
    assert!(item.refundable);
    assert!(!item.telegram_refunded);
    assert!(item.created_at <= item.updated_at);
    let json = serde_json::to_value(item).unwrap();
    assert_eq!(json["event"], "successful_payment");
    assert_eq!(json["stars"], 349);
    assert_eq!(json["telegram_user_id"], 801);
    assert_eq!(json["payment"]["id"], payment.id.0);
    assert_eq!(json["payment"]["status"], "pending");
    assert_eq!(json["payment"]["amount"]["minor"], 350);
    assert_eq!(json["payment"]["amount"]["currency"], "XTR");
    assert_eq!(json["order"]["id"], payment.order_id.0);
    assert_eq!(json["order"]["status"], "pending");
    let garbage = &page.items[1];
    assert_eq!(garbage.charge_id, "stx-broken");
    assert_eq!(
        (garbage.stars, garbage.telegram_user_id, &garbage.payment),
        (None, None, &None)
    );
    assert!(!garbage.refundable);
    let refund = &page.items[0];
    assert_eq!(refund.event, StarsPaymentEvent::RefundedPayment);
    assert_eq!(refund.stars, Some(Stars(150)));
    assert_eq!(refund.payer_client_id, None);
    assert_eq!(refund.payer_name, None);
    assert!(!refund.refundable);
    assert!(refund.telegram_refunded);
    seed(
        &services,
        Seed {
            charge: "stx-801",
            event: REFUNDED_PAYMENT,
            payload: payload(
                REFUNDED_PAYMENT,
                "stx-801",
                801,
                349,
                &payment.public_id.to_string(),
            ),
            status: "done",
            reason: None,
            payment: None,
        },
    )
    .await;
    let first = review_ops
        .list(PageRequest::new(Some(2), None))
        .await
        .unwrap();
    assert_eq!(
        first.items.iter().map(|i| i.id).collect::<Vec<_>>(),
        [stranger, broken]
    );
    assert_eq!(first.next, Some(broken));
    assert_eq!(first.total, 3, "всего на проверке, а не на странице");
    let rest = review_ops
        .list(PageRequest::new(Some(2), first.next))
        .await
        .unwrap();
    assert_eq!(rest.items.len(), 1);
    assert_eq!(rest.items[0].id, review);
    assert!(
        rest.items[0].telegram_refunded,
        "Telegram уже сообщил о возврате по этому списанию"
    );
    assert_eq!(rest.next, None);
    db.cleanup().await;
}

#[tokio::test]
async fn закрытие_требует_пояснения_и_не_обращается_к_telegram() {
    let Some((db, services, admin)) = setup_with(None).await else {
        return;
    };
    assert_eq!(services.bot_token(), None);
    let payment = stars_invoice(&services, 811, 350).await;
    let id = underpaid(&services, &payment, "stx-811", 811).await;
    let telegram = TelegramStars::with_api_base(UNREACHABLE);
    let review = StarsReview::with_telegram(&services, &telegram);
    for empty in ["", "   ", "\n\t"] {
        let err = review.close(id, empty, admin).await.unwrap_err();
        assert_eq!(err.code(), "stars.review_comment", "{empty:?}");
    }
    assert_eq!(stored(&services, id).await.status, STATUS_REVIEW);
    assert_eq!(resolutions(&services).await, 0);
    let long = format!("  Клиенту выдали подписку вручную. {}", "я".repeat(600));
    let closed = review.close(id, &long, admin).await.unwrap();
    assert_eq!(closed.id, id);
    assert_eq!(closed.charge_id, "stx-811");
    assert_eq!(closed.status, StarsResolutionKind::Closed);
    assert!(!closed.repeated);
    assert!(!closed.already_refunded);
    assert_eq!(closed.stars, Some(Stars(349)));
    assert_eq!(closed.telegram_user_id, Some(811));
    assert_eq!(closed.payment_id, Some(payment.id));
    assert_eq!(closed.resolved_at, moment());
    assert_eq!(closed.resolved_by, Some(admin));
    let note = closed.comment.clone().unwrap();
    assert!(note.starts_with("Клиенту выдали подписку вручную."));
    assert_eq!(note.chars().count(), COMMENT_MAX);
    let row = stored(&services, id).await;
    assert_eq!(row.status, STATUS_CLOSED);
    assert_eq!(row.resolved_at, Some(moment()));
    assert_eq!(row.resolved_by, Some(admin.0));
    assert_eq!(row.resolution_note.as_deref(), Some(note.as_str()));
    assert_eq!(
        row.last_error.as_deref(),
        Some("сумма не сошлась: Underpaid"),
        "причина проверки сохраняется"
    );
    let audit = audit_of(&services, CLOSED_ACTION).await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].actor_kind, "admin");
    assert_eq!(audit[0].actor_id, Some(admin.0));
    assert_eq!(audit[0].entity_kind.as_deref(), Some("payment"));
    assert_eq!(audit[0].entity_id, Some(payment.id.0));
    assert_eq!(audit[0].details["event_id"], id);
    assert_eq!(audit[0].details["charge_id"], "stx-811");
    assert_eq!(audit[0].details["stars"], 349);
    assert_eq!(audit[0].details["telegram_user_id"], 811);
    assert_eq!(audit[0].details["comment"], note.as_str());
    assert_eq!(audit[0].details["reason"], "сумма не сошлась: Underpaid");
    assert!(
        review
            .list(PageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let again = review.close(id, "другое пояснение", admin).await.unwrap();
    assert!(again.repeated);
    assert_eq!(again.status, StarsResolutionKind::Closed);
    assert_eq!(again.comment.as_deref(), Some(note.as_str()));
    assert_eq!(again.resolved_at, moment());
    assert_eq!(resolutions(&services).await, 1);
    let err = review.refund(id, None, admin).await.unwrap_err();
    assert_eq!(err.code(), "stars.review_closed");
    assert!(matches!(
        review.close(999_999, "нет такого", admin).await,
        Err(ServiceError::NotFound(_))
    ));
    assert_eq!(
        payment_status(&services, payment.id).await,
        PaymentStatus::Pending
    );
    db.cleanup().await;
}

async fn payment_status(services: &Services, id: PaymentId) -> PaymentStatus {
    let mut conn = services.store().conn().await.unwrap();
    conn.payments().get(id).await.unwrap().status
}

#[tokio::test]
async fn закрытое_без_платежа_событие_попадает_в_историю_плательщика() {
    let Some((db, services, admin)) = setup().await else {
        return;
    };
    let payer = stars_invoice(&services, 815, 350).await.client_id;
    let id = seed(
        &services,
        Seed {
            charge: "stx-815",
            event: SUCCESSFUL_PAYMENT,
            payload: payload(SUCCESSFUL_PAYMENT, "stx-815", 815, 120, "не uuid"),
            status: STATUS_REVIEW,
            reason: Some("платёж выставлен другой платёжной системой"),
            payment: None,
        },
    )
    .await;
    let unknown = seed(
        &services,
        Seed {
            charge: "stx-816",
            event: REFUNDED_PAYMENT,
            payload: serde_json::json!({"total_amount": 10}),
            status: STATUS_REVIEW,
            reason: None,
            payment: None,
        },
    )
    .await;
    let review = StarsReview::new(&services);
    review
        .close(id, "Разобрали с клиентом", admin)
        .await
        .unwrap();
    let closed = review
        .close(unknown, "Плательщик неизвестен", admin)
        .await
        .unwrap();
    assert_eq!(closed.telegram_user_id, None);
    assert_eq!(closed.stars, Some(Stars(10)));
    let audit = audit_of(&services, CLOSED_ACTION).await;
    assert_eq!(audit.len(), 2);
    assert_eq!(audit[0].entity_kind.as_deref(), Some("client"));
    assert_eq!(audit[0].entity_id, Some(payer.0));
    assert_eq!(audit[1].entity_kind, None);
    assert_eq!(audit[1].entity_id, None);
    assert_eq!(audit[1].details["event"], "refunded_payment");
    db.cleanup().await;
}

#[tokio::test]
async fn возврат_отклоняется_до_обращения_к_telegram_когда_возвращать_нечего() {
    let Some((db, services, admin)) = setup().await else {
        return;
    };
    let telegram = TelegramStars::with_api_base(UNREACHABLE);
    let review = StarsReview::with_telegram(&services, &telegram);
    let notice = seed(
        &services,
        Seed {
            charge: "stx-821",
            event: REFUNDED_PAYMENT,
            payload: payload(REFUNDED_PAYMENT, "stx-821", 821, 350, "не uuid"),
            status: STATUS_REVIEW,
            reason: None,
            payment: None,
        },
    )
    .await;
    let broken = seed(
        &services,
        Seed {
            charge: "stx-822",
            event: SUCCESSFUL_PAYMENT,
            payload: serde_json::json!({"total_amount": 350, "telegram_user_id": 0}),
            status: STATUS_REVIEW,
            reason: None,
            payment: None,
        },
    )
    .await;
    let credited_payment = stars_invoice(&services, 823, 350).await;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.payments()
            .transition(
                credited_payment.id,
                PaymentStatus::Paid,
                Some("stx-823"),
                None,
            )
            .await
            .unwrap();
    }
    let credited = seed(
        &services,
        Seed {
            charge: "stx-823",
            event: SUCCESSFUL_PAYMENT,
            payload: payload(SUCCESSFUL_PAYMENT, "stx-823", 823, 350, "другой счёт"),
            status: STATUS_REVIEW,
            reason: Some("транзакция провайдера уже привязана к другому заказу"),
            payment: None,
        },
    )
    .await;
    let mut settled = Vec::new();
    for (charge, status) in [
        ("stx-824", "pending"),
        ("stx-825", "done"),
        ("stx-826", "skipped"),
    ] {
        settled.push(
            seed(
                &services,
                Seed {
                    charge,
                    event: SUCCESSFUL_PAYMENT,
                    payload: payload(SUCCESSFUL_PAYMENT, charge, 824, 350, "не uuid"),
                    status,
                    reason: None,
                    payment: None,
                },
            )
            .await,
        );
    }
    let listed = review.list(PageRequest::default()).await.unwrap();
    assert_eq!(listed.total, 3);
    assert!(
        listed.items.iter().all(|item| !item.refundable),
        "в списке не должно быть кнопки возврата: {:?}",
        listed
            .items
            .iter()
            .map(|item| (&item.charge_id, item.refundable))
            .collect::<Vec<_>>()
    );
    for (id, code) in [
        (notice, "stars.review_not_payment"),
        (broken, "stars.review_no_payer"),
        (credited, "stars.review_credited"),
        (settled[0], "stars.review_status"),
        (settled[1], "stars.review_status"),
        (settled[2], "stars.review_status"),
    ] {
        let err = review.refund(id, Some("вернуть"), admin).await.unwrap_err();
        assert_eq!(err.code(), code, "событие {id}: {err}");
    }
    assert!(matches!(
        review.refund(999_999, None, admin).await,
        Err(ServiceError::NotFound(_))
    ));
    for id in settled {
        let err = review.close(id, "не на проверке", admin).await.unwrap_err();
        assert_eq!(err.code(), "stars.review_status");
    }
    for id in [notice, broken, credited] {
        assert_eq!(stored(&services, id).await.status, STATUS_REVIEW);
    }
    assert_eq!(resolutions(&services).await, 0);
    assert_eq!(
        payment_status(&services, credited_payment.id).await,
        PaymentStatus::Paid
    );
    db.cleanup().await;
}

#[tokio::test]
async fn без_токена_бота_возврат_отклоняется_а_событие_остаётся_на_проверке() {
    let Some((db, services, admin)) = setup_with(None).await else {
        return;
    };
    let payment = stars_invoice(&services, 831, 350).await;
    let id = underpaid(&services, &payment, "stx-831", 831).await;
    let telegram = TelegramStars::with_api_base(UNREACHABLE);
    let err = StarsReview::with_telegram(&services, &telegram)
        .refund(id, None, admin)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "stars.bot_token_missing");
    assert_eq!(stored(&services, id).await.status, STATUS_REVIEW);
    assert_eq!(resolutions(&services).await, 0);
    db.cleanup().await;
}

#[tokio::test]
async fn недоступный_telegram_оставляет_событие_на_проверке_и_пишет_аудит() {
    let Some((db, services, admin)) = setup().await else {
        return;
    };
    let payment = stars_invoice(&services, 841, 350).await;
    let id = underpaid(&services, &payment, "stx-841", 841).await;
    let telegram = TelegramStars::with_api_base(UNREACHABLE);
    let err = StarsReview::with_telegram(&services, &telegram)
        .refund(id, Some("клиент попросил вернуть"), admin)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ServiceError::Payment(sn_payments::PaymentError::Transport(_))
        ),
        "{err}"
    );
    assert!(!err.to_string().contains(TOKEN));
    let row = stored(&services, id).await;
    assert_eq!(row.status, STATUS_REVIEW);
    assert_eq!((row.resolved_at, row.resolved_by), (None, None));
    let failed = audit_of(&services, REFUND_FAILED_ACTION).await;
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].actor_id, Some(admin.0));
    assert_eq!(failed[0].entity_id, Some(payment.id.0));
    assert_eq!(failed[0].details["charge_id"], "stx-841");
    assert_eq!(failed[0].details["comment"], "клиент попросил вернуть");
    assert!(
        !failed[0].details.to_string().contains(TOKEN),
        "токен бота попал в аудит"
    );
    assert!(audit_of(&services, REFUNDED_ACTION).await.is_empty());
    assert_eq!(
        StarsReview::new(&services)
            .list(PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    db.cleanup().await;
}

#[tokio::test]
async fn ручная_отметка_оплаты_звёздами_остаётся_запрещённой() {
    let Some((db, services, admin)) = setup().await else {
        return;
    };
    let payment = stars_invoice(&services, 851, 350).await;
    underpaid(&services, &payment, "stx-851", 851).await;
    let err = services
        .admin()
        .mark_paid(payment.id, &MarkPaidRequest::default(), admin)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "payment.manual_stars");
    let err = services
        .inbox()
        .mark_paid(payment.id, admin, Some("оплатил звёздами"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "payment.manual_stars");
    assert_eq!(
        payment_status(&services, payment.id).await,
        PaymentStatus::Pending
    );
    db.cleanup().await;
}

#[test]
fn пояснение_обрезается_и_пустое_не_принимается() {
    assert_eq!(clean_comment(None), None);
    assert_eq!(clean_comment(Some("  ")), None);
    assert_eq!(clean_comment(Some(" да ")), Some("да".to_string()));
    assert_eq!(
        clean_comment(Some(&"ё".repeat(COMMENT_MAX + 50)))
            .unwrap()
            .chars()
            .count(),
        COMMENT_MAX
    );
}

#[test]
fn плательщик_и_сумма_читаются_из_события_даже_неполного() {
    let full = Payload::read(&payload(SUCCESSFUL_PAYMENT, "c", 42, 349, " счёт "));
    assert_eq!(full.stars, Some(Stars(349)));
    assert_eq!(full.payer, Some(42));
    assert_eq!(full.invoice.as_deref(), Some("счёт"));
    for broken in [
        serde_json::json!("мусор"),
        serde_json::json!([1, 2]),
        serde_json::json!({"total_amount": "349", "telegram_user_id": -5, "invoice_payload": " "}),
    ] {
        let parsed = Payload::read(&broken);
        assert_eq!(
            (parsed.stars, parsed.payer, parsed.invoice),
            (None, None, None),
            "{broken}"
        );
    }
}

async fn failed(services: &Services, payment: &Payment, charge: &str, payer: i64) -> i64 {
    seed(
        services,
        Seed {
            charge,
            event: SUCCESSFUL_PAYMENT,
            payload: payload(
                SUCCESSFUL_PAYMENT,
                charge,
                payer,
                payment.amount.minor,
                &payment.public_id.to_string(),
            ),
            status: STATUS_REVIEW,
            reason: Some("модуль не ответил"),
            payment: Some(payment.id),
        },
    )
    .await
}

#[tokio::test]
async fn провести_снова_можно_только_оплату_после_сбоя() {
    let Some((db, services, admin)) = setup().await else {
        return;
    };
    let review = StarsReview::new(&services);
    let short = stars_invoice(&services, 861, 350).await;
    let mismatch = underpaid(&services, &short, "stx-861", 861).await;
    let broke = stars_invoice(&services, 862, 350).await;
    let retryable = failed(&services, &broke, "stx-862", 862).await;
    let foreign = seed(
        &services,
        Seed {
            charge: "stx-863",
            event: SUCCESSFUL_PAYMENT,
            payload: payload(SUCCESSFUL_PAYMENT, "stx-863", 863, 350, "не uuid"),
            status: STATUS_REVIEW,
            reason: Some("платёж выставлен другой платёжной системой"),
            payment: None,
        },
    )
    .await;
    let refund_notice = seed(
        &services,
        Seed {
            charge: "stx-862",
            event: REFUNDED_PAYMENT,
            payload: payload(
                REFUNDED_PAYMENT,
                "stx-862",
                862,
                350,
                &broke.public_id.to_string(),
            ),
            status: STATUS_REVIEW,
            reason: Some("сбой"),
            payment: Some(broke.id),
        },
    )
    .await;
    let other = stars_invoice(&services, 864, 350).await;
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.payments()
            .transition(other.id, PaymentStatus::Paid, Some("stx-864-a"), None)
            .await
            .unwrap();
    }
    let paid_elsewhere = failed(&services, &other, "stx-864-b", 864).await;
    let page = review.list(PageRequest::default()).await.unwrap();
    let flag = |id: i64| {
        page.items
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.retryable)
            .unwrap()
    };
    assert!(flag(retryable));
    for id in [mismatch, foreign, refund_notice, paid_elsewhere] {
        assert!(!flag(id), "событие {id}");
    }
    let json = serde_json::to_value(&page.items[0]).unwrap();
    assert!(json["retryable"].is_boolean());
    for id in [mismatch, foreign, refund_notice, paid_elsewhere] {
        let err = review.retry(id, admin).await.unwrap_err();
        assert_eq!(err.code(), "stars.review_not_retryable", "событие {id}");
        assert_eq!(stored(&services, id).await.status, STATUS_REVIEW);
    }
    assert!(audit_of(&services, RETRIED_ACTION).await.is_empty());
    review.close(mismatch, "разобрали", admin).await.unwrap();
    assert_eq!(
        review.retry(mismatch, admin).await.unwrap_err().code(),
        "stars.review_closed"
    );
    assert!(matches!(
        review.retry(999_999, admin).await,
        Err(ServiceError::NotFound(_))
    ));
    let done = seed(
        &services,
        Seed {
            charge: "stx-865",
            event: SUCCESSFUL_PAYMENT,
            payload: payload(SUCCESSFUL_PAYMENT, "stx-865", 865, 350, "не uuid"),
            status: "done",
            reason: None,
            payment: None,
        },
    )
    .await;
    assert_eq!(
        review.retry(done, admin).await.unwrap_err().code(),
        "stars.review_status"
    );
    let outcome = review.retry(retryable, admin).await.unwrap();
    assert_eq!(outcome.id, retryable);
    assert_eq!(outcome.charge_id, "stx-862");
    let expected = match outcome.outcome {
        StarsRetryOutcome::Done => "done",
        StarsRetryOutcome::Review => STATUS_REVIEW,
        StarsRetryOutcome::Pending => "pending",
    };
    assert_eq!(stored(&services, retryable).await.status, expected);
    let retried = audit_of(&services, RETRIED_ACTION).await;
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0].actor_id, Some(admin.0));
    assert_eq!(retried[0].entity_id, Some(broke.id.0));
    assert_eq!(retried[0].details["reason"], "модуль не ответил");
    db.cleanup().await;
}
