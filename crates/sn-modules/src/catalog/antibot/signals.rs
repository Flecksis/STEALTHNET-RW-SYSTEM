use serde::{Deserialize, Serialize};

pub const THRESHOLD: i32 = 3;
pub const BURST_MINUTES: i64 = 10;
pub const BURST_SPREAD: i64 = 2_000_000;
pub const MIN_HISTORY: i64 = 20;
pub const SESSION_GRACE_MINUTES: i64 = 10;
pub const SIGNUP_WINDOW_HOURS: i64 = 24;
pub const SHARED_SESSIONS_DAYS: i64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    YoungAccount,
    NoUsername,
    Burst,
    IpLimit,
    DisposableEmail,
    EmailAlias,
    SelfInvite,
    MassInvites,
}

impl Signal {
    pub const ALL: [Signal; 8] = [
        Signal::YoungAccount,
        Signal::NoUsername,
        Signal::Burst,
        Signal::IpLimit,
        Signal::DisposableEmail,
        Signal::EmailAlias,
        Signal::SelfInvite,
        Signal::MassInvites,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Signal::YoungAccount => "young_account",
            Signal::NoUsername => "no_username",
            Signal::Burst => "burst",
            Signal::IpLimit => "ip_limit",
            Signal::DisposableEmail => "disposable_email",
            Signal::EmailAlias => "email_alias",
            Signal::SelfInvite => "self_invite",
            Signal::MassInvites => "mass_invites",
        }
    }

    pub const fn title_ru(self) -> &'static str {
        match self {
            Signal::YoungAccount => "новый аккаунт Telegram",
            Signal::NoUsername => "нет @username",
            Signal::Burst => "серия регистраций с соседними номерами Telegram",
            Signal::IpLimit => "много регистраций с одного адреса",
            Signal::DisposableEmail => "одноразовая почта",
            Signal::EmailAlias => "почта — вариант адреса другого клиента",
            Signal::SelfInvite => "пригласил сам себя",
            Signal::MassInvites => "массовые приглашения с новых аккаунтов",
        }
    }

    pub const fn weight(self) -> i32 {
        match self {
            Signal::YoungAccount => 2,
            Signal::NoUsername => 1,
            _ => THRESHOLD,
        }
    }

    pub const fn from_telegram(self) -> bool {
        matches!(
            self,
            Signal::YoungAccount | Signal::NoUsername | Signal::Burst
        )
    }
}

pub fn score(signals: &[Signal], premium: bool) -> i32 {
    signals
        .iter()
        .map(|s| {
            if premium && s.from_telegram() {
                0
            } else {
                s.weight()
            }
        })
        .sum()
}

pub fn suspicious(signals: &[Signal], premium: bool) -> bool {
    score(signals, premium) >= THRESHOLD
}

pub fn merge(into: &mut Vec<Signal>, more: &[Signal]) {
    for signal in more {
        if !into.contains(signal) {
            into.push(*signal);
        }
    }
    into.sort();
}

pub fn titles(signals: &[Signal]) -> String {
    signals
        .iter()
        .map(|s| s.title_ru())
        .collect::<Vec<_>>()
        .join(", ")
}

pub const DISPOSABLE: &[&str] = &[
    "10minemail.com",
    "10minutemail.com",
    "10minutemail.net",
    "1secmail.com",
    "1secmail.net",
    "1secmail.org",
    "burnermail.io",
    "discard.email",
    "discardmail.com",
    "dispostable.com",
    "dropmail.me",
    "emailfake.com",
    "emailondeck.com",
    "emlpro.com",
    "emltmp.com",
    "fakeinbox.com",
    "generator.email",
    "getnada.com",
    "grr.la",
    "guerrillamail.biz",
    "guerrillamail.com",
    "guerrillamail.de",
    "guerrillamail.net",
    "guerrillamail.org",
    "guerrillamailblock.com",
    "harakirimail.com",
    "inboxkitten.com",
    "mail.gw",
    "mail.tm",
    "mailcatch.com",
    "maildrop.cc",
    "mailinator.com",
    "mailinator.net",
    "mailnesia.com",
    "mailpoof.com",
    "mintemail.com",
    "moakt.com",
    "mohmal.com",
    "mytemp.email",
    "nada.email",
    "sharklasers.com",
    "spamgourmet.com",
    "temp-mail.io",
    "temp-mail.org",
    "tempail.com",
    "tempinbox.com",
    "tempmail.dev",
    "tempmail.net",
    "tempmailo.com",
    "tempr.email",
    "throwawaymail.com",
    "tmpmail.net",
    "tmpmail.org",
    "trashmail.com",
    "trashmail.de",
    "trashmail.net",
    "yopmail.com",
    "yopmail.fr",
    "yopmail.net",
];

pub fn email_domain(email: &str) -> Option<String> {
    let (_, domain) = email.trim().rsplit_once('@')?;
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    (!domain.is_empty()).then_some(domain)
}

fn listed(domain: &str, list: impl IntoIterator<Item = impl AsRef<str>>) -> bool {
    list.into_iter().any(|entry| {
        let entry = entry.as_ref();
        domain == entry
            || domain
                .strip_suffix(entry)
                .is_some_and(|head| head.ends_with('.'))
    })
}

pub fn disposable(email: &str, extra: &[String]) -> bool {
    let Some(domain) = email_domain(email) else {
        return false;
    };
    listed(&domain, DISPOSABLE.iter()) || listed(&domain, extra.iter())
}

pub fn parse_domains(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = raw
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';'))
        .map(|part| {
            part.trim()
                .trim_start_matches('@')
                .trim_matches('.')
                .to_ascii_lowercase()
        })
        .filter(|part| {
            part.contains('.')
                && part.len() <= 253
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

pub fn email_key(email: &str) -> String {
    let lower = email.trim().to_lowercase();
    let mut parts = lower.splitn(2, '@');
    let local = parts.next().unwrap_or_default();
    let domain = parts
        .next()
        .map(|d| d.split('@').next().unwrap_or_default())
        .unwrap_or_default();
    let local = match local.split('+').next() {
        Some(head) if !head.is_empty() => head,
        _ => local,
    };
    let domain = match domain {
        "googlemail.com" => "gmail.com",
        "ya.ru" | "yandex.com" | "yandex.by" | "yandex.kz" | "yandex.ua" => "yandex.ru",
        other => other,
    };
    let local = match domain {
        "gmail.com" => local.replace('.', ""),
        "yandex.ru" => local.replace('.', "-"),
        _ => local.to_string(),
    };
    format!("{local}@{domain}")
}
