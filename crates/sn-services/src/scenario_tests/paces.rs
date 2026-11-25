use super::email_change::{NO_PROOF, principal};
use super::site_limits::{limits, register, save, site};
use super::*;
use crate::limits::IPV6_WIDE_BITS;
use sn_domain::access::{ClientChannel, ClientPrincipal};
use sn_modules::catalog::antibot;

async fn letters_from(services: &Services, ip: &str, name: &str) -> u32 {
    for sent in 0..100 {
        match services
            .auth()
            .email_login_start(&format!("{name}{sent}@example.com"), &meta(ip))
            .await
        {
            Ok(_) => {}
            Err(err) => {
                assert_eq!(err.code(), RATE_LIMITED);
                return sent;
            }
        }
    }
    panic!("предела нет");
}

async fn links_from(services: &Services, ip: &str) -> u32 {
    for made in 0..100 {
        match services.auth().telegram_login_start(&meta(ip)).await {
            Ok(_) => {}
            Err(err) => {
                assert_eq!(err.code(), RATE_LIMITED);
                return made;
            }
        }
    }
    panic!("предела нет");
}

async fn antibot_switch(services: &Services, admin: AdminId, on: bool) {
    services
        .settings_ops()
        .toggle_module(ModuleId::Antibot, on, admin)
        .await
        .unwrap();
}

#[tokio::test]
async fn пороги_обращений_с_адреса_по_умолчанию_прежние_и_меняются_настройкой() {
    let Some((env, admin, _letters)) = site("code", false).await else {
        return;
    };
    let services = &env.services;
    assert_eq!(antibot::Paces::default().email, 5);
    assert_eq!(antibot::Paces::default().telegram, 10);
    assert_eq!(antibot::Paces::default().verify, 60);
    assert_eq!(rules::EMAIL_START_IP.limit, 5);
    assert_eq!(rules::TELEGRAM_START_IP.limit, 10);
    assert_eq!(rules::VERIFY_PAGE_IP.limit, 60);
    assert_eq!(letters_from(services, "203.0.113.1", "first").await, 5);
    assert_eq!(links_from(services, "203.0.113.1").await, 10);
    limits(
        services,
        serde_json::json!({ "email_ip": 8, "telegram_ip": 12, "verify_ip": 70 }),
    )
    .await;
    assert_eq!(letters_from(services, "203.0.113.2", "more").await, 8);
    assert_eq!(links_from(services, "203.0.113.2").await, 12);
    assert_eq!(services.limits().verify_page().limit, 70);
    limits(
        services,
        serde_json::json!({ "email_ip": "2", "telegram_ip": 1, "verify_ip": 1 }),
    )
    .await;
    assert_eq!(letters_from(services, "203.0.113.3", "less").await, 2);
    assert_eq!(links_from(services, "203.0.113.3").await, 1);
    assert_eq!(services.limits().verify_page().limit, 1);
    antibot_switch(services, admin, false).await;
    assert_eq!(letters_from(services, "203.0.113.4", "off").await, 5);
    assert_eq!(links_from(services, "203.0.113.4").await, 10);
    assert_eq!(services.limits().verify_page().limit, 60);
    env.db.cleanup().await;
}

#[tokio::test]
async fn порог_по_почте_общий_для_регистрации_сброса_и_смены_почты_а_в_боте_прежний() {
    let Some((env, ..)) = site("password", false).await else {
        return;
    };
    let services = &env.services;
    let auth = services.auth();
    limits(services, serde_json::json!({ "email_ip": 2 })).await;
    let home = "203.0.113.20";
    register(services, "one@example.com", home).await.unwrap();
    auth.email_reset_start("one@example.com", &meta(home))
        .await
        .unwrap();
    assert_eq!(
        code(register(services, "two@example.com", home).await),
        RATE_LIMITED
    );
    assert_eq!(
        code(auth.email_reset_start("one@example.com", &meta(home)).await),
        RATE_LIMITED
    );
    let mini = principal(telegram_client(services, 5_093_118_422).await);
    assert_eq!(
        code(
            auth.email_change_start(&mini, "mini@example.com", NO_PROOF, &meta(home))
                .await
        ),
        RATE_LIMITED
    );
    let bot = ClientPrincipal {
        channel: ClientChannel::Bot,
        ..principal(telegram_client(services, 5_093_118_423).await)
    };
    for n in 0..4 {
        auth.email_change_start(&bot, &format!("bot{n}@example.com"), NO_PROOF, &meta(home))
            .await
            .unwrap();
    }
    env.db.cleanup().await;
}

#[tokio::test]
async fn регистрация_по_паролю_ограничена_сетью_ipv6_48_как_письма_с_кодом() {
    let Some((env, ..)) = site("password", false).await else {
        return;
    };
    let services = &env.services;
    assert_eq!(IPV6_WIDE_BITS, 48);
    let wide = rules::EMAIL_START_WIDE.limit;
    assert_eq!(wide, 15);
    for n in 1..=wide {
        register(
            services,
            &format!("net{n}@example.com"),
            &format!("2001:db8:77:{n:x}::1"),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        code(register(services, "last@example.com", "2001:db8:77:ff::1").await),
        RATE_LIMITED
    );
    register(services, "other@example.com", "2001:db8:78:1::1")
        .await
        .unwrap();
    register(services, "four@example.com", "203.0.113.9")
        .await
        .unwrap();
    limits(services, serde_json::json!({ "email_ip": 6 })).await;
    let limits_now = services.limits();
    assert_eq!(limits_now.email_start_wide().limit, 18);
    register(services, "again@example.com", "2001:db8:77:100::1")
        .await
        .unwrap();
    limits(services, serde_json::json!({ "email_ip": 2 })).await;
    assert_eq!(services.limits().email_start_wide().limit, wide);
    env.db.cleanup().await;
}

#[tokio::test]
async fn с_порогом_по_почте_растёт_и_предел_ответов_капчи_но_не_ниже_прежнего() {
    let Some((env, ..)) = site("password", false).await else {
        return;
    };
    let services = &env.services;
    assert_eq!(services.limits().captcha().limit, 30);
    limits(services, serde_json::json!({ "email_ip": 1 })).await;
    assert_eq!(services.limits().captcha().limit, 30);
    limits(services, serde_json::json!({ "email_ip": 40 })).await;
    assert_eq!(services.limits().captcha().limit, 120);
    assert_eq!(rules::EMAIL_VERIFY_IP.limit, 20);
    env.db.cleanup().await;
}

#[tokio::test]
async fn список_доменов_капчи_без_домена_панели_не_сохраняется() {
    let Some((env, admin, _letters)) = site("password", false).await else {
        return;
    };
    let services = &env.services;
    save(
        services,
        admin,
        serde_json::json!({ "captcha_domains": "shop.example" }),
    )
    .await
    .unwrap();
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set(
                sn_domain::settings::keys::PUBLIC_URL,
                &serde_json::json!("https://LK.example.com:8443/panel"),
                None,
            )
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
    let refused = save(
        services,
        admin,
        serde_json::json!({ "captcha_domains": "shop.example, other.example" }),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code(), "modules.config");
    assert!(refused.to_string().contains("lk.example.com"), "{refused}");
    for domains in ["", "lk.example.com", "shop.example, example.com"] {
        save(
            services,
            admin,
            serde_json::json!({ "captcha_domains": domains }),
        )
        .await
        .unwrap();
    }
    save(
        services,
        admin,
        serde_json::json!({ "captcha": "none", "captcha_domains": "shop.example" }),
    )
    .await
    .unwrap();
    let broken = save(
        services,
        admin,
        serde_json::json!({ "captcha_domains": "lk.example.com, не-домен" }),
    )
    .await
    .unwrap_err();
    assert!(broken.to_string().contains("не домен"), "{broken}");
    env.db.cleanup().await;
}
