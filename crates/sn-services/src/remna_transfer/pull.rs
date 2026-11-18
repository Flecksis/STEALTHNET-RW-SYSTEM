use super::push::bind;
use super::remna::{Pace, RemnaIndex, panel_down};
use super::runs::NoteRow;
use super::snapshot;
use super::{Control, Direction, Example, Plan, Progress, RunEnd, merge_progress, text};
use crate::error::ServiceResult;
use crate::services::Services;
use chrono::{DateTime, FixedOffset, Utc};
use sn_domain::client::{IdentityKind, normalize_identity};
use sn_domain::ids::{RemnaUserId, SubscriptionId};
use sn_domain::subscription::{RemnaLink, SubscriptionKind, TRIAL_TAG};
use sn_domain::tariff::Tariff;
use sn_remnawave::RemnaUser;
use sn_store::Db;
use std::collections::{BTreeSet, HashMap, HashSet};
use uuid::Uuid;

const CHUNK: usize = 500;
const MARKER: &str = "stealthnet client ";

#[derive(Debug, Clone, sqlx::FromRow)]
struct SubRef {
    id: i64,
    client_id: i64,
    client_public_id: Uuid,
    remna_user_id: Option<i64>,
    remna_username: Option<String>,
    short_uuid: Option<String>,
}

impl SubRef {
    fn link(&self) -> Option<RemnaLink> {
        Some(RemnaLink {
            user_id: RemnaUserId(self.remna_user_id?),
            username: self.remna_username.clone()?,
            short_uuid: self.short_uuid.clone(),
        })
    }
}

#[derive(Default)]
struct Lookups {
    by_user: HashMap<i64, SubRef>,
    by_short: HashMap<String, SubRef>,
    by_name: HashMap<String, SubRef>,
    by_public: HashMap<Uuid, i64>,
    by_telegram: HashMap<i64, i64>,
    by_email: HashMap<String, i64>,
    by_tg_name: HashMap<String, Vec<i64>>,
    live: HashMap<i64, i64>,
}

fn marker(user: &RemnaUser) -> Option<Uuid> {
    user.description
        .as_deref()?
        .trim()
        .strip_prefix(MARKER)
        .and_then(|rest| Uuid::parse_str(rest.trim()).ok())
}

fn email_of(user: &RemnaUser) -> Option<String> {
    let email = user.email.as_deref()?.trim().to_lowercase();
    normalize_identity(IdentityKind::Email, &email).ok()
}

fn tg_from_name(name: &str) -> Option<i64> {
    name.strip_prefix("tg")
        .filter(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
        .and_then(|rest| rest.parse().ok())
}

async fn lookups(db: &mut impl Db, users: &[RemnaUser]) -> ServiceResult<Lookups> {
    let ids: Vec<i64> = users.iter().map(|u| u.id.0).collect();
    let shorts: Vec<String> = users
        .iter()
        .map(|u| u.short_uuid.clone())
        .filter(|s| !s.is_empty())
        .collect();
    let names: Vec<String> = users.iter().map(|u| u.username.clone()).collect();
    let subs: Vec<SubRef> = sqlx::query_as(
        "SELECT s.id, s.client_id, c.public_id AS client_public_id, s.remna_user_id, s.remna_username, s.short_uuid \
         FROM subscriptions s JOIN clients c ON c.id = s.client_id \
         WHERE s.remna_user_id = ANY($1) OR s.short_uuid = ANY($2) OR s.remna_username = ANY($3)",
    )
    .bind(&ids)
    .bind(&shorts)
    .bind(&names)
    .fetch_all(db.conn())
    .await?;
    let mut out = Lookups::default();
    for sub in subs {
        if let Some(id) = sub.remna_user_id {
            out.by_user.insert(id, sub.clone());
        }
        if let Some(short) = sub.short_uuid.clone().filter(|s| !s.is_empty()) {
            out.by_short.insert(short, sub.clone());
        }
        if let Some(name) = sub.remna_username.clone() {
            out.by_name.insert(name, sub);
        }
    }
    let publics: Vec<Uuid> = users.iter().filter_map(marker).collect();
    let rows: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT public_id, id FROM clients WHERE public_id = ANY($1) AND deleted_at IS NULL",
    )
    .bind(&publics)
    .fetch_all(db.conn())
    .await?;
    out.by_public = rows.into_iter().collect();
    let mut telegrams: Vec<String> = users
        .iter()
        .filter_map(|u| u.telegram_id)
        .map(|t| t.to_string())
        .collect();
    telegrams.extend(
        users
            .iter()
            .filter_map(|u| tg_from_name(&u.username))
            .map(|t| t.to_string()),
    );
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT i.value, i.client_id FROM client_identities i JOIN clients c ON c.id = i.client_id \
         WHERE i.kind = 'telegram' AND i.value = ANY($1) AND c.deleted_at IS NULL",
    )
    .bind(&telegrams)
    .fetch_all(db.conn())
    .await?;
    out.by_telegram = rows
        .into_iter()
        .filter_map(|(value, client)| value.parse().ok().map(|tg| (tg, client)))
        .collect();
    let emails: Vec<String> = users.iter().filter_map(email_of).collect();
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT i.value, i.client_id FROM client_identities i JOIN clients c ON c.id = i.client_id \
         WHERE i.kind = 'email' AND i.value = ANY($1) AND c.deleted_at IS NULL",
    )
    .bind(&emails)
    .fetch_all(db.conn())
    .await?;
    out.by_email = rows.into_iter().collect();
    let lowered: Vec<String> = users.iter().map(|u| u.username.to_lowercase()).collect();
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT lower(i.meta->>'username'), i.client_id FROM client_identities i JOIN clients c ON c.id = i.client_id \
         WHERE i.kind = 'telegram' AND lower(i.meta->>'username') = ANY($1) AND c.deleted_at IS NULL",
    )
    .bind(&lowered)
    .fetch_all(db.conn())
    .await?;
    for (name, client) in rows {
        let list = out.by_tg_name.entry(name).or_default();
        if !list.contains(&client) {
            list.push(client);
        }
    }
    let mut clients: Vec<i64> = out.by_public.values().copied().collect();
    clients.extend(out.by_telegram.values().copied());
    clients.extend(out.by_email.values().copied());
    clients.extend(out.by_tg_name.values().flatten().copied());
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT client_id, count(*) FROM subscriptions WHERE client_id = ANY($1) AND ended_at IS NULL \
         AND remna_user_id IS NOT NULL GROUP BY client_id",
    )
    .bind(&clients)
    .fetch_all(db.conn())
    .await?;
    out.live = rows.into_iter().collect();
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum By {
    Marker,
    Telegram,
    Email,
    Name,
}

impl By {
    const fn key(self) -> &'static str {
        match self {
            By::Marker => "marker",
            By::Telegram => "telegram",
            By::Email => "email",
            By::Name => "name",
        }
    }

    const fn words(self) -> &'static str {
        match self {
            By::Marker => "по отметке панели в описании",
            By::Telegram => "по Telegram ID",
            By::Email => "по почте",
            By::Name => "по имени в Telegram",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TariffMatch {
    One { id: i64, name: String, extra: u32 },
    Many(Vec<String>),
    None,
}

fn tariff_hwid(tariff: &Tariff, extra: u32) -> Option<i64> {
    let devices = tariff.devices.included.saturating_add(extra);
    (devices > 0).then_some(i64::from(devices))
}

fn match_tariff(user: &RemnaUser, tariffs: &[Tariff]) -> TariffMatch {
    let squads: BTreeSet<Uuid> = user.active_internal_squads.iter().map(|s| s.uuid).collect();
    let mut found: Vec<(&Tariff, u32)> = Vec::new();
    for tariff in tariffs {
        let own: BTreeSet<Uuid> = tariff.squads.iter().map(|s| s.0).collect();
        if own != squads || own.is_empty() {
            continue;
        }
        if tariff.external_squad.map(|s| s.0) != user.external_squad_uuid {
            continue;
        }
        let limit = i64::try_from(tariff.traffic.remna_limit_bytes()).unwrap_or(i64::MAX);
        if limit != user.traffic_limit_bytes
            || tariff.traffic.reset.strategy() != user.traffic_limit_strategy
        {
            continue;
        }
        let max_extra = tariff.devices.extra.as_ref().map_or(0, |e| e.max);
        let extra = (0..=max_extra).find(|k| tariff_hwid(tariff, *k) == user.hwid_device_limit);
        if let Some(extra) = extra {
            found.push((tariff, extra));
        }
    }
    if found.len() > 1 {
        let on_sale: Vec<(&Tariff, u32)> =
            found.iter().copied().filter(|(t, _)| t.on_sale).collect();
        if on_sale.len() == 1 {
            found = on_sale;
        }
    }
    match found.as_slice() {
        [] => TariffMatch::None,
        [(tariff, extra)] => TariffMatch::One {
            id: tariff.id.0,
            name: tariff.name.clone(),
            extra: *extra,
        },
        many => TariffMatch::Many(many.iter().map(|(t, _)| t.name.clone()).collect()),
    }
}

enum Pull {
    Already,
    Relink(SubRef),
    Skip(&'static str, &'static str, String),
    Attach { client: i64, by: By },
    Create,
}

fn decide(user: &RemnaUser, found: &Lookups, index: &RemnaIndex) -> Pull {
    if let Some(sub) = found.by_user.get(&user.id.0)
        && sub
            .link()
            .is_some_and(|link| crate::remna_sync::same_user(&link, user))
    {
        return Pull::Already;
    }
    if let Some(sub) = found.by_short.get(&user.short_uuid)
        && sub.remna_user_id != Some(user.id.0)
    {
        return Pull::Relink(sub.clone());
    }
    if let Some(sub) = found.by_name.get(&user.username)
        && sub.remna_user_id != Some(user.id.0)
    {
        let unknown_link = sub.short_uuid.as_deref().is_none_or(str::is_empty);
        if unknown_link {
            if marker(user) == Some(sub.client_public_id) {
                return Pull::Relink(sub.clone());
            }
            return Pull::Skip(
                "name_unverified",
                "имя совпадает с подпиской панели, но пользователь создан не этой панелью",
                format!(
                    "имя {} уже у подписки №{}, а в описании пользователя нет отметки этой панели — проверьте вручную",
                    user.username, sub.id
                ),
            );
        }
        let holder = sub
            .remna_user_id
            .and_then(|id| index.by_id(id))
            .filter(|other| {
                sub.link()
                    .is_some_and(|link| crate::remna_sync::same_user(&link, other))
            });
        return match holder {
            Some(other) => Pull::Skip(
                "name_linked",
                "имя совпадает с подпиской панели, привязанной к другому пользователю Remnawave",
                format!(
                    "имя {} уже у подписки №{}, она привязана к пользователю Remnawave №{}",
                    user.username, sub.id, other.id.0
                ),
            ),
            None => Pull::Skip(
                "name_link_differs",
                "имя совпадает с подпиской панели, но ссылка другая",
                format!(
                    "имя {} уже у подписки №{}, а ссылка в Remnawave другая — проверьте вручную",
                    user.username, sub.id
                ),
            ),
        };
    }
    if let Some(client) = marker(user).and_then(|id| found.by_public.get(&id)) {
        return Pull::Attach {
            client: *client,
            by: By::Marker,
        };
    }
    if let Some(client) = user.telegram_id.and_then(|tg| found.by_telegram.get(&tg)) {
        return Pull::Attach {
            client: *client,
            by: By::Telegram,
        };
    }
    if let Some(client) = email_of(user).and_then(|e| found.by_email.get(&e)) {
        return Pull::Attach {
            client: *client,
            by: By::Email,
        };
    }
    if let Some(client) = tg_from_name(&user.username).and_then(|tg| found.by_telegram.get(&tg)) {
        return Pull::Attach {
            client: *client,
            by: By::Name,
        };
    }
    match found
        .by_tg_name
        .get(&user.username.to_lowercase())
        .map(Vec::as_slice)
    {
        Some([client]) => Pull::Attach {
            client: *client,
            by: By::Name,
        },
        Some([_, _, ..]) => Pull::Skip(
            "name_ambiguous",
            "по имени подходит несколько клиентов",
            format!(
                "имя {} есть у нескольких клиентов в Telegram — привяжите вручную",
                user.username
            ),
        ),
        _ => Pull::Create,
    }
}

fn who(user: &RemnaUser) -> String {
    let mut parts = vec![format!("{} (№{})", user.username, user.id.0)];
    if let Some(tg) = user.telegram_id {
        parts.push(format!("Telegram {tg}"));
    }
    if let Some(email) = &user.email {
        parts.push(email.clone());
    }
    parts.join(" · ")
}

fn lines(user: &RemnaUser, tariff: &TariffMatch, offset: FixedOffset) -> Vec<String> {
    let mut lines = vec![format!("Срок до {}", text::date(user.expire_at, offset))];
    lines.push(match tariff {
        TariffMatch::One { name, extra, .. } if *extra > 0 => {
            format!("Тариф «{name}», доп. устройств: {extra}")
        }
        TariffMatch::One { name, .. } => format!("Тариф «{name}»"),
        TariffMatch::Many(names) => format!(
            "Без тарифа: подходят сразу несколько — {}",
            names.join(", ")
        ),
        TariffMatch::None => "Без тарифа: сквады и лимиты не совпали ни с одним".into(),
    });
    lines
}

fn note(user: &RemnaUser, code: &str, details: impl Into<String>) -> NoteRow {
    NoteRow {
        remna_user_id: Some(user.id.0),
        who: Some(who(user)),
        ..NoteRow::new(code, details)
    }
}

async fn tariffs(services: &Services) -> ServiceResult<Vec<Tariff>> {
    let mut conn = services.store().conn().await?;
    Ok(conn.tariffs().list(true).await?)
}

async fn load(services: &Services, pace: &Pace) -> ServiceResult<RemnaIndex> {
    let remna = services.remna()?;
    Ok(RemnaIndex::load(remna.as_ref(), pace).await?)
}

pub(crate) async fn plan(services: &Services, pace: &Pace) -> ServiceResult<(Plan, Vec<NoteRow>)> {
    let index = load(services, pace).await?;
    let tariffs = tariffs(services).await?;
    let offset = services.settings().remna_offset();
    let mut plan = Plan {
        direction: Direction::Pull,
        remna_users: index.len() as u64,
        secrets: services.secrets().is_some(),
        ..Plan::default()
    };
    let mut notes = Vec::new();
    let mut extra_subs: HashMap<i64, i64> = HashMap::new();
    let mut conn = services.store().conn().await?;
    for chunk in index.users().chunks(CHUNK) {
        let found = lookups(&mut conn, chunk).await?;
        for user in chunk {
            plan.scanned += 1;
            match decide(user, &found, &index) {
                Pull::Already => plan.same += 1,
                Pull::Relink(sub) => plan.relink.add(|| Example {
                    who: who(user),
                    details: vec![format!("Подписка №{} панели", sub.id)],
                }),
                Pull::Skip(code, title, details) => {
                    notes.push(note(user, code, details.clone()));
                    plan.skip(code, title, Some(format!("{}: {details}", who(user))));
                }
                Pull::Attach { client, by } => {
                    let tariff = match_tariff(user, &tariffs);
                    count_tariff(&mut plan, &tariff);
                    *plan.matched_by.entry(by.key().into()).or_default() += 1;
                    let had = found.live.get(&client).copied().unwrap_or(0)
                        + extra_subs.get(&client).copied().unwrap_or(0);
                    if had > 0 {
                        plan.several += 1;
                    }
                    *extra_subs.entry(client).or_default() += 1;
                    let mut details = vec![format!("Клиент №{client}, найден {}", by.words())];
                    details.extend(lines(user, &tariff, offset));
                    if !matches!(tariff, TariffMatch::One { .. }) {
                        notes.push(note(user, "tariff_none", details[1..].join("; ")));
                    }
                    plan.attach.add(|| Example {
                        who: who(user),
                        details,
                    });
                }
                Pull::Create => {
                    let tariff = match_tariff(user, &tariffs);
                    count_tariff(&mut plan, &tariff);
                    if user.telegram_id.is_none() && email_of(user).is_none() {
                        plan.no_contact += 1;
                    }
                    let details = lines(user, &tariff, offset);
                    if !matches!(tariff, TariffMatch::One { .. }) {
                        notes.push(note(user, "tariff_none", details[1..].join("; ")));
                    }
                    plan.create.add(|| Example {
                        who: who(user),
                        details,
                    });
                }
            }
        }
    }
    if plan.no_contact > 0 {
        plan.warnings.push(format!(
            "У {} нет ни Telegram ID, ни почты. Такие клиенты появятся в панели, но сами войти в бот или кабинет не смогут. Чтобы они могли войти, до запуска впишите им Telegram ID или почту в Remnawave и проверьте снова.",
            text::count(plan.no_contact, "пользователя", "пользователей", "пользователей")
        ));
    }
    Ok((plan, notes))
}

fn count_tariff(plan: &mut Plan, tariff: &TariffMatch) {
    match tariff {
        TariffMatch::One { .. } => plan.tariff_matched += 1,
        _ => plan.tariff_none += 1,
    }
}

async fn new_client(
    tx: &mut sn_store::Tx,
    user: &RemnaUser,
    created_at: DateTime<Utc>,
) -> ServiceResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO clients (locale, registration_source, note, created_at) VALUES ('ru', 'import', $1, $2) RETURNING id",
    )
    .bind(format!("Из Remnawave: {}", user.username))
    .bind(created_at)
    .fetch_one(tx.conn())
    .await?;
    let mut identities = Vec::new();
    if let Some(tg) = user.telegram_id {
        identities.push(sn_domain::client::NewIdentity {
            kind: IdentityKind::Telegram,
            value: tg.to_string(),
            verified: true,
            meta: serde_json::json!({}),
        });
    }
    if let Some(email) = email_of(user) {
        identities.push(sn_domain::client::NewIdentity {
            kind: IdentityKind::Email,
            value: email,
            verified: false,
            meta: serde_json::json!({}),
        });
    }
    for identity in identities {
        tx.clients()
            .add_identity_if_free(sn_domain::ids::ClientId(id), &identity)
            .await?;
    }
    Ok(id)
}

enum Placed {
    Done { subscription: i64, client: i64 },
    Taken,
}

async fn place(
    services: &Services,
    user: &RemnaUser,
    client: Option<i64>,
    tariff: &TariffMatch,
) -> ServiceResult<Placed> {
    let now = services.now();
    let mut tx = services.store().begin().await?;
    let client = match client {
        Some(client) => client,
        None => new_client(&mut tx, user, user.created_at).await?,
    };
    let (tariff_id, extra) = match tariff {
        TariffMatch::One { id, extra, .. } => (Some(*id), *extra),
        _ => (None, 0),
    };
    let kind = if user.tag.as_deref() == Some(TRIAL_TAG) {
        SubscriptionKind::Trial
    } else if tariff_id.is_some() {
        SubscriptionKind::Paid
    } else {
        SubscriptionKind::Manual
    };
    sqlx::query("UPDATE subscriptions SET remna_user_id = NULL WHERE remna_user_id = $1")
        .bind(user.id.0)
        .execute(tx.conn())
        .await?;
    let created: Option<i64> = sqlx::query_scalar(
        "INSERT INTO subscriptions (client_id, tariff_id, kind, extra_devices, remna_user_id, remna_username, short_uuid, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT DO NOTHING RETURNING id",
    )
    .bind(client)
    .bind(tariff_id)
    .bind(kind.as_str())
    .bind(i32::try_from(extra).unwrap_or(0))
    .bind(user.id.0)
    .bind(&user.username)
    .bind(Some(user.short_uuid.clone()).filter(|s| !s.is_empty()))
    .bind(user.created_at)
    .fetch_optional(tx.conn())
    .await?;
    let Some(subscription) = created else {
        tx.rollback().await?;
        return Ok(Placed::Taken);
    };
    tx.subscriptions()
        .apply_mirror(SubscriptionId(subscription), &user.mirror(None, now))
        .await?;
    snapshot::save(
        &mut tx,
        services.secrets(),
        SubscriptionId(subscription),
        user,
    )
    .await?;
    tx.commit().await?;
    Ok(Placed::Done {
        subscription,
        client,
    })
}

pub(crate) async fn apply(
    services: &Services,
    pace: &Pace,
    ctl: &Control<'_>,
) -> ServiceResult<RunEnd> {
    let base = ctl.base();
    let index = match load(services, pace).await {
        Ok(index) => index,
        Err(crate::error::ServiceError::Remna(err)) if panel_down(&err) => {
            return Ok(RunEnd::Stopped(
                base,
                format!("Remnawave не отвечает: {err}"),
            ));
        }
        Err(err) => return Err(err),
    };
    let tariffs = tariffs(services).await?;
    let mut pass = Progress {
        total: index.len() as u64,
        ..Progress::default()
    };
    let mut placed: HashSet<i64> = HashSet::new();
    for chunk in index.users().chunks(pace.batch.max(1) * 4) {
        let found = {
            let mut conn = services.store().conn().await?;
            lookups(&mut conn, chunk).await?
        };
        let mut notes: Vec<NoteRow> = Vec::new();
        for user in chunk {
            pass.done += 1;
            match decide(user, &found, &index) {
                Pull::Already => pass.same += 1,
                Pull::Relink(sub) => match bind(services, SubscriptionId(sub.id), user).await? {
                    None => {
                        pass.relinked += 1;
                        notes.push(NoteRow {
                            stage: "apply".into(),
                            client_id: Some(sub.client_id),
                            subscription_id: Some(sub.id),
                            ..note(user, "relinked", format!("привязан к подписке №{}", sub.id))
                        });
                    }
                    Some(other) => {
                        pass.failed += 1;
                        notes.push(NoteRow {
                            stage: "apply".into(),
                            ..note(user, "failed", format!("имя уже у подписки №{other}"))
                        });
                    }
                },
                Pull::Skip(code, _, details) => {
                    pass.skipped += 1;
                    notes.push(NoteRow {
                        stage: "apply".into(),
                        ..note(user, code, details)
                    });
                }
                Pull::Attach { client, .. } => {
                    let tariff = match_tariff(user, &tariffs);
                    record(
                        services,
                        user,
                        Some(client),
                        &tariff,
                        &mut pass,
                        &mut notes,
                        &mut placed,
                    )
                    .await?;
                }
                Pull::Create => {
                    let tariff = match_tariff(user, &tariffs);
                    record(
                        services,
                        user,
                        None,
                        &tariff,
                        &mut pass,
                        &mut notes,
                        &mut placed,
                    )
                    .await?;
                }
            }
        }
        let progress = merge_progress(&base, &pass);
        if ctl.save(&progress, &notes).await? {
            return Ok(RunEnd::Paused(progress));
        }
        if !pace.pause.is_zero() {
            tokio::time::sleep(pace.pause).await;
        }
    }
    Ok(RunEnd::Done(merge_progress(&base, &pass)))
}

async fn record(
    services: &Services,
    user: &RemnaUser,
    client: Option<i64>,
    tariff: &TariffMatch,
    pass: &mut Progress,
    notes: &mut Vec<NoteRow>,
    placed: &mut HashSet<i64>,
) -> ServiceResult<()> {
    if !placed.insert(user.id.0) {
        return Ok(());
    }
    match place(services, user, client, tariff).await? {
        Placed::Done {
            subscription,
            client: owner,
        } => {
            pass.attached += 1;
            if client.is_none() {
                pass.clients += 1;
            }
            let code = if client.is_none() {
                "client_created"
            } else {
                "attached"
            };
            let tariff_line = match tariff {
                TariffMatch::One { name, .. } => format!("тариф «{name}»"),
                _ => "без тарифа".into(),
            };
            notes.push(NoteRow {
                stage: "apply".into(),
                client_id: Some(owner),
                subscription_id: Some(subscription),
                ..note(
                    user,
                    code,
                    format!("подписка №{subscription}, {tariff_line}"),
                )
            });
        }
        Placed::Taken => {
            pass.failed += 1;
            notes.push(NoteRow {
                stage: "apply".into(),
                ..note(
                    user,
                    "failed",
                    "имя или пользователь уже заняты другой подпиской панели",
                )
            });
        }
    }
    Ok(())
}
