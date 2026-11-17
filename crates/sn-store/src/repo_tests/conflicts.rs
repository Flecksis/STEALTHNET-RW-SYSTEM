use super::fixtures::*;
use crate::testing::test_store;
use crate::*;
use sn_domain::access::AdminRole;
use sn_domain::client::{IdentityKind, NewIdentity};

#[tokio::test]
async fn конфликт_называет_поле_по_имени_ограничения() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let admin = NewAdmin {
        email: "Owner@Example.com".into(),
        password_hash: "hash".into(),
        role: AdminRole::Owner,
        must_change_password: false,
    };
    conn.admins().create(&admin).await.unwrap();
    let again = NewAdmin {
        email: "owner@example.com".into(),
        ..admin
    };
    let err = conn.admins().create(&again).await.unwrap_err();
    assert_eq!(err.constraint(), Some("admins_email_key"));
    assert_eq!(err.field(), Some("Почта"));
    assert_eq!(
        err.to_string(),
        "конфликт: значение поля «Почта» уже занято"
    );

    let first = client(&mut conn, &Person::NOBODY).await;
    let second = client(&mut conn, &Person::NOBODY).await;
    let email = NewIdentity::new(IdentityKind::Email, "a.morozov@example.com", true).unwrap();
    conn.clients().add_identity(first, &email).await.unwrap();
    let taken = conn
        .clients()
        .add_identity(second, &email)
        .await
        .unwrap_err();
    assert_eq!(taken.field(), Some("Telegram или почта"));

    let paid = paid(&mut conn, first, "cryptopay", usd(100)).await;
    conn.payments()
        .attach_invoice(
            paid,
            &InvoiceUpdate {
                pay_url: None,
                provider_txid: Some("inv-7".into()),
                expires_at: None,
                payload: serde_json::json!({}),
            },
        )
        .await
        .unwrap();
    let other = payment(&mut conn, second, "cryptopay", usd(100)).await;
    let reused = conn
        .payments()
        .attach_invoice(
            other,
            &InvoiceUpdate {
                pay_url: None,
                provider_txid: Some("inv-7".into()),
                expires_at: None,
                payload: serde_json::json!({}),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(reused.field(), Some("Номер транзакции"));

    let job = sn_domain::jobs::JobPayload::FulfillOrder {
        order_id: sn_domain::ids::OrderId(1),
    };
    let internal = sqlx::query(
        "INSERT INTO jobs (kind, dedupe_key) VALUES ('fulfill_order', $1), ('fulfill_order', $1)",
    )
    .bind(job.dedupe_key())
    .execute(&mut *conn.conn())
    .await
    .map_err(StoreError::from)
    .unwrap_err();
    assert_eq!(internal.constraint(), Some("jobs_dedupe_key"));
    assert_eq!(internal.field(), None);
    assert_eq!(
        internal.to_string(),
        "конфликт: запись с таким значением уже существует"
    );
    drop(conn);
    db.cleanup().await;
}
