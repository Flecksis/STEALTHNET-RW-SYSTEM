use super::archive::OpenError;
use super::model::Source;
use super::runs::{self, Outcome, Preview};
use super::*;
use crate::{EnvFallbacks, FixedClock, ServiceDeps, Services};
use chrono::{TimeZone, Utc};
use sn_domain::ids::AdminId;
use sn_domain::money::Currency;
use sn_modules::ModuleRegistry;
use sn_payments::ProviderRegistry;
use sn_remnawave::fake::{FakeRemnawave, sample_user};
use sn_remnawave::{InternalSquad, SquadInfo};
use sn_store::SecretBox;
use sn_store::testing::{TestDb, test_store};
use std::io::Write;
use std::sync::Arc;

const REMNASHOP: &[u8] = include_bytes!("testdata/remnashop_db_backup_2026-11-18_09-30-00.sql");
const REMNASHOP_OLD: &[u8] = include_bytes!("testdata/remnashop_old_schema.sql.gz");
const MINISHOP: &[u8] = include_bytes!("testdata/minishop-20261118-09-40.zip");
const S1: &str = "11111111-1111-4111-8111-111111111111";
const S2: &str = "22222222-2222-4222-8222-222222222222";

struct Env {
    _db: TestDb,
    services: Services,
    remna: Arc<FakeRemnawave>,
}

async fn env() -> Option<Env> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(
        Utc.with_ymd_and_hms(2026, 11, 18, 10, 0, 0).unwrap(),
    ));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock,
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    remna.set_squads(vec![squad(S1, "NL"), squad(S2, "DE")]);
    services.set_remna(Some(remna.clone()));
    Some(Env {
        _db: db,
        services,
        remna,
    })
}

fn squad(uuid: &str, name: &str) -> InternalSquad {
    InternalSquad {
        uuid: uuid.parse().unwrap(),
        name: name.into(),
        view_position: 0,
        info: SquadInfo::default(),
        inbounds: Vec::new(),
    }
}

fn user(
    id: i64,
    short: &str,
    name: &str,
    tg: Option<i64>,
    email: Option<&str>,
) -> sn_remnawave::RemnaUser {
    let mut user = sample_user(id, name);
    user.short_uuid = short.into();
    user.telegram_id = tg;
    user.email = email.map(Into::into);
    user.hwid_device_limit = Some(3);
    user
}

async fn scalar(services: &Services, sql: &str) -> i64 {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_scalar(&format!("SELECT ({sql})::bigint"))
        .fetch_one(sn_store::Db::conn(&mut conn))
        .await
        .unwrap()
}

async fn owner(services: &Services) -> AdminId {
    services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap()
        .0
}

async fn check(env: &Env, legacy: &model::Legacy) -> check::Checked {
    let remna = check::fetch_remna(&env.services).await;
    check::preview(&env.services, legacy, remna).await.unwrap()
}

async fn apply(env: &Env, legacy: &model::Legacy) -> (Preview, Outcome) {
    let checked = check(env, legacy).await;
    let mut conn = env.services.store().conn().await.unwrap();
    let (run_id, key): (i64, uuid::Uuid) = sqlx::query_as(
        "INSERT INTO import_runs (source, state, file_name, file_size, started_at) VALUES ($1, 'running', 'test', 1, now()) \
         RETURNING id, public_id",
    )
    .bind(legacy.source.as_str())
    .fetch_one(sn_store::Db::conn(&mut conn))
    .await
    .unwrap();
    runs::add_issues(&mut conn, run_id, &checked.issues)
        .await
        .unwrap();
    drop(conn);
    let admin = Some(owner(&env.services).await);
    let outcome = apply_legacy(&env.services, legacy, run_id, key, admin, None)
        .await
        .unwrap();
    env.services.set_remna(Some(env.remna.clone()));
    let mut conn = env.services.store().conn().await.unwrap();
    runs::finish(&mut conn, run_id, &outcome).await.unwrap();
    (checked.preview, outcome)
}

fn read(bytes: &[u8]) -> model::Legacy {
    map_dump(&archive::open_bytes(bytes, &wanted).unwrap()).unwrap()
}

fn skipped(preview: &Preview) -> Vec<&str> {
    preview.skipped.iter().map(|s| s.code.as_str()).collect()
}

fn calm(remna: &FakeRemnawave) {
    let calls = remna.calls();
    assert!(
        calls
            .iter()
            .all(|c| matches!(c.as_str(), "stream_users" | "internal_squads")),
        "в Remnawave ничего не меняем: {calls:?}"
    );
}

fn seed_remnashop(remna: &FakeRemnawave) {
    remna.insert_user(user(201, "pPetr7xQ", "rs_2001", Some(2001), None));
    remna.insert_user(user(202, "oLga0001", "rs_2002", Some(2002), None));
    remna.insert_user(user(203, "nEwLink3", "rs_web_3", None, None));
    remna.insert_user(user(299, "zzzz0299", "rs_9999", Some(9999), None));
}

#[tokio::test]
async fn remnashop_проверка_без_записи_сходится_с_копией() {
    let Some(env) = env().await else { return };
    seed_remnashop(&env.remna);
    let legacy = read(REMNASHOP);
    assert_eq!(legacy.source, Source::Remnashop);
    assert_eq!(legacy.currency, Currency::Rub);
    let checked = check(&env, &legacy).await;
    let preview = &checked.preview;
    assert_eq!(preview.detected, Source::Remnashop);
    assert_eq!(preview.counts.clients, 4);
    assert_eq!(preview.counts.subscriptions, 3);
    assert_eq!(preview.counts.tariffs, 3);
    assert_eq!(preview.counts.promo_codes, 4);
    assert_eq!(preview.counts.referrals, 1);
    assert_eq!(preview.counts.payments, 5);
    assert_eq!(preview.counts.tickets, 0);
    assert_eq!(preview.remna.found, 3);
    assert_eq!(preview.remna.missing, 0);
    let codes = skipped(preview);
    for code in [
        "referral_points",
        "personal_discount",
        "passwords",
        "oauth_other",
        "subscriptions_history",
        "payments_unpaid",
        "payments_test",
        "payments_free",
        "promo_unsupported",
        "payment_keys",
        "referral_settings",
    ] {
        assert!(codes.contains(&code), "нет {code} в {codes:?}");
    }
    let keys = preview
        .skipped
        .iter()
        .find(|s| s.code == "payment_keys")
        .unwrap();
    assert_eq!(keys.count, 3);
    assert!(keys.text.contains("ЮKassa"), "{}", keys.text);
    assert!(
        checked
            .issues
            .iter()
            .any(|i| i.details.contains("Баллов реферальной программы: 120"))
    );
    assert!(
        checked
            .issues
            .iter()
            .any(|i| i.code == "tariff_note" && i.details.contains("бессрочного срока"))
    );
    assert_eq!(
        scalar(&env.services, "SELECT count(*) FROM clients").await,
        0
    );
    assert_eq!(
        scalar(&env.services, "SELECT count(*) FROM tariffs").await,
        0
    );
    calm(&env.remna);
}

#[tokio::test]
async fn remnashop_переносится_и_повтор_не_даёт_дублей() {
    let Some(env) = env().await else { return };
    seed_remnashop(&env.remna);
    let legacy = read(REMNASHOP);
    let (_, outcome) = apply(&env, &legacy).await;
    assert_eq!(outcome.counts.clients, 4);
    assert_eq!(outcome.counts.subscriptions, 3);
    assert_eq!(outcome.counts.tariffs, 3);
    assert_eq!(outcome.counts.payments, 5);
    assert_eq!(outcome.counts.promo_codes, 4);
    assert_eq!(outcome.counts.referrals, 1);
    let s = &env.services;
    assert_eq!(s.settings().currency, Currency::Rub);
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM subscriptions WHERE remna_user_id IN (201, 202, 203)"
        )
        .await,
        3
    );
    assert_eq!(
        scalar(
            s,
            "SELECT extra_devices FROM subscriptions WHERE remna_user_id = 201"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM tariff_periods WHERE price_rub_minor = 19900 AND days = 30"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM tariffs WHERE name IN ('Пробный', 'Безлимит') AND NOT on_sale"
        )
        .await,
        2
    );
    assert_eq!(
        scalar(
            s,
            "SELECT devices_included FROM tariffs WHERE name = 'Безлимит'"
        )
        .await,
        100
    );
    for (provider, count) in [
        ("yookassa", 1),
        ("stars", 1),
        ("cryptopay", 1),
        ("heleket", 1),
        ("platega", 1),
    ] {
        assert_eq!(
            scalar(
                s,
                &format!("SELECT count(*) FROM payments WHERE provider = '{provider}'")
            )
            .await,
            count,
            "{provider}"
        );
    }
    assert_eq!(
        scalar(s, "SELECT count(*) FROM payments WHERE status = 'refunded'").await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM orders WHERE intent->>'label' = 'Тариф «Старый тариф», 90 дн.'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM promo_codes WHERE code = 'PLAN90' AND kind = 'days' AND days = 90"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM promo_codes WHERE code = 'SALE20' AND kind = 'percent' AND first_purchase_only").await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM promo_redemptions").await, 2);
    assert_eq!(scalar(s, "SELECT count(*) FROM referral_links").await, 1);
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM referral_codes WHERE code = 'AB3DE9'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM client_identities WHERE kind = 'google' AND value = 'google-1093'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM client_identities WHERE kind = 'email' AND value = 'maria@example.com'").await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM client_passwords").await, 0);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM clients WHERE status = 'blocked'").await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM clients WHERE bot_unreachable").await,
        1
    );
    calm(&env.remna);

    let (preview, again) = apply(&env, &read(REMNASHOP)).await;
    assert_eq!(preview.existing.again, 4);
    assert_eq!(again.created, 0);
    for (table, count) in [
        ("clients", 4),
        ("subscriptions", 3),
        ("tariffs", 3),
        ("payments", 5),
        ("orders", 5),
        ("promo_codes", 4),
        ("promo_redemptions", 2),
        ("referral_links", 1),
    ] {
        assert_eq!(
            scalar(s, &format!("SELECT count(*) FROM {table}")).await,
            count,
            "{table}"
        );
    }
}

#[tokio::test]
async fn старая_схема_remnashop_связана_через_telegram() {
    let Some(env) = env().await else { return };
    env.remna
        .insert_user(user(301, "aNnaOld1", "rs_3001", Some(3001), None));
    env.remna
        .insert_user(user(302, "rotated2", "rs_3002", Some(3002), None));
    let legacy = read(REMNASHOP_OLD);
    assert_eq!(legacy.source, Source::Remnashop);
    assert_eq!(legacy.referrals(), 1);
    assert!(legacy.promos.is_empty());
    let (preview, outcome) = apply(&env, &legacy).await;
    assert_eq!(preview.counts.clients, 2);
    assert_eq!(preview.counts.subscriptions, 2);
    assert_eq!(preview.counts.payments, 2);
    assert_eq!(preview.remna.found, 2);
    assert!(skipped(&preview).contains(&"referral_settings"));
    assert_eq!(outcome.counts.referrals, 1);
    let s = &env.services;
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM payments WHERE provider = 'yoomoney'"
        )
        .await,
        2
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM subscriptions WHERE remna_user_id IN (301, 302) AND tariff_id IS NOT NULL").await,
        2
    );
    calm(&env.remna);
}

fn seed_minishop(remna: &FakeRemnawave) {
    remna.insert_user(user(101, "lEnaShrt", "ms_fixture4001", Some(4001), None));
    remna.insert_user(user(102, "maxx0102", "tg_4002", Some(4002), None));
    remna.insert_user(user(
        103,
        "webu0103",
        "ms_fixtureweb",
        None,
        Some("web.user@example.com"),
    ));
}

fn minishop_file(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("minishop-20261118-09-40.zip");
    std::fs::write(&path, MINISHOP).unwrap();
    path
}

#[tokio::test]
async fn minishop_из_своей_копии_переносится_и_повтор_не_даёт_дублей() {
    let Some(env) = env().await else { return };
    seed_minishop(&env.remna);
    let dir = tempdir();
    let path = minishop_file(&dir);
    let (legacy, extracted) = read_file(&path, None).unwrap();
    assert!(
        extracted.is_none(),
        "копию бота не распаковываем по одному файлу"
    );
    assert!(path.exists());
    assert_eq!(legacy.source, Source::Minishop);
    assert_eq!(legacy.currency, Currency::Rub);
    assert_eq!(
        legacy.dumped_at,
        Some(Utc.with_ymd_and_hms(2026, 11, 18, 9, 40, 0).unwrap())
    );
    let checked = check(&env, &legacy).await;
    let preview = &checked.preview;
    assert_eq!(preview.detected, Source::Minishop);
    assert_eq!(preview.counts.clients, 4);
    assert_eq!(preview.counts.subscriptions, 3);
    assert_eq!(preview.counts.tariffs, 2);
    assert_eq!(preview.counts.payments, 6);
    assert_eq!(preview.counts.promo_codes, 3);
    assert_eq!(preview.counts.referrals, 1);
    assert_eq!(preview.counts.tickets, 1);
    assert_eq!(preview.remna.found, 3);
    assert_eq!(preview.balances.total_minor, 20_000);
    let codes = skipped(preview);
    for code in [
        "tariffs_missing",
        "autorenew_cards",
        "partner_balance",
        "withdrawal_pending",
        "balance_currency",
        "passwords",
        "subscriptions_history",
        "payments_unpaid",
        "payments_free",
        "promo_unsupported",
    ] {
        assert!(codes.contains(&code), "нет {code} в {codes:?}");
    }
    assert!(
        checked
            .issues
            .iter()
            .any(|i| i.details.contains("На партнёрском балансе 500.00"))
    );
    assert_eq!(
        scalar(&env.services, "SELECT count(*) FROM clients").await,
        0
    );

    let (_, outcome) = apply(&env, &legacy).await;
    assert_eq!(outcome.counts.clients, 4);
    assert_eq!(outcome.counts.subscriptions, 3);
    assert_eq!(outcome.counts.payments, 6);
    assert_eq!(outcome.counts.tickets, 1);
    assert_eq!(outcome.balance_total_minor, 20_000);
    let s = &env.services;
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM subscriptions WHERE remna_user_id IN (101, 102, 103)"
        )
        .await,
        3
    );
    assert_eq!(
        scalar(
            s,
            "SELECT extra_devices FROM subscriptions WHERE remna_user_id = 101"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM subscriptions s JOIN tariffs t ON t.id = s.tariff_id WHERE t.name = 'Стандарт'").await,
        2
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM subscriptions WHERE remna_user_id = 102 AND kind = 'trial'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM tariff_periods p JOIN tariffs t ON t.id = p.tariff_id WHERE t.name = 'Стандарт'").await,
        2
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM tariff_periods WHERE days = 90 AND price_rub_minor = 54000"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM tariffs WHERE name = 'Гигабайты' AND NOT on_sale"
        )
        .await,
        1
    );
    for (provider, count) in [
        ("yookassa", 2),
        ("stars", 1),
        ("balance", 1),
        ("cryptopay", 1),
        ("platega", 1),
    ] {
        assert_eq!(
            scalar(
                s,
                &format!("SELECT count(*) FROM payments WHERE provider = '{provider}'")
            )
            .await,
            count,
            "{provider}"
        );
    }
    assert_eq!(
        scalar(s, "SELECT count(*) FROM orders WHERE kind = 'topup'").await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT COALESCE(sum(amount_minor), 0)::bigint FROM client_balances WHERE currency = 'RUB' AND wallet = 'main'").await,
        20_000
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM client_balances WHERE currency = 'USD' AND amount_minor > 0"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM referral_codes WHERE code = 'VOVA4004' AND percent = 25"
        )
        .await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM referral_links").await, 1);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM promo_codes WHERE code = 'DAYS14' AND kind = 'days' AND first_purchase_only").await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM promo_codes WHERE code = 'OLDCODE'").await,
        0
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM ticket_messages").await, 2);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM clients WHERE status = 'blocked'").await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM clients WHERE bot_unreachable").await,
        1
    );
    calm(&env.remna);

    let (preview, again) = apply(&env, &read_file(&path, None).unwrap().0).await;
    assert_eq!(preview.existing.again, 4);
    assert_eq!(again.created, 0);
    for (table, count) in [
        ("clients", 4),
        ("subscriptions", 3),
        ("tariffs", 2),
        ("payments", 6),
        ("promo_codes", 3),
        ("tickets", 1),
        ("ticket_messages", 2),
        ("referral_links", 1),
    ] {
        assert_eq!(
            scalar(s, &format!("SELECT count(*) FROM {table}")).await,
            count,
            "{table}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn тарифы_minishop_в_месяцах_переводятся_в_дни() {
    let mut dump = table::Dump::default();
    dump.tables.insert(
        "users".into(),
        table::Table::new(vec!["user_id".into(), "panel_user_uuid".into()]),
    );
    dump.files.insert(
        minishop::TARIFFS_FILE.into(),
        r#"{"default_currency": "usd", "tariffs": [{"key": "m", "names": {"en": "Monthly"},
            "billing_model": "period", "monthly_gb": 0, "hwid_device_limit": 2,
            "prices": {"usd": {"1": 3, "3": 8, "12": 30}, "stars": {"1": 0}}}]}"#
            .into(),
    );
    let legacy = minishop::map(&dump);
    assert_eq!(legacy.source, Source::Minishop);
    assert_eq!(legacy.currency, Currency::Usd);
    let tariff = &legacy.tariffs[0];
    assert_eq!(tariff.name, "Monthly");
    assert_eq!(tariff.devices_included, 2);
    assert_eq!(tariff.traffic_limit_bytes, None);
    let periods: Vec<(u32, Option<i64>)> = tariff
        .periods
        .iter()
        .map(|p| (p.days, p.price_minor))
        .collect();
    assert_eq!(
        periods,
        [(30, Some(300)), (90, Some(800)), (365, Some(3000))]
    );
    assert!(tariff.note.is_none(), "{:?}", tariff.note);
    assert!(
        legacy
            .skipped
            .iter()
            .all(|s| !s.code.starts_with("tariffs_"))
    );
}

#[test]
fn старый_minishop_хранит_telegram_в_номере_клиента() {
    let sql = "--\n-- PostgreSQL database dump\n--\n\n\
COPY public.users (user_id, username, language_code, panel_user_uuid, referred_by_id) FROM stdin;\n\
7001\tolder\ten\t2b8f3c1e-0000-4000-8000-000000000001\t\\N\n\
7002\t\\N\tru\t\\N\t7001\n\
\\.\n";
    let legacy = read(sql.as_bytes());
    assert_eq!(legacy.source, Source::Minishop);
    let ids: Vec<Option<i64>> = legacy.clients.iter().map(|c| c.telegram_id).collect();
    assert_eq!(ids, [Some(7001), Some(7002)]);
    assert_eq!(legacy.clients[0].locale, sn_domain::client::Locale::En);
    assert_eq!(legacy.referrals(), 1);
}

#[test]
fn копия_minishop_без_файла_тарифов_просит_завести_тарифы() {
    let sql = "--\n-- PostgreSQL database dump\n--\n\n\
COPY public.users (user_id, username, telegram_id, language_code, panel_user_uuid, telegram_notifications_status) FROM stdin;\n\
5001\tnina\t5001\tru\t555\tenabled\n\
\\.\n\
COPY public.subscriptions (subscription_id, user_id, panel_user_uuid, end_date, is_active, provider, tariff_key, extra_hwid_devices) FROM stdin;\n\
1\t5001\t555\t2026-12-01 00:00:00+00\tt\tyookassa\tstandard\t0\n\
\\.\n\
COPY public.transactions (id) FROM stdin;\n\
\\.\n";
    let legacy = read(sql.as_bytes());
    assert_eq!(legacy.source, Source::Minishop);
    assert!(legacy.tariffs.is_empty());
    assert_eq!(legacy.subscriptions[0].remna_id, Some(555));
    assert_eq!(legacy.subscriptions[0].tariff_old_id, None);
    let note = legacy
        .skipped
        .iter()
        .find(|s| s.code == "tariffs_file")
        .unwrap();
    assert_eq!(note.count, 1);
    assert!(note.text.contains("standard") && note.text.contains("tariffs.json"));
}

fn tempdir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sn-shop-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in files {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn open_path(bytes: &[u8]) -> Result<model::Legacy, OpenError> {
    let dir = tempdir();
    let path = dir.join("upload.bin");
    std::fs::write(&path, bytes).unwrap();
    let result = read_file(&path, None).map(|(legacy, _)| legacy);
    let _ = std::fs::remove_dir_all(dir);
    result
}

fn unsupported(result: Result<model::Legacy, OpenError>) -> String {
    match result {
        Err(OpenError::Unsupported(text)) => text,
        Err(other) => panic!("ждали понятную ошибку, а пришла {other:?}"),
        Ok(legacy) => panic!(
            "файл не должен читаться, а распознан как {:?}",
            legacy.source
        ),
    }
}

#[test]
fn битый_или_чужой_файл_даёт_понятную_ошибку() {
    let cut = &MINISHOP[..MINISHOP.len() / 2];
    assert!(unsupported(open_path(cut)).contains("повреждён"));

    let mut inner = zip::ZipArchive::new(std::io::Cursor::new(MINISHOP)).unwrap();
    let mut dump = Vec::new();
    std::io::copy(
        &mut inner.by_name("database/minishop.dump").unwrap(),
        &mut dump,
    )
    .unwrap();
    let half = &dump[..dump.len() * 3 / 4];
    assert!(unsupported(open_path(half)).contains("не до конца"));

    let parts = br#"{
  "format": "remnawave-minishop-backup-parts",
  "version": 1,
  "archive_name": "minishop-20261118-09-40.zip",
  "parts": []
}"#;
    assert!(unsupported(open_path(parts)).contains("список частей"));

    let manifest = br#"{"app": "remnawave-minishop", "format_version": 1}"#;
    let no_database = zip_of(&[
        ("manifest.json", manifest),
        ("database/tariffs.json", br#"{"tariffs": []}"#),
    ]);
    assert!(unsupported(open_path(&no_database)).contains("нет выгрузки базы"));

    let foreign = "--\n-- PostgreSQL database dump\n--\n\nCOPY public.users (id, nickname) FROM stdin;\n1\tкто-то\n\\.\n";
    assert!(unsupported(open_path(foreign.as_bytes())).contains("ни одного из ботов"));

    assert!(unsupported(open_path(b"\x89PNG\r\n\x1a\npicture")).contains("Это не выгрузка базы"));
}
