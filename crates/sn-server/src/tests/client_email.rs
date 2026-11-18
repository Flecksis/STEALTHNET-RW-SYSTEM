use super::mini_account::{Letters, letter_code, local_smtp, mini_http};
use super::*;
use sn_domain::access::{ClientChannel, ClientPrincipal};
use sn_domain::module::ModuleId;
use sn_services::auth::RequestMeta;
use sn_services::email_change::EmailProofInput;
use sn_services::settings::ModuleConfigUpdate;

async fn mail_with(h: &Http, port: u16, login: &str) {
    let admin = h
        .services()
        .auth()
        .upsert_owner("owner@example.com", PASSWORD)
        .await
        .unwrap()
        .0;
    let config = json!({
        "transport": "smtp",
        "smtp_host": "127.0.0.1",
        "smtp_port": port,
        "security": "none",
        "from_email": "robot@example.com",
        "login": login,
    });
    let ops = h.services().settings_ops();
    ops.update_module_config(
        ModuleId::Mail,
        &ModuleConfigUpdate {
            config: config.as_object().unwrap().clone(),
            clear: Vec::new(),
        },
        admin,
    )
    .await
    .unwrap();
    ops.toggle_module(ModuleId::Mail, true, admin)
        .await
        .unwrap();
}

fn in_bot(client: ClientId) -> ClientPrincipal {
    ClientPrincipal {
        client_id: client,
        channel: ClientChannel::Bot,
        full_access: true,
        session_id: None,
    }
}

async fn link_in_bot(h: &Http, letters: &Letters, client: ClientId, email: &str) {
    let auth = h.services().auth();
    let before = letters.lock().unwrap().len();
    auth.email_change_start(
        &in_bot(client),
        email,
        EmailProofInput::default(),
        &RequestMeta::default(),
    )
    .await
    .unwrap();
    let code = letter_code(&letters.lock().unwrap()[before]);
    let state = auth
        .email_change_confirm(&in_bot(client), email, &code, &RequestMeta::default())
        .await
        .unwrap();
    assert_eq!(state.email.as_deref(), Some(email));
}

async fn jobs_of(h: &Http, client: ClientId, template: &str) -> Vec<Value> {
    let mut conn = h.services().store().conn().await.unwrap();
    sqlx::query_scalar(
        "SELECT payload FROM jobs WHERE payload->>'template' = $1 \
         AND (payload->>'client_id')::bigint = $2 ORDER BY id",
    )
    .bind(template)
    .bind(client.0)
    .fetch_all(conn.conn())
    .await
    .unwrap()
}

async fn email_identity(h: &Http, client: ClientId) -> Option<(String, bool)> {
    let mut conn = h.services().store().conn().await.unwrap();
    conn.clients()
        .identities(client)
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.kind == sn_domain::client::IdentityKind::Email)
        .map(|i| (i.value, i.verified))
}

async fn has_password(h: &Http, client: ClientId) -> bool {
    h.services().auth().has_password(client).await.unwrap()
}

#[tokio::test]
async fn администратор_указывает_меняет_и_отвязывает_почту_клиента_из_бота() {
    let Some(h) = mini_http().await else { return };
    let (port, letters) = local_smtp().await;
    mail_with(&h, port, "password").await;
    let owner = h.login("owner@example.com").await;
    let client = h.client(5_093_118_422).await;
    let cabinet = h.client_token(client).await;
    let path = format!("/api/admin/clients/{}/email", client.0);

    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, body) = h
        .write(
            Method::PUT,
            &path,
            &support,
            json!({ "email": "nina@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _, body) = h.send(Method::DELETE, &path, Some(&support), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, body) = h
        .write(Method::PUT, &path, &owner, json!({ "email": "не почта" }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = h
        .write(
            Method::PUT,
            &path,
            &owner,
            json!({ "email": " Nina@Example.com " }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "saved");
    assert_eq!(body["email"], "nina@example.com");
    assert_eq!(body["previous"], Value::Null);
    assert_eq!(
        email_identity(&h, client).await,
        Some(("nina@example.com".into(), true))
    );
    assert_eq!(
        h.get("/api/client/me", &cabinet).await.0,
        StatusCode::UNAUTHORIZED
    );
    let set = jobs_of(&h, client, "email_set").await;
    assert_eq!(set.len(), 1);
    assert_eq!(set[0]["params"]["email"], "nina@example.com");
    let (status, card) = h
        .get(&format!("/api/admin/clients/{}", client.0), &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{card}");
    assert!(
        card["identities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["kind"] == "email"
                && i["value"] == "nina@example.com"
                && i["verified"] == true),
        "{card}"
    );
    let (status, history) = h
        .get(&format!("/api/admin/clients/{}/history", client.0), &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert!(
        history.to_string().contains("client.email_set"),
        "{history}"
    );

    let (status, body) = h
        .write(
            Method::PUT,
            &path,
            &owner,
            json!({ "email": "nina@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "client.email_same");

    let other = h.client(5_093_118_423).await;
    let (status, body) = h
        .write(
            Method::PUT,
            &format!("/api/admin/clients/{}/email", other.0),
            &owner,
            json!({ "email": "olga@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = h
        .write(
            Method::PUT,
            &path,
            &owner,
            json!({ "email": "OLGA@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "client.email_taken");
    assert_eq!(body["error"]["client_id"], other.0);
    assert_eq!(
        email_identity(&h, client).await,
        Some(("nina@example.com".into(), true))
    );

    let sent_before = letters.lock().unwrap().len();
    let (status, body) = h
        .write(
            Method::PUT,
            &path,
            &owner,
            json!({ "email": "lena@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["previous"], "nina@example.com");
    let notice = letters.lock().unwrap()[sent_before..].join("\n");
    assert!(notice.contains("nina@example.com"), "{notice}");
    assert!(
        jobs_of(&h, client, "email_set").await.len() == 2,
        "уведомление о второй смене"
    );

    let token = h.client_token(client).await;
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/security/password",
            &token,
            json!({ "password": "пароль-лены-12" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/password",
            None,
            Some(json!({ "email": "lena@example.com", "password": "пароль-лены-12" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["principal"]["client_id"], client.0);

    let (status, _, body) = h.send(Method::DELETE, &path, Some(&owner), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["email"], "lena@example.com");
    assert_eq!(body["password_removed"], true);
    assert_eq!(email_identity(&h, client).await, None);
    assert!(!has_password(&h, client).await);
    assert_eq!(
        h.get("/api/client/me", &token).await.0,
        StatusCode::UNAUTHORIZED
    );
    let removed = jobs_of(&h, client, "email_removed").await;
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0]["params"]["email"], "lena@example.com");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/password",
            None,
            Some(json!({ "email": "lena@example.com", "password": "пароль-лены-12" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (_, history) = h
        .get(&format!("/api/admin/clients/{}/history", client.0), &owner)
        .await;
    assert!(
        history.to_string().contains("client.email_removed"),
        "{history}"
    );

    let (status, _, body) = h.send(Method::DELETE, &path, Some(&owner), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "client.no_email");

    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/register",
            None,
            Some(json!({ "email": "vera@example.com", "password": "пароль-веры-12" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let vera = ClientId(body["principal"]["client_id"].as_i64().unwrap());
    let (status, _, body) = h
        .send(
            Method::DELETE,
            &format!("/api/admin/clients/{}/email", vera.0),
            Some(&owner),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "client.email_only_login");
    assert_eq!(
        email_identity(&h, vera).await,
        Some(("vera@example.com".into(), false))
    );
    let (status, body) = h
        .write(
            Method::PUT,
            &format!("/api/admin/clients/{}/email", vera.0),
            &owner,
            json!({ "email": "vera@example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        email_identity(&h, vera).await,
        Some(("vera@example.com".into(), true))
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn клиент_из_бота_входит_по_почте_и_паролю_и_восстанавливает_пароль_кодом() {
    let Some(h) = mini_http().await else { return };
    let (port, letters) = local_smtp().await;
    mail_with(&h, port, "password").await;
    let client = h.client(5_093_118_422).await;
    link_in_bot(&h, &letters, client, "alex@example.com").await;
    assert_eq!(
        email_identity(&h, client).await,
        Some(("alex@example.com".into(), true))
    );
    h.services()
        .auth()
        .change_password(&in_bot(client), None, "пароль-алексея-1")
        .await
        .unwrap();
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/password",
            None,
            Some(json!({ "email": "alex@example.com", "password": "пароль-алексея-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["principal"]["client_id"], client.0);
    assert_eq!(body["created"], false);

    let before = letters.lock().unwrap().len();
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/reset",
            None,
            Some(json!({ "email": "alex@example.com" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let code = letter_code(&letters.lock().unwrap()[before]);
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/reset/confirm",
            None,
            Some(json!({ "email": "alex@example.com", "code": code, "password": "новый-пароль-алексея" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["principal"]["client_id"], client.0);
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/password",
            None,
            Some(json!({ "email": "alex@example.com", "password": "новый-пароль-алексея" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["principal"]["client_id"], client.0);
    h.db.cleanup().await;
}

#[tokio::test]
async fn клиент_из_бота_входит_по_коду_из_письма_в_свой_аккаунт() {
    let Some(h) = mini_http().await else { return };
    let (port, letters) = local_smtp().await;
    mail_with(&h, port, "code").await;
    let client = h.client(5_093_118_422).await;
    link_in_bot(&h, &letters, client, "alex@example.com").await;
    let before = letters.lock().unwrap().len();
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email",
            None,
            Some(json!({ "email": "alex@example.com" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let code = letter_code(&letters.lock().unwrap()[before]);
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/verify",
            None,
            Some(json!({ "email": "alex@example.com", "code": code })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["principal"]["client_id"], client.0);
    assert_eq!(body["created"], false);
    h.db.cleanup().await;
}

#[tokio::test]
async fn клиенты_бота_не_делят_общий_лимит_запросов_кода() {
    let Some(h) = mini_http().await else { return };
    let (port, letters) = local_smtp().await;
    mail_with(&h, port, "password").await;
    for n in 0..7_i64 {
        let client = h.client(5_093_118_500 + n).await;
        h.services()
            .auth()
            .email_change_start(
                &in_bot(client),
                &format!("client{n}@example.com"),
                EmailProofInput::default(),
                &RequestMeta::default(),
            )
            .await
            .unwrap_or_else(|err| panic!("клиент {n}: {err}"));
    }
    assert_eq!(letters.lock().unwrap().len(), 7);
    let client = h.client(5_093_118_600).await;
    let mut limited = None;
    for n in 0..8 {
        if let Err(err) = h
            .services()
            .auth()
            .email_change_start(
                &in_bot(client),
                &format!("many{n}@example.com"),
                EmailProofInput::default(),
                &RequestMeta::default(),
            )
            .await
        {
            limited = Some(err.code());
            break;
        }
    }
    assert_eq!(limited, Some(sn_services::limits::RATE_LIMITED));
    h.db.cleanup().await;
}
