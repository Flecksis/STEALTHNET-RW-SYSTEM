use super::auto::{self, RuleDraft, Trigger};
use super::tests::{
    FakeMail, FakeSender, admin, code, context_with, fast, fast_mail, json_of, mail_client, owner,
    png, spawn_engine, spawn_mail, tg_client,
};
use super::*;
use crate::registry::ModuleRegistry;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sn_domain::access::AdminRole;
use sn_store::Store;
use sn_store::testing::test_store;
use std::time::Instant;
use tower::ServiceExt;

async fn now_of(store: &Store) -> DateTime<Utc> {
    let mut conn = store.conn().await.unwrap();
    crate::catalog::db_now(&mut conn).await.unwrap()
}

async fn sub(store: &Store, client: ClientId, kind: &str, expire_at: DateTime<Utc>) -> i64 {
    let mut conn = store.conn().await.unwrap();
    sqlx::query_scalar(
        "INSERT INTO subscriptions (client_id, kind, expire_at, first_connected_at) \
         VALUES ($1, $2, $3, CASE WHEN $2 = 'trial' THEN NULL ELSE now() END) RETURNING id",
    )
    .bind(client.0)
    .bind(kind)
    .bind(expire_at)
    .fetch_one(conn.conn())
    .await
    .unwrap()
}

async fn run(store: &Store, sql: &str, id: i64) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(sql)
        .bind(id)
        .execute(conn.conn())
        .await
        .unwrap();
}

async fn run_at(store: &Store, sql: &str, id: i64, at: DateTime<Utc>) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(sql)
        .bind(id)
        .bind(at)
        .execute(conn.conn())
        .await
        .unwrap();
}

async fn paid_order(store: &Store, client: ClientId, subscription: Option<i64>, at: DateTime<Utc>) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(
        "INSERT INTO orders (client_id, subscription_id, kind, status, intent, quote, currency, total_minor, \
         paid_at, fulfilled_at) VALUES ($1, $2, 'new', 'fulfilled', '{}', '{}', 'USD', 500, $3, $3)",
    )
    .bind(client.0)
    .bind(subscription)
    .bind(at)
    .execute(conn.conn())
    .await
    .unwrap();
}

fn rule_draft(trigger: Trigger, days: u16, channel: Channel) -> RuleDraft {
    RuleDraft {
        trigger,
        days,
        with_autorenew: false,
        channel,
        subject: (channel != Channel::Bot).then(|| "О подписке".to_string()),
        text: format!("<b>Правило</b> {}", trigger.as_str()),
        image_id: None,
        buttons: vec![ButtonSpec {
            text: "Продлить".into(),
            url: None,
            section: Some(BotSection::Buy),
        }],
        enabled: true,
    }
}

async fn case_clients(store: &Store, rule: i64) -> Vec<(i64, bool)> {
    let mut conn = store.conn().await.unwrap();
    sqlx::query_as(
        "SELECT client_id, queued FROM auto_broadcast_cases WHERE rule_id = $1 ORDER BY client_id, case_key",
    )
    .bind(rule)
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

async fn fired(store: &Store, rule: i64) -> Vec<i64> {
    case_clients(store, rule)
        .await
        .into_iter()
        .map(|(client, _)| client)
        .collect()
}

async fn queue(store: &Store, rule: i64) -> Vec<(String, i64, String)> {
    let mut conn = store.conn().await.unwrap();
    sqlx::query_as(
        "SELECT r.channel, r.client_id, r.state FROM broadcast_recipients r \
         JOIN broadcasts b ON b.id = r.broadcast_id WHERE b.rule_id = $1 ORDER BY r.channel, r.client_id",
    )
    .bind(rule)
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

async fn batches(store: &Store, rule: i64) -> Vec<(i64, String, i32)> {
    let mut conn = store.conn().await.unwrap();
    sqlx::query_as("SELECT id, text, rule_version FROM broadcasts WHERE rule_id = $1 ORDER BY id")
        .bind(rule)
        .fetch_all(conn.conn())
        .await
        .unwrap()
}

async fn settle(store: &Store, rule: i64) {
    let started = Instant::now();
    loop {
        let mut conn = store.conn().await.unwrap();
        let left: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM broadcast_recipients r JOIN broadcasts b ON b.id = r.broadcast_id \
             WHERE b.rule_id = $1 AND (r.state IN ('pending', 'sending') OR b.status = 'sending')",
        )
        .bind(rule)
        .fetch_one(conn.conn())
        .await
        .unwrap();
        drop(conn);
        if left == 0 {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "очередь правила {rule} не разошлась"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn ids(clients: &[ClientId]) -> Vec<i64> {
    let mut out: Vec<i64> = clients.iter().map(|c| c.0).collect();
    out.sort_unstable();
    out
}

#[test]
fn сроки_событий_пишутся_по_русски_и_проверяются() {
    assert_eq!(Trigger::Expiring.timing(1), "за 1 день до окончания");
    assert_eq!(Trigger::Expiring.timing(3), "за 3 дня до окончания");
    assert_eq!(Trigger::Expiring.timing(5), "за 5 дней до окончания");
    assert_eq!(Trigger::Expired.timing(0), "сразу после окончания");
    assert_eq!(
        Trigger::TrialEnded.timing(21),
        "через 21 день после окончания"
    );
    assert_eq!(
        Trigger::NoPurchase.timing(2),
        "через 2 дня после регистрации"
    );
    assert_eq!(
        Trigger::NotConnected.timing(11),
        "через 11 дней после покупки"
    );
    for trigger in Trigger::ALL {
        assert_eq!(Trigger::parse(trigger.as_str()), Some(trigger));
        assert_eq!(
            serde_json::to_value(trigger).unwrap(),
            serde_json::json!(trigger.as_str())
        );
    }
    assert_eq!(Trigger::Expiring.min_days(), 1);
    assert_eq!(Trigger::Expired.min_days(), 0);
}

#[tokio::test]
async fn скорое_окончание_срабатывает_один_раз_в_свой_срок_и_снова_после_продления() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let ctx = context_with(store, Some(sender.clone()), None);
    let now = now_of(store).await;
    let due = tg_client(store, 9301).await;
    let trial = tg_client(store, 9302).await;
    let later = tg_client(store, 9303).await;
    let stale = tg_client(store, 9304).await;
    let renewing = tg_client(store, 9305).await;
    let gone = tg_client(store, 9306).await;
    let due_sub = sub(store, due, "paid", now + Span::days(3) - Span::hours(1)).await;
    sub(store, trial, "trial", now + Span::days(2) + Span::hours(1)).await;
    sub(store, later, "paid", now + Span::days(3) + Span::hours(2)).await;
    sub(store, stale, "paid", now + Span::days(1)).await;
    let renewing_sub = sub(
        store,
        renewing,
        "paid",
        now + Span::days(2) + Span::hours(12),
    )
    .await;
    run(
        store,
        "UPDATE subscriptions SET autorenew_enabled = true WHERE id = $1",
        renewing_sub,
    )
    .await;
    sub(store, gone, "paid", now - Span::hours(1)).await;

    let rule = create_rule(&ctx, &rule_draft(Trigger::Expiring, 3, Channel::Bot), None)
        .await
        .unwrap();
    assert_eq!(rule.timing, "за 3 дня до окончания");
    let probe = rule_reach(
        &ctx,
        &ReachQuery {
            trigger: Trigger::Expiring,
            days: 3,
            with_autorenew: false,
            id: Some(rule.id),
        },
    )
    .await
    .unwrap();
    assert_eq!((probe.cases, probe.recipients), (2, 2));

    let first = auto::check(&ctx, now).await.unwrap();
    assert_eq!((first.cases, first.queued), (2, 2));
    assert_eq!(fired(store, rule.id).await, ids(&[due, trial]));
    let again = auto::check(&ctx, now).await.unwrap();
    assert_eq!(again.cases, 0);

    let restarted = context_with(store, Some(sender.clone()), None);
    let after = auto::check(&restarted, now + Span::hours(3)).await.unwrap();
    assert_eq!(after.cases, 1);
    assert_eq!(fired(store, rule.id).await, ids(&[due, trial, later]));

    let engine = spawn_engine(&ctx, &sender, fast());
    settle(store, rule.id).await;
    engine.stop().await;
    assert_eq!(sender.chats(), vec![9301, 9302, 9303]);
    assert_eq!(
        auto::check(&restarted, now + Span::hours(3))
            .await
            .unwrap()
            .cases,
        0
    );

    let view = auto::rule(&ctx, rule.id).await.unwrap();
    assert_eq!(view.stats.cases, 3);
    assert_eq!(view.stats.progress.sent, 3);
    assert!(view.stats.last_sent_at.is_some());

    run_at(
        store,
        "UPDATE subscriptions SET expire_at = $2 WHERE id = $1",
        due_sub,
        now + Span::days(33),
    )
    .await;
    let renewed = auto::check(&ctx, now + Span::days(30) + Span::hours(1))
        .await
        .unwrap();
    assert_eq!(renewed.cases, 1);
    let keys: Vec<(i64, bool)> = case_clients(store, rule.id).await;
    assert_eq!(keys.iter().filter(|(c, _)| *c == due.0).count(), 2);

    let mut with_renewal = rule_draft(Trigger::Expiring, 3, Channel::Bot);
    with_renewal.with_autorenew = true;
    update_rule(&ctx, rule.id, &with_renewal, None)
        .await
        .unwrap();
    let chosen = auto::check(&ctx, now).await.unwrap();
    assert_eq!(chosen.cases, 1);
    assert!(fired(store, rule.id).await.contains(&renewing.0));
    assert!(!fired(store, rule.id).await.contains(&stale.0));
    assert!(!fired(store, rule.id).await.contains(&gone.0));
    db.cleanup().await;
}

#[tokio::test]
async fn закончилась_и_не_продлена_через_заданные_дни() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let lapsed = tg_client(store, 9401).await;
    let soon = tg_client(store, 9402).await;
    let moved = tg_client(store, 9403).await;
    let trial = tg_client(store, 9404).await;
    let old = tg_client(store, 9405).await;
    let disabled = tg_client(store, 9406).await;
    sub(store, lapsed, "paid", now - Span::days(2) - Span::hours(1)).await;
    sub(store, soon, "paid", now - Span::days(1)).await;
    sub(store, moved, "paid", now - Span::days(2) - Span::hours(1)).await;
    sub(store, moved, "paid", now + Span::days(20)).await;
    sub(store, trial, "trial", now - Span::days(2) - Span::hours(1)).await;
    sub(store, old, "paid", now - Span::days(5)).await;
    let off = sub(
        store,
        disabled,
        "paid",
        now - Span::days(2) - Span::hours(1),
    )
    .await;
    run(
        store,
        "UPDATE subscriptions SET status = 'disabled' WHERE id = $1",
        off,
    )
    .await;
    let rule = create_rule(&ctx, &rule_draft(Trigger::Expired, 2, Channel::Bot), None)
        .await
        .unwrap();
    assert_eq!(auto::check(&ctx, now).await.unwrap().cases, 1);
    assert_eq!(fired(store, rule.id).await, ids(&[lapsed]));
    assert_eq!(
        auto::check(&ctx, now + Span::hours(23))
            .await
            .unwrap()
            .cases,
        0
    );
    assert_eq!(
        auto::check(&ctx, now + Span::days(1) + Span::hours(1))
            .await
            .unwrap()
            .cases,
        1
    );
    assert_eq!(fired(store, rule.id).await, ids(&[lapsed, soon]));
    db.cleanup().await;
}

#[tokio::test]
async fn пробный_период_закончился_без_покупки() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let idle = tg_client(store, 9501).await;
    let buyer = tg_client(store, 9502).await;
    let upgraded = tg_client(store, 9503).await;
    let trying = tg_client(store, 9504).await;
    sub(store, idle, "trial", now - Span::hours(1)).await;
    let bought = sub(store, buyer, "trial", now - Span::hours(1)).await;
    paid_order(store, buyer, Some(bought), now - Span::hours(2)).await;
    sub(store, upgraded, "trial", now - Span::hours(1)).await;
    sub(store, upgraded, "paid", now + Span::days(29)).await;
    sub(store, trying, "trial", now + Span::days(1)).await;
    let rule = create_rule(
        &ctx,
        &rule_draft(Trigger::TrialEnded, 0, Channel::Bot),
        None,
    )
    .await
    .unwrap();
    assert_eq!(rule.timing, "сразу после окончания");
    assert_eq!(auto::check(&ctx, now).await.unwrap().cases, 1);
    assert_eq!(fired(store, rule.id).await, ids(&[idle]));
    assert_eq!(
        auto::check(&ctx, now + Span::days(1) + Span::hours(1))
            .await
            .unwrap()
            .cases,
        1
    );
    assert_eq!(fired(store, rule.id).await, ids(&[idle, trying]));
    db.cleanup().await;
}

#[tokio::test]
async fn нет_покупок_после_регистрации() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let quiet = tg_client(store, 9601).await;
    let buyer = tg_client(store, 9602).await;
    let fresh = tg_client(store, 9603).await;
    let moved = tg_client(store, 9604).await;
    let trial = tg_client(store, 9605).await;
    for (client, at) in [
        (quiet, now - Span::days(3) - Span::hours(1)),
        (buyer, now - Span::days(3) - Span::hours(1)),
        (fresh, now - Span::days(2)),
        (moved, now - Span::days(3) - Span::hours(1)),
        (trial, now - Span::days(3) - Span::hours(1)),
    ] {
        run_at(
            store,
            "UPDATE clients SET created_at = $2 WHERE id = $1",
            client.0,
            at,
        )
        .await;
    }
    paid_order(store, buyer, None, now - Span::days(1)).await;
    run(
        store,
        "UPDATE clients SET registration_source = 'migration' WHERE id = $1",
        moved.0,
    )
    .await;
    sub(store, trial, "trial", now + Span::days(1)).await;
    let rule = create_rule(
        &ctx,
        &rule_draft(Trigger::NoPurchase, 3, Channel::Bot),
        None,
    )
    .await
    .unwrap();
    assert_eq!(auto::check(&ctx, now).await.unwrap().cases, 2);
    assert_eq!(fired(store, rule.id).await, ids(&[quiet, trial]));
    assert_eq!(
        auto::check(&ctx, now + Span::days(1) + Span::hours(1))
            .await
            .unwrap()
            .cases,
        1
    );
    assert_eq!(fired(store, rule.id).await, ids(&[quiet, fresh, trial]));
    db.cleanup().await;
}

#[tokio::test]
async fn нет_подключений_после_покупки() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let silent = tg_client(store, 9701).await;
    let online = tg_client(store, 9702).await;
    let recent = tg_client(store, 9703).await;
    let imported = tg_client(store, 9704).await;
    let lapsed = tg_client(store, 9705).await;
    let mut subs = Vec::new();
    for (client, expire) in [
        (silent, now + Span::days(28)),
        (online, now + Span::days(28)),
        (recent, now + Span::days(29)),
        (imported, now + Span::days(28)),
        (lapsed, now - Span::hours(1)),
    ] {
        let id = sub(store, client, "paid", expire).await;
        run(
            store,
            "UPDATE subscriptions SET first_connected_at = NULL WHERE id = $1",
            id,
        )
        .await;
        subs.push(id);
    }
    run(
        store,
        "UPDATE subscriptions SET first_connected_at = now() WHERE id = $1",
        subs[1],
    )
    .await;
    paid_order(
        store,
        silent,
        Some(subs[0]),
        now - Span::days(2) - Span::hours(1),
    )
    .await;
    paid_order(
        store,
        online,
        Some(subs[1]),
        now - Span::days(2) - Span::hours(1),
    )
    .await;
    paid_order(store, recent, Some(subs[2]), now - Span::days(1)).await;
    paid_order(
        store,
        lapsed,
        Some(subs[4]),
        now - Span::days(2) - Span::hours(1),
    )
    .await;
    let rule = create_rule(
        &ctx,
        &rule_draft(Trigger::NotConnected, 2, Channel::Bot),
        None,
    )
    .await
    .unwrap();
    assert_eq!(auto::check(&ctx, now).await.unwrap().cases, 1);
    assert_eq!(fired(store, rule.id).await, ids(&[silent]));
    assert_eq!(
        auto::check(&ctx, now + Span::days(1) + Span::hours(1))
            .await
            .unwrap()
            .cases,
        1
    );
    assert_eq!(fired(store, rule.id).await, ids(&[silent, recent]));
    db.cleanup().await;
}

#[tokio::test]
async fn остановившие_бота_и_отписавшиеся_не_получают() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let mail = FakeMail::new();
    let ctx = context_with(store, Some(sender.clone()), Some(mail.clone()));
    let now = now_of(store).await;
    let both = mail_client(store, Some(9801), Some("both@example.com")).await;
    let none = mail_client(store, Some(9802), Some("gone@example.com")).await;
    let mailbox = mail_client(store, Some(9803), Some("mail@example.com")).await;
    let left = mail_client(store, None, Some("left@example.com")).await;
    let chat = mail_client(store, Some(9805), None).await;
    for client in [none, mailbox] {
        run(
            store,
            "UPDATE clients SET bot_unreachable = true WHERE id = $1",
            client.0,
        )
        .await;
    }
    {
        let mut conn = store.conn().await.unwrap();
        sqlx::query(
            "INSERT INTO broadcast_unsubscribes (email) VALUES ('gone@example.com'), ('left@example.com')",
        )
        .execute(conn.conn())
        .await
        .unwrap();
    }
    for client in [both, none, mailbox, left, chat] {
        sub(store, client, "paid", now + Span::days(3) - Span::hours(2)).await;
    }
    let rule = create_rule(&ctx, &rule_draft(Trigger::Expiring, 3, Channel::Both), None)
        .await
        .unwrap();
    let probe = rule_reach(
        &ctx,
        &ReachQuery {
            trigger: Trigger::Expiring,
            days: 3,
            with_autorenew: false,
            id: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (
            probe.cases,
            probe.recipients,
            probe.stopped,
            probe.emails,
            probe.unsubscribed
        ),
        (5, 2, 2, 2, 2)
    );
    let report = auto::check(&ctx, now).await.unwrap();
    assert_eq!((report.cases, report.queued), (5, 3));
    assert_eq!(
        case_clients(store, rule.id).await,
        vec![
            (both.0, true),
            (none.0, false),
            (mailbox.0, true),
            (left.0, false),
            (chat.0, true)
        ]
    );
    let bot = spawn_engine(&ctx, &sender, fast());
    let letters = spawn_mail(&ctx, &mail, fast_mail());
    settle(store, rule.id).await;
    bot.stop().await;
    letters.stop().await;
    assert_eq!(sender.chats(), vec![9801, 9805]);
    let mut to: Vec<String> = mail.mailed().into_iter().map(|m| m.to).collect();
    to.sort();
    assert_eq!(to, vec!["both@example.com", "mail@example.com"]);
    assert!(mail.mailed().iter().all(|m| m.unsubscribe.is_some()));
    let view = auto::rule(&ctx, rule.id).await.unwrap();
    assert_eq!(
        (
            view.stats.cases,
            view.stats.unreachable,
            view.stats.progress.sent
        ),
        (5, 2, 4)
    );
    db.cleanup().await;
}

#[tokio::test]
async fn выключение_останавливает_очередь_а_сломанная_почта_не_мешает_боту() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let mail = FakeMail::new();
    let ctx = context_with(store, Some(sender.clone()), Some(mail.clone()));
    let now = now_of(store).await;
    let first = mail_client(store, Some(9901), Some("one@example.com")).await;
    sub(store, first, "paid", now + Span::days(3) - Span::hours(2)).await;
    let rule = create_rule(&ctx, &rule_draft(Trigger::Expiring, 3, Channel::Both), None)
        .await
        .unwrap();
    auto::check(&ctx, now).await.unwrap();
    let off = switch_rule(&ctx, rule.id, false, None).await.unwrap();
    assert!(!off.enabled);
    assert_eq!(
        queue(store, rule.id).await,
        vec![
            ("bot".to_string(), first.0, "skipped".to_string()),
            ("email".to_string(), first.0, "skipped".to_string()),
        ]
    );
    let second = mail_client(store, Some(9902), Some("two@example.com")).await;
    sub(store, second, "paid", now + Span::days(3) - Span::hours(1)).await;
    assert_eq!(auto::check(&ctx, now).await.unwrap().cases, 0);
    switch_rule(&ctx, rule.id, true, None).await.unwrap();
    *mail.ready.lock().unwrap() = Err("почта выключена".into());
    let report = auto::check(&ctx, now).await.unwrap();
    assert_eq!((report.cases, report.queued), (1, 1));
    let rows = queue(store, rule.id).await;
    assert!(rows.contains(&("bot".to_string(), second.0, "pending".to_string())));
    assert!(
        !rows
            .iter()
            .any(|(lane, client, _)| lane == "email" && *client == second.0)
    );
    let view = auto::rule(&ctx, rule.id).await.unwrap();
    assert!(
        view.notice
            .as_deref()
            .is_some_and(|n| n.contains("Письма по правилу не уходят")),
        "{:?}",
        view.notice
    );
    let mailed = create_rule(&ctx, &rule_draft(Trigger::Expired, 1, Channel::Email), None)
        .await
        .unwrap_err();
    assert_eq!(code(mailed), "broadcast.mail");
    let mut quiet = rule_draft(Trigger::Expired, 1, Channel::Email);
    quiet.enabled = false;
    let parked = create_rule(&ctx, &quiet, None).await.unwrap();
    assert!(!parked.enabled);
    assert_eq!(
        code(switch_rule(&ctx, parked.id, true, None).await.unwrap_err()),
        "broadcast.mail"
    );
    let mut wrong = rule_draft(Trigger::Expiring, 0, Channel::Bot);
    wrong.enabled = false;
    assert_eq!(
        code(create_rule(&ctx, &wrong, None).await.unwrap_err()),
        "broadcast.invalid"
    );
    wrong.days = DAYS_MAX + 1;
    assert_eq!(
        code(create_rule(&ctx, &wrong, None).await.unwrap_err()),
        "broadcast.invalid"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn правка_текста_идёт_новой_пачкой_а_картинка_правила_не_удаляется() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context_with(store, Some(FakeSender::new()), None);
    let now = now_of(store).await;
    let image = upload_image(&ctx, &png(640, 480), None).await.unwrap();
    let mut draft = rule_draft(Trigger::Expiring, 3, Channel::Bot);
    draft.image_id = Some(image.id);
    let rule = create_rule(&ctx, &draft, None).await.unwrap();
    run(
        store,
        "UPDATE broadcast_images SET created_at = now() - interval '3 days' WHERE id = $1",
        image.id,
    )
    .await;
    cleanup(&ctx).await.unwrap();
    assert!(image_file(&ctx, image.id).await.is_ok());
    let a = tg_client(store, 9951).await;
    sub(store, a, "paid", now + Span::days(3) - Span::hours(2)).await;
    auto::check(&ctx, now).await.unwrap();
    draft.text = "Новый текст".into();
    update_rule(&ctx, rule.id, &draft, None).await.unwrap();
    let b = tg_client(store, 9952).await;
    sub(store, b, "paid", now + Span::days(3) - Span::hours(1)).await;
    auto::check(&ctx, now).await.unwrap();
    let made = batches(store, rule.id).await;
    assert_eq!(made.len(), 2);
    assert_eq!(made[0].1, "<b>Правило</b> expiring");
    assert_eq!((made[1].1.as_str(), made[1].2), ("Новый текст", 2));
    draft.days = 4;
    update_rule(&ctx, rule.id, &draft, None).await.unwrap();
    let c = tg_client(store, 9953).await;
    sub(store, c, "paid", now + Span::days(4) - Span::hours(1)).await;
    auto::check(&ctx, now).await.unwrap();
    assert_eq!(batches(store, rule.id).await.len(), 2);
    delete_rule(&ctx, rule.id, None).await.unwrap();
    let mut conn = store.conn().await.unwrap();
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM broadcasts WHERE rule_id = $1")
        .bind(rule.id)
        .fetch_one(conn.conn())
        .await
        .unwrap();
    assert_eq!(kept, 2);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn админка_ведёт_правила_и_не_путает_их_с_разовыми() {
    let Some(db) = test_store().await else {
        return;
    };
    owner(&db).await;
    let store = &db.store;
    let sender = FakeSender::new();
    let ctx = context_with(store, Some(sender.clone()), None);
    let now = now_of(store).await;
    let quiet = tg_client(store, 9971).await;
    run_at(
        store,
        "UPDATE clients SET created_at = $2 WHERE id = $1",
        quiet.0,
        now - Span::days(3) - Span::hours(1),
    )
    .await;
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.admin_router(ctx.clone());
    let call = |method: &str, uri: &str, role: AdminRole, body: Option<serde_json::Value>| {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("/broadcast{uri}"))
            .extension(admin(role));
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        request
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap()
    };
    let denied = router
        .clone()
        .oneshot(call("GET", "/auto", AdminRole::Support, None))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let empty = router
        .clone()
        .oneshot(call("GET", "/auto", AdminRole::Admin, None))
        .await
        .unwrap();
    assert_eq!(json_of(empty).await["items"], serde_json::json!([]));
    let bad = router
        .clone()
        .oneshot(call(
            "POST",
            "/auto",
            AdminRole::Admin,
            Some(serde_json::json!({ "trigger": "expiring", "days": 0, "text": "x" })),
        ))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_of(bad).await["error"]["code"], "broadcast.invalid");
    let reach = router
        .clone()
        .oneshot(call(
            "POST",
            "/auto/reach",
            AdminRole::Admin,
            Some(serde_json::json!({ "trigger": "no_purchase", "days": 3 })),
        ))
        .await
        .unwrap();
    assert_eq!(reach.status(), StatusCode::OK);
    assert_eq!(json_of(reach).await["recipients"], 1);
    let created = router
        .clone()
        .oneshot(call(
            "POST",
            "/auto",
            AdminRole::Admin,
            Some(serde_json::json!({
                "trigger": "no_purchase",
                "days": 3,
                "text": "Привет! Попробуйте <b>месяц</b> со скидкой",
                "buttons": [{ "text": "Купить", "section": "buy" }],
                "enabled": true
            })),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created = json_of(created).await;
    let id = created["id"].as_i64().unwrap();
    assert_eq!(created["title"], "Нет покупок после регистрации");
    assert_eq!(created["timing"], "через 3 дня после регистрации");
    assert_eq!(created["enabled"], true);
    let edited = router
        .clone()
        .oneshot(call(
            "PUT",
            &format!("/auto/{id}"),
            AdminRole::Admin,
            Some(serde_json::json!({
                "trigger": "no_purchase",
                "days": 5,
                "text": "Привет!",
                "enabled": true
            })),
        ))
        .await
        .unwrap();
    assert_eq!(edited.status(), StatusCode::OK);
    assert_eq!(
        json_of(edited).await["timing"],
        "через 5 дней после регистрации"
    );
    let back = router
        .clone()
        .oneshot(call(
            "PUT",
            &format!("/auto/{id}"),
            AdminRole::Admin,
            Some(serde_json::json!({ "trigger": "no_purchase", "days": 3, "text": "Привет!", "enabled": true })),
        ))
        .await
        .unwrap();
    assert_eq!(back.status(), StatusCode::OK);
    assert_eq!(auto::check(&ctx, now).await.unwrap().cases, 1);
    let listed = router
        .clone()
        .oneshot(call("GET", "/auto", AdminRole::Admin, None))
        .await
        .unwrap();
    let listed = json_of(listed).await;
    assert_eq!(listed["items"][0]["id"], id);
    assert_eq!(listed["items"][0]["stats"]["cases"], 1);
    assert_eq!(listed["items"][0]["stats"]["progress"]["pending"], 1);
    let once = router
        .clone()
        .oneshot(call("GET", "/broadcasts", AdminRole::Admin, None))
        .await
        .unwrap();
    assert_eq!(json_of(once).await["items"], serde_json::json!([]));
    let batch = batches(store, id).await[0].0;
    let hidden = router
        .clone()
        .oneshot(call(
            "GET",
            &format!("/broadcasts/{batch}"),
            AdminRole::Admin,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    let stopped = router
        .clone()
        .oneshot(call(
            "POST",
            &format!("/auto/{id}/switch"),
            AdminRole::Admin,
            Some(serde_json::json!({ "enabled": false })),
        ))
        .await
        .unwrap();
    let stopped = json_of(stopped).await;
    assert_eq!(stopped["enabled"], false);
    assert_eq!(stopped["stats"]["progress"]["skipped"], 1);
    let removed = router
        .clone()
        .oneshot(call(
            "DELETE",
            &format!("/auto/{id}"),
            AdminRole::Admin,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::OK);
    let missing = router
        .clone()
        .oneshot(call("GET", &format!("/auto/{id}"), AdminRole::Admin, None))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let empty = router
        .clone()
        .oneshot(call("GET", "/auto", AdminRole::Admin, None))
        .await
        .unwrap();
    assert_eq!(json_of(empty).await["items"], serde_json::json!([]));
    db.cleanup().await;
}
