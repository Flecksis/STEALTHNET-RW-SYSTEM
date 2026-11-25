use super::captcha::{self, Captcha, Expect, Outcome, Widget};
use super::store::{Action, NewEvent};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sn_domain::ids::ClientId;
use sn_store::{Db, StoreError};

pub const MIN_SECONDS: i64 = 2;
pub const BUTTON_TTL_MINUTES: i64 = 5;
pub const CAPTCHA_TTL_MINUTES: i64 = 15;
pub const PASS_MINUTES: i64 = 10;
pub const WORK_BITS: u8 = 18;
pub const MAX_FAILURES: i32 = 5;
pub const MAX_PROOF: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Bot,
    Web,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Check {
    Button,
    Work { bits: u8 },
    Captcha(Widget),
}

impl Check {
    const fn kind(&self) -> &'static str {
        match self {
            Check::Button => "button",
            Check::Work { .. } => "work",
            Check::Captcha(_) => "captcha",
        }
    }

    const fn bits(&self) -> i16 {
        match self {
            Check::Work { bits } => *bits as i16,
            Check::Button | Check::Captcha(_) => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Challenge {
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub check: Check,
}

impl Challenge {
    pub fn captcha(&self) -> bool {
        matches!(self.check, Check::Captcha(_))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Answer<'a> {
    pub proof: Option<&'a str>,
    pub captcha: Option<&'a str>,
    pub ip: Option<&'a str>,
}

impl<'a> Answer<'a> {
    pub fn button() -> Self {
        Answer::default()
    }

    pub fn web(proof: Option<&'a str>, captcha: Option<&'a str>, ip: Option<&'a str>) -> Self {
        Answer { proof, captcha, ip }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    Confirmed,
    TooFast,
    Expired,
    Failed,
    Unavailable,
}

fn digest(token: &str) -> Vec<u8> {
    Sha256::digest(format!("antibot:{token}").as_bytes()).to_vec()
}

pub fn cdata(token: &str) -> String {
    hex::encode(Sha256::digest(format!("cdata:{token}").as_bytes()))[..32].to_string()
}

pub fn leading_zero_bits(bytes: &[u8]) -> u32 {
    let mut bits = 0;
    for byte in bytes {
        if *byte == 0 {
            bits += 8;
        } else {
            bits += byte.leading_zeros();
            break;
        }
    }
    bits
}

pub fn work_done(seed: &str, proof: &str, bits: u8) -> bool {
    let proof = proof.trim();
    if proof.is_empty() || proof.len() > MAX_PROOF || !proof.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    leading_zero_bits(&Sha256::digest(format!("{seed}:{proof}").as_bytes())) >= u32::from(bits)
}

pub fn solve(seed: &str, bits: u8) -> String {
    let mut nonce: u64 = 0;
    loop {
        let proof = nonce.to_string();
        if work_done(seed, &proof, bits) {
            return proof;
        }
        nonce += 1;
    }
}

async fn issue(
    db: &mut impl Db,
    client_id: ClientId,
    target: &str,
    check: Check,
    ttl_minutes: i64,
    details: serde_json::Value,
) -> Result<Challenge, StoreError> {
    let token = uuid::Uuid::new_v4().simple().to_string();
    let expires_at: DateTime<Utc> = sqlx::query_scalar(
        "WITH replaced AS ( \
             DELETE FROM antibot_checks \
             WHERE client_id = $1 AND target = $2 AND passed_at IS NULL \
             RETURNING kind, created_at, expires_at) \
         INSERT INTO antibot_checks (client_id, target, kind, token_hash, bits, created_at, expires_at, details) \
         VALUES ($1, $2, $3, $4, $5, \
             COALESCE((SELECT min(r.created_at) FROM replaced r \
                       WHERE r.kind = $3 AND r.expires_at > now()), now()), \
             now() + make_interval(mins => $6), $7) RETURNING expires_at",
    )
    .bind(client_id.0)
    .bind(target)
    .bind(check.kind())
    .bind(digest(&token))
    .bind(check.bits())
    .bind(i32::try_from(ttl_minutes).unwrap_or(5))
    .bind(details)
    .fetch_one(db.conn())
    .await?;
    let check = match check {
        Check::Captcha(widget) => Check::Captcha(Widget {
            cdata: Some(cdata(&token)),
            ..widget
        }),
        other => other,
    };
    Ok(Challenge {
        token,
        expires_at,
        check,
    })
}

pub async fn challenge(
    ctx: &ModuleContext,
    client_id: ClientId,
    target: &str,
    surface: Surface,
) -> Result<Challenge, ModuleError> {
    let check = match surface {
        Surface::Bot => Check::Button,
        Surface::Web => Check::Work { bits: WORK_BITS },
    };
    let mut conn = ctx.store.conn().await?;
    Ok(issue(
        &mut conn,
        client_id,
        target,
        check,
        BUTTON_TTL_MINUTES,
        serde_json::json!({}),
    )
    .await?)
}

pub async fn captcha_challenge(
    ctx: &ModuleContext,
    client_id: ClientId,
    target: &str,
    captcha: &Captcha,
    details: serde_json::Value,
) -> Result<Challenge, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    Ok(issue(
        &mut conn,
        client_id,
        target,
        Check::Captcha(captcha.widget(target, None)),
        CAPTCHA_TTL_MINUTES,
        details,
    )
    .await?)
}

pub async fn passed_recently(
    db: &mut impl Db,
    client_id: ClientId,
    target: &str,
) -> Result<bool, StoreError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM antibot_checks WHERE client_id = $1 AND target = $2 \
         AND kind = 'captcha' AND passed_at > now() - make_interval(mins => $3))",
    )
    .bind(client_id.0)
    .bind(target)
    .bind(i32::try_from(PASS_MINUTES).unwrap_or(10))
    .fetch_one(db.conn())
    .await?)
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    client_id: i64,
    target: String,
    kind: String,
    bits: i16,
    age: i64,
    expires_at: DateTime<Utc>,
    details: serde_json::Value,
}

async fn open(db: &mut impl Db, token: &str) -> Result<Option<Row>, StoreError> {
    Ok(sqlx::query_as(
        "SELECT id, client_id, target, kind, bits, \
                EXTRACT(EPOCH FROM now() - created_at)::bigint AS age, expires_at, details \
         FROM antibot_checks WHERE token_hash = $1 AND passed_at IS NULL AND expires_at > now()",
    )
    .bind(digest(token))
    .fetch_optional(db.conn())
    .await?)
}

fn usable(token: &str) -> Option<&str> {
    let token = token.trim();
    (!token.is_empty() && token.len() <= 80 && token.bytes().all(|b| b.is_ascii_hexdigit()))
        .then_some(token)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCheck {
    pub client_id: ClientId,
    pub target: String,
    pub expires_at: DateTime<Utc>,
    pub widget: Widget,
    pub variant: Option<u32>,
}

pub async fn page_check(
    ctx: &ModuleContext,
    token: &str,
) -> Result<Option<PageCheck>, ModuleError> {
    let Some(token) = usable(token) else {
        return Ok(None);
    };
    let Some(captcha) = captcha::active(ctx) else {
        return Ok(None);
    };
    let mut conn = ctx.store.conn().await?;
    let Some(row) = open(&mut conn, token).await? else {
        return Ok(None);
    };
    if row.kind != "captcha" {
        return Ok(None);
    }
    let variant = row
        .details
        .get("variant")
        .and_then(serde_json::Value::as_u64)
        .and_then(|id| u32::try_from(id).ok());
    Ok(Some(PageCheck {
        client_id: ClientId(row.client_id),
        widget: captcha.widget(&row.target, Some(cdata(token))),
        target: row.target,
        expires_at: row.expires_at,
        variant,
    }))
}

async fn fail(db: &mut impl Db, row: &Row, burn: bool) -> Result<(), StoreError> {
    sqlx::query(
        "UPDATE antibot_checks SET failures = failures + 1, \
         expires_at = CASE WHEN $2 OR failures + 1 >= $3 THEN now() ELSE expires_at END \
         WHERE id = $1",
    )
    .bind(row.id)
    .bind(burn)
    .bind(MAX_FAILURES)
    .execute(db.conn())
    .await?;
    Ok(())
}

async fn pass(
    db: &mut impl Db,
    row: &Row,
    client_id: ClientId,
    target: &str,
) -> Result<bool, StoreError> {
    let passed = sqlx::query(
        "UPDATE antibot_checks SET passed_at = now() WHERE id = $1 AND passed_at IS NULL AND expires_at > now()",
    )
    .bind(row.id)
    .execute(db.conn())
    .await?
    .rows_affected();
    if passed == 0 {
        return Ok(false);
    }
    let reasons: Option<serde_json::Value> = sqlx::query_scalar(
        "UPDATE antibot_marks SET confirmed_at = COALESCE(confirmed_at, now()) \
         WHERE client_id = $1 RETURNING reasons",
    )
    .bind(client_id.0)
    .fetch_optional(db.conn())
    .await?;
    if let Some(reasons) = reasons {
        let reasons: Vec<super::signals::Signal> =
            serde_json::from_value(reasons).unwrap_or_default();
        super::store::event(
            db,
            &NewEvent {
                client_id,
                action: Action::Confirmed,
                reasons: &reasons,
                details: serde_json::json!({ "target": target, "via": row.kind }),
                dedupe: Some(format!("confirmed:{client_id}")),
                admin: None,
            },
        )
        .await?;
    }
    Ok(true)
}

async fn refuse(ctx: &ModuleContext, row: &Row, burn: bool) -> Result<Confirmation, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    fail(&mut conn, row, burn).await?;
    Ok(Confirmation::Failed)
}

pub async fn confirm(
    ctx: &ModuleContext,
    client_id: ClientId,
    token: &str,
    target: &str,
    answer: Answer<'_>,
) -> Result<Confirmation, ModuleError> {
    let Some(token) = usable(token) else {
        return Ok(Confirmation::Expired);
    };
    let found = {
        let mut conn = ctx.store.conn().await?;
        open(&mut conn, token).await?
    };
    let Some(row) = found else {
        return Ok(Confirmation::Expired);
    };
    if row.client_id != client_id.0 || row.target != target {
        return Ok(Confirmation::Expired);
    }
    match row.kind.as_str() {
        "button" => {
            if row.age < MIN_SECONDS {
                return Ok(Confirmation::TooFast);
            }
        }
        "work" => {
            if row.age < MIN_SECONDS {
                return Ok(Confirmation::TooFast);
            }
            let bits = u8::try_from(row.bits).unwrap_or(u8::MAX);
            if !answer
                .proof
                .is_some_and(|proof| work_done(token, proof, bits))
            {
                return refuse(ctx, &row, true).await;
            }
        }
        "captcha" => {
            let Some(captcha) = captcha::active(ctx) else {
                return Ok(Confirmation::Expired);
            };
            let Some(given) = answer.captcha.map(str::trim).filter(|t| !t.is_empty()) else {
                return Ok(Confirmation::Failed);
            };
            let bound = cdata(token);
            let outcome = captcha::verify(
                &ctx.store,
                &ctx.core.captcha_endpoints(),
                &captcha,
                given,
                answer.ip,
                Expect {
                    action: &row.target,
                    cdata: Some(&bound),
                },
            )
            .await?;
            match outcome {
                Outcome::Passed => {}
                Outcome::Rejected(_) => return refuse(ctx, &row, false).await,
                Outcome::Unavailable(_) => return Ok(Confirmation::Unavailable),
            }
        }
        _ => return Ok(Confirmation::Expired),
    }
    let mut tx = ctx.store.begin().await?;
    if !pass(&mut tx, &row, client_id, target).await? {
        return Ok(Confirmation::Expired);
    }
    tx.commit().await?;
    Ok(Confirmation::Confirmed)
}
