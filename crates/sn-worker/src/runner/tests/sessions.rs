use super::*;
use sn_modules::catalog::antibot::signals::Signal;
use sn_modules::catalog::antibot::{self, AntibotSettings};
use sn_services::auth::RequestMeta;
use sn_store::{Conn, SessionChannel};

async fn registered(conn: &mut Conn, minute: i32) -> ClientId {
    ClientId(
        sqlx::query_scalar(
            "INSERT INTO clients (locale, registration_source, created_at) \
             VALUES ('ru', 'web', now() - interval '365 days' + make_interval(mins => $1)) RETURNING id",
        )
        .bind(minute)
        .fetch_one(conn.conn())
        .await
        .unwrap(),
    )
}

async fn visit(conn: &mut Conn, client_id: ClientId, days: i32, minute: i32, ip: &str) {
    sqlx::query(
        "INSERT INTO client_sessions (client_id, token_hash, channel, ip, created_at, last_seen_at, expires_at, revoked_at) \
         SELECT $1, decode(md5($1 || ':' || $2), 'hex'), 'cabinet', $4, at, at, at + interval '30 days', \
                CASE WHEN $2 = 200 THEN at END \
         FROM (SELECT now() - make_interval(days => $2) + make_interval(mins => $3) AS at) moment",
    )
    .bind(client_id.0)
    .bind(days)
    .bind(minute)
    .bind(ip)
    .execute(conn.conn())
    .await
    .unwrap();
}

async fn limited(conn: &mut Conn, clients: &[ClientId]) -> Vec<bool> {
    let now = sn_modules::catalog::db_now(conn).await.unwrap();
    let mut found = Vec::new();
    for id in clients {
        let assessment = antibot::assess(conn, *id, &AntibotSettings::default(), now)
            .await
            .unwrap();
        found.push(assessment.signals.contains(&Signal::IpLimit));
    }
    found
}

#[tokio::test]
async fn обслуживание_убирает_старые_сессии_а_входы_и_антибот_прежние() {
    let Some(setup) = setup(Vec::new()).await else {
        return;
    };
    let services = &setup.services;
    let mut conn = services.store().conn().await.unwrap();
    let mut clients = Vec::new();
    for minute in 0..5 {
        let id = registered(&mut conn, minute).await;
        visit(&mut conn, id, 365, minute + 1, "203.0.113.7").await;
        for days in [300, 200, 91, 89] {
            visit(&mut conn, id, days, 0, "198.51.100.9").await;
        }
        clients.push(id);
    }
    let before = limited(&mut conn, &clients).await;
    assert_eq!(before, [false, false, false, true, true]);
    drop(conn);
    let login = services
        .auth()
        .client_session_for(
            clients[0],
            SessionChannel::Cabinet,
            &RequestMeta::default(),
            false,
        )
        .await
        .unwrap();
    let early = Worker::new(services.clone(), WorkerConfig::default());
    assert_eq!(
        early.housekeeping().await.unwrap()["removed_sessions"],
        0,
        "сразу после запуска уборка ждёт"
    );
    let worker = Worker::new(
        services.clone(),
        WorkerConfig {
            session_sweep_delay: Duration::ZERO,
            ..WorkerConfig::default()
        },
    );
    let report = worker
        .run_periodic(Periodic::Housekeeping)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(report["removed_sessions"], 15, "{report}");
    let principal = services.auth().client_session(&login.token).await.unwrap();
    assert_eq!(principal.client_id, clients[0]);
    let mut conn = services.store().conn().await.unwrap();
    assert_eq!(limited(&mut conn, &clients).await, before);
    let left: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT client_id, extract(day FROM now() - created_at)::bigint FROM client_sessions \
         WHERE client_id = ANY ($1) ORDER BY client_id, created_at",
    )
    .bind(clients.iter().map(|id| id.0).collect::<Vec<_>>())
    .fetch_all(conn.conn())
    .await
    .unwrap();
    let mut expected = Vec::new();
    for id in &clients {
        expected.extend([(id.0, 364), (id.0, 89)]);
    }
    expected.insert(2, (clients[0].0, 0));
    assert_eq!(left, expected);
    drop(conn);
    setup.db.cleanup().await;
}
