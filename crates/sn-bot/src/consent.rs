use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sn_store::{Db, StoreResult};

pub const DOCUMENTS_KEY: &str = "bot.documents";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub title: String,
    pub url: String,
}

impl Document {
    fn valid(&self) -> bool {
        !self.title.trim().is_empty()
            && (self.url.starts_with("https://") || self.url.starts_with("http://"))
    }
}

pub async fn documents(db: &mut impl Db) -> StoreResult<Vec<Document>> {
    let raw = db.settings().get(DOCUMENTS_KEY).await?;
    Ok(raw
        .and_then(|value| serde_json::from_value::<Vec<Document>>(value).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(Document::valid)
        .collect())
}

pub fn version(documents: &[Document]) -> Option<String> {
    if documents.is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    for document in documents {
        hasher.update(document.title.trim().as_bytes());
        hasher.update(b"\n");
        hasher.update(document.url.trim().as_bytes());
        hasher.update(b"\n");
    }
    Some(hex::encode(&hasher.finalize()[..12]))
}

pub async fn accepted(db: &mut impl Db, telegram_id: i64, version: &str) -> StoreResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM bot_consents WHERE telegram_id = $1 AND version = $2)",
    )
    .bind(telegram_id)
    .bind(version)
    .fetch_one(db.conn())
    .await?)
}

pub async fn accept(
    db: &mut impl Db,
    telegram_id: i64,
    version: &str,
    documents: &[Document],
) -> StoreResult<()> {
    sqlx::query(
        "INSERT INTO bot_consents (telegram_id, version, documents) VALUES ($1, $2, $3) \
         ON CONFLICT (telegram_id, version) DO NOTHING",
    )
    .bind(telegram_id)
    .bind(version)
    .bind(serde_json::to_value(documents)?)
    .execute(db.conn())
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn версия_согласия_меняется_вместе_с_документами() {
        assert_eq!(version(&[]), None);
        let offer = Document {
            title: "Оферта".into(),
            url: "https://example.com/offer".into(),
        };
        let first = version(std::slice::from_ref(&offer)).unwrap();
        assert_eq!(first.len(), 24);
        let mut changed = offer.clone();
        changed.url = "https://example.com/offer-v2".into();
        assert_ne!(version(&[changed]), Some(first.clone()));
        assert_eq!(version(&[offer]), Some(first));
    }
}
