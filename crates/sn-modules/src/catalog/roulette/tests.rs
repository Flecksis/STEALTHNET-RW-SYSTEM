use super::*;
use crate::catalog::testing::{TestCore, client, context, order, paid_payment, pooled, usd};
use sn_domain::ids::{PeriodId, RemnaUserId, TariffId};
use sn_domain::ledger::Wallet;
use sn_domain::money::{Amount, PayCurrency};
use sn_domain::order::PurchaseIntent;
use sn_domain::pricing::{Quote, QuoteLine, QuoteLineKind};
use sn_domain::subscription::{AutoRenewState, RemnaLink, SubscriptionKind};
use sn_store::testing::test_store;
use sn_store::{NewSubscription, Store};
use std::time::Duration as StdDuration;

#[test]
fn выбор_приза_по_весам() {
    let weights = [(0, 1), (2, 3)];
    assert_eq!(pick(&weights, 0), Some(0));
    assert_eq!(pick(&weights, 1), Some(2));
    assert_eq!(pick(&weights, 3), Some(2));
    assert_eq!(pick(&weights, 4), Some(0));
    assert_eq!(pick(&[], 5), None);
    assert_eq!(pick(&[(1, 0)], 5), None);
}

fn sector(prize: PrizeDraft, weight: u32) -> SectorDraft {
    SectorDraft {
        prize,
        weight,
        label: None,
    }
}

fn config(sectors: Vec<SectorDraft>) -> ConfigDraft {
    ConfigDraft {
        sectors,
        free_every_days: Some(7),
        free_for: FreeFor::All,
        per_purchase: true,
        purchase_min: None,
        budget: BudgetDraft {
            period: Period::Month,
            balance: None,
            days: None,
            promos: None,
        },
        version: None,
    }
}

#[test]
fn настройки_рулетки_проверяются() {
    let ok = validate(
        &config(vec![
            sector(PrizeDraft::Empty, 3),
            sector(
                PrizeDraft::Balance {
                    amount: "2.50".into(),
                },
                1,
            ),
        ]),
        Currency::Usd,
    )
    .unwrap();
    assert_eq!(ok.sectors.len(), 2);
    let views = sector_views(&ok);
    assert_eq!(views[0].chance, 75.0);
    assert_eq!(views[1].chance, 25.0);
    assert_eq!(views[1].title, "$2.50 на баланс");
    assert!(validate(&config(vec![sector(PrizeDraft::Empty, 1)]), Currency::Usd).is_err());
    assert!(
        validate(
            &config(vec![
                sector(PrizeDraft::Empty, 0),
                sector(PrizeDraft::Empty, 0)
            ]),
            Currency::Usd
        )
        .is_err()
    );
    let mut nothing = config(vec![
        sector(PrizeDraft::Empty, 1),
        sector(PrizeDraft::Empty, 1),
    ]);
    nothing.free_every_days = None;
    nothing.per_purchase = false;
    assert!(validate(&nothing, Currency::Usd).is_err());
    let mut rub = config(vec![
        sector(PrizeDraft::Empty, 1),
        sector(
            PrizeDraft::Balance {
                amount: "10.50".into(),
            },
            1,
        ),
    ]);
    let err = validate(&rub, Currency::Rub).unwrap_err();
    assert!(err.to_string().contains("Приз 2"));
    rub.sectors[1].prize = PrizeDraft::Balance {
        amount: "10".into(),
    };
    rub.budget.balance = Some("500".into());
    let saved = validate(&rub, Currency::Rub).unwrap();
    assert_eq!(
        saved.budget.balance,
        Some(Money::new(50_000, Currency::Rub))
    );
    let starter = Config::starter(Currency::Rub);
    assert!(starter.sectors.len() >= SECTORS_MIN);
}

async fn live_sub(store: &Store, owner: ClientId, kind: SubscriptionKind) -> SubscriptionId {
    let mut conn = store.conn().await.unwrap();
    let sub = conn
        .subscriptions()
        .create(&NewSubscription {
            client_id: owner,
            tariff_id: None,
            kind,
            extra_devices: 0,
            autorenew: AutoRenewState::default(),
        })
        .await
        .unwrap();
    conn.subscriptions()
        .link_remna(
            sub.id,
            &RemnaLink {
                user_id: RemnaUserId(sub.id.0 + 7000),
                username: format!("sn_wheel_{}", sub.id),
                short_uuid: None,
            },
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE subscriptions SET expire_at = now() + interval '30 days', status = 'active' WHERE id = $1",
    )
    .bind(sub.id.0)
    .execute(conn.conn())
    .await
    .unwrap();
    sub.id
}

async fn balance(store: &Store, owner: ClientId) -> i64 {
    let mut conn = store.conn().await.unwrap();
    conn.ledger()
        .balance(owner, Wallet::Main, Currency::Usd)
        .await
        .unwrap()
        .minor
}

fn quote(total: Money) -> Quote {
    Quote {
        currency: total.currency,
        lines: vec![QuoteLine {
            kind: QuoteLineKind::Period {
                tariff_id: TariffId(1),
                period_id: PeriodId(1),
                days: 30,
                months: 1,
            },
            label: "Подписка · 30 дней".into(),
            amount: total,
            stars: None,
        }],
        subtotal: total,
        discount: Money::zero(total.currency),
        total,
        stars: None,
        days: 30,
        extra_devices: 0,
    }
}

async fn paid(ctx: &ModuleContext, owner: ClientId, total: Money) -> Order {
    let created = order(
        &ctx.store,
        owner,
        OrderKind::New,
        PurchaseIntent::Subscription {
            tariff_id: TariffId(1),
            period_id: PeriodId(1),
            extra_devices: 0,
            target: None,
        },
        quote(total),
    )
    .await;
    let payment = paid_payment(
        &ctx.store,
        created.id,
        owner,
        "balance",
        Amount::new(total.minor, PayCurrency::Usd),
    )
    .await;
    let mut tx = ctx.store.begin().await.unwrap();
    RouletteModule
        .on_order_paid(ctx, &mut tx, &created, &payment)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    created
}

#[tokio::test]
async fn сервер_решает_исход_выдаёт_приз_и_не_повторяет_вращение() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let player = client(&db.store).await;
    assert!(!ready(&ctx).await.unwrap());
    let unset = state(&ctx, player).await.unwrap();
    assert!(!unset.ready);
    assert!(!unset.spins.free_ready);
    let refused = spin(&ctx, player, Uuid::new_v4(), None).await.unwrap_err();
    assert!(matches!(
        refused,
        RouletteError::Rejected {
            code: "roulette.not_ready",
            ..
        }
    ));
    let mut draft = config(vec![
        sector(PrizeDraft::Empty, 0),
        sector(PrizeDraft::Balance { amount: "3".into() }, 5),
    ]);
    draft.version = Some(0);
    let saved = save(&ctx, &draft, None).await.unwrap();
    assert_eq!(saved.version, 1);
    assert!(ready(&ctx).await.unwrap());
    assert!(save(&ctx, &draft, None).await.is_err());
    let before = state(&ctx, player).await.unwrap();
    assert!(before.spins.free_ready);
    assert_eq!(before.spins.total, 1);
    let request = Uuid::new_v4();
    let first = spin(&ctx, player, request, None).await.unwrap();
    assert_eq!(first.spin.sector, 1);
    assert_eq!(first.spin.state, "issued");
    assert_eq!(first.spin.note.as_deref(), Some("Деньги уже на балансе"));
    assert_eq!(balance(&db.store, player).await, 300);
    assert!(!first.state.spins.free_ready);
    assert!(first.state.spins.free_next_at.is_some());
    let repeated = spin(&ctx, player, request, None).await.unwrap();
    assert_eq!(repeated.spin.id, first.spin.id);
    assert_eq!(balance(&db.store, player).await, 300);
    let none = spin(&ctx, player, Uuid::new_v4(), None).await.unwrap_err();
    assert!(matches!(
        none,
        RouletteError::Rejected {
            code: "roulette.no_spins",
            ..
        }
    ));
    let history = state(&ctx, player).await.unwrap().history;
    assert_eq!(history.len(), 1);
    let view = settings(&ctx).await.unwrap();
    assert_eq!(view.tally.spins, 1);
    assert_eq!(view.spent.balance.minor, 300);
    let listed = spins(&ctx, PageRequest::default()).await.unwrap();
    assert_eq!(listed.items[0].client_id, player);
    db.cleanup().await;
}

#[tokio::test]
async fn вращение_за_покупку_сгорает_при_возврате() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let buyer = client(&db.store).await;
    let mut draft = config(vec![
        sector(PrizeDraft::Empty, 1),
        sector(PrizeDraft::Empty, 1),
    ]);
    draft.free_every_days = None;
    draft.purchase_min = Some("2".into());
    save(&ctx, &draft, None).await.unwrap();
    paid(&ctx, buyer, usd(150)).await;
    assert_eq!(state(&ctx, buyer).await.unwrap().spins.purchase, 0);
    let first = paid(&ctx, buyer, usd(500)).await;
    let second = paid(&ctx, buyer, usd(700)).await;
    let mut tx = db.store.begin().await.unwrap();
    assert!(!grant_for_order(&mut tx, &first).await.unwrap());
    tx.commit().await.unwrap();
    assert_eq!(state(&ctx, buyer).await.unwrap().spins.purchase, 2);
    let spun = spin(&ctx, buyer, Uuid::new_v4(), None).await.unwrap();
    assert_eq!(spun.spin.state, "empty");
    assert_eq!(spun.state.spins.purchase, 1);
    RouletteModule
        .on_event(
            &ctx,
            &DomainEvent::PaymentRefunded {
                payment_id: sn_domain::ids::PaymentId(0),
                order_id: second.id,
                client_id: buyer,
                amount: Amount::new(700, PayCurrency::Usd),
            },
        )
        .await
        .unwrap();
    assert_eq!(state(&ctx, buyer).await.unwrap().spins.purchase, 0);
    assert!(spin(&ctx, buyer, Uuid::new_v4(), None).await.is_err());
    db.cleanup().await;
}

#[tokio::test]
async fn бюджет_и_подписка_ограничивают_призы() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let plain = client(&db.store).await;
    let wheel = |empty: u32| {
        let mut draft = config(vec![
            sector(PrizeDraft::Days { days: 5 }, 100),
            sector(PrizeDraft::Balance { amount: "4".into() }, 100),
            sector(PrizeDraft::Empty, empty),
        ]);
        draft.budget.balance = Some("4".into());
        draft.free_every_days = Some(1);
        draft
    };
    save(&ctx, &wheel(0), None).await.unwrap();
    let won = spin(&ctx, plain, Uuid::new_v4(), None).await.unwrap();
    assert_eq!(won.spin.sector, 1);
    assert!(core.days().is_empty());
    {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query(
            "UPDATE roulette_spins SET created_at = now() - interval '2 days' WHERE client_id = $1",
        )
        .bind(plain.0)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let dry = spin(&ctx, plain, Uuid::new_v4(), None).await.unwrap_err();
    assert!(matches!(
        dry,
        RouletteError::Rejected {
            code: "roulette.empty",
            ..
        }
    ));
    save(&ctx, &wheel(1), None).await.unwrap();
    let empty = spin(&ctx, plain, Uuid::new_v4(), None).await.unwrap();
    assert_eq!(empty.spin.sector, 2);
    assert_eq!(empty.spin.state, "empty");
    save(&ctx, &wheel(0), None).await.unwrap();
    let owner = client(&db.store).await;
    let sub = live_sub(&db.store, owner, SubscriptionKind::Paid).await;
    let days = spin(&ctx, owner, Uuid::new_v4(), Some(sub)).await.unwrap();
    assert_eq!(days.spin.sector, 0);
    assert_eq!(days.spin.state, "issued");
    assert_eq!(core.days(), vec![(sub, 5, "рулетка".to_string())]);
    db.cleanup().await;
}

#[tokio::test]
async fn бесплатное_вращение_только_тем_кому_разрешено() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let mut draft = config(vec![
        sector(PrizeDraft::Empty, 1),
        sector(PrizeDraft::Empty, 1),
    ]);
    draft.free_for = FreeFor::Paid;
    save(&ctx, &draft, None).await.unwrap();
    let trial = client(&db.store).await;
    live_sub(&db.store, trial, SubscriptionKind::Trial).await;
    let seen = state(&ctx, trial).await.unwrap();
    assert!(!seen.spins.free_ready);
    assert_eq!(
        seen.spins.free_blocked.as_deref(),
        Some("Бесплатно крутить можно клиентам с оплаченной подпиской")
    );
    assert!(spin(&ctx, trial, Uuid::new_v4(), None).await.is_err());
    let payer = client(&db.store).await;
    live_sub(&db.store, payer, SubscriptionKind::Paid).await;
    assert!(state(&ctx, payer).await.unwrap().spins.free_ready);
    let blocked = client(&db.store).await;
    {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query("UPDATE clients SET status = 'blocked' WHERE id = $1")
            .bind(blocked.0)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let refused = spin(&ctx, blocked, Uuid::new_v4(), None).await.unwrap_err();
    assert!(matches!(
        refused,
        RouletteError::Rejected {
            code: "roulette.blocked",
            ..
        }
    ));
    db.cleanup().await;
}

#[tokio::test]
async fn два_одновременных_вращения_тратят_одну_попытку() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = pooled(&db, 6, StdDuration::from_secs(5)).await;
    let core = TestCore::new(&store);
    let ctx = context(&store, &core, Currency::Usd);
    save(
        &ctx,
        &config(vec![
            sector(PrizeDraft::Balance { amount: "1".into() }, 1),
            sector(PrizeDraft::Empty, 0),
        ]),
        None,
    )
    .await
    .unwrap();
    let player = client(&store).await;
    let (a, b) = tokio::join!(
        spin(&ctx, player, Uuid::new_v4(), None),
        spin(&ctx, player, Uuid::new_v4(), None)
    );
    assert_eq!(u8::from(a.is_ok()) + u8::from(b.is_ok()), 1);
    assert_eq!(balance(&store, player).await, 100);
    store.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn промокод_в_рулетке_требует_модуль_промокодов() {
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.modules()
            .ensure_defaults(&[("promo", true)])
            .await
            .unwrap();
        sqlx::query("UPDATE module_states SET enabled = false WHERE module = 'promo'")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let draft = config(vec![
        sector(
            PrizeDraft::Promo {
                percent: 15,
                valid_days: 5,
            },
            1,
        ),
        sector(PrizeDraft::Empty, 1),
    ]);
    assert!(save(&ctx, &draft, None).await.is_err());
    {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query("UPDATE module_states SET enabled = true WHERE module = 'promo'")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let mut only = draft.clone();
    only.sectors[1].weight = 0;
    save(&ctx, &only, None).await.unwrap();
    let player = client(&db.store).await;
    let won = spin(&ctx, player, Uuid::new_v4(), None).await.unwrap();
    let code = won.spin.code.unwrap();
    assert!(code.starts_with("WIN-"));
    assert_eq!(
        won.spin.note.as_deref(),
        Some("Промокод действует 5\u{a0}дней")
    );
    db.cleanup().await;
}

async fn json_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

#[tokio::test]
async fn рулетка_настраивается_и_крутится_через_маршруты() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sn_domain::access::{AdminPrincipal, AdminRole, ClientChannel, ClientPrincipal};
    use std::sync::Arc;
    use tower::ServiceExt;
    let Some(db) = test_store().await else {
        return;
    };
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Usd);
    let registry = Arc::new(crate::registry::ModuleRegistry::new(crate::catalog::all()).unwrap());
    registry
        .set_enabled(&db.store, ModuleId::Roulette, true, None)
        .await
        .unwrap();
    let admin_id = {
        let mut conn = db.store.conn().await.unwrap();
        conn.admins()
            .create(&sn_store::NewAdmin {
                email: "owner@example.com".into(),
                password_hash: "x".into(),
                role: AdminRole::Owner,
                must_change_password: false,
            })
            .await
            .unwrap()
            .id
    };
    let admin = AdminPrincipal {
        admin_id,
        role: AdminRole::Admin,
        permissions: Vec::new(),
        session_id: 1,
        api_key: None,
    };
    let admin_router = registry.admin_router(ctx.clone());
    let saved = admin_router
        .clone()
        .oneshot(
            Request::put("/roulette/settings")
                .header("content-type", "application/json")
                .extension(admin.clone())
                .body(Body::from(
                    serde_json::json!({
                        "sectors": [
                            {"prize": {"kind": "balance", "amount": "1.25"}, "weight": 3},
                            {"prize": {"kind": "empty"}, "weight": 0, "label": "Мимо"}
                        ],
                        "free_every_days": 3,
                        "free_for": "all",
                        "per_purchase": false,
                        "budget": {"period": "week", "balance": "10"}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let saved = json_of(saved).await;
    assert_eq!(saved["version"], 1);
    assert_eq!(saved["sectors"][1]["title"], "Мимо");
    assert_eq!(saved["sectors"][1]["custom"], true);
    let player = client(&db.store).await;
    let principal = |full: bool| ClientPrincipal {
        client_id: player,
        channel: ClientChannel::Miniapp,
        full_access: full,
        session_id: Some(1),
    };
    let client_router = registry.client_router(ctx);
    let spin = |full: bool, request: Uuid| {
        Request::post("/roulette/spin")
            .header("content-type", "application/json")
            .extension(principal(full))
            .body(Body::from(
                serde_json::json!({ "request_id": request }).to_string(),
            ))
            .unwrap()
    };
    let limited = client_router
        .clone()
        .oneshot(spin(false, Uuid::new_v4()))
        .await
        .unwrap();
    assert_eq!(limited.status(), StatusCode::FORBIDDEN);
    let state = json_of(
        client_router
            .clone()
            .oneshot(
                Request::get("/roulette/state")
                    .extension(principal(false))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(state["ready"], true);
    assert_eq!(state["spins"]["total"], 1);
    let won = client_router
        .clone()
        .oneshot(spin(true, Uuid::new_v4()))
        .await
        .unwrap();
    assert_eq!(won.status(), StatusCode::OK);
    let won = json_of(won).await;
    assert_eq!(won["spin"]["sector"], 0);
    assert_eq!(won["state"]["spins"]["total"], 0);
    assert!(won["state"]["spins"]["free_next_at"].is_string());
    let empty = client_router
        .oneshot(spin(true, Uuid::new_v4()))
        .await
        .unwrap();
    assert_eq!(empty.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_of(empty).await["error"]["code"], "roulette.no_spins");
    assert_eq!(balance(&db.store, player).await, 125);
    db.cleanup().await;
}
