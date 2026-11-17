use super::*;

#[tokio::test]
async fn режим_поддержки_меняет_сайт_и_приём_обращений() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let client = h.client(5_100_200).await;
    let token = h.client_token(client).await;
    let ticket = json!({ "body": "Не подключается iPhone" });

    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["support_tickets"], true);
    assert_eq!(site["support_url"], Value::Null);

    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/core",
            &owner,
            json!({ "support_mode": "link" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/core",
            &owner,
            json!({ "support_url": "https://t.me/help_chat" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["support_url"], Value::Null);
    assert_eq!(site["support_tickets"], true);

    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/core",
            &owner,
            json!({ "support_mode": "both" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["support_url"], "https://t.me/help_chat");
    assert_eq!(site["support_tickets"], true);
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/support/tickets",
            Some(&token),
            Some(ticket.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/core",
            &owner,
            json!({ "support_mode": "link" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["settings"]["support_mode"], "link");
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["support_tickets"], false);
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/support/tickets",
            Some(&token),
            Some(ticket),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "support.tickets_off");
    let (status, list) = h.get("/api/client/support/tickets", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().map(Vec::len), Some(1));
    h.db.cleanup().await;
}
