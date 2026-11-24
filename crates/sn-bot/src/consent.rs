use sha2::{Digest, Sha256};
use sn_services::bot_content::Document;
use sn_store::{Db, StoreResult};

pub use sn_services::bot_content::DOCUMENTS_KEY;

pub fn version(documents: &[Document]) -> Option<String> {
    if documents.is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    for document in documents {
        hasher.update(document.title.trim().as_bytes());
        hasher.update(b"\n");
        hasher.update(document.target().trim().as_bytes());
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
        let offer = Document::link("Оферта", "https://example.com/offer");
        let first = version(std::slice::from_ref(&offer)).unwrap();
        assert_eq!(first.len(), 24);
        let mut changed = offer.clone();
        changed.url = "https://example.com/offer-v2".into();
        assert_ne!(version(&[changed]), Some(first.clone()));
        assert_eq!(version(std::slice::from_ref(&offer)), Some(first.clone()));
        let mut with_id = offer.clone();
        with_id.id = "d1".into();
        with_id.title_en = Some("Offer".into());
        assert_eq!(version(&[with_id]), Some(first));
        let page = Document {
            id: "k3f9a1b2c3".into(),
            title: "Политика".into(),
            path: Some("/privacy-policy".into()),
            ..Document::default()
        };
        let both = version(&[offer, page.clone()]).unwrap();
        let mut moved = page;
        moved.path = Some("/privacy".into());
        assert_ne!(
            version(&[Document::link("Оферта", "https://example.com/offer"), moved]),
            Some(both)
        );
    }
}
