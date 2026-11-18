use super::dump::qualified;
use crate::error::ServiceResult;
use serde::Serialize;
use sn_store::{SealedSecret, SecretBox};
use sqlx::PgConnection;

pub const KEEP_PREFIX: &str = "backups.";

pub const COLUMNS: [(&str, &str); 5] = [
    ("secrets", "nonce"),
    ("admins", "totp_nonce"),
    ("client_credentials", "totp_nonce"),
    ("saved_payment_methods", "token_nonce"),
    ("remna_snapshots", "keys_nonce"),
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Outcome {
    pub mode: &'static str,
    pub secrets: Vec<String>,
    pub admins_totp: Vec<String>,
    pub clients_totp: u64,
    pub cards: u64,
    pub remna_keys: u64,
}

impl Outcome {
    pub fn touched(&self) -> bool {
        !self.secrets.is_empty()
            || !self.admins_totp.is_empty()
            || self.clients_totp > 0
            || self.cards > 0
            || self.remna_keys > 0
    }
}

type CardRow = (i64, i64, String, Vec<u8>, Vec<u8>, bool);

pub enum Plan<'a> {
    Keep,
    Rekey {
        from: &'a SecretBox,
        to: &'a SecretBox,
    },
    Clear,
}

async fn table_exists(conn: &mut PgConnection, schema: &str, table: &str) -> ServiceResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind IN ('r', 'p'))",
    )
    .bind(schema)
    .bind(table)
    .fetch_one(&mut *conn)
    .await?)
}

fn reseal(
    plan: &Plan<'_>,
    context: &str,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
) -> Option<SealedSecret> {
    let Plan::Rekey { from, to } = plan else {
        return None;
    };
    let plain = from
        .open(context, &SealedSecret { nonce, ciphertext })
        .ok()?;
    to.seal(context, &plain).ok()
}

pub async fn apply(
    conn: &mut PgConnection,
    schema: &str,
    plan: Plan<'_>,
) -> ServiceResult<Outcome> {
    let mut outcome = Outcome {
        mode: match plan {
            Plan::Keep => "kept",
            Plan::Rekey { .. } => "rekeyed",
            Plan::Clear => "cleared",
        },
        ..Outcome::default()
    };
    if matches!(plan, Plan::Keep) {
        return Ok(outcome);
    }
    if table_exists(conn, schema, "secrets").await? {
        let rows: Vec<(String, Vec<u8>, Vec<u8>)> = sqlx::query_as(&format!(
            "SELECT key, nonce, ciphertext FROM {} WHERE NOT starts_with(key, $1) ORDER BY key",
            qualified(schema, "secrets")
        ))
        .bind(KEEP_PREFIX)
        .fetch_all(&mut *conn)
        .await?;
        for (key, nonce, ciphertext) in rows {
            match reseal(&plan, &key, nonce, ciphertext) {
                Some(sealed) => {
                    sqlx::query(&format!(
                        "UPDATE {} SET nonce = $2, ciphertext = $3 WHERE key = $1",
                        qualified(schema, "secrets")
                    ))
                    .bind(&key)
                    .bind(&sealed.nonce)
                    .bind(&sealed.ciphertext)
                    .execute(&mut *conn)
                    .await?;
                }
                None => {
                    sqlx::query(&format!(
                        "DELETE FROM {} WHERE key = $1",
                        qualified(schema, "secrets")
                    ))
                    .bind(&key)
                    .execute(&mut *conn)
                    .await?;
                    outcome.secrets.push(key);
                }
            }
        }
    }
    if table_exists(conn, schema, "admins").await? {
        let rows: Vec<(i64, String, Vec<u8>, Vec<u8>)> = sqlx::query_as(&format!(
            "SELECT id, email, totp_nonce, totp_secret FROM {} \
             WHERE totp_secret IS NOT NULL AND totp_nonce IS NOT NULL ORDER BY id",
            qualified(schema, "admins")
        ))
        .fetch_all(&mut *conn)
        .await?;
        for (id, email, nonce, ciphertext) in rows {
            match reseal(
                &plan,
                &crate::auth::admin_totp_context(sn_domain::ids::AdminId(id)),
                nonce,
                ciphertext,
            ) {
                Some(sealed) => {
                    sqlx::query(&format!(
                        "UPDATE {} SET totp_nonce = $2, totp_secret = $3 WHERE id = $1",
                        qualified(schema, "admins")
                    ))
                    .bind(id)
                    .bind(&sealed.nonce)
                    .bind(&sealed.ciphertext)
                    .execute(&mut *conn)
                    .await?;
                }
                None => {
                    sqlx::query(&format!(
                        "UPDATE {} SET totp_nonce = NULL, totp_secret = NULL WHERE id = $1",
                        qualified(schema, "admins")
                    ))
                    .bind(id)
                    .execute(&mut *conn)
                    .await?;
                    outcome.admins_totp.push(email);
                }
            }
        }
    }
    if table_exists(conn, schema, "client_credentials").await? {
        let rows: Vec<(i64, Vec<u8>, Vec<u8>)> = sqlx::query_as(&format!(
            "SELECT client_id, totp_nonce, totp_secret FROM {} \
             WHERE totp_secret IS NOT NULL AND totp_nonce IS NOT NULL ORDER BY client_id",
            qualified(schema, "client_credentials")
        ))
        .fetch_all(&mut *conn)
        .await?;
        for (client_id, nonce, ciphertext) in rows {
            let context =
                crate::two_factor::client_totp_context(sn_domain::ids::ClientId(client_id));
            match reseal(&plan, &context, nonce, ciphertext) {
                Some(sealed) => {
                    sqlx::query(&format!(
                        "UPDATE {} SET totp_nonce = $2, totp_secret = $3 WHERE client_id = $1",
                        qualified(schema, "client_credentials")
                    ))
                    .bind(client_id)
                    .bind(&sealed.nonce)
                    .bind(&sealed.ciphertext)
                    .execute(&mut *conn)
                    .await?;
                }
                None => {
                    sqlx::query(&format!(
                        "UPDATE {} SET totp_nonce = NULL, totp_secret = NULL WHERE client_id = $1",
                        qualified(schema, "client_credentials")
                    ))
                    .bind(client_id)
                    .execute(&mut *conn)
                    .await?;
                    outcome.clients_totp += 1;
                }
            }
        }
    }
    if table_exists(conn, schema, "saved_payment_methods").await? {
        let rows: Vec<CardRow> = sqlx::query_as(&format!(
            "SELECT id, client_id, provider, token_nonce, token_ciphertext, revoked_at IS NULL FROM {} ORDER BY id",
            qualified(schema, "saved_payment_methods")
        ))
        .fetch_all(&mut *conn)
        .await?;
        for (id, client_id, provider, nonce, ciphertext, active) in rows {
            let context =
                crate::inbox::saved_method_context(sn_domain::ids::ClientId(client_id), &provider);
            match reseal(&plan, &context, nonce, ciphertext) {
                Some(sealed) => {
                    sqlx::query(&format!(
                        "UPDATE {} SET token_nonce = $2, token_ciphertext = $3 WHERE id = $1",
                        qualified(schema, "saved_payment_methods")
                    ))
                    .bind(id)
                    .bind(&sealed.nonce)
                    .bind(&sealed.ciphertext)
                    .execute(&mut *conn)
                    .await?;
                }
                None => {
                    if active {
                        sqlx::query(&format!(
                            "UPDATE {} SET revoked_at = now() WHERE id = $1",
                            qualified(schema, "saved_payment_methods")
                        ))
                        .bind(id)
                        .execute(&mut *conn)
                        .await?;
                        outcome.cards += 1;
                    }
                }
            }
        }
    }
    if table_exists(conn, schema, "remna_snapshots").await? {
        let rows: Vec<(i64, Vec<u8>, Vec<u8>)> = sqlx::query_as(&format!(
            "SELECT subscription_id, keys_nonce, keys_sealed FROM {} \
             WHERE keys_nonce IS NOT NULL AND keys_sealed IS NOT NULL ORDER BY subscription_id",
            qualified(schema, "remna_snapshots")
        ))
        .fetch_all(&mut *conn)
        .await?;
        for (id, nonce, ciphertext) in rows {
            match reseal(&plan, &format!("remna.keys:{id}"), nonce, ciphertext) {
                Some(sealed) => {
                    sqlx::query(&format!(
                        "UPDATE {} SET keys_nonce = $2, keys_sealed = $3 WHERE subscription_id = $1",
                        qualified(schema, "remna_snapshots")
                    ))
                    .bind(id)
                    .bind(&sealed.nonce)
                    .bind(&sealed.ciphertext)
                    .execute(&mut *conn)
                    .await?;
                }
                None => {
                    sqlx::query(&format!(
                        "UPDATE {} SET keys_nonce = NULL, keys_sealed = NULL WHERE subscription_id = $1",
                        qualified(schema, "remna_snapshots")
                    ))
                    .bind(id)
                    .execute(&mut *conn)
                    .await?;
                    outcome.remna_keys += 1;
                }
            }
        }
    }
    Ok(outcome)
}
