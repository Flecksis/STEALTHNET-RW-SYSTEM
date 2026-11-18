use crate::error::{ServiceError, ServiceResult};
use crate::remna_sync::same_user;
use chrono::{DateTime, Utc};
use sn_domain::ids::SubscriptionId;
use sn_remnawave::{RemnaUser, UserKeys};
use sn_store::{Db, SealedSecret, SecretBox, Store};
use uuid::Uuid;

fn context(id: SubscriptionId) -> String {
    format!("remna.keys:{}", id.0)
}

fn sealed_keys(
    secrets: Option<&SecretBox>,
    id: SubscriptionId,
    user: &RemnaUser,
) -> ServiceResult<Option<SealedSecret>> {
    let keys = user.keys().usable();
    let Some(secrets) = secrets.filter(|_| !keys.is_empty()) else {
        return Ok(None);
    };
    let plain = serde_json::to_vec(&keys).map_err(|e| ServiceError::Internal(e.to_string()))?;
    Ok(Some(secrets.seal(&context(id), &plain)?))
}

pub(crate) async fn save(
    db: &mut impl Db,
    secrets: Option<&SecretBox>,
    id: SubscriptionId,
    user: &RemnaUser,
) -> ServiceResult<()> {
    let sealed = sealed_keys(secrets, id, user)?;
    let squads: Vec<Uuid> = user.active_internal_squads.iter().map(|s| s.uuid).collect();
    let names: Vec<String> = user
        .active_internal_squads
        .iter()
        .map(|s| s.name.clone())
        .collect();
    let short = Some(user.short_uuid.as_str()).filter(|s| !s.is_empty());
    let url = Some(user.subscription_url.as_str()).filter(|s| !s.is_empty());
    sqlx::query(
        "INSERT INTO remna_snapshots (subscription_id, remna_user_id, username, short_uuid, squads, squad_names, \
         external_squad_uuid, tag, telegram_id, email, description, remna_created_at, last_traffic_reset_at, \
         subscription_url, keys_nonce, keys_sealed, seen_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, now()) \
         ON CONFLICT (subscription_id) DO UPDATE SET remna_user_id = EXCLUDED.remna_user_id, \
         username = EXCLUDED.username, short_uuid = COALESCE(EXCLUDED.short_uuid, remna_snapshots.short_uuid), \
         squads = EXCLUDED.squads, squad_names = EXCLUDED.squad_names, \
         external_squad_uuid = EXCLUDED.external_squad_uuid, tag = EXCLUDED.tag, \
         telegram_id = EXCLUDED.telegram_id, email = EXCLUDED.email, description = EXCLUDED.description, \
         remna_created_at = EXCLUDED.remna_created_at, last_traffic_reset_at = EXCLUDED.last_traffic_reset_at, \
         subscription_url = COALESCE(EXCLUDED.subscription_url, remna_snapshots.subscription_url), \
         keys_nonce = COALESCE(EXCLUDED.keys_nonce, remna_snapshots.keys_nonce), \
         keys_sealed = COALESCE(EXCLUDED.keys_sealed, remna_snapshots.keys_sealed), seen_at = now()",
    )
    .bind(id.0)
    .bind(user.id.0)
    .bind(&user.username)
    .bind(short)
    .bind(&squads)
    .bind(&names)
    .bind(user.external_squad_uuid)
    .bind(&user.tag)
    .bind(user.telegram_id)
    .bind(&user.email)
    .bind(&user.description)
    .bind(user.created_at)
    .bind(user.last_traffic_reset_at)
    .bind(url)
    .bind(sealed.as_ref().map(|s| s.nonce.clone()))
    .bind(sealed.as_ref().map(|s| s.ciphertext.clone()))
    .execute(db.conn())
    .await?;
    Ok(())
}

pub(crate) async fn capture(
    store: &Store,
    secrets: Option<&SecretBox>,
    user: &RemnaUser,
) -> ServiceResult<bool> {
    let mut conn = store.conn().await?;
    let Some(subscription) = conn.subscriptions().by_remna_user(user.id).await? else {
        return Ok(false);
    };
    let Some(link) = &subscription.remna else {
        return Ok(false);
    };
    if !same_user(link, user) {
        return Ok(false);
    }
    save(&mut conn, secrets, subscription.id, user).await?;
    Ok(true)
}

pub(crate) fn open_keys(
    secrets: Option<&SecretBox>,
    id: SubscriptionId,
    nonce: Option<&[u8]>,
    sealed: Option<&[u8]>,
) -> UserKeys {
    let (Some(secrets), Some(nonce), Some(sealed)) = (secrets, nonce, sealed) else {
        return UserKeys::default();
    };
    let sealed = SealedSecret {
        nonce: nonce.to_vec(),
        ciphertext: sealed.to_vec(),
    };
    secrets
        .open(&context(id), &sealed)
        .ok()
        .and_then(|plain| serde_json::from_slice::<UserKeys>(&plain).ok())
        .map(|keys| keys.usable())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub squads: Vec<Uuid>,
    pub squad_names: Vec<String>,
    pub external_squad: Option<Uuid>,
    pub tag: Option<String>,
    pub telegram_id: Option<i64>,
    pub email: Option<String>,
    pub description: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub last_traffic_reset_at: Option<DateTime<Utc>>,
    pub subscription_url: Option<String>,
    pub keys: UserKeys,
}
