use super::model::{LClient, LSubscription, Legacy, Source};
use chrono::Duration;
use serde::{Deserialize, Serialize};
use sn_remnawave::models::RemnaUser;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    Id,
    ShortUuid,
    Telegram,
    Email,
    Username,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Matched {
    Found(usize, Method),
    Missing,
    Ambiguous(usize),
    Shared(usize, usize),
}

pub struct RemnaIndex {
    pub users: Vec<RemnaUser>,
    by_id: HashMap<i64, usize>,
    by_short: HashMap<String, usize>,
    by_tg: HashMap<i64, Vec<usize>>,
    by_email: HashMap<String, Vec<usize>>,
    by_name: HashMap<String, Vec<usize>>,
}

fn email_key(email: Option<&str>) -> Option<String> {
    email
        .map(|e| e.trim().to_lowercase())
        .filter(|e| !e.is_empty())
}

impl RemnaIndex {
    pub fn new(users: Vec<RemnaUser>) -> Self {
        let mut index = RemnaIndex {
            by_id: HashMap::new(),
            by_short: HashMap::new(),
            by_tg: HashMap::new(),
            by_email: HashMap::new(),
            by_name: HashMap::new(),
            users: Vec::new(),
        };
        for (i, user) in users.iter().enumerate() {
            index.by_id.insert(user.id.0, i);
            index.by_short.insert(user.short_uuid.clone(), i);
            if let Some(tg) = user.telegram_id {
                index.by_tg.entry(tg).or_default().push(i);
            }
            if let Some(email) = email_key(user.email.as_deref()) {
                index.by_email.entry(email).or_default().push(i);
            }
            index
                .by_name
                .entry(user.username.to_lowercase())
                .or_default()
                .push(i);
        }
        index.users = users;
        index
    }

    pub fn len(&self) -> usize {
        self.users.len()
    }

    pub fn is_empty(&self) -> bool {
        self.users.is_empty()
    }
}

fn panel_sanitize(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        } else {
            out.extend(std::iter::repeat_n('_', c.len_utf16()));
        }
    }
    out.chars().take(36).collect()
}

fn panel_base(client: &LClient) -> String {
    if let Some(name) = client.username.as_deref().map(str::trim) {
        let name = panel_sanitize(name);
        if name.len() >= 3 {
            return name;
        }
    }
    if let Some(tg) = client.telegram_id {
        return format!("tg{tg}").chars().take(36).collect();
    }
    if let Some(email) = client
        .email_raw
        .as_deref()
        .or(client.email.as_deref())
        .map(str::trim)
        .filter(|e| !e.is_empty())
    {
        let local = email.split('@').next().unwrap_or_default().trim();
        let local = panel_sanitize(local);
        if local.len() >= 3 {
            return local;
        }
        let full = panel_sanitize(email);
        if full.len() >= 3 {
            return full;
        }
    }
    let chars: Vec<char> = client.old_id.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(8)..]
        .iter()
        .map(|c| {
            if c.is_ascii_alphanumeric() || *c == '_' || *c == '-' {
                *c
            } else {
                '0'
            }
        })
        .collect();
    panel_sanitize(&format!("user{tail}"))
}

fn with_suffix(source: Source, base: &str, ordinal: u32) -> Vec<String> {
    let cut = |name: String| name.chars().take(36).collect::<String>();
    match (source, ordinal) {
        (Source::Stealthnet, 0) => vec![cut(base.to_string()), cut(format!("{base}_0"))],
        (Source::Stealthnet, n) => vec![cut(format!("{base}_{n}"))],
        (Source::Bedolaga, 0) => vec![base.to_string(), format!("{base}_1")],
        (Source::Bedolaga, n) => vec![format!("{base}_{}", n + 1)],
        (_, 0) => vec![base.to_string()],
        (_, n) => vec![format!("{base}_{n}")],
    }
}

#[derive(Default)]
struct Names {
    exact: Vec<String>,
    loose: Vec<String>,
}

fn names_for(source: Source, client: &LClient, sub: &LSubscription) -> Names {
    if sub.gifted {
        return Names::default();
    }
    let exact = match source {
        Source::Stealthnet => with_suffix(source, &panel_base(client), sub.ordinal),
        Source::TelegramShop | Source::Chill => vec![sub.old_id.clone()],
        _ => Vec::new(),
    };
    let mut bases = Vec::new();
    if let Some(name) = &client.username {
        bases.push(name.to_lowercase());
    }
    if let Some(tg) = client.telegram_id {
        bases.push(format!("tg{tg}"));
        bases.push(format!("tg_{tg}"));
        bases.push(tg.to_string());
    }
    if let Some(email) = client.email_raw.as_deref().or(client.email.as_deref())
        && let Some((local, _)) = email.trim().split_once('@')
    {
        bases.push(local.to_lowercase());
        bases.push(panel_sanitize(local).to_lowercase());
    }
    if source == Source::Bedolaga
        && let Some(tg) = client.telegram_id
    {
        bases.push(format!("user_{tg}"));
    }
    let mut loose: Vec<String> = exact.iter().map(|n| n.to_lowercase()).collect();
    for base in bases {
        for name in with_suffix(source, &base, sub.ordinal) {
            if !loose.contains(&name) {
                loose.push(name);
            }
        }
    }
    Names { exact, loose }
}

fn name_suffix(username: &str) -> Option<u32> {
    let (_, tail) = username.rsplit_once('_')?;
    (!tail.is_empty() && tail.len() <= 4 && tail.bytes().all(|b| b.is_ascii_digit()))
        .then(|| tail.parse().ok())
        .flatten()
}

fn contradicts(user: &RemnaUser, client: &LClient, check_email: bool) -> bool {
    if user.telegram_id.is_some() && user.telegram_id != client.telegram_id {
        return true;
    }
    check_email
        && email_key(user.email.as_deref())
            .is_some_and(|e| client.email.as_deref() != Some(e.as_str()))
}

fn narrow(set: &mut Vec<usize>, keep: impl Fn(usize) -> bool) -> Option<usize> {
    let kept: Vec<usize> = set.iter().copied().filter(|i| keep(*i)).collect();
    match kept.len() {
        0 => None,
        1 => Some(kept[0]),
        _ => {
            *set = kept;
            None
        }
    }
}

#[derive(Clone, Copy)]
struct Ask<'a> {
    method: Method,
    source: Source,
    client: &'a LClient,
    sub: &'a LSubscription,
    names: &'a Names,
}

fn suffix_fits(username: &str, ordinal: u32) -> bool {
    match name_suffix(username) {
        Some(n) => n == ordinal,
        None => ordinal == 0,
    }
}

fn exact_email(user: &RemnaUser, client: &LClient) -> bool {
    client
        .email_raw
        .as_deref()
        .map(str::trim)
        .is_some_and(|raw| user.email.as_deref().map(str::trim) == Some(raw))
}

fn evidence(index: &RemnaIndex, at: usize, ask: &Ask<'_>) -> u32 {
    let user = &index.users[at];
    let mut score = 0;
    if ask.method != Method::Telegram && exact_email(user, ask.client) {
        score += 16;
    }
    if ask.names.exact.contains(&user.username) {
        score += 8;
    }
    if ask.source == Source::Stealthnet
        && !ask.sub.gifted
        && suffix_fits(&user.username, ask.sub.ordinal)
    {
        score += 4;
    }
    if let Some(expire) = ask.sub.expire_at {
        let gap = (user.expire_at - expire).abs();
        if gap <= Duration::minutes(5) {
            score += 3;
        }
        if gap <= Duration::hours(36) {
            score += 2;
        }
    }
    if ask.names.loose.contains(&user.username.to_lowercase()) {
        score += 1;
    }
    score * 2 + u32::from(!ask.sub.from_client_card())
}

fn pick(free: Vec<usize>, index: &RemnaIndex, ask: &Ask<'_>) -> Option<Result<usize, usize>> {
    let Ask {
        method,
        source,
        client,
        sub,
        names,
    } = *ask;
    let mut set = free;
    match set.len() {
        0 => return None,
        1 => return Some(Ok(set[0])),
        _ => {}
    }
    let user = |i: usize| &index.users[i];
    if method != Method::Telegram
        && let Some(i) = narrow(&mut set, |i| exact_email(user(i), client))
    {
        return Some(Ok(i));
    }
    if let Some(i) = narrow(&mut set, |i| names.exact.contains(&user(i).username)) {
        return Some(Ok(i));
    }
    if source == Source::Stealthnet
        && !sub.gifted
        && let Some(i) = narrow(&mut set, |i| suffix_fits(&user(i).username, sub.ordinal))
    {
        return Some(Ok(i));
    }
    if let Some(expire) = sub.expire_at {
        if let Some(i) = narrow(&mut set, |i| {
            (user(i).expire_at - expire).abs() <= Duration::minutes(5)
        }) {
            return Some(Ok(i));
        }
        if let Some(i) = narrow(&mut set, |i| {
            (user(i).expire_at - expire).abs() <= Duration::hours(36)
        }) {
            return Some(Ok(i));
        }
    }
    if let Some(i) = narrow(&mut set, |i| {
        names.loose.contains(&user(i).username.to_lowercase())
    }) {
        return Some(Ok(i));
    }
    Some(Err(set.len()))
}

fn open(m: &Matched) -> bool {
    matches!(m, Matched::Missing | Matched::Ambiguous(_))
}

fn raw_key(sub: &LSubscription) -> Option<String> {
    sub.remna_id
        .map(|id| id.to_string())
        .or_else(|| sub.remna_uuid.as_ref().map(|u| u.trim().to_lowercase()))
}

pub fn match_all(legacy: &Legacy, index: &RemnaIndex, blocked: &HashSet<i64>) -> Vec<Matched> {
    let source = legacy.source;
    let clients: HashMap<&str, &LClient> = legacy
        .clients
        .iter()
        .map(|c| (c.old_id.as_str(), c))
        .collect();
    let mut claimed: HashSet<usize> = index
        .users
        .iter()
        .enumerate()
        .filter(|(_, u)| blocked.contains(&u.id.0))
        .map(|(i, _)| i)
        .collect();
    let mut result = vec![Matched::Missing; legacy.subscriptions.len()];
    let mut direct: HashMap<usize, Vec<(usize, Method)>> = HashMap::new();
    for (at, sub) in legacy.subscriptions.iter().enumerate() {
        let found = sub
            .remna_id
            .and_then(|id| index.by_id.get(&id).map(|i| (*i, Method::Id)))
            .or_else(|| {
                sub.short_uuid
                    .as_ref()
                    .and_then(|s| index.by_short.get(s).map(|i| (*i, Method::ShortUuid)))
            });
        if let Some((i, how)) = found
            && !claimed.contains(&i)
        {
            direct.entry(i).or_default().push((at, how));
        }
    }
    let mut direct: Vec<(usize, Vec<(usize, Method)>)> = direct.into_iter().collect();
    direct.sort_by_key(|(_, subs)| subs[0].0);
    for (i, subs) in direct {
        let user = &index.users[i];
        let owner = |at: usize| clients.get(legacy.subscriptions[at].client_old_id.as_str());
        let best = subs
            .iter()
            .find(|(at, _)| {
                owner(*at).is_some_and(|c| {
                    user.telegram_id.is_some() && c.telegram_id == user.telegram_id
                })
            })
            .or_else(|| {
                subs.iter().find(|(at, _)| {
                    owner(*at).is_some_and(|c| {
                        email_key(user.email.as_deref()).is_some()
                            && c.email == email_key(user.email.as_deref())
                    })
                })
            })
            .unwrap_or(&subs[0]);
        claimed.insert(i);
        result[best.0] = Matched::Found(i, best.1);
    }
    loop {
        for m in result.iter_mut() {
            if matches!(m, Matched::Ambiguous(_)) {
                *m = Matched::Missing;
            }
        }
        let mut progress = false;
        for method in [Method::Telegram, Method::Email, Method::Username] {
            let mut wanted: Vec<(usize, Vec<usize>, Option<Names>)> = Vec::new();
            let mut wants: HashMap<usize, Vec<usize>> = HashMap::new();
            for (at, sub) in legacy.subscriptions.iter().enumerate() {
                if !open(&result[at]) {
                    continue;
                }
                let Some(client) = clients.get(sub.client_old_id.as_str()) else {
                    continue;
                };
                let mut names = None;
                let candidates: Vec<usize> = match method {
                    Method::Telegram => client
                        .telegram_id
                        .and_then(|tg| index.by_tg.get(&tg).cloned())
                        .unwrap_or_default(),
                    Method::Email => client
                        .email
                        .as_ref()
                        .and_then(|e| index.by_email.get(e).cloned())
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|i| !contradicts(&index.users[*i], client, false))
                        .collect(),
                    _ => {
                        let all = names.insert(names_for(source, client, sub));
                        let mut found: Vec<usize> = Vec::new();
                        for name in &all.loose {
                            for i in index.by_name.get(name).into_iter().flatten() {
                                if !found.contains(i)
                                    && !contradicts(&index.users[*i], client, true)
                                {
                                    found.push(*i);
                                }
                            }
                        }
                        found
                    }
                };
                let free: Vec<usize> = candidates
                    .into_iter()
                    .filter(|i| !claimed.contains(i))
                    .collect();
                if free.is_empty() {
                    continue;
                }
                for i in &free {
                    wants.entry(*i).or_default().push(at);
                }
                wanted.push((at, free, names));
            }
            for (at, free, names) in wanted {
                let sub = &legacy.subscriptions[at];
                let Some(client) = clients.get(sub.client_old_id.as_str()).copied() else {
                    continue;
                };
                let free: Vec<usize> = free.into_iter().filter(|i| !claimed.contains(i)).collect();
                if free.is_empty() {
                    continue;
                }
                let count = free.len();
                let names = names.unwrap_or_else(|| {
                    if count > 1 {
                        names_for(source, client, sub)
                    } else {
                        Names::default()
                    }
                });
                let ask = Ask {
                    method,
                    source,
                    client,
                    sub,
                    names: &names,
                };
                match pick(free, index, &ask) {
                    Some(Ok(i)) => {
                        let rivals: Vec<usize> = wants
                            .get(&i)
                            .into_iter()
                            .flatten()
                            .copied()
                            .filter(|r| *r != at && open(&result[*r]))
                            .collect();
                        let fair = rivals.is_empty() || {
                            let fresh;
                            let own = if count > 1 {
                                &names
                            } else {
                                fresh = names_for(source, client, sub);
                                &fresh
                            };
                            let mine = evidence(index, i, &Ask { names: own, ..ask });
                            rivals.iter().all(|r| {
                                let rival = &legacy.subscriptions[*r];
                                clients.get(rival.client_old_id.as_str()).is_none_or(|rc| {
                                    let theirs = names_for(source, rc, rival);
                                    let claim = Ask {
                                        method,
                                        source,
                                        client: rc,
                                        sub: rival,
                                        names: &theirs,
                                    };
                                    evidence(index, i, &claim) < mine
                                })
                            })
                        };
                        if fair {
                            claimed.insert(i);
                            result[at] = Matched::Found(i, method);
                            progress = true;
                        } else {
                            result[at] = Matched::Ambiguous(count);
                        }
                    }
                    Some(Err(n)) => result[at] = Matched::Ambiguous(n),
                    None => {}
                }
            }
        }
        if !progress {
            break;
        }
    }
    let mut holder: HashMap<String, (usize, usize)> = HashMap::new();
    for (at, sub) in legacy.subscriptions.iter().enumerate() {
        if let (Matched::Found(i, _), Some(key)) = (&result[at], raw_key(sub)) {
            holder.entry(key).or_insert((*i, at));
        }
    }
    for (at, sub) in legacy.subscriptions.iter().enumerate() {
        if matches!(result[at], Matched::Missing | Matched::Ambiguous(_))
            && let Some((i, other)) = raw_key(sub).and_then(|k| holder.get(&k))
        {
            result[at] = Matched::Shared(*i, *other);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy::model::*;
    use chrono::{TimeZone, Utc};
    use sn_domain::client::Locale;
    use sn_domain::ids::RemnaUserId;
    use sn_domain::money::Currency;
    use sn_domain::subscription::SubscriptionKind;

    fn user(
        id: i64,
        short: &str,
        name: &str,
        tg: Option<i64>,
        email: Option<&str>,
        day: u32,
    ) -> RemnaUser {
        let mut value = serde_json::json!({
            "id": id,
            "shortUuid": short,
            "username": name,
            "status": "ACTIVE",
            "trafficLimitBytes": 0,
            "trafficLimitStrategy": "NO_RESET",
            "expireAt": Utc.with_ymd_and_hms(2026, 12, day, 0, 0, 0).unwrap(),
            "createdAt": Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            "updatedAt": Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        });
        value["telegramId"] = serde_json::json!(tg);
        value["email"] = serde_json::json!(email);
        let parsed: RemnaUser = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.id, RemnaUserId(id));
        parsed
    }

    fn client(id: &str, tg: Option<i64>, email: Option<&str>, username: Option<&str>) -> LClient {
        LClient {
            old_id: id.into(),
            telegram_id: tg,
            username: username.map(Into::into),
            first_name: None,
            last_name: None,
            email: email.map(|e| e.trim().to_lowercase()),
            email_raw: email.map(Into::into),
            email_verified: true,
            password_hash: None,
            google_id: None,
            apple_id: None,
            locale: Locale::Ru,
            blocked: false,
            block_reason: None,
            bot_unreachable: false,
            balance_minor: 0,
            balance_mixed: false,
            referral_code: None,
            referrer_old_id: None,
            referral_percent: None,
            utm: None,
            trial_used: false,
            created_at: None,
        }
    }

    fn sub(
        id: &str,
        client: &str,
        remna: Option<i64>,
        short: Option<&str>,
        ordinal: u32,
        day: u32,
    ) -> LSubscription {
        LSubscription {
            old_id: id.into(),
            client_old_id: client.into(),
            tariff_old_id: None,
            kind: SubscriptionKind::Paid,
            remna_id: remna,
            remna_uuid: None,
            short_uuid: short.map(Into::into),
            expire_at: Some(Utc.with_ymd_and_hms(2026, 12, day, 0, 0, 0).unwrap()),
            extra_devices: 0,
            autorenew: false,
            autorenew_days: None,
            autorenew_period_old_id: None,
            ordinal,
            gifted: false,
            created_at: None,
        }
    }

    #[test]
    fn подписки_находятся_по_всем_признакам_без_двойной_привязки() {
        let index = RemnaIndex::new(vec![
            user(1, "aaaa1111", "ann", Some(100), None, 1),
            user(2, "bbbb2222", "tg200", Some(200), None, 2),
            user(3, "cccc3333", "boris", None, Some("boris@example.com"), 3),
            user(4, "dddd4444", "tg300", Some(300), None, 4),
            user(5, "eeee5555", "tg300_2", Some(300), None, 5),
            user(6, "ffff6666", "mila", None, None, 6),
            user(7, "gggg7777", "x1", Some(400), None, 7),
            user(8, "hhhh8888", "x2", Some(400), None, 7),
            user(9, "iiii9999", "foreign", Some(900), None, 9),
        ]);
        let mut legacy = Legacy::new(Source::Stealthnet, Currency::Usd);
        legacy.clients = vec![
            client("c1", Some(100), None, Some("ann")),
            client("c2", Some(200), None, None),
            client("c3", None, Some("boris@example.com"), None),
            client("c4", Some(300), None, None),
            client("c5", None, None, Some("mila")),
            client("c6", Some(400), None, None),
            client("c7", Some(900), None, None),
        ];
        legacy.subscriptions = vec![
            sub("s1", "c1", Some(1), None, 0, 1),
            sub("s2", "c2", None, Some("bbbb2222"), 0, 2),
            sub("s3", "c3", None, None, 0, 3),
            sub("s4", "c4", None, None, 0, 4),
            sub("s5", "c4", None, None, 1, 5),
            sub("s6", "c5", None, None, 0, 6),
            sub("s7", "c6", None, None, 0, 7),
            sub("s8", "c1", Some(1), None, 1, 1),
            sub("s9", "c7", Some(77), None, 0, 9),
        ];
        let blocked: HashSet<i64> = [9].into_iter().collect();
        let matched = match_all(&legacy, &index, &blocked);
        assert_eq!(matched[0], Matched::Found(0, Method::Id));
        assert_eq!(matched[1], Matched::Found(1, Method::ShortUuid));
        assert_eq!(matched[2], Matched::Found(2, Method::Email));
        assert_eq!(matched[3], Matched::Found(3, Method::Telegram));
        assert_eq!(matched[4], Matched::Found(4, Method::Telegram));
        assert_eq!(matched[5], Matched::Found(5, Method::Username));
        assert_eq!(matched[6], Matched::Ambiguous(2));
        assert_eq!(matched[7], Matched::Shared(0, 0));
        assert_eq!(matched[8], Matched::Missing);
    }

    fn found(matched: &[Matched], index: &RemnaIndex) -> Vec<String> {
        matched
            .iter()
            .map(|m| match m {
                Matched::Found(i, _) => index.users[*i].username.clone(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn подписки_с_одним_сроком_различаются_по_именам_старой_панели() {
        let index = RemnaIndex::new(vec![
            user(1, "s1", "Alex_2", Some(500), None, 10),
            user(2, "s2", "Alex", Some(500), None, 10),
            user(3, "s3", "Alex_1", Some(500), None, 10),
            user(4, "s4", "oldname_1", Some(501), None, 11),
            user(5, "s5", "oldname", Some(501), None, 11),
        ]);
        let mut legacy = Legacy::new(Source::Stealthnet, Currency::Rub);
        legacy.clients = vec![
            client("c1", Some(500), None, Some("Alex")),
            client("c2", Some(501), None, Some("newname")),
        ];
        legacy.subscriptions = vec![
            sub("s0", "c1", None, None, 0, 10),
            sub("s1", "c1", None, None, 1, 10),
            sub("s2", "c1", None, None, 2, 10),
            sub("t1", "c2", None, None, 1, 11),
            sub("t0", "c2", None, None, 0, 11),
        ];
        let matched = match_all(&legacy, &index, &HashSet::new());
        assert_eq!(
            found(&matched, &index),
            ["Alex", "Alex_1", "Alex_2", "oldname_1", "oldname"]
        );
    }

    #[test]
    fn по_имени_не_привязываем_пользователя_с_чужим_telegram_или_почтой() {
        let index = RemnaIndex::new(vec![
            user(1, "s1", "kate", Some(601), None, 1),
            user(2, "s2", "mike", None, Some("other@example.com"), 2),
            user(3, "s3", "olga", None, Some("olga@example.com"), 3),
        ]);
        let mut legacy = Legacy::new(Source::Stealthnet, Currency::Rub);
        legacy.clients = vec![
            client("c1", Some(600), None, Some("kate")),
            client("c2", None, Some("mike@example.com"), None),
            client("c3", None, None, Some("olga")),
        ];
        legacy.subscriptions = vec![
            sub("s1", "c1", None, None, 0, 1),
            sub("s2", "c2", None, None, 0, 2),
            sub("s3", "c3", None, None, 0, 3),
        ];
        let matched = match_all(&legacy, &index, &HashSet::new());
        assert_eq!(
            matched,
            [Matched::Missing, Matched::Missing, Matched::Missing]
        );
    }

    #[test]
    fn почты_в_разном_регистре_разводятся_по_точному_написанию() {
        let index = RemnaIndex::new(vec![
            user(1, "s1", "anna", None, Some("anna@example.com"), 5),
            user(2, "s2", "x2", None, Some("Anna@Example.com"), 5),
        ]);
        let mut legacy = Legacy::new(Source::Stealthnet, Currency::Rub);
        legacy.clients = vec![
            client("c1", None, Some("Anna@Example.com"), None),
            client("c2", None, Some("anna@example.com"), None),
        ];
        legacy.subscriptions = vec![
            sub("s1", "c1", None, None, 0, 5),
            sub("s2", "c2", None, None, 0, 5),
        ];
        let matched = match_all(&legacy, &index, &HashSet::new());
        assert_eq!(found(&matched, &index), ["x2", "anna"]);
    }

    #[test]
    fn общая_подписка_достаётся_одному_клиенту_второму_объясняем() {
        let index = RemnaIndex::new(vec![
            user(9, "s9", "tg701", Some(701), None, 4),
            user(10, "s10", "tg800", Some(800), None, 4),
        ]);
        let mut legacy = Legacy::new(Source::Stealthnet, Currency::Rub);
        legacy.clients = vec![
            client("c1", Some(700), None, None),
            client("c2", Some(701), None, None),
            client("c3", Some(800), None, None),
            client("c4", Some(801), None, None),
        ];
        let shared_uuid = |id: &str, owner: &str| {
            let mut s = sub(id, owner, None, None, 0, 4);
            s.remna_uuid = Some("6f1c2f3e-0000-4000-8000-000000000001".into());
            s
        };
        legacy.subscriptions = vec![
            sub("s1", "c1", Some(9), None, 0, 4),
            sub("s2", "c2", Some(9), None, 0, 4),
            shared_uuid("s3", "c4"),
            shared_uuid("s4", "c3"),
        ];
        let matched = match_all(&legacy, &index, &HashSet::new());
        assert_eq!(matched[0], Matched::Shared(0, 1));
        assert_eq!(matched[1], Matched::Found(0, Method::Id));
        assert_eq!(matched[2], Matched::Shared(1, 3));
        assert_eq!(matched[3], Matched::Found(1, Method::Telegram));
    }

    #[test]
    fn подписка_находится_когда_остальные_уже_разобраны() {
        let index = RemnaIndex::new(vec![
            user(1, "s1", "gifter_3", Some(900), None, 12),
            user(2, "s2", "rita", Some(900), None, 12),
        ]);
        let mut legacy = Legacy::new(Source::Stealthnet, Currency::Rub);
        legacy.clients = vec![client("c1", Some(900), None, Some("rita"))];
        legacy.subscriptions = vec![
            sub("gift", "c1", None, None, 1, 12),
            sub("main", "c1", None, None, 0, 12),
        ];
        let matched = match_all(&legacy, &index, &HashSet::new());
        assert_eq!(found(&matched, &index), ["gifter_3", "rita"]);
    }

    #[test]
    fn подписка_без_пользователя_не_забирает_пользователя_соседней() {
        let index = RemnaIndex::new(vec![
            user(1, "s1", "Ivan", Some(700), None, 10),
            user(3, "s3", "Ivan_2", Some(700), None, 10),
            user(4, "s4", "mail", None, Some("same@example.com"), 11),
        ]);
        let mut legacy = Legacy::new(Source::Stealthnet, Currency::Rub);
        legacy.clients = vec![
            client("c1", Some(700), None, Some("Ivan")),
            client("c2", None, Some("same@example.com"), None),
            client("c3", None, Some("same@example.com"), None),
        ];
        let mut pseudo = sub("p2", "c2", None, None, 0, 11);
        pseudo.expire_at = None;
        legacy.subscriptions = vec![
            sub("s0", "c1", None, None, 0, 10),
            sub("s1", "c1", None, None, 1, 10),
            sub("s2", "c1", None, None, 2, 10),
            pseudo,
            sub("m3", "c3", None, None, 0, 11),
        ];
        let matched = match_all(&legacy, &index, &HashSet::new());
        assert_eq!(
            found(&matched, &index),
            ["Ivan", "Missing", "Ivan_2", "Missing", "mail"]
        );
    }

    #[test]
    fn подаренные_подписки_различаются_по_сроку_а_не_по_имени_дарителя() {
        let mut first = user(1, "s1", "gifter_1", Some(800), None, 12);
        first.expire_at += chrono::Duration::hours(20);
        let index = RemnaIndex::new(vec![
            first,
            user(2, "s2", "rita", Some(800), None, 9),
            user(3, "s3", "giver_2", Some(800), None, 12),
        ]);
        let mut legacy = Legacy::new(Source::Stealthnet, Currency::Rub);
        legacy.clients = vec![client("c1", Some(800), None, Some("rita"))];
        let gift = |id: &str, ordinal: u32, shift: i64| {
            let mut s = sub(id, "c1", None, None, ordinal, 12);
            s.gifted = true;
            s.expire_at = s.expire_at.map(|at| at + chrono::Duration::hours(shift));
            s
        };
        legacy.subscriptions = vec![
            gift("g1", 1, 0),
            gift("g2", 2, 20),
            sub("own", "c1", None, None, 0, 9),
        ];
        let matched = match_all(&legacy, &index, &HashSet::new());
        assert_eq!(found(&matched, &index), ["giver_2", "gifter_1", "rita"]);
    }

    #[test]
    fn подписка_из_карточки_клиента_уступает_настоящей_при_равных_признаках() {
        let index = RemnaIndex::new(vec![user(1, "s1", "Same", None, None, 3)]);
        let mut legacy = Legacy::new(Source::Stealthnet, Currency::Rub);
        legacy.clients = vec![
            client("c1", None, None, Some("Same")),
            client("c2", None, None, Some("Same")),
        ];
        let mut card = sub("client:c2", "c2", None, None, 0, 3);
        card.expire_at = None;
        let mut row = sub("r1", "c1", None, None, 0, 3);
        row.expire_at = None;
        legacy.subscriptions = vec![card, row];
        let matched = match_all(&legacy, &index, &HashSet::new());
        assert_eq!(found(&matched, &index), ["Missing", "Same"]);
    }

    #[test]
    fn имя_строится_как_в_старой_панели() {
        let mut c = client("cmabcdefgh12345678", None, None, None);
        assert_eq!(panel_base(&c), "user12345678");
        c.email_raw = Some(" Ivan.Petrov+vpn@Example.com ".into());
        assert_eq!(panel_base(&c), "Ivan_Petrov_vpn");
        c.email_raw = Some("ab@x.io".into());
        assert_eq!(panel_base(&c), "ab_x_io");
        c.telegram_id = Some(42);
        assert_eq!(panel_base(&c), "tg42");
        c.username = Some("Пётр".into());
        assert_eq!(panel_base(&c), "____");
        c.username = Some("ab".into());
        assert_eq!(panel_base(&c), "tg42");
        assert_eq!(
            with_suffix(Source::Stealthnet, "tg42", 0),
            ["tg42", "tg42_0"]
        );
        assert_eq!(with_suffix(Source::Stealthnet, "tg42", 3), ["tg42_3"]);
    }
}
