use super::model::Source;
use super::runs::RunState;
use super::tests::{S1, S2, env, import, owner, scalar, squad, user, wait};
use super::*;
use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Write;

const SHOP: &[u8] = include_bytes!("fixtures/telegram_shop.sql");
const CHILL_BAK: &[u8] = include_bytes!("fixtures/chill.bak");
const CHILL_DB: &[u8] = include_bytes!("fixtures/chill.db");
const CHILL_PG: &[u8] = include_bytes!("fixtures/chill_pg.sql");
const PASSWORD: &str = "fixture-pass";

fn gz(bytes: &[u8]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
    enc.write_all(bytes).unwrap();
    enc.finish().unwrap()
}

fn legacy_of(bytes: &[u8]) -> model::Legacy {
    map_dump(&archive::open_bytes(bytes, &wanted).unwrap()).unwrap()
}

fn chill_plain() -> Vec<u8> {
    chill_file::open_sealed(CHILL_BAK, PASSWORD).unwrap()
}

fn skipped(preview: &runs::Preview, code: &str) -> u64 {
    preview
        .skipped
        .iter()
        .find(|s| s.code == code)
        .map_or(0, |s| s.count)
}

fn seed_shop(remna: &sn_remnawave::fake::FakeRemnawave) {
    remna.insert_user(user(801, "rtsAAA111", "1_3001", Some(3001), None));
    remna.insert_user(user(802, "rtsBBB222", "2_3002", Some(3002), None));
    remna.insert_user(user(804, "rtsDDD444", "4_3004", Some(3004), None));
    remna.insert_user(user(805, "moved555", "5_3005", Some(3005), None));
    remna.insert_user(user(806, "trial555", "trial_3005", Some(3005), None));
    remna.insert_user(user(899, "stranger1", "stranger", Some(9999), None));
}

fn seed_chill(remna: &sn_remnawave::fake::FakeRemnawave) {
    remna.set_squads(vec![squad(S1, "NL"), squad(S2, "DE")]);
    remna.insert_user(user(901, "chill5001", "tg_5001", Some(5001), None));
    remna.insert_user(user(902, "chill5002", "tg_5002", Some(5002), None));
    remna.insert_user(user(904, "chill5004", "tg_5004", Some(5004), None));
    remna.insert_user(user(905, "chill5005", "tg_5005", Some(5005), None));
    remna.insert_user(user(906, "chill5005b", "tg_5005_addsub", Some(5005), None));
    remna.insert_user(user(
        907,
        "chill7001",
        "tg_-7001",
        None,
        Some("mira@example.com"),
    ));
    remna.insert_user(user(999, "stranger9", "stranger", Some(9999), None));
}

#[test]
fn копия_remnawave_telegram_shop_читается_без_записи() {
    let legacy = legacy_of(SHOP);
    assert_eq!(legacy.source, Source::TelegramShop);
    assert_eq!(legacy.currency, sn_domain::money::Currency::Rub);
    assert_eq!(legacy.clients.len(), 5);
    assert_eq!(legacy.subscriptions.len(), 4);
    assert!(legacy.tariffs.is_empty());
    assert_eq!(legacy.payments.len(), 6);
    assert_eq!(legacy.referrals(), 2);
    let names: Vec<&str> = legacy
        .subscriptions
        .iter()
        .map(|s| s.old_id.as_str())
        .collect();
    assert_eq!(names, ["1_3001", "2_3002", "4_3004", "5_3005"]);
    assert_eq!(
        legacy.subscriptions[2].short_uuid.as_deref(),
        Some("rtsDDD444")
    );
    assert_eq!(
        legacy.clients[0].referral_code.as_deref(),
        Some("3001"),
        "старые ссылки ref_<Telegram ID> продолжают работать"
    );
    let gzipped = legacy_of(&gz(SHOP));
    assert_eq!(gzipped.clients.len(), 5);
}

#[tokio::test]
async fn remnawave_telegram_shop_переносится_и_повтор_не_задваивает() {
    let Some(env) = env().await else { return };
    seed_shop(&env.remna);
    let admin = owner(&env.services).await;
    let (preview, outcome) = import(&env, SHOP, Some(admin)).await;
    assert_eq!(preview.detected, Source::TelegramShop);
    assert_eq!(preview.counts.clients, 5);
    assert_eq!(preview.counts.subscriptions, 4);
    assert_eq!(preview.counts.tariffs, 0);
    assert_eq!(preview.counts.payments, 6);
    assert_eq!(preview.counts.referrals, 2);
    assert_eq!(preview.remna.found, 4);
    assert_eq!(preview.remna.missing, 0);
    assert_eq!(preview.balances.total_minor, 0);
    assert_eq!(skipped(&preview, "tariffs_env"), 4);
    assert_eq!(skipped(&preview, "payments_unpaid"), 2);
    assert_eq!(skipped(&preview, "payments_free"), 1);

    assert_eq!(outcome.created, 5);
    assert_eq!(outcome.counts.subscriptions, 4);
    assert_eq!(outcome.counts.payments, 6);
    assert_eq!(outcome.counts.referrals, 2);
    assert!(
        env.remna
            .calls()
            .iter()
            .all(|c| matches!(c.as_str(), "stream_users" | "internal_squads")),
        "в Remnawave ничего не меняем"
    );
    let s = &env.services;
    assert_eq!(
        scalar(s, "SELECT count(*) FROM subscriptions WHERE remna_user_id IN (801, 802, 804, 805) AND tariff_id IS NULL").await,
        4
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM subscriptions WHERE remna_user_id IN (806, 899)"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM subscriptions WHERE remna_user_id = 804 AND kind = 'trial'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(DISTINCT provider) FROM payments WHERE provider IN ('yookassa', 'cryptopay', 'stars', 'platega', 'tribute', 'wata')").await,
        6
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM payments WHERE currency = 'XTR' AND amount_minor = 150"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM payments WHERE currency = 'RUB' AND amount_minor = 29950"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM orders WHERE status IN ('pending', 'paid', 'review')"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM referral_codes WHERE code = '3001'").await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM clients WHERE locale = 'en'").await,
        1
    );
    assert!(scalar(s, "SELECT count(*) FROM trial_claims").await >= 4);

    let (again_preview, again) = import(&env, SHOP, Some(admin)).await;
    assert_eq!(again_preview.existing.again, 5);
    assert_eq!(again.created, 0);
    assert_eq!(again.updated, 5);
    assert_eq!(again.counts.payments, 0);
    assert_eq!(scalar(s, "SELECT count(*) FROM clients").await, 5);
    assert_eq!(scalar(s, "SELECT count(*) FROM subscriptions").await, 4);
    assert_eq!(scalar(s, "SELECT count(*) FROM payments").await, 6);
    assert_eq!(scalar(s, "SELECT count(*) FROM referral_links").await, 2);
}

#[test]
fn копия_chill_читается_из_всех_трёх_видов() {
    let plain = chill_plain();
    let from_bak = legacy_of(&plain);
    let from_sqlite = legacy_of(CHILL_DB);
    let from_pg = legacy_of(CHILL_PG);
    for legacy in [&from_bak, &from_sqlite, &from_pg] {
        assert_eq!(legacy.source, Source::Chill);
        assert_eq!(legacy.currency, sn_domain::money::Currency::Rub);
        assert_eq!(legacy.clients.len(), 6);
        assert_eq!(legacy.tariffs.len(), 3);
        assert_eq!(legacy.payments.len(), 10);
        assert_eq!(legacy.promos.len(), 1);
        assert_eq!(legacy.promo_uses.len(), 1);
        assert_eq!(legacy.referrals(), 3);
        let mut names: Vec<&str> = legacy
            .subscriptions
            .iter()
            .map(|s| s.old_id.as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "tg_-7001",
                "tg_5001",
                "tg_5002",
                "tg_5004",
                "tg_5005",
                "tg_5005_addsub"
            ]
        );
        let balances: i64 = legacy.clients.iter().map(|c| c.balance_minor).sum();
        assert_eq!(balances, 27_200);
        let web = legacy.clients.iter().find(|c| c.old_id == "-7001").unwrap();
        assert_eq!(web.telegram_id, None);
        assert_eq!(web.email.as_deref(), Some("mira@example.com"));
        let std = legacy.tariffs.iter().find(|t| t.old_id == "std").unwrap();
        assert_eq!(
            std.periods.iter().map(|p| p.days).collect::<Vec<_>>(),
            [30, 90, 360]
        );
        assert_eq!(std.periods[0].price_minor, Some(19_900));
        assert!(
            std.description
                .as_deref()
                .is_some_and(|d| d.chars().count() > 2000)
        );
    }
    let crypto = from_bak.payments.iter().find(|p| p.old_id == "5").unwrap();
    assert_eq!(crypto.amount_minor, 19_900);
    assert_eq!(crypto.provider, "cryptopay");
    let p2p = from_bak.payments.iter().find(|p| p.old_id == "3").unwrap();
    assert_eq!(p2p.provider, "manual");
}

#[tokio::test]
async fn chill_переносится_с_доп_подпиской_и_повтор_не_задваивает() {
    let Some(env) = env().await else { return };
    seed_chill(&env.remna);
    let admin = owner(&env.services).await;
    let plain = chill_plain();
    let (preview, outcome) = import(&env, &plain, Some(admin)).await;
    assert_eq!(preview.detected, Source::Chill);
    assert_eq!(preview.counts.clients, 6);
    assert_eq!(preview.counts.subscriptions, 6);
    assert_eq!(preview.counts.tariffs, 3);
    assert_eq!(preview.counts.payments, 10);
    assert_eq!(preview.counts.promo_codes, 1);
    assert_eq!(preview.counts.referrals, 3);
    assert_eq!(preview.remna.found, 6);
    assert_eq!(preview.balances.total_minor, 27_200);
    for (code, count) in [
        ("users_blocked", 1),
        ("addsub_split", 1),
        ("autopay_cards", 1),
        ("p2p_waiting", 1),
        ("plan_access", 1),
        ("payments_unpaid", 1),
        ("payments_free", 1),
        ("promo_unsupported", 3),
    ] {
        assert_eq!(skipped(&preview, code), count, "{code}");
    }

    assert_eq!(outcome.counts.clients, 6);
    assert_eq!(outcome.counts.subscriptions, 6);
    assert_eq!(outcome.counts.tariffs, 3);
    assert_eq!(outcome.counts.payments, 10);
    assert_eq!(outcome.counts.referrals, 3);
    assert_eq!(outcome.balance_total_minor, 27_200);
    let s = &env.services;
    assert_eq!(
        scalar(s, "SELECT count(*) FROM clients WHERE status = 'blocked'").await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM clients WHERE bot_unreachable").await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM client_identities WHERE kind = 'email' AND value IN ('katya@example.com', 'mira@example.com')").await,
        2
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM client_passwords").await, 2);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM subscriptions s JOIN tariffs t ON t.id = s.tariff_id WHERE t.name = 'Про' AND s.remna_user_id IN (905, 906)").await,
        2
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM subscriptions WHERE remna_user_id = 904 AND kind = 'trial'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM subscriptions WHERE remna_user_id = 907"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM tariffs WHERE on_sale").await,
        2
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
        scalar(s, "SELECT count(*) FROM payments WHERE provider IN ('yookassa', 'stars', 'manual', 'cryptopay', 'heleket', 'balance', 'platega', 'tribute')").await,
        10
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM payments WHERE status = 'refunded'").await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM orders WHERE kind = 'topup'").await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM promo_codes WHERE code = 'FREE7' AND kind = 'days'"
        )
        .await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM promo_redemptions").await, 1);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM referral_codes WHERE code = '5001'").await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM referral_links").await, 3);

    let (_, again) = import(&env, &plain, Some(admin)).await;
    assert_eq!(again.created, 0);
    assert_eq!(again.counts.payments, 0);
    assert_eq!(scalar(s, "SELECT count(*) FROM clients").await, 6);
    assert_eq!(scalar(s, "SELECT count(*) FROM subscriptions").await, 6);
    assert_eq!(scalar(s, "SELECT count(*) FROM payments").await, 10);
    assert_eq!(scalar(s, "SELECT count(*) FROM tariffs").await, 3);
    assert_eq!(scalar(s, "SELECT count(*) FROM promo_codes").await, 1);
    assert_eq!(
        scalar(s, "SELECT COALESCE(sum(amount_minor), 0)::bigint FROM client_balances WHERE currency = 'RUB'").await,
        27_200
    );
}

#[tokio::test]
async fn копия_chill_с_паролем_проходит_мастер_целиком() {
    let Some(env) = env().await else { return };
    seed_chill(&env.remna);
    let admin = owner(&env.services).await;
    let dir = std::env::temp_dir().join(format!("sn-chill-test-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("upload.upload");
    std::fs::write(&path, CHILL_BAK).unwrap();
    let ops = env.services.legacy();
    let run = ops
        .uploaded(
            Source::Chill,
            "chill-remna-bot-backup-2026-11-18-1454.bak",
            3700,
            &path,
            Some(admin),
        )
        .await
        .unwrap();
    let locked = wait(
        &env.services,
        run.id,
        &[RunState::NeedsPassword, RunState::Failed],
    )
    .await;
    assert_eq!(locked.state, RunState::NeedsPassword, "{:?}", locked.error);
    ops.password(run.id, "не тот".into()).await.unwrap();
    let wrong = wait(
        &env.services,
        run.id,
        &[RunState::NeedsPassword, RunState::Ready, RunState::Failed],
    )
    .await;
    assert_eq!(wrong.state, RunState::NeedsPassword);
    assert!(wrong.error.is_some());
    ops.password(run.id, PASSWORD.into()).await.unwrap();
    let ready = wait(&env.services, run.id, &[RunState::Ready, RunState::Failed]).await;
    assert_eq!(ready.state, RunState::Ready, "{:?}", ready.error);
    assert_eq!(ready.source, Source::Chill);
    assert_eq!(ready.preview.as_ref().map(|p| p.counts.clients), Some(6));
    ops.start(run.id, Some(admin)).await.unwrap();
    let done = wait(&env.services, run.id, &[RunState::Done, RunState::Failed]).await;
    assert_eq!(done.state, RunState::Done, "{:?}", done.error);
    assert!(!path.exists());
    assert!(
        !path.with_extension("inner").exists(),
        "расшифрованная копия удаляется после переноса"
    );
    assert_eq!(
        scalar(&env.services, "SELECT count(*) FROM subscriptions").await,
        6
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn файл_sqlite_читается_и_из_архива_и_в_режиме_wal() {
    let dir =
        std::env::temp_dir().join(format!("sn-sqlite-test-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut wal = CHILL_DB.to_vec();
    wal[18] = 2;
    wal[19] = 2;
    let path = dir.join("bot.db");
    std::fs::write(&path, &wal).unwrap();
    let (legacy, extracted) = read_file(&path, None).unwrap();
    assert!(extracted.is_none());
    assert_eq!(legacy.clients.len(), 6);
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(CHILL_DB.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, "bot-data/bot.db", CHILL_DB)
        .unwrap();
    let tarred = gz(&builder.into_inner().unwrap());
    assert_eq!(legacy_of(&tarred).subscriptions.len(), 6);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn битый_или_чужой_файл_даёт_понятную_ошибку() {
    let message =
        |bytes: &[u8]| match archive::open_bytes(bytes, &wanted).and_then(|d| map_dump(&d)) {
            Err(archive::OpenError::Unsupported(text)) => text,
            other => panic!(
                "ждали понятную ошибку, а получили {:?}",
                other.map(|l| l.source)
            ),
        };
    let mut cut_db = CHILL_DB.to_vec();
    cut_db.truncate(700);
    assert!(message(&cut_db).contains("повреждён"));
    assert!(message(b"SQLite format 3\0and nothing else").contains("SQLite"));
    let mut shop = gz(SHOP);
    shop.truncate(shop.len() / 2);
    assert!(!message(&shop).is_empty());
    assert!(message(b"PGDMP\x01\x0f\x00").contains("-Fc"));
    let mut plain = chill_plain();
    plain.truncate(plain.len() - 40);
    assert!(!message(&plain).is_empty());
    let mut sealed_tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(CHILL_BAK.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    sealed_tar
        .append_data(&mut header, "backup.sql.gz", CHILL_BAK)
        .unwrap();
    assert!(message(&sealed_tar.into_inner().unwrap()).contains(".bak"));
    let foreign = "--\n-- PostgreSQL database dump\n--\n\nCOPY public.users (id, name) FROM stdin;\n1\tАнна\n\\.\n";
    assert!(message(foreign.as_bytes()).contains("ни одного из ботов"));
    let dir = std::env::temp_dir().join(format!("sn-bak-test-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("backup.bak");
    std::fs::write(&path, CHILL_BAK).unwrap();
    assert_eq!(
        read_file(&path, None).unwrap_err(),
        archive::OpenError::NeedsPassword
    );
    assert_eq!(
        read_file(&path, Some("не тот")).unwrap_err(),
        archive::OpenError::WrongPassword
    );
    let (legacy, inner) = read_file(&path, Some(PASSWORD)).unwrap();
    assert_eq!(legacy.source, Source::Chill);
    assert!(inner.is_some_and(|p| p.exists()));
    std::fs::remove_dir_all(&dir).ok();
}
