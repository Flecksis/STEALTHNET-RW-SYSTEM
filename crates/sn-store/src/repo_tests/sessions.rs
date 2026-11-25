use super::fixtures::*;
use crate::testing::test_store;
use crate::*;
use chrono::{DateTime, Utc};
use sn_domain::client::RegistrationSource;
use sn_domain::ids::ClientId;
use sn_domain::money::Currency;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

const KEEP: i64 = 90;
const TTL: i64 = 30;

#[derive(Debug, Clone, Copy)]
struct Visit {
    channel: SessionChannel,
    ip: Option<&'static str>,
    days: i64,
    seen: i64,
    left: i64,
    revoked: bool,
}

fn cabinet(days: i64) -> Visit {
    Visit {
        channel: SessionChannel::Cabinet,
        ip: Some("203.0.113.7"),
        days,
        seen: days,
        left: TTL - days,
        revoked: false,
    }
}

fn miniapp(days: i64) -> Visit {
    Visit {
        channel: SessionChannel::Miniapp,
        ..cabinet(days)
    }
}

async fn visit(db: &mut impl Db, client: ClientId, name: &str, visit: Visit) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO client_sessions (client_id, token_hash, channel, ip, user_agent, created_at, last_seen_at, expires_at, revoked_at) \
         VALUES ($1, $2, $3, $4, 'Mozilla/5.0', now() - make_interval(days => $5::int), \
                 now() - make_interval(days => $6::int), now() + make_interval(days => $7::int), \
                 CASE WHEN $8 THEN now() - make_interval(days => $5::int) + interval '1 hour' END) RETURNING id",
    )
    .bind(client.0)
    .bind(format!("{}:{name}", client.0).into_bytes())
    .bind(visit.channel.as_str())
    .bind(visit.ip)
    .bind(visit.days)
    .bind(visit.seen)
    .bind(visit.left)
    .bind(visit.revoked)
    .fetch_one(db.conn())
    .await
    .unwrap()
}

async fn left(db: &mut impl Db, client: ClientId) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT split_part(convert_from(token_hash, 'UTF8'), ':', 2) FROM client_sessions \
         WHERE client_id = $1 ORDER BY id",
    )
    .bind(client.0)
    .fetch_all(db.conn())
    .await
    .unwrap()
}

async fn portion_of(store: &Store, after: i64, portion: i64) -> SessionPortion {
    let mut tx = store.begin().await.unwrap();
    let done = tx
        .sessions()
        .sweep_client(after, portion, KEEP)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    done
}

async fn sweep_by(store: &Store, portion: i64) -> i64 {
    let mut after = 0;
    let mut removed = 0;
    loop {
        let done = portion_of(store, after, portion).await;
        removed += done.removed;
        match done.last {
            Some(last) => after = last,
            None => return removed,
        }
    }
}

async fn sweep(store: &Store) -> i64 {
    sweep_by(store, 500).await
}

#[tokio::test]
async fn старые_сессии_удаляются_а_живые_свежие_и_первая_остаются() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let owner = client(&mut conn, &Person::NOBODY).await;
    for (name, row) in [
        ("первая", cabinet(400)),
        ("старая", cabinet(300)),
        (
            "старая отозванная",
            Visit {
                left: 10,
                revoked: true,
                ..cabinet(200)
            },
        ),
        (
            "старая живая",
            Visit {
                left: 100,
                ..cabinet(150)
            },
        ),
        (
            "старая, но недавно были",
            Visit {
                seen: 59,
                ..cabinet(120)
            },
        ),
        ("ровно за порогом", cabinet(KEEP + 1)),
        ("ещё не за порогом", cabinet(KEEP - 1)),
        (
            "свежая отозванная",
            Visit {
                revoked: true,
                ..cabinet(40)
            },
        ),
        ("живая", cabinet(2)),
    ] {
        visit(&mut conn, owner, name, row).await;
    }
    assert_eq!(sweep(&db.store).await, 3);
    assert_eq!(
        left(&mut conn, owner).await,
        [
            "первая",
            "старая живая",
            "старая, но недавно были",
            "ещё не за порогом",
            "свежая отозванная",
            "живая"
        ]
    );
    assert_eq!(sweep(&db.store).await, 0, "второй проход ничего не находит");
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn действующий_вход_не_прерывается_уборкой() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let owner = client(&mut conn, &Person::NOBODY).await;
    for days in [400, 350, 300, 250] {
        visit(&mut conn, owner, &format!("давно-{days}"), cabinet(days)).await;
    }
    for (name, row) in [
        (
            "длинная",
            Visit {
                left: 1,
                ..cabinet(200)
            },
        ),
        ("обычная", cabinet(5)),
        ("мини", miniapp(29)),
    ] {
        visit(&mut conn, owner, name, row).await;
    }
    assert_eq!(sweep(&db.store).await, 3);
    for name in ["длинная", "обычная", "мини"] {
        let found = conn
            .sessions()
            .client_by_token(format!("{}:{name}", owner.0).as_bytes())
            .await
            .unwrap();
        assert_eq!(found.map(|s| s.client_id), Some(owner), "{name}");
    }
    assert!(
        conn.sessions()
            .client_by_token(format!("{}:давно-300", owner.0).as_bytes())
            .await
            .unwrap()
            .is_none()
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn в_карточке_клиента_остаются_те_же_последние_входы() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let gone = client(&mut conn, &Person::NOBODY).await;
    for (name, row) in [
        ("кабинет-300", cabinet(300)),
        ("мини-280", miniapp(280)),
        ("кабинет-250", cabinet(250)),
        ("мини-220", miniapp(220)),
        ("кабинет-200", cabinet(200)),
    ] {
        visit(&mut conn, gone, name, row).await;
    }
    let back = client(&mut conn, &Person::NOBODY).await;
    for (name, row) in [
        ("кабинет-300", cabinet(300)),
        ("мини-250", miniapp(250)),
        ("кабинет-200", cabinet(200)),
        ("мини-1", miniapp(1)),
    ] {
        visit(&mut conn, back, name, row).await;
    }
    let once = client(&mut conn, &Person::NOBODY).await;
    visit(&mut conn, once, "мини-500", miniapp(500)).await;
    let mut before = Vec::new();
    for id in [gone, back, once] {
        let stats = conn.clients().card_stats(id, Currency::Usd).await.unwrap();
        before.push((stats.cabinet_seen_at, stats.miniapp_seen_at));
    }
    assert!(before[0].0.is_some() && before[0].1.is_some());
    sweep(&db.store).await;
    assert_eq!(
        left(&mut conn, gone).await,
        ["кабинет-300", "мини-220", "кабинет-200"]
    );
    assert_eq!(
        left(&mut conn, back).await,
        ["кабинет-300", "кабинет-200", "мини-1"]
    );
    assert_eq!(left(&mut conn, once).await, ["мини-500"]);
    for (id, seen) in [gone, back, once].into_iter().zip(before) {
        let stats = conn.clients().card_stats(id, Currency::Usd).await.unwrap();
        assert_eq!((stats.cabinet_seen_at, stats.miniapp_seen_at), seen);
    }
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn строка_с_несогласованными_датами_не_теряется() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let owner = client(&mut conn, &Person::NOBODY).await;
    for (name, row) in [
        ("первая", cabinet(300)),
        (
            "видели позже всех",
            Visit {
                seen: 150,
                ..cabinet(200)
            },
        ),
        (
            "видели до входа",
            Visit {
                seen: 180,
                ..cabinet(100)
            },
        ),
        (
            "свежая, но видели давно",
            Visit {
                seen: 170,
                revoked: true,
                ..cabinet(10)
            },
        ),
    ] {
        visit(&mut conn, owner, name, row).await;
    }
    let before = conn
        .clients()
        .card_stats(owner, Currency::Usd)
        .await
        .unwrap()
        .cabinet_seen_at;
    sweep(&db.store).await;
    assert_eq!(
        left(&mut conn, owner).await,
        ["первая", "видели позже всех", "свежая, но видели давно"]
    );
    let after = conn
        .clients()
        .card_stats(owner, Currency::Usd)
        .await
        .unwrap()
        .cabinet_seen_at;
    assert_eq!(after, before);
    let mut tx = db.store.begin().await.unwrap();
    let pair = client(&mut tx, &Person::NOBODY).await;
    visit(&mut tx, pair, "первая", miniapp(300)).await;
    for (name, days) in [("пара-1", 100), ("пара-2", 95)] {
        visit(
            &mut tx,
            pair,
            name,
            Visit {
                seen: 180,
                ..cabinet(days)
            },
        )
        .await;
    }
    tx.commit().await.unwrap();
    let fresh = client(&mut conn, &Person::NOBODY).await;
    for (name, row) in [
        ("первая", cabinet(300)),
        (
            "свежая, но видели давно",
            Visit {
                seen: 170,
                revoked: true,
                ..cabinet(10)
            },
        ),
        ("живая", cabinet(1)),
    ] {
        visit(&mut conn, fresh, name, row).await;
    }
    sweep(&db.store).await;
    assert_eq!(left(&mut conn, pair).await, ["первая", "пара-2"]);
    assert_eq!(
        left(&mut conn, fresh).await,
        ["первая", "свежая, но видели давно", "живая"]
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn первый_вход_с_адресом_остаётся_даже_если_раньше_были_без_адреса() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let owner = client(&mut conn, &Person::NOBODY).await;
    for (name, days, ip) in [
        ("без адреса-1", 300, None),
        ("без адреса-2", 290, None),
        ("с адресом-1", 280, Some("198.51.100.4")),
        ("с адресом-2", 270, Some("198.51.100.5")),
        ("без адреса-3", 260, None),
        ("последняя", 100, Some("198.51.100.6")),
    ] {
        visit(
            &mut conn,
            owner,
            name,
            Visit {
                ip,
                ..cabinet(days)
            },
        )
        .await;
    }
    sweep(&db.store).await;
    assert_eq!(
        left(&mut conn, owner).await,
        ["без адреса-1", "с адресом-1", "последняя"]
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn из_одновременных_входов_первым_остаётся_меньший_номер() {
    let Some(db) = test_store().await else { return };
    let mut tx = db.store.begin().await.unwrap();
    let owner = client(&mut tx, &Person::NOBODY).await;
    for name in ["а", "б", "в"] {
        visit(&mut tx, owner, name, cabinet(300)).await;
    }
    visit(&mut tx, owner, "последняя", cabinet(200)).await;
    tx.commit().await.unwrap();
    let mut conn = db.store.conn().await.unwrap();
    assert_eq!(sweep(&db.store).await, 2);
    assert_eq!(left(&mut conn, owner).await, ["а", "последняя"]);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn у_удалённого_клиента_из_старого_не_остаётся_ничего() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let removed = client(&mut conn, &Person::NOBODY).await;
    let neighbor = client(&mut conn, &Person::NOBODY).await;
    for id in [removed, neighbor] {
        visit(&mut conn, id, "первая", cabinet(300)).await;
        visit(&mut conn, id, "мини", miniapp(200)).await;
    }
    visit(
        &mut conn,
        removed,
        "свежая",
        Visit {
            revoked: true,
            ..miniapp(10)
        },
    )
    .await;
    conn.clients().soft_delete(removed).await.unwrap();
    assert_eq!(sweep(&db.store).await, 2);
    assert_eq!(left(&mut conn, removed).await, ["свежая"]);
    assert_eq!(left(&mut conn, neighbor).await, ["первая", "мини"]);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn порция_ограничивает_просмотр_и_отдаёт_место_для_продолжения() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let owner = client(&mut conn, &Person::NOBODY).await;
    let mut ids = Vec::new();
    for n in 0..7 {
        ids.push(visit(&mut conn, owner, &n.to_string(), cabinet(300 - n)).await);
    }
    sqlx::query("UPDATE client_sessions SET user_agent = 'переставлена' WHERE id = ANY ($1)")
        .bind(vec![ids[0], ids[4]])
        .execute(conn.conn())
        .await
        .unwrap();
    let mut after = 0;
    let mut seen = Vec::new();
    for _ in 0..4 {
        let done = portion_of(&db.store, after, 3).await;
        seen.push((done.last, done.removed));
        after = done.last.unwrap_or(after);
    }
    assert_eq!(
        seen,
        [
            (Some(ids[2]), 2),
            (Some(ids[5]), 3),
            (Some(ids[6]), 0),
            (None, 0)
        ]
    );
    assert_eq!(left(&mut conn, owner).await, ["0", "6"]);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn занятая_строка_пропускается_без_ожидания() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let owner = client(&mut conn, &Person::NOBODY).await;
    for days in [400, 350, 300, 250] {
        visit(&mut conn, owner, &days.to_string(), cabinet(days)).await;
    }
    let mut busy = db.store.begin().await.unwrap();
    sqlx::query("UPDATE client_sessions SET revoked_at = now() WHERE token_hash = $1")
        .bind(format!("{}:350", owner.0).into_bytes())
        .execute(busy.conn())
        .await
        .unwrap();
    let removed = tokio::time::timeout(Duration::from_secs(10), sweep(&db.store))
        .await
        .expect("уборка ждёт чужую блокировку");
    assert_eq!(removed, 1);
    assert_eq!(left(&mut conn, owner).await, ["400", "350", "250"]);
    busy.rollback().await.unwrap();
    assert_eq!(sweep(&db.store).await, 1);
    assert_eq!(left(&mut conn, owner).await, ["400", "250"]);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn с_сессией_удаляется_её_привязка_а_вход_через_google_остаётся() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let owner = client(&mut conn, &Person::NOBODY).await;
    visit(&mut conn, owner, "первая", cabinet(300)).await;
    let old = visit(&mut conn, owner, "старая", cabinet(200)).await;
    let live = visit(&mut conn, owner, "живая", cabinet(1)).await;
    for (state, purpose, session) in [
        ("старая", "link", Some(old)),
        ("живая", "link", Some(live)),
        ("вход", "login", None),
    ] {
        sqlx::query(
            "INSERT INTO oauth_requests (provider, purpose, state_hash, browser_hash, client_id, session_id, expires_at) \
             VALUES ('google', $1, $2, $2, $3, $4, now())",
        )
        .bind(purpose)
        .bind(state.as_bytes())
        .bind(session.map(|_| owner.0))
        .bind(session)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    assert_eq!(sweep(&db.store).await, 1);
    let kept: Vec<String> = sqlx::query_scalar(
        "SELECT convert_from(state_hash, 'UTF8') FROM oauth_requests ORDER BY id",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap();
    assert_eq!(kept, ["живая", "вход"]);
    drop(conn);
    let mut tx = db.store.begin().await.unwrap();
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(tx.conn())
        .await
        .unwrap();
    let plan: Vec<String> =
        sqlx::query_scalar("EXPLAIN DELETE FROM ONLY oauth_requests WHERE 1::bigint = session_id")
            .fetch_all(tx.conn())
            .await
            .unwrap();
    assert!(
        plan.iter()
            .any(|line| line.contains("oauth_requests_session_idx")),
        "удаление сессии ищет привязки по индексу: {plan:?}"
    );
    tx.rollback().await.unwrap();
    db.cleanup().await;
}

async fn entries_read(tx: &mut Tx) -> i64 {
    sqlx::query_scalar(
        "SELECT (pg_stat_get_xact_tuples_returned('client_sessions'::regclass) \
             + COALESCE((SELECT sum(pg_stat_get_xact_tuples_returned(i.indexrelid)) FROM pg_index i \
                 WHERE i.indrelid = 'client_sessions'::regclass), 0))::bigint",
    )
    .fetch_one(tx.conn())
    .await
    .unwrap()
}

#[tokio::test]
async fn клиент_с_тысячами_сессий_не_замедляет_уборку() {
    const HEAVY: i64 = 6_000;
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let heavy = client(&mut conn, &Person::NOBODY).await;
    sqlx::query(
        "WITH others AS (INSERT INTO clients (locale, registration_source) \
             SELECT 'ru', 'web' FROM generate_series(1, 3000) RETURNING id), \
         visits AS (SELECT id AS client_id, n * 65.0 + id % 65 AS days FROM others CROSS JOIN generate_series(1, 6) n \
             UNION ALL SELECT $1, 455.0 * n / $2 FROM generate_series(1, $2) n) \
         INSERT INTO client_sessions (client_id, token_hash, channel, ip, user_agent, created_at, last_seen_at, expires_at) \
         SELECT client_id, decode(md5(client_id || ':' || days), 'hex'), 'miniapp', '203.0.113.7', repeat('x', 120), at, at, \
                at + interval '30 days' \
         FROM (SELECT client_id, days, now() - make_interval(secs => (days * 86400)::float8) AS at FROM visits ORDER BY days DESC) made",
    )
    .bind(heavy.0)
    .bind(HEAVY)
    .execute(conn.conn())
    .await
    .unwrap();
    sqlx::query("ANALYZE client_sessions, clients")
        .execute(conn.conn())
        .await
        .unwrap();
    let first: i64 = sqlx::query_scalar("SELECT min(id) FROM client_sessions")
        .fetch_one(conn.conn())
        .await
        .unwrap();
    drop(conn);
    for skipped in [5_000, 9_000] {
        let mut tx = db.store.begin().await.unwrap();
        let before = entries_read(&mut tx).await;
        let done = tx
            .sessions()
            .sweep_client(first + skipped, 200, KEEP)
            .await
            .unwrap();
        let read = entries_read(&mut tx).await - before;
        assert!(done.removed > 100, "удалено {}", done.removed);
        assert!(read < 200 * 20, "прочитано {read} записей на порцию");
        tx.rollback().await.unwrap();
    }
    assert!(sweep_by(&db.store, 200).await > HEAVY / 2);
    db.cleanup().await;
}

struct Dice(u64);

impl Dice {
    fn roll(&mut self, sides: u64) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) % sides
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Readers {
    first_ip: BTreeMap<i64, String>,
    shared_ips: BTreeSet<(i64, String)>,
    seen: BTreeSet<(i64, String, DateTime<Utc>)>,
    untouched: BTreeSet<i64>,
}

async fn readers(db: &mut impl Db) -> Readers {
    let first_ip: Vec<(i64, String)> = sqlx::query_as(
        "SELECT DISTINCT ON (s.client_id) s.client_id, s.ip FROM client_sessions s \
         JOIN clients c ON c.id = s.client_id AND c.deleted_at IS NULL WHERE s.ip IS NOT NULL \
         ORDER BY s.client_id, s.created_at, s.id",
    )
    .fetch_all(db.conn())
    .await
    .unwrap();
    let shared_ips: Vec<(i64, String)> = sqlx::query_as(
        "SELECT client_id, ip FROM client_sessions WHERE ip IS NOT NULL \
           AND (created_at >= now() - interval '60 days' OR last_seen_at >= now() - interval '60 days')",
    )
    .fetch_all(db.conn())
    .await
    .unwrap();
    let seen: Vec<(i64, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT s.client_id, s.channel, max(s.last_seen_at) FROM client_sessions s \
         JOIN clients c ON c.id = s.client_id AND c.deleted_at IS NULL GROUP BY s.client_id, s.channel",
    )
    .fetch_all(db.conn())
    .await
    .unwrap();
    let untouched: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM client_sessions WHERE (revoked_at IS NULL AND expires_at > now()) \
           OR GREATEST(created_at, last_seen_at) > now() - interval '89 days'",
    )
    .fetch_all(db.conn())
    .await
    .unwrap();
    Readers {
        first_ip: first_ip.into_iter().collect(),
        shared_ips: shared_ips.into_iter().collect(),
        seen: seen.into_iter().collect(),
        untouched: untouched.into_iter().collect(),
    }
}

#[tokio::test]
async fn на_случайных_сессиях_читатели_видят_то_же_а_лишнего_не_остаётся() {
    const IPS: [Option<&str>; 5] = [
        None,
        Some("203.0.113.1"),
        Some("203.0.113.2"),
        Some("2001:db8::1"),
        Some("198.51.100.9"),
    ];
    let Some(db) = test_store().await else { return };
    let mut dice = Dice(20_261_125);
    for portion in [1, 7, 500] {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query("TRUNCATE clients CASCADE")
            .execute(conn.conn())
            .await
            .unwrap();
        let mut owners = Vec::new();
        for _ in 0..25 {
            owners.push(client(&mut conn, &Person::NOBODY).await);
        }
        let mut visits = Vec::new();
        for owner in &owners {
            for _ in 0..dice.roll(14) {
                let days = i64::try_from(dice.roll(400)).unwrap();
                let seen = if dice.roll(4) == 0 {
                    i64::try_from(dice.roll(u64::try_from(days).unwrap() + 1)).unwrap()
                } else {
                    days
                };
                visits.push((
                    *owner,
                    Visit {
                        channel: if dice.roll(2) == 0 {
                            SessionChannel::Cabinet
                        } else {
                            SessionChannel::Miniapp
                        },
                        ip: IPS[usize::try_from(dice.roll(5)).unwrap()],
                        days,
                        seen,
                        left: if dice.roll(20) == 0 { 5 } else { TTL - days },
                        revoked: dice.roll(8) == 0,
                    },
                ));
            }
        }
        if portion != 7 {
            visits.sort_by_key(|(_, row)| std::cmp::Reverse(row.days));
        }
        for (n, (owner, row)) in visits.iter().enumerate() {
            visit(&mut conn, *owner, &n.to_string(), *row).await;
        }
        conn.clients().soft_delete(owners[0]).await.unwrap();
        let before = readers(&mut conn).await;
        let total = i64::try_from(visits.len()).unwrap();
        let removed = sweep_by(&db.store, portion).await;
        assert!(
            removed > total / 3,
            "порция {portion}: {removed} из {total}"
        );
        assert_eq!(readers(&mut conn).await, before, "порция {portion}");
        let spare: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM client_sessions s JOIN clients c ON c.id = s.client_id \
             WHERE GREATEST(s.created_at, s.last_seen_at) < now() - interval '91 days' \
               AND (s.expires_at <= now() OR s.revoked_at IS NOT NULL) \
               AND (c.deleted_at IS NOT NULL OR ( \
                   s.id <> (SELECT f.id FROM client_sessions f WHERE f.client_id = s.client_id ORDER BY f.created_at, f.id LIMIT 1) \
                   AND s.id IS DISTINCT FROM (SELECT f.id FROM client_sessions f WHERE f.client_id = s.client_id AND f.ip IS NOT NULL \
                                              ORDER BY f.created_at, f.id LIMIT 1) \
                   AND EXISTS (SELECT 1 FROM client_sessions l WHERE l.client_id = s.client_id AND l.channel = s.channel \
                               AND l.created_at > s.last_seen_at)))",
        )
        .fetch_one(conn.conn())
        .await
        .unwrap();
        assert_eq!(spare, 0, "порция {portion}");
        assert_eq!(sweep_by(&db.store, portion).await, 0);
        drop(conn);
    }
    db.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn уборки_и_входы_сразу_не_мешают_друг_другу() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let clients: Vec<i64> = sqlx::query_scalar(
        "INSERT INTO clients (locale, registration_source) SELECT 'ru', $1 FROM generate_series(1, 60) RETURNING id",
    )
    .bind(RegistrationSource::Web.as_str())
    .fetch_all(conn.conn())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO client_sessions (client_id, token_hash, channel, ip, created_at, last_seen_at, expires_at) \
         SELECT c.id, decode(md5(c.id || ':' || n), 'hex'), 'cabinet', '203.0.113.7', \
                now() - make_interval(days => 400 - n), now() - make_interval(days => 400 - n), \
                now() - make_interval(days => 370 - n) \
         FROM generate_series(1, 40) n CROSS JOIN clients c",
    )
    .execute(conn.conn())
    .await
    .unwrap();
    drop(conn);
    let sweeper = || {
        let store = db.store.clone();
        async move { sweep_by(&store, 50).await }
    };
    let visitor = {
        let store = db.store.clone();
        let clients = clients.clone();
        async move {
            for id in clients {
                let mut tx = store.begin().await.unwrap();
                tx.sessions().revoke_all_client(ClientId(id)).await.unwrap();
                visit(&mut tx, ClientId(id), "новая", cabinet(0)).await;
                tx.commit().await.unwrap();
            }
        }
    };
    let (first, second, ()) = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::join!(sweeper(), sweeper(), visitor)
    })
    .await
    .expect("уборка и входы ждут друг друга");
    let mut conn = db.store.conn().await.unwrap();
    let rest = sweep(&db.store).await;
    assert_eq!(first + second + rest, 60 * 39);
    for id in clients {
        let kept: Vec<(i64, bool)> = sqlx::query_as(
            "SELECT extract(day FROM now() - created_at)::bigint, revoked_at IS NULL FROM client_sessions \
             WHERE client_id = $1 ORDER BY created_at",
        )
        .bind(id)
        .fetch_all(conn.conn())
        .await
        .unwrap();
        assert_eq!(kept, [(399, false), (0, true)], "клиент {id}");
    }
    drop(conn);
    db.cleanup().await;
}
