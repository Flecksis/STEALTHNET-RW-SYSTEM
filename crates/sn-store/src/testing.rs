use crate::db::{Store, StoreOptions};
use sqlx::postgres::PgConnectOptions;
use std::str::FromStr;

pub const TEST_DATABASE_ENV: &str = "SN_TEST_DATABASE_URL";

pub struct TestDb {
    pub store: Store,
    schema: String,
    url: String,
}

pub fn test_database_url() -> Option<String> {
    std::env::var(TEST_DATABASE_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
}

pub async fn test_store() -> Option<TestDb> {
    let url = test_database_url()?;
    let database = PgConnectOptions::from_str(&url)
        .ok()
        .and_then(|o| o.get_database().map(str::to_string))
        .unwrap_or_default();
    assert!(
        database.starts_with("sn_test"),
        "{TEST_DATABASE_ENV} должен указывать на базу с именем sn_test*, сейчас: {database}"
    );
    let schema = format!("sn_t_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
    let admin = Store::connect(
        &url,
        StoreOptions {
            max_connections: 1,
            ..StoreOptions::default()
        },
    )
    .await
    .expect("тестовая база недоступна");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(admin.pool())
        .await
        .expect("не удалось создать схему для теста");
    admin.close().await;
    let store = Store::connect(
        &url,
        StoreOptions {
            max_connections: 4,
            search_path: Some(schema.clone()),
            ..StoreOptions::default()
        },
    )
    .await
    .expect("тестовая база недоступна");
    store.migrate().await.expect("миграции не применились");
    Some(TestDb { store, schema, url })
}

impl TestDb {
    pub async fn cleanup(self) {
        self.store.close().await;
        let admin = Store::connect(
            &self.url,
            StoreOptions {
                max_connections: 1,
                ..StoreOptions::default()
            },
        )
        .await;
        if let Ok(admin) = admin {
            let _ = sqlx::query(&format!("DROP SCHEMA IF EXISTS {} CASCADE", self.schema))
                .execute(admin.pool())
                .await;
            admin.close().await;
        }
    }
}
