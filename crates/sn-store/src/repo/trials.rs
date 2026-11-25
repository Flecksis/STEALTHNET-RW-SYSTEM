use crate::error::StoreResult;
use sn_domain::client::IdentityKind;
use sn_domain::ids::ClientId;
use sqlx::PgConnection;

pub struct TrialsRepo<'c> {
    conn: &'c mut PgConnection,
}

fn split(identities: &[(IdentityKind, String)]) -> (Vec<&'static str>, Vec<String>) {
    identities
        .iter()
        .map(|(kind, value)| (kind.as_str(), value.clone()))
        .unzip()
}

impl<'c> TrialsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        TrialsRepo { conn }
    }

    pub async fn was_claimed(
        &mut self,
        client_id: ClientId,
        identities: &[(IdentityKind, String)],
    ) -> StoreResult<bool> {
        let (kinds, values) = split(identities);
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM trial_claims t JOIN unnest($1::text[], $2::text[]) AS i(kind, value) \
                 ON t.kind = i.kind AND t.value = i.value) \
                 OR EXISTS (SELECT 1 FROM trial_claims t WHERE t.client_id = $3)",
        )
        .bind(&kinds)
        .bind(&values)
        .bind(client_id.0)
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn claim(
        &mut self,
        client_id: ClientId,
        identities: &[(IdentityKind, String)],
    ) -> StoreResult<bool> {
        if identities.is_empty() {
            return Ok(false);
        }
        let (kinds, values) = split(identities);
        let done = sqlx::query(
            "INSERT INTO trial_claims (kind, value, client_id) \
             SELECT kind, value, $3 FROM unnest($1::text[], $2::text[]) AS i(kind, value) \
             WHERE NOT EXISTS (SELECT 1 FROM trial_claims t WHERE t.client_id = $3) \
             ON CONFLICT (kind, value) DO NOTHING",
        )
        .bind(&kinds)
        .bind(&values)
        .bind(client_id.0)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() as usize == identities.len())
    }
}
