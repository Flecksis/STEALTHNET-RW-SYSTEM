use super::*;
use sn_domain::ids::SquadId;
use sn_domain::module::ModuleId;
use sn_domain::order::{OrderKind, OrderStatus};
use sn_domain::settings::keys;
use sn_domain::subscription::SubscriptionKind;
use sn_domain::traffic::TrafficReset;
use sn_modules::catalog::gifts::{GiftStatus, TEMPLATE_READY, TEMPLATE_REDEEMED};
use sn_modules::catalog::promolinks::{self, NewLink};

fn remnawave() -> Arc<sn_remnawave::fake::FakeRemnawave> {
    let fake = Arc::new(sn_remnawave::fake::FakeRemnawave::new());
    fake.set_squads(vec![sn_remnawave::InternalSquad {
        uuid: uuid::Uuid::from_u128(1),
        name: "Германия".into(),
        view_position: 0,
        info: sn_remnawave::SquadInfo::default(),
        inbounds: Vec::new(),
    }]);
    fake
}

async fn enable(services: &Services, module: ModuleId) {
    services
        .modules()
        .set_enabled(services.store(), module, true, None)
        .await
        .unwrap();
}

async fn standard(db: &TestDb) -> sn_domain::tariff::Tariff {
    let mut tx = db.store.begin().await.unwrap();
    let category = tx
        .tariffs()
        .create_category(&NewCategory {
            name: "Основные".into(),
            sort: 0,
            visible: true,
        })
        .await
        .unwrap();
    let tariff = tx
        .tariffs()
        .create(&NewTariff {
            category_id: category.id,
            name: "Стандарт".into(),
            description: None,
            on_sale: true,
            sort: 0,
            devices: DevicePolicy {
                included: 3,
                extra: None,
            },
            traffic: TrafficPolicy::UNLIMITED,
            traffic_addon: None,
            squads: vec![SquadId(uuid::Uuid::from_u128(1))],
            external_squad: None,
            periods: vec![NewPeriod {
                days: 30,
                prices: CurrencyPrices {
                    usd: Some(299),
                    rub: Some(19_900),
                },
                stars: Some(Stars(150)),
                sort: 0,
                active: true,
            }],
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    tariff
}

async fn top_up(db: &TestDb, client_id: ClientId, minor: i64) {
    let mut conn = db.store.conn().await.unwrap();
    conn.ledger()
        .append(&sn_domain::ledger::NewLedgerEntry {
            client_id,
            wallet: sn_domain::ledger::Wallet::Main,
            amount: sn_domain::money::Money::new(minor, sn_domain::money::Currency::Usd),
            kind: sn_domain::ledger::LedgerKind::TopUp,
            order_id: None,
            payment_id: None,
            admin_id: None,
            comment: None,
            idempotency_key: Some(format!("test:topup:{}", client_id.0)),
        })
        .await
        .unwrap();
}

async fn notifications(db: &TestDb, template: &str) -> Vec<(ClientId, serde_json::Value)> {
    let mut conn = db.store.conn().await.unwrap();
    let rows: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT payload FROM jobs WHERE kind = 'notify' ORDER BY id")
            .fetch_all(conn.conn())
            .await
            .unwrap();
    rows.into_iter()
        .filter_map(|payload| serde_json::from_value::<JobPayload>(payload).ok())
        .filter_map(|job| match job {
            JobPayload::Notify {
                client_id,
                template: found,
                params,
            } if found == template => Some((client_id, params)),
            _ => None,
        })
        .collect()
}

async fn orders_of(db: &TestDb, client_id: ClientId) -> Vec<sn_domain::order::Order> {
    let mut conn = db.store.conn().await.unwrap();
    conn.orders()
        .for_client(
            client_id,
            sn_domain::paging::PageRequest::new(Some(20), None),
        )
        .await
        .unwrap()
        .items
}

#[tokio::test]
async fn подарок_в_боте_покупается_и_активируется_другом_по_ссылке() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    enable(&services, ModuleId::Gifts).await;
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(
                keys::BOT_USERNAME,
                &serde_json::json!("stealth_test_bot"),
                None,
            )
            .await
            .unwrap();
    }
    services.set_remna(Some(remnawave()));
    let tariff = standard(&db).await;
    let period = tariff.periods[0].id;
    handle_update(&ctx, text_update(1, 731, "/start")).await;
    assert!(
        last_labels(&server)
            .await
            .contains(&"Подарить подписку".to_string()),
        "{:?}",
        last_labels(&server).await
    );
    let giver = client_of(&services, 731).await.unwrap();
    handle_update(&ctx, callback_update(2, 731, "x:gifts")).await;
    assert!(last_text(&server).await.contains("Подписка в подарок"));
    let home = last_buttons(&server).await;
    assert!(home.contains(&"gb".to_string()), "{home:?}");
    assert!(home.contains(&"ga".to_string()), "{home:?}");
    assert!(!home.contains(&"gl".to_string()), "{home:?}");
    handle_update(&ctx, callback_update(3, 731, "gb")).await;
    assert!(
        last_buttons(&server)
            .await
            .contains(&format!("gt:{}", tariff.id))
    );
    handle_update(&ctx, callback_update(4, 731, &format!("gt:{}", tariff.id))).await;
    assert!(last_text(&server).await.contains("На какой срок подарить?"));
    let pick = format!("gp:{}:{}", tariff.id, period);
    assert!(last_buttons(&server).await.contains(&pick));
    assert!(
        last_labels(&server)
            .await
            .contains(&"1\u{a0}месяц · $2.99".to_string())
    );
    top_up(&db, giver.id, 1_000).await;
    handle_update(&ctx, callback_update(5, 731, &pick)).await;
    let quote = last_text(&server).await;
    assert!(
        quote.contains("Подарок: «Стандарт» на 1\u{a0}месяц — $2.99 или 150\u{a0}звёзд"),
        "{quote}"
    );
    let pay = format!("gy:{}:{}:balance", tariff.id, period);
    assert!(last_buttons(&server).await.contains(&pay));
    handle_update(&ctx, callback_update(6, 731, &pay)).await;
    assert!(
        last_text(&server)
            .await
            .contains("Код подарка придёт сюда через минуту")
    );
    let order = orders_of(&db, giver.id)
        .await
        .into_iter()
        .find(|o| o.kind == OrderKind::Module)
        .expect("заказ подарка");
    assert_eq!(order.quote.total.minor, 299);
    let outcome = services.fulfillment().run(order.id).await.unwrap();
    assert!(
        matches!(
            outcome,
            sn_services::fulfillment::FulfillmentOutcome::Done { .. }
        ),
        "{outcome:?}"
    );
    let ready = notifications(&db, TEMPLATE_READY).await;
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].0, giver.id);
    let code = ready[0].1["code"].as_str().unwrap().to_string();
    let content = services.bot_content();
    let rendered = crate::notify::render(
        &content.content,
        TEMPLATE_READY,
        &ready[0].1,
        services.settings().remna_offset(),
    )
    .unwrap();
    assert!(
        rendered.text.contains(&format!("<code>{code}</code>")),
        "{}",
        rendered.text
    );
    assert!(
        rendered
            .text
            .contains(&format!("https://t.me/stealth_test_bot?start=gift_{code}")),
        "{}",
        rendered.text
    );
    handle_update(&ctx, text_update(7, 732, &format!("/start gift_{code}"))).await;
    let preview = last_text(&server).await;
    assert!(
        preview.contains("<b>Подарок:</b> подписка «Стандарт» на 1\u{a0}месяц."),
        "{preview}"
    );
    assert!(preview.contains("Подписка начнёт работать сразу после активации."));
    assert!(last_buttons(&server).await.contains(&"gr".to_string()));
    handle_update(&ctx, callback_update(8, 732, "gr")).await;
    assert!(
        last_text(&server)
            .await
            .contains("Подарок активирован: «Стандарт» на 1\u{a0}месяц"),
        "{}",
        last_text(&server).await
    );
    let friend = client_of(&services, 732).await.unwrap();
    let granted = orders_of(&db, friend.id)
        .await
        .into_iter()
        .find(|o| o.kind == OrderKind::ModuleGrant)
        .expect("заказ выдачи подарка");
    assert_eq!(granted.status, OrderStatus::Paid);
    let delivered = services.fulfillment().run(granted.id).await.unwrap();
    assert!(
        matches!(
            delivered,
            sn_services::fulfillment::FulfillmentOutcome::Done { .. }
        ),
        "{delivered:?}"
    );
    let subscriptions = services
        .subscriptions()
        .list_for_client(friend.id)
        .await
        .unwrap();
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(subscriptions[0].subscription.kind, SubscriptionKind::Gift);
    let redeemed = notifications(&db, TEMPLATE_REDEEMED).await;
    assert_eq!(redeemed.len(), 1);
    assert_eq!(redeemed[0].0, giver.id);
    handle_update(&ctx, text_update(9, 732, &format!("/start gift_{code}"))).await;
    assert!(
        last_text(&server)
            .await
            .contains("Вы уже активировали этот подарок")
    );
    handle_update(&ctx, text_update(10, 733, &format!("/start gift_{code}"))).await;
    assert!(
        last_text(&server)
            .await
            .contains("Этот подарок уже активирован")
    );
    handle_update(&ctx, callback_update(11, 731, "gl")).await;
    let mine = last_text(&server).await;
    assert!(mine.contains(&code), "{mine}");
    assert!(mine.contains("Активирован"), "{mine}");
    let mut conn = db.store.conn().await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM gifts WHERE code = $1")
        .bind(&code)
        .fetch_one(conn.conn())
        .await
        .unwrap();
    assert_eq!(status, "redeemed");
    drop(conn);
    let _ = GiftStatus::Redeemed;
    db.cleanup().await;
}

#[tokio::test]
async fn возврат_активированного_подарка_отключает_подписку_получателя() {
    let Some((db, services, _server, ctx)) = setup().await else {
        return;
    };
    enable(&services, ModuleId::Gifts).await;
    {
        let mut conn = db.store.conn().await.unwrap();
        conn.settings()
            .set(
                keys::BOT_USERNAME,
                &serde_json::json!("stealth_test_bot"),
                None,
            )
            .await
            .unwrap();
    }
    let remna = remnawave();
    services.set_remna(Some(remna.clone()));
    let tariff = standard(&db).await;
    let period = tariff.periods[0].id;
    handle_update(&ctx, text_update(1, 741, "/start")).await;
    let giver = client_of(&services, 741).await.unwrap();
    top_up(&db, giver.id, 1_000).await;
    handle_update(
        &ctx,
        callback_update(2, 741, &format!("gy:{}:{}:balance", tariff.id, period)),
    )
    .await;
    let order = orders_of(&db, giver.id)
        .await
        .into_iter()
        .find(|o| o.kind == OrderKind::Module)
        .expect("заказ подарка");
    services.fulfillment().run(order.id).await.unwrap();
    let code = notifications(&db, TEMPLATE_READY).await[0].1["code"]
        .as_str()
        .unwrap()
        .to_string();
    handle_update(&ctx, text_update(3, 742, &format!("/start gift_{code}"))).await;
    handle_update(&ctx, callback_update(4, 742, "gr")).await;
    let friend = client_of(&services, 742).await.unwrap();
    let granted = orders_of(&db, friend.id)
        .await
        .into_iter()
        .find(|o| o.kind == OrderKind::ModuleGrant)
        .expect("заказ выдачи подарка");
    services.fulfillment().run(granted.id).await.unwrap();
    let gifted = services
        .subscriptions()
        .list_for_client(friend.id)
        .await
        .unwrap()[0]
        .subscription
        .clone();
    let payment = {
        let mut conn = db.store.conn().await.unwrap();
        conn.payments()
            .for_order(order.id)
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.status.is_paid_like())
            .expect("оплата подарка")
    };
    let options = services.inbox().refund_options(payment.id).await.unwrap();
    assert!(options.subscription, "{options:?}");
    let gift = options.gift.clone().expect("получатель подарка");
    assert_eq!(gift.client_id, friend.id);
    assert_eq!(gift.subscription_id, Some(gifted.id));
    assert_eq!(options.days, 30);
    let admin = services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap()
        .0;
    services
        .inbox()
        .refund(
            payment.id,
            &sn_services::inbox::RefundCommand {
                amount: None,
                method: sn_domain::payment::RefundMethod::Balance,
                subscription_action: sn_domain::payment::RefundSubscriptionAction::Disable,
                reason: Some("подарок вернули".into()),
                key: None,
                outside: false,
            },
            admin,
        )
        .await
        .unwrap();
    let link = gifted
        .remna
        .clone()
        .expect("пользователь Remnawave получателя");
    let user = services
        .remna()
        .unwrap()
        .get_user(link.user_id)
        .await
        .unwrap();
    assert_eq!(user.status, sn_remnawave::UserStatus::Disabled);
    db.cleanup().await;
}

#[tokio::test]
async fn код_подарка_можно_прислать_сообщением() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    enable(&services, ModuleId::Gifts).await;
    handle_update(&ctx, text_update(1, 741, "/start")).await;
    handle_update(&ctx, callback_update(2, 741, "ga")).await;
    assert!(last_text(&server).await.contains("Пришлите код подарка"));
    assert_eq!(
        Session::load(&services, 741).await.unwrap().awaiting,
        Some(Awaiting::GiftCode)
    );
    handle_update(&ctx, text_update(3, 741, "abcd efgh jkmn")).await;
    assert!(
        last_text(&server)
            .await
            .contains("Подарка с таким кодом нет")
    );
    assert!(last_buttons(&server).await.contains(&"ga".to_string()));
    services
        .modules()
        .set_enabled(services.store(), ModuleId::Gifts, false, None)
        .await
        .unwrap();
    handle_update(&ctx, text_update(4, 741, "/start gift_ABCD-EFGH-JKMN")).await;
    assert!(
        last_text(&server).await.contains("Подписки пока нет"),
        "{}",
        last_text(&server).await
    );
    db.cleanup().await;
}

#[tokio::test]
async fn промо_ссылка_в_боте_выдаёт_подписку_один_раз_и_объясняет_отказы() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    enable(&services, ModuleId::Promolinks).await;
    services.set_remna(Some(remnawave()));
    let module_ctx = services.module_context();
    let link = |name: &str, max: Option<u32>| NewLink {
        name: name.into(),
        code: None,
        squads: vec![SquadId(uuid::Uuid::from_u128(1))],
        days: 7,
        traffic_gb: Some(10),
        traffic_reset: TrafficReset::NoReset,
        devices: 1,
        max_activations: max,
        expires_at: None,
    };
    let blogger = promolinks::create(&module_ctx, &link("Блогер", Some(1)), None)
        .await
        .unwrap()
        .link;
    handle_update(
        &ctx,
        text_update(1, 751, &format!("/start pl_{}__s_youtube", blogger.code)),
    )
    .await;
    assert!(
        last_text(&server)
            .await
            .contains("Подписка на 7\u{a0}дней оформлена"),
        "{}",
        last_text(&server).await
    );
    assert!(last_buttons(&server).await.contains(&"subs".to_string()));
    let first = client_of(&services, 751).await.unwrap();
    let granted = orders_of(&db, first.id)
        .await
        .into_iter()
        .find(|o| o.kind == OrderKind::ModuleGrant)
        .expect("заказ выдачи по ссылке");
    let delivered = services.fulfillment().run(granted.id).await.unwrap();
    assert!(
        matches!(
            delivered,
            sn_services::fulfillment::FulfillmentOutcome::Done { .. }
        ),
        "{delivered:?}"
    );
    let subscriptions = services
        .subscriptions()
        .list_for_client(first.id)
        .await
        .unwrap();
    assert_eq!(subscriptions[0].subscription.kind, SubscriptionKind::Promo);
    handle_update(
        &ctx,
        text_update(2, 751, &format!("/start pl_{}", blogger.code)),
    )
    .await;
    assert!(
        last_text(&server)
            .await
            .contains("Вы уже получили подписку по этой ссылке")
    );
    handle_update(
        &ctx,
        text_update(3, 752, &format!("/start pl_{}", blogger.code)),
    )
    .await;
    assert!(
        last_text(&server)
            .await
            .contains("Подписки по этой ссылке закончились")
    );
    let off = promolinks::create(&module_ctx, &link("Выключенная", None), None)
        .await
        .unwrap()
        .link;
    promolinks::set_enabled(&module_ctx, off.id, false, None)
        .await
        .unwrap();
    handle_update(
        &ctx,
        text_update(4, 752, &format!("/start pl_{}", off.code)),
    )
    .await;
    assert!(last_text(&server).await.contains("больше не работает"));
    let old = promolinks::create(&module_ctx, &link("Старая", None), None)
        .await
        .unwrap()
        .link;
    {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query(
            "UPDATE promo_links SET expires_at = now() - interval '1 minute' WHERE id = $1",
        )
        .bind(old.id)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    handle_update(
        &ctx,
        text_update(5, 752, &format!("/start pl_{}", old.code)),
    )
    .await;
    assert!(
        last_text(&server)
            .await
            .contains("Срок действия этой промо-ссылки закончился")
    );
    handle_update(&ctx, text_update(6, 752, "/start pl_nothing")).await;
    assert!(last_text(&server).await.contains("Такой промо-ссылки нет"));
    let fresh = promolinks::create(&module_ctx, &link("Новая", None), None)
        .await
        .unwrap()
        .link;
    services
        .modules()
        .set_enabled(services.store(), ModuleId::Promolinks, false, None)
        .await
        .unwrap();
    handle_update(
        &ctx,
        text_update(7, 752, &format!("/start pl_{}", fresh.code)),
    )
    .await;
    assert!(last_text(&server).await.contains("Подписки пока нет"));
    let second = client_of(&services, 752).await.unwrap();
    assert!(
        orders_of(&db, second.id)
            .await
            .iter()
            .all(|o| o.kind != OrderKind::ModuleGrant)
    );
    db.cleanup().await;
}

async fn mark(services: &Services, telegram_id: i64) -> ClientId {
    let client = client_of(services, telegram_id).await.unwrap();
    let mut conn = services.store().conn().await.unwrap();
    sn_modules::catalog::antibot::store::save_mark(
        &mut conn,
        client.id,
        &[sn_modules::catalog::antibot::signals::Signal::DisposableEmail],
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    client.id
}

#[tokio::test]
async fn промо_ссылка_в_боте_просит_кнопку_у_отмеченного_и_не_выдаёт_при_запрете() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    enable(&services, ModuleId::Promolinks).await;
    services.set_remna(Some(remnawave()));
    antibot_mode(&services, "confirm").await;
    let link = promolinks::create(
        &services.module_context(),
        &NewLink {
            name: "Блогер".into(),
            code: None,
            squads: vec![SquadId(uuid::Uuid::from_u128(1))],
            days: 7,
            traffic_gb: None,
            traffic_reset: TrafficReset::NoReset,
            devices: 1,
            max_activations: None,
            expires_at: None,
        },
        None,
    )
    .await
    .unwrap()
    .link;
    handle_update(&ctx, text_update(1, 761, "/start")).await;
    let marked = mark(&services, 761).await;
    handle_update(
        &ctx,
        text_update(2, 761, &format!("/start pl_{}", link.code)),
    )
    .await;
    let asked = last_text(&server).await;
    assert!(asked.contains("подтвердите, что вы не робот"), "{asked}");
    assert!(
        last_labels(&server)
            .await
            .contains(&"Я не робот".to_string())
    );
    let button = last_buttons(&server)
        .await
        .into_iter()
        .find(|b| b.starts_with("plr:"))
        .unwrap();
    assert!(button.len() <= 64, "{button}");
    assert!(!button.contains(&link.code));
    assert!(
        orders_of(&db, marked)
            .await
            .iter()
            .all(|o| o.kind != OrderKind::ModuleGrant)
    );
    handle_update(&ctx, callback_update(3, 761, "plr:0123456789abcdef")).await;
    assert!(
        last_text(&server)
            .await
            .contains("подтвердите, что вы не робот")
    );
    let renewed = last_buttons(&server)
        .await
        .into_iter()
        .find(|b| b.starts_with("plr:"))
        .unwrap();
    assert_ne!(renewed, button);
    let button = renewed;
    {
        let mut conn = services.store().conn().await.unwrap();
        sqlx::query(
            "UPDATE antibot_checks SET created_at = now() - interval '5 seconds' WHERE client_id = $1",
        )
        .bind(marked.0)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    handle_update(&ctx, callback_update(4, 761, &button)).await;
    let done = last_text(&server).await;
    assert!(done.contains("Подписка на 7\u{a0}дней оформлена"), "{done}");
    assert!(
        orders_of(&db, marked)
            .await
            .iter()
            .any(|o| o.kind == OrderKind::ModuleGrant)
    );
    handle_update(&ctx, callback_update(5, 761, &button)).await;
    assert!(
        last_text(&server)
            .await
            .contains("Откройте промо-ссылку ещё раз")
    );

    antibot_mode(&services, "block").await;
    handle_update(&ctx, text_update(6, 762, "/start")).await;
    let blocked = mark(&services, 762).await;
    handle_update(
        &ctx,
        text_update(7, 762, &format!("/start pl_{}", link.code)),
    )
    .await;
    assert!(
        last_text(&server)
            .await
            .contains("Подписка по промо-ссылкам для этого аккаунта недоступна")
    );
    assert!(
        orders_of(&db, blocked)
            .await
            .iter()
            .all(|o| o.kind != OrderKind::ModuleGrant)
    );
    db.cleanup().await;
}
