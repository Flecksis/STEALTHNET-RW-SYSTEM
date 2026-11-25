use crate::Actor;
use crate::clients::ClientOps;
use crate::email_change::masked_email;
use crate::error::{ServiceError, ServiceResult};
use serde::Serialize;
use sn_domain::client::{Identity, IdentityKind, NewIdentity, normalize_identity};
use sn_domain::ids::ClientId;
use sn_domain::jobs::JobPayload;
use sn_domain::mail_letters::{LetterId, Values};
use sn_store::{Db, SessionChannel, Tx};

pub const TEMPLATE_EMAIL_SET: &str = "email_set";
pub const TEMPLATE_EMAIL_REMOVED: &str = "email_removed";
pub const EMAIL_SET_AUDIT: &str = "client.email_set";
pub const EMAIL_REMOVED_AUDIT: &str = "client.email_removed";
pub const EMAIL_TAKEN: &str = "client.email_taken";
pub const EMAIL_SAME: &str = "client.email_same";
pub const EMAIL_MISSING: &str = "client.no_email";
pub const EMAIL_ONLY_LOGIN: &str = "client.email_only_login";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EmailSet {
    Saved {
        email: String,
        previous: Option<String>,
        revoked_sessions: u64,
    },
    Taken {
        client_id: ClientId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmailRemoved {
    pub email: String,
    pub password_removed: bool,
    pub revoked_sessions: u64,
}

async fn owner_of(tx: &mut Tx, email: &str) -> ServiceResult<Option<ClientId>> {
    let owner: Option<i64> = sqlx::query_scalar(
        "SELECT client_id FROM client_identities WHERE kind = 'email' AND value = $1",
    )
    .bind(email)
    .fetch_optional(tx.conn())
    .await?;
    Ok(owner.map(ClientId))
}

async fn cancel_codes(tx: &mut Tx, id: ClientId, emails: &[String]) -> ServiceResult<u64> {
    let done = sqlx::query(
        "UPDATE client_login_requests SET status = 'rejected' WHERE status = 'pending' \
         AND ((method = 'email_change' AND client_id = $1) \
         OR (method IN ('email', 'reset') AND email = ANY($2)))",
    )
    .bind(id.0)
    .bind(emails)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected())
}

async fn notify(tx: &mut Tx, id: ClientId, template: &str, email: &str) -> ServiceResult<()> {
    tx.jobs()
        .enqueue(
            &JobPayload::Notify {
                client_id: id,
                template: template.into(),
                params: serde_json::json!({ "email": email }),
            },
            None,
        )
        .await?;
    Ok(())
}

impl<'a> ClientOps<'a> {
    pub async fn set_email(
        &self,
        id: ClientId,
        raw: &str,
        actor: Actor,
    ) -> ServiceResult<EmailSet> {
        let email = normalize_identity(IdentityKind::Email, raw)?;
        let mut tx = self.services.store().begin().await?;
        tx.clients().lock(id).await?;
        if let Some(owner) = owner_of(&mut tx, &email).await?
            && owner != id
        {
            return Ok(EmailSet::Taken { client_id: owner });
        }
        let current: Vec<Identity> = tx
            .clients()
            .identities(id)
            .await?
            .into_iter()
            .filter(|i| i.kind == IdentityKind::Email)
            .collect();
        if current.iter().any(|i| i.value == email && i.verified) {
            return Err(ServiceError::rejected(
                EMAIL_SAME,
                "У клиента уже указана эта почта",
            ));
        }
        sqlx::query("DELETE FROM client_identities WHERE client_id = $1 AND kind = 'email'")
            .bind(id.0)
            .execute(tx.conn())
            .await?;
        if !tx
            .clients()
            .add_identity_if_free(id, &NewIdentity::new(IdentityKind::Email, &email, true)?)
            .await?
        {
            let owner = owner_of(&mut tx, &email).await?;
            return match owner {
                Some(owner) if owner != id => Ok(EmailSet::Taken { client_id: owner }),
                _ => Err(ServiceError::rejected(
                    EMAIL_TAKEN,
                    "Эта почта уже указана у другого клиента",
                )),
            };
        }
        if current
            .iter()
            .any(|i| !i.verified && !i.linked_without_code())
        {
            crate::oauth::unlink_unconfirmed(&mut tx, id).await?;
        }
        let previous: Vec<Identity> = current.into_iter().filter(|i| i.value != email).collect();
        let addresses: Vec<String> = previous.iter().map(|i| i.value.clone()).collect();
        cancel_codes(&mut tx, id, &addresses).await?;
        let revoked = tx
            .sessions()
            .revoke_client_channel(id, SessionChannel::Cabinet)
            .await?;
        tx.audit()
            .record(&actor.audit(
                EMAIL_SET_AUDIT,
                "client",
                Some(id.0),
                serde_json::json!({
                    "email": email,
                    "previous": addresses.first(),
                    "revoked_sessions": revoked,
                }),
            ))
            .await?;
        notify(&mut tx, id, TEMPLATE_EMAIL_SET, &email).await?;
        tx.commit().await?;
        for old in previous.iter().filter(|i| i.takes_letters()) {
            let sent = self
                .services
                .letters()
                .send(
                    LetterId::EmailChanged,
                    &old.value,
                    Values::new().with("new_email", masked_email(&email)),
                )
                .await;
            if let Err(err) = sent {
                tracing::warn!(client = %id, error = %err, "письмо о смене почты на прежний адрес не отправилось");
            }
        }
        Ok(EmailSet::Saved {
            email,
            previous: addresses.into_iter().next(),
            revoked_sessions: revoked,
        })
    }

    pub async fn remove_email(&self, id: ClientId, actor: Actor) -> ServiceResult<EmailRemoved> {
        let mut tx = self.services.store().begin().await?;
        tx.clients().lock(id).await?;
        let identities = tx.clients().identities(id).await?;
        let Some(email) = identities
            .iter()
            .find(|i| i.kind == IdentityKind::Email)
            .map(|i| i.value.clone())
        else {
            return Err(ServiceError::rejected(EMAIL_MISSING, "У клиента нет почты"));
        };
        if identities.iter().all(|i| i.kind == IdentityKind::Email) {
            return Err(ServiceError::rejected(
                EMAIL_ONLY_LOGIN,
                "Клиент входит только по почте: без неё доступ к аккаунту пропадёт. Укажите другую почту вместо этой.",
            ));
        }
        sqlx::query("DELETE FROM client_identities WHERE client_id = $1 AND kind = 'email'")
            .bind(id.0)
            .execute(tx.conn())
            .await?;
        let password_removed = sqlx::query("DELETE FROM client_passwords WHERE client_id = $1")
            .bind(id.0)
            .execute(tx.conn())
            .await?
            .rows_affected()
            > 0;
        cancel_codes(&mut tx, id, std::slice::from_ref(&email)).await?;
        let revoked = tx
            .sessions()
            .revoke_client_channel(id, SessionChannel::Cabinet)
            .await?;
        tx.audit()
            .record(&actor.audit(
                EMAIL_REMOVED_AUDIT,
                "client",
                Some(id.0),
                serde_json::json!({
                    "email": email,
                    "password_removed": password_removed,
                    "revoked_sessions": revoked,
                }),
            ))
            .await?;
        notify(&mut tx, id, TEMPLATE_EMAIL_REMOVED, &email).await?;
        tx.commit().await?;
        Ok(EmailRemoved {
            email,
            password_removed,
            revoked_sessions: revoked,
        })
    }
}
