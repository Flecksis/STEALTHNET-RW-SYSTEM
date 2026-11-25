use super::*;
use crate::checkout::testkit::{Kit, client, kit};
use sn_domain::ids::ClientId;

const TAIL: i64 = 70;

async fn visits(services: &Services, client_id: ClientId, count: i64, days: i64) {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query(
        "INSERT INTO client_sessions (client_id, token_hash, channel, ip, created_at, last_seen_at, expires_at) \
         SELECT $1, decode(md5($1 || ':' || $3 || ':' || n), 'hex'), 'miniapp', '203.0.113.7', at, at, at + interval '30 days' \
         FROM generate_series(1, $2) n, \
              LATERAL (SELECT now() - make_interval(days => $3::int) + make_interval(secs => n::float8) AS at) moment",
    )
    .bind(client_id.0)
    .bind(count)
    .bind(days)
    .execute(conn.conn())
    .await
    .unwrap();
}

async fn rows(services: &Services) -> i64 {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_scalar("SELECT count(*) FROM client_sessions")
        .fetch_one(conn.conn())
        .await
        .unwrap()
}

async fn old_visits(count: i64) -> Option<(Kit, ClientId)> {
    let kit = kit().await?;
    let owner = client(&kit.services, 7_001).await;
    visits(&kit.services, owner, count, 200).await;
    Some((kit, owner))
}

#[tokio::test]
async fn проход_убирает_всё_старое_и_повторяется_через_сутки() {
    let Some((kit, owner)) = old_visits(PORTION * 2 + TAIL).await else {
        return;
    };
    let services = &kit.services;
    let start = Instant::now();
    let mut sweep = SessionSweep::default();
    let long = Duration::from_secs(120);
    assert_eq!(
        sweep.run_at(services, start, long).await.unwrap(),
        PORTION * 2 + TAIL - 2
    );
    assert_eq!(rows(services).await, 2);
    visits(services, owner, 10, 150).await;
    for (passed, removed) in [
        (Duration::ZERO, 0),
        (REST - Duration::from_secs(1), 0),
        (REST, 10),
        (REST, 0),
    ] {
        assert_eq!(
            sweep.run_at(services, start + passed, long).await.unwrap(),
            removed,
            "{passed:?}"
        );
    }
    assert_eq!(rows(services).await, 2);
    assert_eq!(
        SessionSweep::default().run(services).await.unwrap(),
        0,
        "после перезапуска панели лишнего не удаляется"
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn без_времени_проход_идёт_по_порции_и_продолжается_с_того_же_места() {
    let Some((kit, _)) = old_visits(PORTION + TAIL).await else {
        return;
    };
    let services = &kit.services;
    let start = Instant::now();
    let mut sweep = SessionSweep::default();
    let mut seen = Vec::new();
    for _ in 0..4 {
        let removed = sweep.run_at(services, start, Duration::ZERO).await.unwrap();
        seen.push((removed, sweep.finished.is_some()));
    }
    assert_eq!(
        seen,
        [
            (PORTION - 1, false),
            (TAIL - 1, false),
            (0, true),
            (0, true)
        ]
    );
    assert_eq!(rows(services).await, 2);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn после_сбоя_базы_проход_продолжается() {
    let Some((kit, _)) = old_visits(PORTION + TAIL).await else {
        return;
    };
    let services = &kit.services;
    let start = Instant::now();
    let mut sweep = SessionSweep::default();
    assert_eq!(
        sweep.run_at(services, start, Duration::ZERO).await.unwrap(),
        PORTION - 1
    );
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query("ALTER TABLE client_sessions RENAME TO client_sessions_away")
        .execute(conn.conn())
        .await
        .unwrap();
    drop(conn);
    assert!(sweep.run_at(services, start, Duration::ZERO).await.is_err());
    assert!(sweep.finished.is_none());
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query("ALTER TABLE client_sessions_away RENAME TO client_sessions")
        .execute(conn.conn())
        .await
        .unwrap();
    drop(conn);
    assert_eq!(
        sweep
            .run_at(services, start, Duration::from_secs(120))
            .await
            .unwrap(),
        TAIL - 1
    );
    assert_eq!(rows(services).await, 2);
    kit.db.cleanup().await;
}
