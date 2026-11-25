use super::active;
use crate::catalog::web::{address_network, is_internal};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sn_domain::ids::ClientId;
use sn_store::{Db, LockSpace, StoreError, Tx};
use std::net::{IpAddr, Ipv6Addr};
use std::sync::OnceLock;

type StoreResult<T> = Result<T, StoreError>;

pub const LIMIT_CODE: &str = "signup.limit";
pub const BUSY_CODE: &str = "signup.busy";
pub const MAX_LIMIT: u32 = 100_000;
pub const HIDDEN_KEY: &str = "antibot.site_address";
const NETWORK_BITS: u32 = 48;
const HIDDEN_MINUTES: i32 = 10;
const HIDDEN_HOURS: i32 = 24;
const KEEP_HOURS: i32 = 48;
const KEEP_REFUSALS_DAYS: i32 = 7;

const CLOUDFLARE: [&str; 22] = [
    "173.245.48.0/20",
    "103.21.244.0/22",
    "103.22.200.0/22",
    "103.31.4.0/22",
    "141.101.64.0/18",
    "108.162.192.0/18",
    "190.93.240.0/20",
    "188.114.96.0/20",
    "197.234.240.0/22",
    "198.41.128.0/17",
    "162.158.0.0/15",
    "104.16.0.0/13",
    "104.24.0.0/14",
    "172.64.0.0/13",
    "131.0.72.0/22",
    "2400:cb00::/32",
    "2606:4700::/32",
    "2803:f800::/32",
    "2405:b500::/32",
    "2405:8100::/32",
    "2a06:98c0::/29",
    "2c0f:f248::/32",
];

pub fn trusted_line() -> String {
    format!(
        "SN_TRUSTED_PROXIES=127.0.0.1,172.16.0.0/12,{}",
        CLOUDFLARE.join(",")
    )
}

fn cloudflare_networks() -> &'static [(IpAddr, u32)] {
    static NETWORKS: OnceLock<Vec<(IpAddr, u32)>> = OnceLock::new();
    NETWORKS.get_or_init(|| {
        CLOUDFLARE
            .iter()
            .filter_map(|raw| {
                let (address, bits) = raw.split_once('/')?;
                Some((address.parse().ok()?, bits.parse().ok()?))
            })
            .collect()
    })
}

pub fn cloudflare(ip: IpAddr) -> bool {
    cloudflare_networks()
        .iter()
        .any(|(network, bits)| match (network, ip.to_canonical()) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => {
                (u32::from(*network) ^ u32::from(ip)).leading_zeros() >= *bits
            }
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                (u128::from(*network) ^ u128::from(ip)).leading_zeros() >= *bits
            }
            _ => false,
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hidden {
    Cloudflare,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Known {
        address: String,
        network: Option<String>,
    },
    Hidden {
        kind: Hidden,
        address: IpAddr,
    },
    Unknown,
}

impl Origin {
    pub fn of(ip: Option<IpAddr>) -> Self {
        let Some(ip) = ip.map(|ip| ip.to_canonical()) else {
            return Origin::Unknown;
        };
        if cloudflare(ip) {
            return Origin::Hidden {
                kind: Hidden::Cloudflare,
                address: ip,
            };
        }
        if is_internal(ip) {
            return Origin::Hidden {
                kind: Hidden::Internal,
                address: ip,
            };
        }
        Origin::Known {
            address: address_network(ip),
            network: match ip {
                IpAddr::V4(_) => None,
                IpAddr::V6(v6) => Some(format!(
                    "{}/{NETWORK_BITS}",
                    Ipv6Addr::from(u128::from(v6) & (u128::MAX << (128 - NETWORK_BITS)))
                )),
            },
        }
    }

    pub fn parse(ip: Option<&str>) -> Self {
        Self::of(ip.and_then(|raw| raw.trim().parse().ok()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Password,
    Code,
    Google,
    Apple,
    Telegram,
}

impl Via {
    pub const fn as_str(self) -> &'static str {
        match self {
            Via::Password => "password",
            Via::Code => "code",
            Via::Google => "google",
            Via::Apple => "apple",
            Via::Telegram => "telegram",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Address,
    Network,
    Minute,
}

impl Refusal {
    pub const fn as_str(self) -> &'static str {
        match self {
            Refusal::Address => "address",
            Refusal::Network => "network",
            Refusal::Minute => "minute",
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Refusal::Address | Refusal::Network => LIMIT_CODE,
            Refusal::Minute => BUSY_CODE,
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            Refusal::Address | Refusal::Network => {
                "Сейчас с вашего адреса нельзя создать новый аккаунт. Зайдите через бота или попробуйте позже"
            }
            Refusal::Minute => {
                "Сейчас создаётся слишком много новых аккаунтов. Попробуйте через минуту"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    origin: Origin,
}

fn lock_id(kind: &str, value: &str) -> i64 {
    let digest = Sha256::digest(format!("{kind}\n{value}"));
    let mut head = [0u8; 8];
    head.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(head)
}

async fn in_a_day(db: &mut impl Db, column: &str, value: &str) -> StoreResult<i64> {
    sqlx::query_scalar(&format!(
        "SELECT count(*) FROM site_signups WHERE {column} = $1 AND created_at > now() - interval '24 hours'"
    ))
    .bind(value)
    .fetch_one(db.conn())
    .await
    .map_err(StoreError::from)
}

async fn in_the_minute(db: &mut impl Db) -> StoreResult<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM site_signups WHERE created_at >= date_trunc('minute', now()) \
           AND created_at < date_trunc('minute', now()) + interval '1 minute'",
    )
    .fetch_one(db.conn())
    .await
    .map_err(StoreError::from)
}

pub async fn admit(
    tx: &mut Tx,
    ip: Option<&str>,
    human: bool,
) -> StoreResult<Result<Option<Admission>, Refusal>> {
    let Some(settings) = active(tx).await? else {
        return Ok(Ok(None));
    };
    let origin = Origin::parse(ip);
    if let Origin::Known { address, network } = &origin {
        let network = network.as_deref().filter(|_| settings.site_network_day > 0);
        if let Some(network) = network {
            tx.lock(LockSpace::SiteSignup, lock_id("network", network))
                .await?;
            if in_a_day(tx, "network", network).await? >= i64::from(settings.site_network_day) {
                return Ok(Err(Refusal::Network));
            }
        }
        if settings.site_address_day > 0 {
            tx.lock(LockSpace::SiteSignup, lock_id("address", address))
                .await?;
            if in_a_day(tx, "address", address).await? >= i64::from(settings.site_address_day) {
                return Ok(Err(Refusal::Address));
            }
        }
    }
    if settings.site_minute > 0 && !human {
        tx.lock(LockSpace::SiteSignup, lock_id("minute", ""))
            .await?;
        if in_the_minute(tx).await? >= i64::from(settings.site_minute) {
            return Ok(Err(Refusal::Minute));
        }
    }
    Ok(Ok(Some(Admission { origin })))
}

pub async fn refuse_early(
    db: &mut impl Db,
    ip: Option<&str>,
    human: bool,
) -> StoreResult<Option<Refusal>> {
    let Some(settings) = active(db).await? else {
        return Ok(None);
    };
    if let Origin::Known { address, network } = Origin::parse(ip) {
        if settings.site_network_day > 0
            && let Some(network) = network
            && in_a_day(db, "network", &network).await? >= i64::from(settings.site_network_day)
        {
            return Ok(Some(Refusal::Network));
        }
        if settings.site_address_day > 0
            && in_a_day(db, "address", &address).await? >= i64::from(settings.site_address_day)
        {
            return Ok(Some(Refusal::Address));
        }
    }
    if settings.site_minute > 0
        && !human
        && in_the_minute(db).await? >= i64::from(settings.site_minute)
    {
        return Ok(Some(Refusal::Minute));
    }
    Ok(None)
}

pub async fn record(
    tx: &mut Tx,
    admission: Option<Admission>,
    client_id: ClientId,
    via: Via,
) -> StoreResult<()> {
    let Some(Admission { origin }) = admission else {
        return Ok(());
    };
    let (address, network) = match &origin {
        Origin::Known { address, network } => (Some(address.as_str()), network.as_deref()),
        Origin::Hidden { .. } | Origin::Unknown => (None, None),
    };
    sqlx::query(
        "INSERT INTO site_signups (client_id, via, address, network) VALUES ($1, $2, $3, $4)",
    )
    .bind(client_id.0)
    .bind(via.as_str())
    .bind(address)
    .bind(network)
    .execute(tx.conn())
    .await?;
    if let Origin::Hidden { kind, address } = origin {
        note_hidden(tx, kind, address).await?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HiddenAddress {
    pub kind: Hidden,
    pub address: String,
    #[serde(default = "Utc::now")]
    pub at: DateTime<Utc>,
}

pub async fn note_hidden(db: &mut impl Db, kind: Hidden, address: IpAddr) -> StoreResult<()> {
    let fresh: Option<bool> = sqlx::query_scalar(
        "SELECT updated_at > now() - make_interval(mins => $2) AND value->>'kind' = $3 \
         FROM bot_state WHERE key = $1",
    )
    .bind(HIDDEN_KEY)
    .bind(HIDDEN_MINUTES)
    .bind(match kind {
        Hidden::Cloudflare => "cloudflare",
        Hidden::Internal => "internal",
    })
    .fetch_optional(db.conn())
    .await?
    .flatten();
    if fresh == Some(true) {
        return Ok(());
    }
    db.bot_state()
        .set(
            HIDDEN_KEY,
            &serde_json::json!({ "kind": kind, "address": address.to_string() }),
        )
        .await
}

pub async fn hidden(db: &mut impl Db) -> StoreResult<Option<HiddenAddress>> {
    let row: Option<(serde_json::Value, DateTime<Utc>)> = sqlx::query_as(
        "SELECT value, updated_at FROM bot_state \
         WHERE key = $1 AND updated_at > now() - make_interval(hours => $2)",
    )
    .bind(HIDDEN_KEY)
    .bind(HIDDEN_HOURS)
    .fetch_optional(db.conn())
    .await?;
    Ok(row.and_then(|(value, at)| {
        let found: HiddenAddress = serde_json::from_value(value).ok()?;
        Some(HiddenAddress { at, ..found })
    }))
}

pub async fn refused(db: &mut impl Db, kind: &str) -> StoreResult<()> {
    sqlx::query(
        "INSERT INTO site_signup_refusals (kind, hour, hits) VALUES ($1, date_trunc('hour', now()), 1) \
         ON CONFLICT (kind, hour) DO UPDATE SET hits = site_signup_refusals.hits + 1",
    )
    .bind(kind)
    .execute(db.conn())
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Day {
    pub created: i64,
    pub address: i64,
    pub network: i64,
    pub minute: i64,
    pub asked: i64,
}

pub async fn day(db: &mut impl Db) -> StoreResult<Day> {
    let (created, address, network, minute, asked): (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM site_signups WHERE created_at > now() - interval '24 hours'), \
                coalesce(sum(hits) FILTER (WHERE kind = 'address'), 0)::bigint, \
                coalesce(sum(hits) FILTER (WHERE kind = 'network'), 0)::bigint, \
                coalesce(sum(hits) FILTER (WHERE kind = 'minute'), 0)::bigint, \
                coalesce(sum(hits) FILTER (WHERE kind = 'asked'), 0)::bigint \
         FROM site_signup_refusals WHERE hour > now() - interval '24 hours'",
    )
    .fetch_one(db.conn())
    .await?;
    Ok(Day {
        created,
        address,
        network,
        minute,
        asked,
    })
}

pub async fn sweep(db: &mut impl Db) -> StoreResult<()> {
    sqlx::query("DELETE FROM site_signups WHERE created_at < now() - make_interval(hours => $1)")
        .bind(KEEP_HOURS)
        .execute(db.conn())
        .await?;
    sqlx::query("DELETE FROM site_signup_refusals WHERE hour < now() - make_interval(days => $1)")
        .bind(KEEP_REFUSALS_DAYS)
        .execute(db.conn())
        .await?;
    Ok(())
}
