use super::model::Source;
use super::runs::{self, Outcome, Preview, RunState};
use super::*;
use crate::{EnvFallbacks, FixedClock, ServiceDeps, Services};
use chrono::{TimeZone, Utc};
use flate2::Compression;
use flate2::write::GzEncoder;
use sn_domain::client::{RegistrationSource, TelegramProfile};
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
        Utc.with_ymd_and_hms(2026, 11, 17, 10, 0, 0).unwrap(),
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
    remna.set_squads(vec![squad(S1, "NL")]);
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
    user.traffic_limit_bytes = 50 * 1024 * 1024 * 1024;
    user.hwid_device_limit = Some(3);
    user
}

fn copy(table: &str, columns: &[&str], rows: &[&[Option<&str>]]) -> String {
    let mut out = format!("COPY public.{table} ({}) FROM stdin;\n", columns.join(", "));
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .map(|cell| match cell {
                None => "\\N".to_string(),
                Some(v) => v
                    .replace('\\', "\\\\")
                    .replace('\t', "\\t")
                    .replace('\n', "\\n"),
            })
            .collect();
        out.push_str(&cells.join("\t"));
        out.push('\n');
    }
    out.push_str("\\.\n\n");
    out
}

fn gz(bytes: &[u8]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
    enc.write_all(bytes).unwrap();
    enc.finish().unwrap()
}

fn stealthnet_dump(fresh: bool) -> Vec<u8> {
    let mut sql = String::from(
        "--\n-- PostgreSQL database dump\n--\n-- StealthNet v7.0.1 Database Dump\n-- Created at: 2026-11-17T09:12:44.000Z\n\nSET statement_timeout = 0;\n\n",
    );
    sql += &copy(
        "system_settings",
        &["id", "key", "value"],
        &[&[Some("ss1"), Some("default_currency"), Some("rub")]],
    );
    sql += &copy(
        "tariff_categories",
        &[
            "id",
            "name",
            "emoji_key",
            "sort_order",
            "single_subscription_mode",
        ],
        &[
            &[Some("cat1"), Some("Обычные"), None, Some("0"), Some("f")],
            &[Some("cat2"), Some("Широкие"), None, Some("1"), Some("f")],
        ],
    );
    let squads1 = format!("{{{S1}}}");
    let squads2 = format!("{{{S1},{S2}}}");
    sql += &copy(
        "tariffs",
        &[
            "id",
            "category_id",
            "name",
            "description",
            "duration_days",
            "internal_squad_uuids",
            "traffic_limit_bytes",
            "traffic_reset_mode",
            "device_limit",
            "included_devices",
            "price_per_extra_device",
            "max_extra_devices",
            "price",
            "currency",
            "sort_order",
        ],
        &[
            &[
                Some("t1"),
                Some("cat1"),
                Some("Базовый"),
                Some("Для одного"),
                Some("30"),
                Some(&squads1),
                Some("107374182400"),
                Some("monthly"),
                None,
                Some("1"),
                Some("50"),
                Some("2"),
                Some("199"),
                Some("rub"),
                Some("0"),
            ],
            &[
                Some("t2"),
                Some("cat2"),
                Some("Широкий"),
                None,
                Some("30"),
                Some(&squads2),
                None,
                Some("carry_over"),
                None,
                Some("3"),
                Some("0"),
                Some("0"),
                Some("4.99"),
                Some("usd"),
                Some("1"),
            ],
        ],
    );
    sql += &copy(
        "tariff_price_options",
        &["id", "tariff_id", "duration_days", "price", "sort_order"],
        &[
            &[Some("o1"), Some("t1"), Some("30"), Some("199"), Some("0")],
            &[Some("o2"), Some("t1"), Some("90"), Some("499"), Some("1")],
            &[Some("o3"), Some("t2"), Some("30"), Some("4.99"), Some("0")],
        ],
    );
    let anna_balance = if fresh { "170.5" } else { "150.5" };
    let boris_balance = if fresh { "5" } else { "20" };
    let mut clients: Vec<[Option<&str>; 16]> = vec![
        [
            Some("c1"),
            Some("anna@example.com"),
            Some("$2b$12$abcdefghijklmnopqrstuuOdGq1xQmB0PZ5d4fN4m0wE1gVyZcH3W"),
            Some("CLIENT"),
            Some("REF-1A2B3C4D"),
            None,
            Some(anna_balance),
            Some("ru"),
            Some("1001"),
            Some("anna"),
            Some("f"),
            None,
            Some("f"),
            Some("t"),
            Some("tiktok"),
            Some("2026-01-10 08:00:00"),
        ],
        [
            Some("c2"),
            None,
            None,
            Some("CLIENT"),
            None,
            Some("c1"),
            Some("0"),
            Some("en"),
            Some("1002"),
            None,
            Some("t"),
            Some("спам"),
            Some("f"),
            Some("f"),
            None,
            Some("2026-02-11 09:00:00"),
        ],
        [
            Some("c3"),
            Some("Boris@Example.com"),
            None,
            Some("CLIENT"),
            None,
            None,
            Some(boris_balance),
            Some("ru"),
            None,
            None,
            Some("f"),
            None,
            Some("t"),
            Some("f"),
            None,
            Some("2026-03-12 10:00:00"),
        ],
        [
            Some("c4"),
            None,
            None,
            Some("CLIENT"),
            None,
            None,
            Some("300"),
            Some("ru"),
            Some("1004"),
            Some("vera"),
            Some("f"),
            None,
            Some("f"),
            Some("f"),
            None,
            Some("2026-04-13 11:00:00"),
        ],
    ];
    if fresh {
        clients.push([
            Some("c6"),
            None,
            None,
            Some("CLIENT"),
            None,
            Some("c1"),
            Some("10"),
            Some("ru"),
            Some("1006"),
            Some("gleb"),
            Some("f"),
            None,
            Some("f"),
            Some("f"),
            None,
            Some("2026-11-17 08:00:00"),
        ]);
    }
    let client_rows: Vec<&[Option<&str>]> = clients.iter().map(|c| c.as_slice()).collect();
    sql += &copy(
        "clients",
        &[
            "id",
            "email",
            "password_hash",
            "role",
            "referral_code",
            "referrer_id",
            "balance",
            "preferred_lang",
            "telegram_id",
            "telegram_username",
            "is_blocked",
            "block_reason",
            "telegram_unreachable",
            "trial_used",
            "utm_source",
            "created_at",
        ],
        &client_rows,
    );
    sql += &copy(
        "subscriptions",
        &[
            "id",
            "owner_id",
            "remnawave_uuid",
            "subscription_index",
            "tariff_id",
            "gift_status",
            "trial_id",
            "auto_renew_enabled",
            "auto_renew_price_option_id",
            "extra_devices",
            "expire_at",
            "short_uuid",
            "promo_granted",
            "purchased_as_gift",
            "custom_price",
            "created_at",
        ],
        &[
            &[
                Some("s1"),
                Some("c1"),
                Some("501"),
                Some("0"),
                Some("t1"),
                None,
                None,
                Some("t"),
                Some("o2"),
                Some("1"),
                Some("2026-12-20 00:00:00"),
                None,
                Some("f"),
                Some("f"),
                None,
                Some("2026-01-10 08:05:00"),
            ],
            &[
                Some("s2"),
                Some("c1"),
                Some("8f2c1d9e-1111-4a4a-9b9b-123456789abc"),
                Some("1"),
                Some("t1"),
                None,
                None,
                Some("f"),
                None,
                Some("0"),
                Some("2026-12-21 00:00:00"),
                Some("shortc1b"),
                Some("f"),
                Some("f"),
                None,
                Some("2026-05-10 08:05:00"),
            ],
            &[
                Some("s3"),
                Some("c3"),
                Some("0a0a0a0a-2222-4b4b-8c8c-abcdefabcdef"),
                Some("0"),
                Some("t1"),
                None,
                None,
                Some("f"),
                None,
                Some("0"),
                Some("2026-12-22 00:00:00"),
                None,
                Some("f"),
                Some("f"),
                None,
                Some("2026-03-12 10:05:00"),
            ],
            &[
                Some("s4"),
                Some("c2"),
                Some("999"),
                Some("0"),
                Some("t1"),
                None,
                None,
                Some("f"),
                None,
                Some("0"),
                Some("2026-02-01 00:00:00"),
                None,
                Some("f"),
                Some("f"),
                None,
                Some("2026-02-11 09:05:00"),
            ],
            &[
                Some("s5"),
                Some("c4"),
                None,
                Some("0"),
                Some("t1"),
                Some("GIFT_RESERVED"),
                None,
                Some("f"),
                None,
                Some("0"),
                None,
                None,
                Some("f"),
                Some("t"),
                None,
                Some("2026-04-13 11:05:00"),
            ],
        ],
    );
    let mut payments: Vec<[Option<&str>; 13]> = vec![
        [
            Some("p1"),
            Some("c1"),
            Some("ord-1"),
            Some("199"),
            Some("RUB"),
            Some("PAID"),
            Some("yookassa"),
            Some("yk-1"),
            Some("t1"),
            Some("o1"),
            Some("s1"),
            None,
            Some("2026-10-01 10:00:00"),
        ],
        [
            Some("p2"),
            Some("c1"),
            Some("ord-2"),
            Some("100"),
            Some("rub"),
            Some("PAID"),
            Some("cryptopay"),
            Some("cp-2"),
            None,
            None,
            None,
            None,
            Some("2026-10-02 10:00:00"),
        ],
        [
            Some("p3"),
            Some("c1"),
            Some("ord-3"),
            Some("199"),
            Some("rub"),
            Some("PENDING"),
            Some("yookassa"),
            None,
            Some("t1"),
            Some("o1"),
            None,
            None,
            None,
        ],
        [
            Some("p4"),
            Some("c4"),
            Some("ord-4"),
            Some("5"),
            Some("usd"),
            Some("PAID"),
            Some("cryptopay"),
            None,
            None,
            None,
            None,
            None,
            Some("2026-10-03 10:00:00"),
        ],
        [
            Some("p5"),
            Some("c4"),
            Some("ord-5"),
            Some("300"),
            Some("rub"),
            Some("PAID"),
            Some("yookassa"),
            None,
            None,
            None,
            None,
            None,
            Some("2026-10-04 10:00:00"),
        ],
        [
            Some("p6"),
            Some("c2"),
            Some("ord-6"),
            Some("199"),
            Some("rub"),
            Some("REFUNDED"),
            Some("yookassa"),
            None,
            Some("t1"),
            Some("o1"),
            Some("s4"),
            None,
            Some("2026-02-11 09:10:00"),
        ],
        [
            Some("p7"),
            Some("c3"),
            Some("ord-7"),
            Some("199"),
            Some("RUB"),
            Some("PAID"),
            Some("telegram_stars"),
            None,
            Some("t1"),
            Some("o1"),
            Some("s3"),
            Some("{\"stars\":{\"total\":150}}"),
            Some("2026-03-12 10:10:00"),
        ],
        [
            Some("p8"),
            Some("c1"),
            Some("ord-8"),
            Some("0"),
            Some("rub"),
            Some("PAID"),
            Some("admin_grant"),
            None,
            Some("t1"),
            None,
            Some("s2"),
            None,
            Some("2026-05-10 08:05:00"),
        ],
    ];
    if fresh {
        payments.push([
            Some("p9"),
            Some("c6"),
            Some("ord-9"),
            Some("10"),
            Some("rub"),
            Some("PAID"),
            Some("yookassa"),
            None,
            None,
            None,
            None,
            None,
            Some("2026-11-17 08:30:00"),
        ]);
    }
    let payment_rows: Vec<&[Option<&str>]> = payments.iter().map(|p| p.as_slice()).collect();
    sql += &copy(
        "payments",
        &[
            "id",
            "client_id",
            "order_id",
            "amount",
            "currency",
            "status",
            "provider",
            "external_id",
            "tariff_id",
            "tariff_price_option_id",
            "subscription_id",
            "metadata",
            "paid_at",
        ],
        &payment_rows,
    );
    sql += &copy(
        "promo_codes",
        &[
            "id",
            "code",
            "name",
            "type",
            "discount_percent",
            "discount_fixed",
            "tariff_id",
            "duration_days",
            "max_uses",
            "max_uses_per_client",
            "is_active",
            "expires_at",
        ],
        &[
            &[
                Some("pr1"),
                Some("welcome10"),
                Some("Скидка новичкам"),
                Some("DISCOUNT"),
                Some("10"),
                None,
                None,
                None,
                Some("100"),
                Some("1"),
                Some("t"),
                None,
            ],
            &[
                Some("pr2"),
                Some("BAD CODE!"),
                Some("Сломанный"),
                Some("DISCOUNT"),
                Some("5"),
                None,
                None,
                None,
                Some("0"),
                Some("1"),
                Some("t"),
                None,
            ],
            &[
                Some("pr3"),
                Some("DAYS7"),
                Some("Неделя"),
                Some("FREE_DAYS"),
                None,
                None,
                Some("t1"),
                Some("7"),
                Some("0"),
                Some("1"),
                Some("t"),
                Some("2027-01-01 00:00:00"),
            ],
        ],
    );
    sql += &copy(
        "promo_code_usages",
        &["id", "promo_code_id", "client_id", "created_at"],
        &[&[
            Some("u1"),
            Some("pr1"),
            Some("c2"),
            Some("2026-02-11 09:09:00"),
        ]],
    );
    sql += &copy(
        "tickets",
        &[
            "id",
            "client_id",
            "subject",
            "status",
            "created_at",
            "closed_at",
        ],
        &[&[
            Some("tk1"),
            Some("c1"),
            Some("Не работает"),
            Some("open"),
            Some("2026-11-01 10:00:00"),
            None,
        ]],
    );
    sql += &copy(
        "ticket_messages",
        &[
            "id",
            "ticket_id",
            "author_type",
            "content",
            "attachments",
            "is_read",
            "created_at",
        ],
        &[
            &[
                Some("m1"),
                Some("tk1"),
                Some("client"),
                Some("Привет\nне работает"),
                None,
                Some("t"),
                Some("2026-11-01 10:00:00"),
            ],
            &[
                Some("m2"),
                Some("tk1"),
                Some("support"),
                Some("Проверьте приложение"),
                None,
                Some("t"),
                Some("2026-11-01 11:00:00"),
            ],
            &[
                Some("m3"),
                Some("tk1"),
                Some("client"),
                Some("Всё ещё"),
                Some("[{\"url\":\"/uploads/x.png\"}]"),
                Some("f"),
                Some("2026-11-01 12:00:00"),
            ],
        ],
    );
    sql += &copy(
        "withdrawal_requests",
        &["id", "client_id", "amount", "status"],
        &[&[Some("w1"), Some("c1"), Some("50"), Some("PENDING")]],
    );
    sql += &copy(
        "proxy_slots",
        &["id", "client_id"],
        &[&[Some("x1"), Some("c1")], &[Some("x2"), Some("c4")]],
    );
    gz(sql.as_bytes())
}

fn seed_stealthnet_remna(remna: &FakeRemnawave) {
    remna.insert_user(user(501, "aaaa501", "anna", Some(1001), None));
    remna.insert_user(user(502, "shortc1b", "anna_2", Some(1001), None));
    remna.insert_user(user(
        503,
        "bbbb503",
        "boris",
        None,
        Some("boris@example.com"),
    ));
    remna.insert_user(user(504, "cccc504", "stranger", Some(9999), None));
}

async fn scalar(services: &Services, sql: &str) -> i64 {
    let mut conn = services.store().conn().await.unwrap();
    sqlx::query_scalar(&format!("SELECT ({sql})::bigint"))
        .fetch_one(sn_store::Db::conn(&mut conn))
        .await
        .unwrap()
}

async fn import(env: &Env, bytes: &[u8], admin: Option<AdminId>) -> (Preview, Outcome) {
    let dump = archive::open_bytes(bytes, &wanted).unwrap();
    let legacy = map_dump(&dump).unwrap();
    let remna = check::fetch_remna(&env.services).await;
    let checked = check::preview(&env.services, &legacy, remna).await.unwrap();
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
    let outcome = apply_legacy(&env.services, &legacy, run_id, key, admin, None)
        .await
        .unwrap();
    env.services.set_remna(Some(env.remna.clone()));
    let mut conn = env.services.store().conn().await.unwrap();
    runs::finish(&mut conn, run_id, &outcome).await.unwrap();
    (checked.preview, outcome)
}

async fn owner(services: &Services) -> AdminId {
    services
        .auth()
        .upsert_owner("owner@example.com", "очень-надёжный-пароль")
        .await
        .unwrap()
        .0
}

#[tokio::test]
async fn stealthnet_переносится_целиком_без_изменений_в_remnawave() {
    let Some(env) = env().await else { return };
    seed_stealthnet_remna(&env.remna);
    let admin = owner(&env.services).await;
    let (preview, outcome) = import(&env, &stealthnet_dump(false), Some(admin)).await;
    assert_eq!(preview.detected, Source::Stealthnet);
    assert_eq!(preview.counts.clients, 4);
    assert_eq!(preview.counts.subscriptions, 4);
    assert_eq!(preview.counts.tariffs, 2);
    assert_eq!(preview.counts.payments, 6);
    assert_eq!(preview.counts.promo_codes, 2);
    assert_eq!(preview.counts.referrals, 1);
    assert_eq!(preview.counts.tickets, 1);
    assert_eq!(preview.remna.found, 3);
    assert_eq!(preview.remna.missing, 1);
    assert_eq!(preview.balances.total_minor, 15_050 + 2_000);
    assert!(preview.currency.switch, "{:?}", preview.currency);
    assert_eq!(preview.currency.source, Currency::Rub);
    let codes: Vec<&str> = preview.skipped.iter().map(|s| s.code.as_str()).collect();
    for code in [
        "balance_mixed",
        "payments_unpaid",
        "payments_free",
        "gift_unclaimed",
        "subscription_missing",
        "withdrawal_pending",
        "products_absent",
        "promo_invalid",
    ] {
        assert!(codes.contains(&code), "нет {code} в {codes:?}");
    }

    assert!(outcome.currency_switched);
    assert_eq!(env.services.settings().currency, Currency::Rub);
    assert_eq!(outcome.counts.clients, 4);
    assert_eq!(outcome.created, 4);
    assert_eq!(outcome.counts.subscriptions, 3);
    assert_eq!(outcome.counts.payments, 6);
    assert_eq!(outcome.counts.promo_codes, 2);
    assert_eq!(outcome.counts.referrals, 1);
    assert_eq!(outcome.counts.tickets, 1);
    assert_eq!(outcome.balance_total_minor, 17_050);
    let calls = env.remna.calls();
    assert!(
        calls
            .iter()
            .all(|c| matches!(c.as_str(), "stream_users" | "internal_squads")),
        "в Remnawave ничего не меняем: {calls:?}"
    );
    let s = &env.services;
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM clients WHERE registration_source = 'migration'"
        )
        .await,
        4
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM clients WHERE status = 'blocked'").await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM client_identities WHERE kind = 'email' AND value = 'boris@example.com'").await, 1);
    assert_eq!(scalar(s, "SELECT count(*) FROM client_passwords").await, 1);
    assert_eq!(
        scalar(s, "SELECT COALESCE(sum(amount_minor), 0)::bigint FROM client_balances WHERE currency = 'RUB'").await,
        17_050
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM subscriptions WHERE remna_user_id IN (501, 502, 503) AND tariff_id IS NOT NULL AND expire_at IS NOT NULL").await,
        3
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM subscriptions WHERE remna_user_id = 504"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(s, "SELECT p.days FROM subscriptions s JOIN tariff_periods p ON p.id = s.autorenew_period_id WHERE s.remna_user_id = 501 AND s.autorenew_enabled").await,
        90
    );
    assert_eq!(
        scalar(
            s,
            "SELECT extra_devices FROM subscriptions WHERE remna_user_id = 501"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM tariffs WHERE on_sale").await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM tariffs WHERE NOT on_sale").await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM payments WHERE provider_payload ? 'imported'"
        )
        .await,
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
        scalar(s, "SELECT count(*) FROM payments WHERE status = 'refunded'").await,
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
    assert_eq!(scalar(s, "SELECT count(*) FROM jobs").await, 0);
    assert_eq!(scalar(s, "SELECT count(*) FROM domain_events").await, 0);
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM promo_codes WHERE code IN ('WELCOME10', 'DAYS7')"
        )
        .await,
        2
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM promo_redemptions WHERE status = 'used'"
        )
        .await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM referral_links").await, 1);
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM referral_codes WHERE code = 'REF1A2B3C4D'"
        )
        .await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM ticket_messages").await, 3);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM tickets t JOIN ticket_messages m ON m.id = t.waiting_message_id WHERE t.status = 'open' AND t.last_author = 'client' AND m.body LIKE 'Всё ещё%'").await,
        1
    );
    assert!(scalar(s, "SELECT count(*) FROM trial_claims").await >= 3);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM client_utm WHERE source = 'tiktok'").await,
        1
    );

    let runs_id: uuid::Uuid = {
        let mut conn = s.store().conn().await.unwrap();
        sqlx::query_scalar("SELECT public_id FROM import_runs ORDER BY id DESC LIMIT 1")
            .fetch_one(sn_store::Db::conn(&mut conn))
            .await
            .unwrap()
    };
    let (name, bytes) = s.legacy().report(runs_id).await.unwrap();
    assert!(name.ends_with(".csv"));
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("не найден"), "{text}");
    assert!(text.contains("и в рублях, и в долларах"), "{text}");
    assert!(text.contains("Заявка на вывод"), "{text}");
}

#[tokio::test]
async fn повторный_перенос_догружает_свежее_без_дублей() {
    let Some(env) = env().await else { return };
    seed_stealthnet_remna(&env.remna);
    let admin = owner(&env.services).await;
    import(&env, &stealthnet_dump(false), Some(admin)).await;
    let (preview, outcome) = import(&env, &stealthnet_dump(true), Some(admin)).await;
    assert_eq!(preview.existing.again, 4);
    assert_eq!(outcome.created, 1);
    assert_eq!(outcome.updated, 4);
    let s = &env.services;
    assert_eq!(scalar(s, "SELECT count(*) FROM clients").await, 5);
    assert_eq!(scalar(s, "SELECT count(*) FROM subscriptions").await, 3);
    assert_eq!(scalar(s, "SELECT count(*) FROM payments").await, 7);
    assert_eq!(scalar(s, "SELECT count(*) FROM tariffs").await, 2);
    assert_eq!(scalar(s, "SELECT count(*) FROM promo_codes").await, 2);
    assert_eq!(scalar(s, "SELECT count(*) FROM ticket_messages").await, 3);
    assert_eq!(scalar(s, "SELECT count(*) FROM referral_links").await, 2);
    assert_eq!(
        scalar(s, "SELECT b.amount_minor FROM client_balances b JOIN client_identities i ON i.client_id = b.client_id WHERE i.kind = 'telegram' AND i.value = '1001'").await,
        17_050
    );
    assert_eq!(
        scalar(s, "SELECT b.amount_minor FROM client_balances b JOIN client_identities i ON i.client_id = b.client_id WHERE i.kind = 'email' AND i.value = 'boris@example.com'").await,
        500
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM balance_entries WHERE kind = 'admin_adjust' AND delta_minor = -1500").await,
        1
    );
    let (_, again) = import(&env, &stealthnet_dump(true), Some(admin)).await;
    assert_eq!(again.created, 0);
    assert_eq!(again.counts.payments, 0);
    assert_eq!(scalar(s, "SELECT count(*) FROM balance_entries").await, 5);
}

#[tokio::test]
async fn клиенты_этой_установки_объединяются_по_telegram_и_почте() {
    let Some(env) = env().await else { return };
    seed_stealthnet_remna(&env.remna);
    let existing = env
        .services
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: 1001,
                username: Some("anna_new".into()),
                first_name: Some("Анна".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
            RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap()
        .0
        .id;
    let (preview, outcome) = import(
        &env,
        &stealthnet_dump(false),
        Some(owner(&env.services).await),
    )
    .await;
    assert_eq!(preview.existing.matched, 1);
    assert_eq!(outcome.merged, 1);
    assert_eq!(outcome.created, 3);
    let s = &env.services;
    assert_eq!(scalar(s, "SELECT count(*) FROM clients").await, 4);
    assert_eq!(
        scalar(
            s,
            &format!(
                "SELECT count(*) FROM subscriptions WHERE client_id = {}",
                existing.0
            )
        )
        .await,
        2
    );
    assert_eq!(
        scalar(
            s,
            &format!(
                "SELECT amount_minor FROM client_balances WHERE client_id = {}",
                existing.0
            )
        )
        .await,
        15_050
    );
    assert_eq!(
        scalar(
            s,
            &format!(
                "SELECT count(*) FROM client_identities WHERE client_id = {} AND kind = 'email'",
                existing.0
            )
        )
        .await,
        1
    );
}

fn bedolaga_dump() -> Vec<u8> {
    let data = serde_json::json!({
        "metadata": {"timestamp": "2026-11-16T21:00:00+00:00", "version": "orm-1.0"},
        "data": {
            "users": [
                {"id": 1, "telegram_id": 2001, "username": "petr", "first_name": "Пётр", "status": "active", "language": "ru",
                 "balance_kopeks": 25000, "referral_code": "refPETR1", "referred_by_id": null, "created_at": "2026-05-01T10:00:00+03:00",
                 "has_had_paid_subscription": true},
                {"id": 2, "telegram_id": 2002, "username": null, "status": "blocked", "language": "en", "balance_kopeks": 5000,
                 "referred_by_id": 1, "created_at": "2026-06-01T10:00:00+00:00"},
                {"id": 3, "telegram_id": 2003, "status": "deleted", "balance_kopeks": 100},
                {"id": 4, "telegram_id": null, "email": "olga@example.com", "email_verified": true,
                 "password_hash": "$2b$12$abcdefghijklmnopqrstuuOdGq1xQmB0PZ5d4fN4m0wE1gVyZcH3W", "status": "active", "balance_kopeks": 0}
            ],
            "tariffs": [
                {"id": 7, "name": "Тариф А", "description": "Пример", "display_order": 1, "is_active": true, "traffic_limit_gb": 0,
                 "device_limit": 2, "device_price_kopeks": 5000, "max_device_limit": 4, "allowed_squads": format!("[\"{S1}\"]"),
                 "period_prices": "{\"30\": 19900, \"90\": 49900}", "traffic_reset_mode": null}
            ],
            "subscriptions": [
                {"id": 11, "user_id": 1, "status": "active", "is_trial": false, "end_date": "2026-12-30T00:00:00+00:00",
                 "traffic_limit_gb": 0, "device_limit": 3, "connected_squads": format!("[\"{S1}\"]"), "autopay_enabled": true,
                 "autopay_period_days": 30, "remnawave_short_uuid": "pppp601", "remnawave_id": 601, "tariff_id": 7},
                {"id": 12, "user_id": 2, "status": "active", "is_trial": false, "end_date": "2026-12-31T00:00:00+00:00",
                 "traffic_limit_gb": 50, "device_limit": 1, "connected_squads": format!("[\"{S1}\", \"{S2}\"]"),
                 "remnawave_short_uuid": "classic602", "tariff_id": null},
                {"id": 13, "user_id": 4, "status": "expired", "is_trial": true, "end_date": "2026-10-01T00:00:00+00:00",
                 "traffic_limit_gb": 50, "device_limit": 2, "connected_squads": format!("[\"{S2}\", \"{S1}\"]"),
                 "subscription_url": "https://sub.example.com/olga603", "tariff_id": null},
                {"id": 14, "user_id": 2, "status": "pending", "end_date": "2026-12-31T00:00:00+00:00"}
            ],
            "transactions": [
                {"id": 21, "user_id": 1, "type": "deposit", "amount_kopeks": 25000, "payment_method": "yookassa",
                 "external_id": "yk-21", "is_completed": true, "created_at": "2026-05-02T10:00:00+00:00", "completed_at": "2026-05-02T10:01:00+00:00"},
                {"id": 22, "user_id": 1, "type": "subscription_payment", "amount_kopeks": -19900, "description": "Покупка подписки на 30 дней",
                 "is_completed": true, "created_at": "2026-05-02T10:02:00+00:00"},
                {"id": 23, "user_id": 2, "type": "referral_reward", "amount_kopeks": 5000, "is_completed": true},
                {"id": 24, "user_id": 1, "type": "deposit", "amount_kopeks": 9900, "payment_method": "cryptobot", "is_completed": false}
            ],
            "promocodes": [
                {"id": 31, "code": "free7", "type": "subscription_days", "subscription_days": 7, "max_uses": 50, "is_active": true},
                {"id": 32, "code": "BONUS100", "type": "balance", "balance_bonus_kopeks": 10000, "max_uses": 1, "is_active": true}
            ],
            "promocode_uses": [{"id": 41, "promocode_id": 31, "user_id": 2, "used_at": "2026-06-02T10:00:00+00:00"}],
            "server_squads": [
                {"id": 51, "squad_uuid": S1, "display_name": "Нидерланды"},
                {"id": 52, "squad_uuid": S2, "display_name": "Германия"}
            ],
            "tickets": [{"id": 61, "user_id": 2, "title": "Вопрос", "status": "closed", "created_at": "2026-06-03T10:00:00+00:00",
                         "closed_at": "2026-06-03T12:00:00+00:00"}],
            "ticket_messages": [
                {"id": 71, "ticket_id": 61, "user_id": 2, "message_text": "Как подключить?", "is_from_admin": false, "created_at": "2026-06-03T10:00:00+00:00"},
                {"id": 72, "ticket_id": 61, "user_id": 1, "message_text": "Инструкция в боте", "is_from_admin": true, "has_media": true, "created_at": "2026-06-03T11:00:00+00:00"}
            ],
            "monitoring_logs": [{"id": 1, "message": "шум"}]
        },
        "associations": {}
    });
    let json = serde_json::to_vec(&data).unwrap();
    let mut builder = tar::Builder::new(Vec::new());
    for (name, bytes) in [
        (
            "metadata.json",
            br#"{"format_version":"2.0","timestamp":"2026-11-16T21:00:00+00:00"}"#.to_vec(),
        ),
        ("database.json", json),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, name, bytes.as_slice())
            .unwrap();
    }
    gz(&builder.into_inner().unwrap())
}

#[tokio::test]
async fn бедолага_переносится_вместе_с_режимом_без_тарифов() {
    let Some(env) = env().await else { return };
    env.remna.set_squads(vec![squad(S1, "NL"), squad(S2, "DE")]);
    env.remna
        .insert_user(user(601, "pppp601", "petr", Some(2001), None));
    env.remna
        .insert_user(user(602, "classic602", "u2002", Some(2002), None));
    env.remna
        .insert_user(user(603, "olga603", "olga", None, Some("olga@example.com")));
    let (preview, outcome) = import(&env, &bedolaga_dump(), Some(owner(&env.services).await)).await;
    assert_eq!(preview.detected, Source::Bedolaga);
    assert_eq!(preview.tariff_mode, model::TariffMode::Classic);
    assert_eq!(preview.counts.clients, 3);
    assert_eq!(preview.counts.subscriptions, 3);
    assert_eq!(preview.remna.found, 3);
    assert_eq!(preview.counts.payments, 2);
    assert_eq!(preview.counts.promo_codes, 1);
    assert!(preview.skipped.iter().any(|s| s.code == "users_deleted"));
    assert!(
        preview
            .skipped
            .iter()
            .any(|s| s.code == "promo_unsupported")
    );
    assert!(
        preview
            .skipped
            .iter()
            .any(|s| s.code == "subscriptions_pending")
    );
    assert_eq!(outcome.counts.subscriptions, 3);
    assert_eq!(outcome.counts.tariffs, 2);
    let s = &env.services;
    assert_eq!(env.services.settings().currency, Currency::Rub);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM clients WHERE status = 'blocked'").await,
        0
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM clients WHERE bot_unreachable").await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM tariffs WHERE name = 'Нидерланды, Германия · 50 ГБ' AND NOT on_sale").await, 1);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM subscriptions s JOIN tariffs t ON t.id = s.tariff_id WHERE t.name LIKE 'Нидерланды%'").await,
        2
    );
    assert_eq!(
        scalar(
            s,
            "SELECT extra_devices FROM subscriptions WHERE remna_user_id = 601"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM subscriptions WHERE kind = 'trial'").await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM tariff_periods WHERE price_rub_minor = 19900"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT COALESCE(sum(amount_minor), 0)::bigint FROM client_balances WHERE currency = 'RUB'").await,
        30_000
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM payments WHERE provider = 'balance'"
        )
        .await,
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
    assert_eq!(scalar(s, "SELECT count(*) FROM referral_links").await, 1);
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM referral_codes WHERE code = 'REFPETR1'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(s, "SELECT count(*) FROM tickets WHERE status = 'closed'").await,
        1
    );
    assert_eq!(
        scalar(
            s,
            "SELECT count(*) FROM ticket_messages WHERE body LIKE '%вложение осталось%'"
        )
        .await,
        1
    );
    assert_eq!(scalar(s, "SELECT count(*) FROM client_passwords").await, 1);
}

#[tokio::test]
async fn без_remnawave_проверка_просит_подключить_панель() {
    let Some(env) = env().await else { return };
    env.services.set_remna(None);
    let dump = archive::open_bytes(&stealthnet_dump(false), &wanted).unwrap();
    let legacy = map_dump(&dump).unwrap();
    let remna = check::fetch_remna(&env.services).await;
    let checked = check::preview(&env.services, &legacy, remna).await.unwrap();
    assert!(!checked.preview.remna.connected);
    assert_eq!(checked.preview.remna.found, 0);
    let err = apply_legacy(&env.services, &legacy, 1, uuid::Uuid::new_v4(), None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "import.remna_required");
    assert_eq!(
        scalar(&env.services, "SELECT count(*) FROM clients").await,
        0
    );
}

async fn wait(services: &Services, id: uuid::Uuid, want: &[RunState]) -> runs::RunView {
    for _ in 0..200 {
        let run = services.legacy().run(id).await.unwrap();
        if want.contains(&run.state) {
            return run;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("перенос не дошёл до {want:?}");
}

#[tokio::test]
async fn загрузка_проверка_и_перенос_идут_в_фоне() {
    let Some(env) = env().await else { return };
    seed_stealthnet_remna(&env.remna);
    let admin = owner(&env.services).await;
    let dir =
        std::env::temp_dir().join(format!("sn-import-test-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("upload.upload");
    std::fs::write(&path, bedolaga_dump()).unwrap();
    let run = env
        .services
        .legacy()
        .uploaded(
            Source::Stealthnet,
            "backup_20261116_210000.tar.gz",
            100,
            &path,
            Some(admin),
        )
        .await
        .unwrap();
    let ready = wait(&env.services, run.id, &[RunState::Ready, RunState::Failed]).await;
    assert_eq!(ready.state, RunState::Ready, "{:?}", ready.error);
    assert_eq!(ready.source, Source::Bedolaga);
    let view = env.services.legacy().view().await.unwrap();
    assert_eq!(view.run.as_ref().map(|r| r.id), Some(run.id));
    env.services
        .legacy()
        .start(run.id, Some(admin))
        .await
        .unwrap();
    let again = env.services.legacy().start(run.id, Some(admin)).await;
    assert!(again.is_err());
    let done = wait(&env.services, run.id, &[RunState::Done, RunState::Failed]).await;
    assert_eq!(done.state, RunState::Done, "{:?}", done.error);
    assert!(done.outcome.is_some());
    assert!(!path.exists(), "файл выгрузки удаляется после переноса");
    let history = env.services.legacy().view().await.unwrap().history;
    assert_eq!(history.len(), 1);
    let fresh = dir.join("fresh.upload");
    std::fs::write(&fresh, bedolaga_dump()).unwrap();
    let second = env
        .services
        .legacy()
        .uploaded(
            Source::Bedolaga,
            "backup_fresh.tar.gz",
            100,
            &fresh,
            Some(admin),
        )
        .await
        .unwrap();
    wait(
        &env.services,
        second.id,
        &[RunState::Ready, RunState::Failed],
    )
    .await;
    env.services.legacy().discard(second.id).await.unwrap();
    let view = env.services.legacy().view().await.unwrap();
    assert!(
        view.run.is_none(),
        "после отказа от файла не показывается прошлый перенос"
    );
    assert_eq!(view.history.len(), 1);
    assert!(!fresh.exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn архив_с_паролем_открывается_только_с_верным_паролем() {
    let dir = std::env::temp_dir().join(format!("sn-zip-test-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("backup.zip");
    {
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .with_aes_encryption(zip::AesMode::Aes256, "верный");
        zip.start_file("backup_20261116_210000.tar.gz", options)
            .unwrap();
        zip.write_all(&bedolaga_dump()).unwrap();
        zip.finish().unwrap();
    }
    assert_eq!(
        read_file(&path, None).unwrap_err(),
        archive::OpenError::NeedsPassword
    );
    assert_eq!(
        read_file(&path, Some("неверный")).unwrap_err(),
        archive::OpenError::WrongPassword
    );
    let (legacy, extracted) = read_file(&path, Some("верный")).unwrap();
    assert_eq!(legacy.source, Source::Bedolaga);
    assert_eq!(legacy.clients.len(), 3);
    let inner = extracted.unwrap();
    assert!(inner.exists());
    assert_eq!(read_file(&inner, None).unwrap().0.clients.len(), 3);
    std::fs::remove_dir_all(&dir).ok();
}

fn quirks_dump() -> Vec<u8> {
    let mut sql = String::from("-- Created at: 2026-11-17T09:00:00.000Z\n\n");
    sql += &copy(
        "system_settings",
        &["id", "key", "value"],
        &[&[Some("ss1"), Some("default_currency"), Some("rub")]],
    );
    sql += &copy(
        "tariff_categories",
        &["id", "name", "sort_order"],
        &[&[Some("cat1"), Some("Все"), Some("0")]],
    );
    sql += &copy(
        "tariffs",
        &[
            "id",
            "category_id",
            "name",
            "duration_days",
            "price",
            "currency",
        ],
        &[&[
            Some("t1"),
            Some("cat1"),
            Some("Месяц"),
            Some("30"),
            Some("300"),
            Some("rub"),
        ]],
    );
    sql += &copy(
        "tariff_price_options",
        &["id", "tariff_id", "duration_days", "price", "sort_order"],
        &[&[Some("o1"), Some("t1"), Some("30"), Some("300"), Some("0")]],
    );
    sql += &copy(
        "clients",
        &[
            "id",
            "email",
            "role",
            "balance",
            "telegram_id",
            "telegram_username",
            "referrer_id",
        ],
        &[
            &[
                Some("k1"),
                None,
                Some("CLIENT"),
                Some("120"),
                Some("2001"),
                Some("Lena"),
                Some("k3"),
            ],
            &[
                Some("k2"),
                None,
                Some("CLIENT"),
                Some("50000"),
                Some("2002"),
                None,
                None,
            ],
            &[
                Some("k3"),
                None,
                Some("CLIENT"),
                Some("0"),
                Some("2003"),
                Some("Oleg"),
                Some("k1"),
            ],
            &[
                Some("k4"),
                Some("Mila@Example.com"),
                Some("CLIENT"),
                Some("0"),
                None,
                None,
                Some("gone"),
            ],
            &[
                Some("k5"),
                Some("mila@example.com"),
                Some("CLIENT"),
                Some("0"),
                None,
                None,
                None,
            ],
            &[
                Some("k6"),
                Some("not-an-email"),
                Some("CLIENT"),
                Some("0"),
                Some("2006"),
                None,
                None,
            ],
        ],
    );
    let u = |n: u32| format!("6f1c2f3e-0000-4000-8000-00000000000{n}");
    let (u1, u2, u3, u4, u5, u6, u7) = (u(1), u(2), u(3), u(4), u(5), u(6), u(7));
    let sub = |id: &'static str,
               owner: &'static str,
               uuid: &str,
               index: &'static str,
               expire: &'static str,
               hidden: Option<&'static str>| {
        [
            Some(id.to_string()),
            Some(owner.to_string()),
            Some(uuid.to_string()),
            Some(index.to_string()),
            Some("t1".to_string()),
            Some(expire.to_string()),
            hidden.map(str::to_string),
        ]
    };
    let rows = [
        sub("a0", "k1", &u1, "0", "2026-12-30 00:00:00", None),
        sub("a1", "k1", &u2, "1", "2026-12-30 00:00:00", None),
        sub("a2", "k1", &u3, "2", "2026-12-30 00:00:00", None),
        sub("b0", "k2", &u4, "0", "2026-12-25 00:00:00", None),
        sub("c0", "k3", &u4, "0", "2026-12-25 00:00:00", None),
        sub(
            "h0",
            "k3",
            &u5,
            "1",
            "2026-01-01 00:00:00",
            Some("2026-02-01 00:00:00"),
        ),
        sub("m0", "k4", &u6, "0", "2026-12-28 00:00:00", None),
        sub("n0", "k5", &u7, "0", "2026-12-28 00:00:00", None),
    ];
    let cells: Vec<Vec<Option<&str>>> = rows
        .iter()
        .map(|r| r.iter().map(|c| c.as_deref()).collect())
        .collect();
    let refs: Vec<&[Option<&str>]> = cells.iter().map(Vec::as_slice).collect();
    sql += &copy(
        "subscriptions",
        &[
            "id",
            "owner_id",
            "remnawave_uuid",
            "subscription_index",
            "tariff_id",
            "expire_at",
            "hidden_at",
        ],
        &refs,
    );
    sql.into_bytes()
}

async fn remna_user_of(services: &Services, old_id: &str) -> i64 {
    scalar(
        services,
        &format!(
            "SELECT s.remna_user_id FROM legacy_ids l JOIN subscriptions s ON s.id = l.new_id \
             WHERE l.entity = 'stealthnet:subscription' AND l.old_id = '{old_id}'"
        ),
    )
    .await
}

#[tokio::test]
async fn особые_случаи_старой_панели_переносятся_и_объясняются() {
    let Some(env) = env().await else { return };
    let expire = |day: u32| Utc.with_ymd_and_hms(2026, 12, day, 0, 0, 0).unwrap();
    let add = |id: i64, name: &str, tg: Option<i64>, email: Option<&str>, day: u32| {
        let mut u = user(id, &format!("short{id}"), name, tg, email);
        u.expire_at = expire(day);
        env.remna.insert_user(u);
    };
    add(11, "Lena_2", Some(2001), None, 30);
    add(12, "Lena", Some(2001), None, 30);
    add(13, "Lena_1", Some(2001), None, 30);
    add(14, "Oleg", Some(2003), None, 25);
    add(15, "x15", None, Some("mila@example.com"), 28);
    add(16, "x16", None, Some("Mila@Example.com"), 28);
    let admin = owner(&env.services).await;
    let (preview, outcome) = import(&env, &quirks_dump(), Some(admin)).await;

    assert_eq!(preview.counts.subscriptions, 7);
    assert_eq!(preview.remna.found, 6);
    assert_eq!(preview.remna.shared, 1);
    assert_eq!(preview.remna.missing + preview.remna.ambiguous, 0);
    assert_eq!(preview.balances.large_clients, 1);
    assert_eq!(preview.balances.large_total_minor, 5_000_000);
    assert_eq!(preview.balances.large_from_minor, Some(300_000));
    assert!(preview.balances.large_by_tariff);
    let skip = |code: &str| {
        preview
            .skipped
            .iter()
            .find(|s| s.code == code)
            .map(|s| s.count)
    };
    assert_eq!(skip("subscription_hidden"), Some(1));
    assert_eq!(skip("subscription_shared"), Some(1));

    let s = &env.services;
    assert_eq!(remna_user_of(s, "a0").await, 12);
    assert_eq!(remna_user_of(s, "a1").await, 13);
    assert_eq!(remna_user_of(s, "a2").await, 11);
    assert_eq!(remna_user_of(s, "c0").await, 14);
    assert_eq!(remna_user_of(s, "m0").await, 16);
    assert_eq!(remna_user_of(s, "n0").await, 15);
    assert_eq!(outcome.counts.subscriptions, 6);
    assert_eq!(
        scalar(s, "SELECT count(*) FROM legacy_ids WHERE entity = 'stealthnet:subscription' AND old_id IN ('b0', 'h0')").await,
        0
    );
    assert_eq!(
        scalar(
            s,
            "SELECT COALESCE(sum(amount_minor), 0)::bigint FROM client_balances"
        )
        .await,
        5_012_000
    );

    let run = scalar(s, "SELECT max(id) FROM import_runs").await;
    let mut conn = s.store().conn().await.unwrap();
    let issues = runs::issues(&mut conn, run).await.unwrap();
    assert_eq!(issues[0].code, "balance_large", "деньги — первыми в отчёте");
    let shared = issues
        .iter()
        .find(|i| i.code == "subscription_shared")
        .expect("общая подписка в отчёте");
    assert_eq!(shared.old_id.as_deref(), Some("b0"));
    assert!(shared.details.contains("Oleg"), "{}", shared.details);
    let large = issues
        .iter()
        .find(|i| i.code == "balance_large")
        .expect("крупный баланс в отчёте");
    assert_eq!(large.old_id.as_deref(), Some("k2"));
    assert_eq!(
        issues
            .iter()
            .filter(|i| i.code.starts_with("subscription_"))
            .count(),
        1,
        "{issues:?}"
    );
    let bad_email = issues
        .iter()
        .find(|i| i.code == "email_invalid")
        .expect("ошибка в почте в отчёте");
    assert_eq!(bad_email.old_id.as_deref(), Some("k6"));
    let skipped: Vec<&str> = issues
        .iter()
        .filter(|i| i.code == "referral_skipped")
        .filter_map(|i| i.old_id.as_deref())
        .collect();
    assert_eq!(skipped, ["k3", "k4"], "{issues:?}");
    assert_eq!(outcome.counts.referrals, 1);
    let calls = env.remna.calls();
    assert!(
        calls
            .iter()
            .all(|c| matches!(c.as_str(), "stream_users" | "internal_squads")),
        "{calls:?}"
    );
}

#[tokio::test]
async fn пустая_или_чужая_remnawave_даёт_понятное_предупреждение() {
    let Some(env) = env().await else { return };
    let dump = archive::open_bytes(&stealthnet_dump(false), &wanted).unwrap();
    let legacy = map_dump(&dump).unwrap();
    let check = || async {
        let remna = check::fetch_remna(&env.services).await;
        check::preview(&env.services, &legacy, remna)
            .await
            .unwrap()
            .preview
    };
    let empty = check().await;
    assert_eq!(empty.remna.users, 0);
    let text = wrong_panel(&empty).expect("предупреждение о пустой панели");
    assert!(text.contains("нет ни одного пользователя"), "{text}");
    env.remna
        .insert_user(user(900, "zzzz900", "stranger900", Some(424_242), None));
    let foreign = check().await;
    let text = wrong_panel(&foreign).expect("предупреждение о чужой панели");
    assert!(text.contains("пользователей в ней: 1"), "{text}");
    seed_stealthnet_remna(&env.remna);
    assert_eq!(wrong_panel(&check().await), None);
}
