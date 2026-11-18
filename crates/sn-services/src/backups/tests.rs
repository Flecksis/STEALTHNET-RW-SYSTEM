use super::*;
use crate::{Clock, EnvFallbacks, FixedClock, ServiceDeps, SystemClock};
use sn_domain::alerts::AlertsConfig;
use sn_domain::payment::providers;
use sn_domain::settings::{keys, secret_keys};
use sn_modules::ModuleRegistry;
use sn_payments::ProviderRegistry;
use sn_remnawave::fake::{FakeRemnawave, sample_user};
use sn_store::Store;
use sn_store::testing::{TestDb, empty_test_store, test_store};
use std::collections::BTreeMap;
use std::sync::Arc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "4321:TEST";
const BOT: &str = "123456:BOTTOKEN";

struct Env {
    db: TestDb,
    store: Store,
    services: Services,
    fake: Arc<FakeRemnawave>,
    dir: PathBuf,
}

impl Env {
    async fn cleanup(self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        self.db.cleanup().await;
    }

    fn ops(&self) -> BackupOps<'_> {
        self.services.backups()
    }
}

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sn-backups-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn services_for(
    store: Store,
    key: &str,
    dir: &Path,
    telegram_api: &str,
    clock: Arc<dyn Clock>,
) -> (Services, Arc<FakeRemnawave>) {
    let services = Services::new(ServiceDeps {
        store,
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(key).unwrap()),
        clock,
        env: EnvFallbacks {
            bot_token: Some(BOT.into()),
            ..EnvFallbacks::default()
        },
    });
    services.set_backup_env(Some(BackupEnv {
        dir: Some(dir.join("copies")),
        host_dir: None,
        telegram_api: telegram_api.to_string(),
        secret_key: Some(key.to_string()),
        kdf: KdfParams::QUICK,
        upload_dir: dir.join("uploads"),
        telegram_part: telegram::PART_SIZE,
    }));
    services.reload().await.unwrap();
    let fake = Arc::new(FakeRemnawave::new());
    services.set_remna(Some(fake.clone()));
    (services, fake)
}

async fn env_with(key: &str, telegram_api: &str) -> Option<Env> {
    env_on(key, telegram_api, Arc::new(SystemClock)).await
}

async fn env_on(key: &str, telegram_api: &str, clock: Arc<dyn Clock>) -> Option<Env> {
    let db = test_store().await?;
    let dir = temp_dir();
    let store = db.store.clone();
    let (services, fake) = services_for(store.clone(), key, &dir, telegram_api, clock).await;
    Some(Env {
        db,
        store,
        services,
        fake,
        dir,
    })
}

async fn env() -> Option<Env> {
    env_with(&SecretBox::generate_key_base64(), "http://127.0.0.1:9").await
}

async fn partial_env(key: &str, last: i64) -> Option<Env> {
    let db = empty_test_store().await?;
    let store = db.store.clone();
    let migrator = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(
            sn_store::MIGRATOR
                .iter()
                .filter(|m| m.version <= last || m.version == 400)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    migrator.run(store.pool()).await.unwrap();
    let dir = temp_dir();
    let (services, fake) = services_for(
        store.clone(),
        key,
        &dir,
        "http://127.0.0.1:9",
        Arc::new(SystemClock),
    )
    .await;
    Some(Env {
        db,
        store,
        services,
        fake,
        dir,
    })
}

async fn exec(store: &Store, sql: &str) {
    let mut conn = store.conn().await.unwrap();
    sqlx::Executor::execute(conn.conn(), sql).await.unwrap();
}

async fn seed(env: &Env) {
    exec(
        &env.store,
        "INSERT INTO clients (registration_source, note) VALUES ('bot', 'первый'), ('web', 'второй'), ('admin', NULL);\
         INSERT INTO subscriptions (client_id, kind, remna_user_id, remna_username, short_uuid, status, expire_at, traffic_limit_bytes) \
           SELECT id, 'paid', 100 + id, 'user' || id, 'short' || (100 + id), 'active', now() + interval '30 days', 107374182400 FROM clients WHERE id <= 2;\
         INSERT INTO orders (client_id, kind, status, intent, quote, currency, total_minor) \
           SELECT id, 'new', 'fulfilled', '{}'::jsonb, '{}'::jsonb, 'USD', 299 FROM clients WHERE id <= 2;\
         INSERT INTO payments (order_id, client_id, provider, amount_minor, currency, status, provider_txid) \
           SELECT id, client_id, 'cryptopay', 299, 'USD', 'paid', 'tx-' || id FROM orders;",
    )
    .await;
    let mut conn = env.store.conn().await.unwrap();
    conn.settings()
        .set(keys::BRAND_NAME, &serde_json::json!("Тестовый VPN"), None)
        .await
        .unwrap();
    crate::settings::write_secret(
        &mut conn,
        env.services.secrets(),
        &secret_keys::provider(providers::CRYPTOPAY, "api_token"),
        Some(TOKEN),
        None,
    )
    .await
    .unwrap();
    drop(conn);
    for id in [101, 102] {
        let mut user = sample_user(id, &format!("user{}", id - 100));
        user.traffic_limit_bytes = 107_374_182_400;
        user.hwid_device_limit = Some(3);
        env.fake.insert_user(user);
    }
    env.fake.insert_user(sample_user(500, "stranger"));
    env.services.reload().await.unwrap();
    env.services.set_remna(Some(env.fake.clone()));
}

struct Remna(Vec<u8>);

impl archive::Visitor for Remna {
    fn manifest(&mut self, _raw: &[u8]) -> std::io::Result<()> {
        Ok(())
    }

    fn remna(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        self.0.extend_from_slice(chunk);
        Ok(())
    }
}

fn remna_snapshot(path: &Path, password: Option<&str>) -> Vec<Value> {
    let mut visitor = Remna(Vec::new());
    archive::read(path, password, &mut visitor).unwrap();
    visitor
        .0
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect()
}

async fn fingerprint(store: &Store) -> BTreeMap<String, String> {
    let mut conn = store.conn().await.unwrap();
    let schema: String = sqlx::query_scalar("SELECT current_schema()::text")
        .fetch_one(conn.conn())
        .await
        .unwrap();
    let found = dump::inspect(conn.conn(), &schema, &load::RESTORE_SKIPPED)
        .await
        .unwrap();
    let mut out = BTreeMap::new();
    for table in found.tables {
        let hash: String = sqlx::query_scalar(&format!(
            "SELECT md5(coalesce(string_agg(t::text, E'\\n' ORDER BY t::text), '')) FROM {} t",
            dump::qualified(&schema, &table.name)
        ))
        .fetch_one(conn.conn())
        .await
        .unwrap();
        out.insert(table.name, hash);
    }
    out
}

async fn secret_token(services: &Services) -> Option<String> {
    let mut conn = services.store().conn().await.unwrap();
    crate::settings::read_secret(
        &mut conn,
        services.secrets(),
        &secret_keys::provider(providers::CRYPTOPAY, "api_token"),
    )
    .await
    .unwrap()
}

async fn set_password(env: &Env, password: &str) {
    let mut conn = env.store.conn().await.unwrap();
    let sealed = env
        .services
        .secrets()
        .unwrap()
        .seal_str(PASSWORD_KEY, password)
        .unwrap();
    conn.secrets()
        .set(PASSWORD_KEY, &sealed, None)
        .await
        .unwrap();
}

async fn scalar_i64(store: &Store, sql: &str) -> i64 {
    let mut conn = store.conn().await.unwrap();
    sqlx::query_scalar(sql)
        .fetch_one(conn.conn())
        .await
        .unwrap()
}

#[tokio::test]
async fn копия_и_восстановление_возвращают_базу_вместе_с_подписками_и_снимком_remnawave() {
    let Some(env) = env().await else { return };
    seed(&env).await;
    let before = fingerprint(&env.store).await;
    let made = env.ops().make_now(RunKind::Manual, None).await.unwrap();
    assert!(made.name.ends_with(".tar.gz"), "{}", made.name);
    assert_eq!(made.manifest.counts.clients, 3);
    assert_eq!(made.manifest.counts.subscriptions, 2);
    assert_eq!(made.manifest.counts.payments, 2);
    let remna = made.manifest.remnawave.clone().unwrap();
    assert_eq!(remna.source, "remnawave");
    assert_eq!((remna.users, remna.linked, remna.missing), (3, 2, 0));
    assert!(!made.manifest.secret_key.included);
    let inspection = inspect_file(&made.path, None).unwrap();
    assert_eq!(inspection.remna_lines, 2);
    exec(
        &env.store,
        "UPDATE clients SET note = 'испорчено';\
         INSERT INTO clients (registration_source) VALUES ('web');\
         UPDATE subscriptions SET expire_at = now() - interval '1 day', status = 'expired';\
         DELETE FROM secrets WHERE NOT starts_with(key, 'backups.');",
    )
    .await;
    assert_ne!(fingerprint(&env.store).await, before);
    let result = env.ops().restore_now(&made.path, None).await.unwrap();
    let after = fingerprint(&env.store).await;
    let changed: Vec<&String> = before
        .iter()
        .filter(|(table, hash)| after.get(*table) != Some(*hash))
        .map(|(table, _)| table)
        .collect();
    assert_eq!(changed, ["audit_log", "bot_state"], "{changed:?}");
    assert_eq!(result["keys"], "same");
    assert_eq!(result["migrated"], 0);
    assert!(
        result["before"]
            .as_str()
            .unwrap()
            .contains("before-restore")
    );
    assert_eq!(secret_token(&env.services).await.as_deref(), Some(TOKEN));
    let snapshot = remna_snapshot(&made.path, None);
    let users: Vec<&Value> = snapshot.iter().skip(1).collect();
    assert_eq!(snapshot[0]["source"], "remnawave");
    assert_eq!(users.len(), 2);
    assert_eq!(users[0]["user"]["shortUuid"], "short101");
    assert_eq!(users[0]["user"]["hwidDeviceLimit"], 3);
    assert_eq!(users[0]["user"]["trafficLimitBytes"], 107_374_182_400i64);
    assert_eq!(users[0]["panel"]["short_uuid"], "short101");
    let runs = scalar_i64(
        &env.store,
        "SELECT count(*) FROM backup_runs WHERE kind = 'restore' AND state = 'done'",
    )
    .await;
    assert_eq!(runs, 1);
    let leftovers = scalar_i64(
        &env.store,
        "SELECT count(*) FROM pg_namespace WHERE nspname LIKE 'sn_restore_%'",
    )
    .await;
    assert_eq!(leftovers, 0);
    let copies = env.ops().copies().await.unwrap();
    assert_eq!(copies.len(), 2);
    assert!(copies.iter().any(|c| c.kind == RunKind::BeforeRestore));
    env.cleanup().await;
}

#[tokio::test]
async fn копия_старой_версии_восстанавливается_с_обновлением_базы() {
    let key = SecretBox::generate_key_base64();
    let Some(old) = partial_env(&key, 300).await else {
        return;
    };
    seed(&old).await;
    let made = old.ops().make_now(RunKind::Manual, None).await.unwrap();
    assert!(
        made.manifest
            .migrations
            .iter()
            .all(|v| *v <= 300 || *v == 400)
    );
    assert!(!made.manifest.migrations.contains(&320));
    let Some(fresh) = env_with(&key, "http://127.0.0.1:9").await else {
        old.cleanup().await;
        return;
    };
    let preview_source = made.path.clone();
    let inspection = inspect_file(&preview_source, None).unwrap();
    let preview = fresh
        .ops()
        .preview_of(&inspection, "старая".into())
        .unwrap();
    assert!(!preview.same_version);
    assert!(preview.migrations_to_apply > 0);
    assert_eq!(preview.keys, "same");
    let result = fresh.ops().restore_now(&made.path, None).await.unwrap();
    assert!(result["migrated"].as_u64().unwrap() > 0);
    assert_eq!(
        scalar_i64(&fresh.store, "SELECT count(*) FROM clients").await,
        3
    );
    assert_eq!(
        scalar_i64(
            &fresh.store,
            "SELECT count(*) FROM subscriptions WHERE short_uuid = 'short101'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar_i64(&fresh.store, "SELECT count(*) FROM legacy_invoices").await,
        0
    );
    let applied = scalar_i64(
        &fresh.store,
        "SELECT count(*) FROM _sqlx_migrations WHERE success",
    )
    .await;
    assert_eq!(applied as usize, load::known_migrations().len());
    assert_eq!(secret_token(&fresh.services).await.as_deref(), Some(TOKEN));
    let next: i64 = scalar_i64(
        &fresh.store,
        "INSERT INTO clients (registration_source) VALUES ('web') RETURNING id",
    )
    .await;
    assert_eq!(next, 4);
    old.cleanup().await;
    fresh.cleanup().await;
}

#[tokio::test]
async fn копия_с_паролем_несёт_ключ_и_переносит_секреты_на_сервер_с_другим_ключом() {
    let Some(source) = env().await else { return };
    seed(&source).await;
    set_password(&source, "длинный пароль копий").await;
    let keys = source
        .services
        .secrets()
        .unwrap()
        .seal("remna.keys:1", b"{\"vless\":\"uuid-1\"}")
        .unwrap();
    {
        let mut conn = source.store.conn().await.unwrap();
        sqlx::query(
            "INSERT INTO remna_snapshots (subscription_id, remna_user_id, short_uuid, keys_nonce, keys_sealed) \
             VALUES (1, 101, 'short101', $1, $2)",
        )
        .bind(&keys.nonce)
        .bind(&keys.ciphertext)
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let made = source.ops().make_now(RunKind::Manual, None).await.unwrap();
    assert!(made.name.ends_with(".tar.gz.enc"), "{}", made.name);
    assert!(made.manifest.secret_key.included);
    assert!(matches!(
        inspect_file(&made.path, None),
        Err(ArchiveError::PasswordNeeded)
    ));
    let err = source
        .ops()
        .preview(
            &Source::File {
                name: made.name.clone(),
            },
            Some("неверный пароль"),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), "backups.wrong_password");
    let Some(target) = env().await else {
        source.cleanup().await;
        return;
    };
    let before = fingerprint(&target.store).await;
    let err = target
        .ops()
        .restore_now(&made.path, Some("неверный пароль".into()))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "backups.wrong_password");
    assert_eq!(fingerprint(&target.store).await, before);
    let result = target
        .ops()
        .restore_now(&made.path, Some("длинный пароль копий".into()))
        .await
        .unwrap();
    assert_eq!(result["keys"], "from_copy");
    assert_eq!(result["secrets"]["mode"], "rekeyed");
    assert_eq!(secret_token(&target.services).await.as_deref(), Some(TOKEN));
    let (nonce, sealed): (Vec<u8>, Vec<u8>) = {
        let mut conn = target.store.conn().await.unwrap();
        sqlx::query_as(
            "SELECT keys_nonce, keys_sealed FROM remna_snapshots WHERE subscription_id = 1",
        )
        .fetch_one(conn.conn())
        .await
        .unwrap()
    };
    let plain = target
        .services
        .secrets()
        .unwrap()
        .open(
            "remna.keys:1",
            &sn_store::SealedSecret {
                nonce,
                ciphertext: sealed,
            },
        )
        .unwrap();
    assert_eq!(plain, b"{\"vless\":\"uuid-1\"}");
    source.cleanup().await;
    target.cleanup().await;
}

#[tokio::test]
async fn копия_без_пароля_на_сервере_с_другим_ключом_предупреждает_и_стирает_секреты() {
    let Some(source) = env().await else { return };
    seed(&source).await;
    let made = source.ops().make_now(RunKind::Manual, None).await.unwrap();
    let Some(target) = env().await else {
        source.cleanup().await;
        return;
    };
    let inspection = inspect_file(&made.path, None).unwrap();
    let preview = target
        .ops()
        .preview_of(&inspection, "чужая".into())
        .unwrap();
    assert_eq!(preview.keys, "lost");
    assert!(
        preview
            .warnings
            .iter()
            .any(|w| w.contains("придётся ввести заново"))
    );
    let result = target.ops().restore_now(&made.path, None).await.unwrap();
    assert_eq!(result["secrets"]["mode"], "cleared");
    assert_eq!(secret_token(&target.services).await, None);
    assert_eq!(
        scalar_i64(&target.store, "SELECT count(*) FROM clients").await,
        3
    );
    assert_eq!(
        scalar_i64(
            &target.store,
            "SELECT count(*) FROM secrets WHERE NOT starts_with(key, 'backups.')"
        )
        .await,
        0
    );
    source.cleanup().await;
    target.cleanup().await;
}

#[tokio::test]
async fn битый_и_обрезанный_файл_не_трогают_базу() {
    let Some(env) = env().await else { return };
    seed(&env).await;
    let made = env.ops().make_now(RunKind::Manual, None).await.unwrap();
    let before = fingerprint(&env.store).await;
    let bytes = std::fs::read(&made.path).unwrap();
    let broken = env.dir.join("broken.tar.gz");
    let mut damaged = bytes.clone();
    let at = damaged.len() / 2;
    damaged[at] ^= 0x5a;
    std::fs::write(&broken, &damaged).unwrap();
    assert!(env.ops().restore_now(&broken, None).await.is_err());
    let cut = env.dir.join("cut.tar.gz");
    std::fs::write(&cut, &bytes[..bytes.len() - 100]).unwrap();
    let err = env.ops().restore_now(&cut, None).await.unwrap_err();
    assert!(err.to_string().contains("обрезан"), "{err}");
    let foreign = env.dir.join("foreign.tar.gz");
    std::fs::write(&foreign, b"PK\x03\x04 not a backup").unwrap();
    assert!(env.ops().restore_now(&foreign, None).await.is_err());
    assert_eq!(fingerprint(&env.store).await, before);
    let failed = scalar_i64(
        &env.store,
        "SELECT count(*) FROM backup_runs WHERE kind = 'restore' AND state = 'failed'",
    )
    .await;
    assert_eq!(failed, 3);
    env.cleanup().await;
}

#[tokio::test]
async fn на_сервере_хранятся_только_последние_копии() {
    let Some(env) = env().await else { return };
    let mut config = BackupConfig {
        keep: 2,
        ..BackupConfig::default()
    };
    config.telegram = TelegramTarget::Off;
    {
        let mut conn = env.store.conn().await.unwrap();
        conn.settings()
            .set_as(CONFIG_KEY, &config, None)
            .await
            .unwrap();
    }
    let mut names = Vec::new();
    for _ in 0..3 {
        names.push(
            env.ops()
                .make_now(RunKind::Manual, None)
                .await
                .unwrap()
                .name,
        );
    }
    let left: Vec<String> = env
        .ops()
        .copies()
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(left.len(), 2, "{left:?}");
    assert!(left.contains(&names[2]));
    assert!(left.contains(&names[1]));
    assert!(!left.contains(&names[0]));
    env.cleanup().await;
}

#[tokio::test]
async fn копия_по_расписанию_делается_один_раз_за_слот() {
    let clock = Arc::new(FixedClock::new(Utc::now()));
    let key = SecretBox::generate_key_base64();
    let Some(env) = env_on(&key, "http://127.0.0.1:9", clock).await else {
        return;
    };
    {
        let mut conn = env.store.conn().await.unwrap();
        conn.settings()
            .set_as(
                CONFIG_KEY,
                &BackupConfig {
                    schedule: Schedule::Hours { hours: 1 },
                    ..BackupConfig::default()
                },
                None,
            )
            .await
            .unwrap();
    }
    assert_eq!(
        env.ops().next_run_in().await.unwrap(),
        Some(std::time::Duration::ZERO)
    );
    let made = env.ops().run_scheduled().await.unwrap().unwrap();
    assert!(made.name.contains("-auto"));
    assert!(env.ops().run_scheduled().await.unwrap().is_none());
    let wait = env.ops().next_run_in().await.unwrap().unwrap();
    assert!(wait > std::time::Duration::ZERO && wait <= std::time::Duration::from_secs(3600));
    env.cleanup().await;
}

#[tokio::test]
async fn копия_уходит_в_тему_группы_уведомлений_частями_на_поддельном_bot_api() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/bot{BOT}/createForumTopic")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {"message_thread_id": 77, "name": "Резервные копии"}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/bot{BOT}/sendDocument")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {"message_id": 5}
        })))
        .mount(&server)
        .await;
    let Some(env) = env_with(&SecretBox::generate_key_base64(), &server.uri()).await else {
        return;
    };
    seed(&env).await;
    let mut backup_env = env.ops().env();
    backup_env.telegram_part = 4096;
    env.services.set_backup_env(Some(backup_env));
    {
        let mut conn = env.store.conn().await.unwrap();
        conn.settings()
            .set_as(
                sn_domain::alerts::SETTING_KEY,
                &AlertsConfig {
                    chat_id: Some(-100_500),
                    title: Some("Админы".into()),
                    forum: true,
                    ..AlertsConfig::default()
                },
                None,
            )
            .await
            .unwrap();
        conn.settings()
            .set_as(
                CONFIG_KEY,
                &BackupConfig {
                    telegram: TelegramTarget::Alerts,
                    ..BackupConfig::default()
                },
                None,
            )
            .await
            .unwrap();
    }
    let made = env.ops().make_now(RunKind::Manual, None).await.unwrap();
    let parts = made.size.div_ceil(4096);
    assert!(parts > 1, "размер {}", made.size);
    let requests = server.received_requests().await.unwrap();
    let documents: Vec<String> = requests
        .iter()
        .filter(|r| r.url.path().ends_with("/sendDocument"))
        .map(|r| String::from_utf8_lossy(&r.body).into_owned())
        .collect();
    assert_eq!(documents.len() as u64, parts);
    assert!(documents[0].contains(&format!("{}.part001", made.name)));
    assert!(documents[0].contains("77"));
    assert!(documents[0].contains("-100500"));
    assert!(documents[0].contains("Часть 1 из"));
    assert!(documents[0].contains("Тестовый VPN"));
    let copies = env.ops().copies().await.unwrap();
    let sent = copies[0].telegram.clone().unwrap();
    assert_eq!(sent.state, "sent");
    assert_eq!(sent.parts, Some(parts));
    let topics = env.services.alerts().topics().await.unwrap();
    assert_eq!(topics.get("Резервные копии"), Some(&77));
    env.cleanup().await;
}

#[tokio::test]
async fn неудачная_отправка_в_telegram_приходит_уведомлением_админам() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/bot{BOT}/sendDocument")))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "ok": false,
            "error_code": 400,
            "description": "Bad Request: chat not found"
        })))
        .mount(&server)
        .await;
    let Some(env) = env_with(&SecretBox::generate_key_base64(), &server.uri()).await else {
        return;
    };
    {
        let mut conn = env.store.conn().await.unwrap();
        conn.settings()
            .set_as(
                sn_domain::alerts::SETTING_KEY,
                &AlertsConfig {
                    chat_id: Some(-100_500),
                    ..AlertsConfig::default()
                },
                None,
            )
            .await
            .unwrap();
        conn.settings()
            .set_as(
                CONFIG_KEY,
                &BackupConfig {
                    telegram: TelegramTarget::Chat { chat_id: -100_777 },
                    ..BackupConfig::default()
                },
                None,
            )
            .await
            .unwrap();
    }
    assert!(env.services.alerts().collect().await.unwrap().is_empty());
    env.ops().make_now(RunKind::Manual, None).await.unwrap();
    let copies = env.ops().copies().await.unwrap();
    let failed = copies[0].telegram.clone().unwrap();
    assert_eq!(failed.state, "failed");
    assert!(failed.error.unwrap().contains("чат не найден"));
    let alerts = env.services.alerts().collect().await.unwrap();
    let backup_alerts: Vec<_> = alerts
        .iter()
        .filter(|a| a.kind == AlertKind::Backups)
        .collect();
    assert_eq!(backup_alerts.len(), 1, "{alerts:?}");
    assert!(
        backup_alerts[0]
            .html
            .contains("Копия не отправлена в Telegram")
    );
    assert!(env.services.alerts().collect().await.unwrap().is_empty());
    env.cleanup().await;
}

#[tokio::test]
async fn все_зашифрованные_столбцы_известны_восстановлению() {
    let Some(env) = env().await else { return };
    let mut conn = env.store.conn().await.unwrap();
    let columns: Vec<(String, String)> = sqlx::query_as(
        "SELECT table_name::text, column_name::text FROM information_schema.columns \
         WHERE table_schema = current_schema() AND data_type = 'bytea' AND column_name LIKE '%nonce' \
         ORDER BY 1, 2",
    )
    .fetch_all(conn.conn())
    .await
    .unwrap();
    drop(conn);
    let unknown: Vec<&(String, String)> = columns
        .iter()
        .filter(|(t, c)| !sealed::COLUMNS.contains(&(t.as_str(), c.as_str())))
        .collect();
    assert!(
        unknown.is_empty(),
        "новый зашифрованный столбец {unknown:?}: добавьте его перешифровку в backups/sealed.rs"
    );
    assert!(columns.len() >= 4, "{columns:?}");
    env.cleanup().await;
}

#[test]
fn часовой_пояс_подписывается_как_utc_со_сдвигом() {
    use chrono::FixedOffset;
    assert_eq!(utc_label(FixedOffset::east_opt(0).unwrap()), "UTC");
    assert_eq!(utc_label(FixedOffset::east_opt(3 * 3600).unwrap()), "UTC+3");
    assert_eq!(
        utc_label(FixedOffset::east_opt(5 * 3600 + 1800).unwrap()),
        "UTC+5:30"
    );
    assert_eq!(utc_label(FixedOffset::west_opt(4 * 3600).unwrap()), "UTC−4");
    assert_eq!(size_text(500), "1 КБ");
    assert_eq!(size_text(19_500_000), "18,6 МБ");
    assert_eq!(
        counts_line(&Counts {
            clients: 1284,
            subscriptions: 1,
            active_subscriptions: None,
            payments: 22
        }),
        "1\u{a0}284 клиента, 1 подписка, 22 оплаты"
    );
}

#[tokio::test]
async fn без_папки_для_копий_панель_объясняет_что_делать() {
    let Some(env) = env().await else { return };
    let mut backup_env = env.ops().env();
    backup_env.dir = None;
    env.services.set_backup_env(Some(backup_env));
    let err = env.ops().make_now(RunKind::Manual, None).await.unwrap_err();
    assert_eq!(err.code(), NO_DIR);
    let view = env.ops().view(None).await.unwrap();
    assert!(!view.storage.configured);
    assert!(
        view.storage
            .problem
            .unwrap()
            .contains("sudo stealthnet update")
    );
    assert_eq!(view.next_at, None);
    assert_eq!(env.ops().next_run_in().await.unwrap(), None);
    env.cleanup().await;
}
