use super::*;
use tokio::task::JoinHandle;

async fn exists(admin: &mut PgConnection, schema: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)")
        .bind(schema)
        .fetch_one(admin)
        .await
        .unwrap()
}

async fn note(admin: &mut PgConnection, schema: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT obj_description(oid, 'pg_namespace') FROM pg_namespace WHERE nspname = $1",
    )
    .bind(schema)
    .fetch_one(admin)
    .await
    .unwrap()
}

async fn run(admin: &mut PgConnection, sql: &str) {
    sqlx::query(sql).execute(admin).await.unwrap();
}

fn undated() -> String {
    format!(
        "{SCHEMA_PREFIX}{}",
        &uuid::Uuid::new_v4().simple().to_string()[..16]
    )
}

#[test]
fn брошенная_схема_узнаётся_по_времени_в_имени_или_по_отметке_уборки() {
    let now = 1_800_000_000;
    let dated = |born: i64| format!("{SCHEMA_PREFIX}{born}_0123456789ab");
    let seen = |at: i64| format!("{SEEN_NOTE}{at}");
    assert_eq!(
        verdict(&dated(now - STALE_AFTER_SECS - 1), None, now),
        Verdict::Remove
    );
    assert_eq!(
        verdict(&dated(now - STALE_AFTER_SECS + 60), None, now),
        Verdict::Keep
    );
    assert_eq!(verdict(&dated(now + 30), None, now), Verdict::Keep);
    let legacy = "sn_t_0123456789abcdef";
    assert_eq!(verdict(legacy, None, now), Verdict::Mark);
    assert_eq!(verdict(legacy, Some("чужая заметка"), now), Verdict::Mark);
    assert_eq!(verdict(legacy, Some(&seen(now - 3600)), now), Verdict::Keep);
    assert_eq!(
        verdict(legacy, Some(&seen(now - UNDATED_STALE_AFTER_SECS - 1)), now),
        Verdict::Remove
    );
    assert_eq!(verdict("sn_t_1234567890123456", None, now), Verdict::Mark);
    assert_eq!(verdict("public", None, now), Verdict::Keep);
    assert_eq!(verdict("sn_restore_0123456789ab", None, now), Verdict::Keep);
    let named = schema_name(now);
    assert_eq!(
        born_at(named.strip_prefix(SCHEMA_PREFIX).unwrap()),
        Some(now)
    );
}

#[test]
fn тест_ждёт_новое_окно_только_если_до_конца_текущего_мало_времени() {
    let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().to_utc();
    let window = Duration::from_secs(15 * 60);
    let needed = Duration::from_secs(60);
    assert_eq!(
        window_wait(at("2026-11-19T21:30:00Z"), window, needed),
        None
    );
    assert_eq!(
        window_wait(at("2026-11-19T21:43:59Z"), window, needed),
        None
    );
    assert_eq!(
        window_wait(at("2026-11-19T21:44:30Z"), window, needed),
        Some(Duration::from_millis(30_200))
    );
    assert_eq!(
        window_wait(at("2026-11-19T21:44:59.900Z"), window, needed),
        Some(Duration::from_millis(300))
    );
    assert_eq!(
        window_wait(at("1969-12-31T23:59:30Z"), window, needed),
        Some(Duration::from_millis(30_200))
    );
    assert_eq!(
        window_wait(at("2026-11-19T21:44:30Z"), Duration::ZERO, needed),
        None
    );
}

#[tokio::test]
async fn схема_без_cleanup_удаляется_когда_тест_отпускает_базу() {
    let Some(url) = test_database_url() else {
        return;
    };
    let started = Utc::now().timestamp();
    let db = test_store().await.unwrap();
    let schema = db.schema().to_owned();
    let born = born_at(schema.strip_prefix(SCHEMA_PREFIX).unwrap()).unwrap();
    assert!((started..=Utc::now().timestamp()).contains(&born));
    let mut admin = admin(&url, "10s").await.unwrap();
    assert!(exists(&mut admin, &schema).await);
    drop(db);
    assert!(!exists(&mut admin, &schema).await);
    admin.close().await.unwrap();
}

#[tokio::test]
async fn схема_упавшего_теста_удаляется_даже_посреди_транзакции() {
    let Some(url) = test_database_url() else {
        return;
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let failed: JoinHandle<()> = tokio::spawn(async move {
        let db = test_store().await.unwrap();
        let mut tx = db.store.pool().begin().await.unwrap();
        sqlx::query("SELECT count(*) FROM clients")
            .execute(&mut *tx)
            .await
            .unwrap();
        sender.send(db.schema().to_owned()).unwrap();
        panic!("тест упал посреди транзакции");
    });
    assert!(failed.await.unwrap_err().is_panic());
    let schema = receiver.await.unwrap();
    let mut admin = admin(&url, "10s").await.unwrap();
    assert!(!exists(&mut admin, &schema).await);
    admin.close().await.unwrap();
}

#[tokio::test]
async fn уборка_удаляет_брошенные_схемы_и_не_трогает_свежие() {
    let Some(url) = test_database_url() else {
        return;
    };
    let mut admin = admin(&url, "120s").await.unwrap();
    run(
        &mut admin,
        &format!("SELECT pg_advisory_lock({SWEEP_LOCK})"),
    )
    .await;
    let now = Utc::now().timestamp();
    let old = schema_name(now - STALE_AFTER_SECS - 60);
    let fresh = schema_name(now - STALE_AFTER_SECS + 600);
    let unmarked = undated();
    let marked_long_ago = undated();
    let marked_recently = undated();
    for schema in [&old, &fresh, &unmarked, &marked_long_ago, &marked_recently] {
        run(&mut admin, &format!("CREATE SCHEMA {}", quoted(schema))).await;
    }
    run(
        &mut admin,
        &format!("CREATE TABLE {}.leftover (id int)", quoted(&old)),
    )
    .await;
    let long_ago = now - UNDATED_STALE_AFTER_SECS - 60;
    run(
        &mut admin,
        &format!(
            "COMMENT ON SCHEMA {} IS '{SEEN_NOTE}{long_ago}'",
            quoted(&marked_long_ago)
        ),
    )
    .await;
    let recently = now - 600;
    run(
        &mut admin,
        &format!(
            "COMMENT ON SCHEMA {} IS '{SEEN_NOTE}{recently}'",
            quoted(&marked_recently)
        ),
    )
    .await;
    sweep_stale(&mut admin, now).await;
    let old_left = exists(&mut admin, &old).await;
    let marked_long_ago_left = exists(&mut admin, &marked_long_ago).await;
    let fresh_left = exists(&mut admin, &fresh).await;
    let recent_note = note(&mut admin, &marked_recently).await;
    let unmarked_note = note(&mut admin, &unmarked).await;
    for schema in [&old, &fresh, &unmarked, &marked_long_ago, &marked_recently] {
        run(
            &mut admin,
            &format!("DROP SCHEMA IF EXISTS {} CASCADE", quoted(schema)),
        )
        .await;
    }
    admin.close().await.unwrap();
    assert!(!old_left, "схема с временем старше {STALE_AFTER_SECS} с");
    assert!(!marked_long_ago_left, "схема с отметкой старше суток");
    assert!(fresh_left, "свежая схема соседнего прогона");
    assert_eq!(recent_note, Some(format!("{SEEN_NOTE}{recently}")));
    assert!(unmarked_note.is_some_and(|n| n.starts_with(SEEN_NOTE)));
}
