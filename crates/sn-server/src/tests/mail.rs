use super::*;

#[tokio::test]
async fn проверка_почты_только_с_правом_настроек_и_признак_для_сайта() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(site["email_login"], false);
    let (status, _) = h
        .write(
            Method::POST,
            "/api/admin/mail/test",
            &support,
            json!({ "to": "support@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/mail/test",
            &owner,
            json!({ "to": "owner@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "mail.unavailable");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email",
            None,
            Some(json!({ "email": "alex@example.com" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    h.db.cleanup().await;
}
