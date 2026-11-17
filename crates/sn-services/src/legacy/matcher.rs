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
}

pub struct RemnaIndex {
    pub users: Vec<RemnaUser>,
    by_id: HashMap<i64, usize>,
    by_short: HashMap<String, usize>,
    by_tg: HashMap<i64, Vec<usize>>,
    by_email: HashMap<String, Vec<usize>>,
    by_name: HashMap<String, usize>,
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
            if let Some(email) = user
                .email
                .as_deref()
                .map(|e| e.trim().to_lowercase())
                .filter(|e| !e.is_empty())
            {
                index.by_email.entry(email).or_default().push(i);
            }
            index.by_name.insert(user.username.to_lowercase(), i);
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

fn username_candidates(source: Source, client: &LClient, sub: &LSubscription) -> Vec<String> {
    let mut bases = Vec::new();
    if let Some(name) = &client.username {
        bases.push(name.to_lowercase());
    }
    if let Some(tg) = client.telegram_id {
        bases.push(format!("tg{tg}"));
        bases.push(format!("tg_{tg}"));
        bases.push(tg.to_string());
    }
    if let Some(email) = &client.email
        && let Some((local, _)) = email.split_once('@')
    {
        bases.push(local.to_lowercase());
    }
    if source == Source::Bedolaga
        && let Some(tg) = client.telegram_id
    {
        bases.push(format!("user_{tg}"));
    }
    let mut out = Vec::new();
    for base in bases {
        if sub.ordinal == 0 {
            out.push(base.clone());
        }
        out.push(format!("{base}_{}", sub.ordinal + 1));
    }
    out
}

fn pick(
    candidates: &[usize],
    claimed: &HashSet<usize>,
    index: &RemnaIndex,
    sub: &LSubscription,
    names: &[String],
) -> Option<Result<usize, usize>> {
    let free: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|i| !claimed.contains(i))
        .collect();
    match free.len() {
        0 => None,
        1 => Some(Ok(free[0])),
        n => {
            if let Some(expire) = sub.expire_at {
                let close: Vec<usize> = free
                    .iter()
                    .copied()
                    .filter(|i| (index.users[*i].expire_at - expire).abs() <= Duration::hours(36))
                    .collect();
                if close.len() == 1 {
                    return Some(Ok(close[0]));
                }
            }
            let named: Vec<usize> = free
                .iter()
                .copied()
                .filter(|i| names.contains(&index.users[*i].username.to_lowercase()))
                .collect();
            if named.len() == 1 {
                return Some(Ok(named[0]));
            }
            Some(Err(n))
        }
    }
}

pub fn match_all(legacy: &Legacy, index: &RemnaIndex, blocked: &HashSet<i64>) -> Vec<Matched> {
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
    for (at, sub) in legacy.subscriptions.iter().enumerate() {
        let direct = sub
            .remna_id
            .and_then(|id| index.by_id.get(&id).map(|i| (*i, Method::Id)))
            .or_else(|| {
                sub.short_uuid
                    .as_ref()
                    .and_then(|s| index.by_short.get(s).map(|i| (*i, Method::ShortUuid)))
            });
        if let Some((i, how)) = direct
            && claimed.insert(i)
        {
            result[at] = Matched::Found(i, how);
        }
    }
    for method in [Method::Telegram, Method::Email, Method::Username] {
        for (at, sub) in legacy.subscriptions.iter().enumerate() {
            if !matches!(result[at], Matched::Missing | Matched::Ambiguous(_)) {
                continue;
            }
            let Some(client) = clients.get(sub.client_old_id.as_str()) else {
                continue;
            };
            let names = username_candidates(legacy.source, client, sub);
            let candidates: Vec<usize> = match method {
                Method::Telegram => client
                    .telegram_id
                    .and_then(|tg| index.by_tg.get(&tg).cloned())
                    .unwrap_or_default(),
                Method::Email => client
                    .email
                    .as_ref()
                    .and_then(|e| index.by_email.get(e).cloned())
                    .unwrap_or_default(),
                _ => names
                    .iter()
                    .filter_map(|n| index.by_name.get(n).copied())
                    .collect(),
            };
            match pick(&candidates, &claimed, index, sub, &names) {
                Some(Ok(i)) => {
                    claimed.insert(i);
                    result[at] = Matched::Found(i, method);
                }
                Some(Err(n)) => result[at] = Matched::Ambiguous(n),
                None => {}
            }
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
            email: email.map(Into::into),
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
        assert_eq!(matched[7], Matched::Missing);
        assert_eq!(matched[8], Matched::Missing);
    }
}
