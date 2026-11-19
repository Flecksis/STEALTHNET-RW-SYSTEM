use super::fixtures::*;
use crate::testing::test_store;
use crate::*;
use std::time::Duration;

const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F'];

#[tokio::test]
async fn фото_клиентов_запрашиваются_хранятся_и_обновляются() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let alexey = client(
        &mut conn,
        &Person {
            telegram: Some(5_093_118_422),
            username: Some("morozov_a"),
            first_name: Some("Алексей"),
            last_name: Some("Морозов"),
            email: None,
        },
    )
    .await;
    let maria = client(
        &mut conn,
        &Person {
            email: Some("m.lebedeva@example.com"),
            ..Person::NOBODY
        },
    )
    .await;
    let gone = client(
        &mut conn,
        &Person {
            telegram: Some(7_412),
            ..Person::NOBODY
        },
    )
    .await;
    conn.clients().soft_delete(gone).await.unwrap();

    let mut photos = conn.client_photos();
    assert_eq!(
        photos
            .request_missing(&[alexey, maria, gone])
            .await
            .unwrap(),
        1
    );
    assert_eq!(photos.request_missing(&[alexey]).await.unwrap(), 0);
    let due = photos.due(10).await.unwrap();
    assert_eq!(
        due,
        vec![PhotoTarget {
            client_id: alexey,
            telegram_id: 5_093_118_422,
            known: None,
        }]
    );
    assert!(photos.versions(&[alexey, maria]).await.unwrap().is_empty());
    assert!(photos.get(alexey).await.unwrap().is_none());

    photos
        .save(
            alexey,
            Some(NewPhoto {
                version: "AQADfirst",
                content: JPEG,
                content_type: "image/jpeg",
            }),
        )
        .await
        .unwrap();
    assert!(photos.due(10).await.unwrap().is_empty());
    assert_eq!(
        photos.versions(&[alexey, maria]).await.unwrap(),
        vec![(alexey, "AQADfirst".to_string())]
    );
    let stored = photos.get(alexey).await.unwrap().unwrap();
    assert_eq!(stored.content, JPEG);
    assert_eq!(stored.content_type, "image/jpeg");
    assert_eq!(stored.version, "AQADfirst");

    assert!(
        !photos
            .request_if_stale(alexey, Duration::from_secs(3600))
            .await
            .unwrap()
    );
    assert!(
        photos
            .request_if_stale(alexey, Duration::ZERO)
            .await
            .unwrap()
    );
    let again = photos.due(10).await.unwrap();
    assert_eq!(again[0].known.as_deref(), Some("AQADfirst"));
    photos.keep(alexey).await.unwrap();
    assert!(photos.due(10).await.unwrap().is_empty());
    assert_eq!(
        photos.get(alexey).await.unwrap().unwrap().version,
        "AQADfirst"
    );

    assert!(
        photos
            .request_if_stale(alexey, Duration::ZERO)
            .await
            .unwrap()
    );
    photos
        .postpone(alexey, Duration::from_secs(600))
        .await
        .unwrap();
    assert!(photos.due(10).await.unwrap().is_empty());

    photos.save(alexey, None).await.unwrap();
    assert!(photos.get(alexey).await.unwrap().is_none());
    assert!(photos.versions(&[alexey]).await.unwrap().is_empty());

    assert!(
        photos
            .request_if_stale(maria, Duration::ZERO)
            .await
            .unwrap()
    );
    assert!(photos.due(10).await.unwrap().is_empty());
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn проверка_свежего_фото_не_трогает_строку() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let alexey = client(
        &mut conn,
        &Person {
            telegram: Some(5_093_118_429),
            username: Some("morozov_b"),
            ..Person::NOBODY
        },
    )
    .await;
    assert_eq!(
        conn.client_photos()
            .request_missing(&[alexey])
            .await
            .unwrap(),
        1
    );
    conn.client_photos()
        .save(
            alexey,
            Some(NewPhoto {
                version: "AQADfresh",
                content: JPEG,
                content_type: "image/jpeg",
            }),
        )
        .await
        .unwrap();
    async fn version(conn: &mut Conn, client: sn_domain::ids::ClientId) -> (String, String) {
        sqlx::query_as::<_, (String, String)>(
            "SELECT xmin::text, xmax::text FROM client_photos WHERE client_id = $1",
        )
        .bind(client.0)
        .fetch_one(conn.conn())
        .await
        .unwrap()
    }
    let before = version(&mut conn, alexey).await;
    for _ in 0..3 {
        assert!(
            !conn
                .client_photos()
                .request_if_stale(alexey, Duration::from_secs(3600))
                .await
                .unwrap()
        );
    }
    assert_eq!(version(&mut conn, alexey).await, before);
    assert!(
        conn.client_photos()
            .request_if_stale(alexey, Duration::ZERO)
            .await
            .unwrap()
    );
    assert_eq!(conn.client_photos().due(10).await.unwrap().len(), 1);
    drop(conn);
    db.cleanup().await;
}
