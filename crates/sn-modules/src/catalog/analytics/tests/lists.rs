use super::money::scene;
use super::*;
use crate::catalog::analytics::lists::{ListQuery, page};
use crate::catalog::testing::{TestCore, context};
use crate::catalog::utm::links::{LinkInput, LinksQuery, check};
use sn_store::testing::test_store;

fn march(set: &str, slice: Option<&str>) -> ListQuery {
    ListQuery {
        set: set.into(),
        slice: slice.map(str::to_string),
        from: Some(day(2026, 3, 1)),
        to: Some(day(2026, 3, 31)),
        ..ListQuery::default()
    }
}

#[tokio::test]
async fn списки_за_цифрами_совпадают_с_цифрами() {
    let Some(db) = test_store().await else {
        return;
    };
    let s = scene(db.store.clone()).await;
    spans::refresh(&db.store, Currency::Rub, None)
        .await
        .unwrap();
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Rub);
    let renewals = page(&ctx, &march("revenue", Some("kind:renewal")), now())
        .await
        .unwrap();
    assert_eq!(renewals.total, 1);
    assert_eq!(renewals.items[0].client_id, s.b);
    assert_eq!(renewals.items[0].amount, Some(99_000));
    assert!(renewals.items[0].what.starts_with("Продления"));
    let all = page(&ctx, &march("revenue", None), now()).await.unwrap();
    assert_eq!(all.total, 4);
    let registered = page(&ctx, &march("registered", None), now()).await.unwrap();
    assert_eq!(registered.total, 4);
    let bots = page(&ctx, &march("registered", Some("channel:bot")), now())
        .await
        .unwrap();
    assert_eq!(bots.total, 3);
    let fresh = page(&ctx, &march("mrr_new", None), now()).await.unwrap();
    assert_eq!(fresh.total, 3);
    let yk = page(&ctx, &march("invoices", Some("provider:yookassa")), now())
        .await
        .unwrap();
    assert_eq!(yk.total, 2);
    let vk = page(&ctx, &march("source", Some("utm:vk")), now())
        .await
        .unwrap();
    assert_eq!(vk.total, 1);
    assert_eq!(vk.items[0].client_id, s.e);
}

#[tokio::test]
async fn ссылки_меток_проверяются_и_получают_цифры() {
    let Some(db) = test_store().await else {
        return;
    };
    let _s = scene(db.store.clone()).await;
    let input = |source: &str, campaign: &str| LinkInput {
        title: "Паблики про игры".into(),
        source: source.into(),
        medium: None,
        campaign: Some(campaign.into()),
        content: None,
        term: None,
    };
    let (_, utm) = check(&input("vk", "games")).unwrap();
    assert_eq!(utm.to_param(), "s_vk__k_games");
    assert!(check(&input("вк", "games")).is_err());
    assert!(check(&input("vk", "a__b")).is_err());
    assert!(check(&input("vk", &"x".repeat(60))).is_err());
    assert!(
        check(&LinkInput {
            title: " ".into(),
            ..input("vk", "games")
        })
        .is_err()
    );
    let core = TestCore::new(&db.store);
    let ctx = context(&db.store, &core, Currency::Rub);
    {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query("INSERT INTO utm_links (title, source, campaign) VALUES ('Паблики про игры', 'vk', 'games')")
            .execute(conn.conn())
            .await
            .unwrap();
        sqlx::query("INSERT INTO settings (key, value) VALUES ('bot.username', '\"example_vpn_bot\"'), ('core.currency', '\"RUB\"') ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    spans::refresh(&db.store, Currency::Rub, Some(chrono::Duration::zero()))
        .await
        .unwrap();
    let q = LinksQuery {
        from: Some(day(2026, 2, 1)),
        to: Some(day(2026, 3, 31)),
    };
    let found = crate::catalog::utm::links::page(&ctx, &q, now())
        .await
        .unwrap();
    assert_eq!(found.bot.as_deref(), Some("example_vpn_bot"));
    assert!(!found.preparing);
    assert_eq!(found.links.len(), 1);
    assert_eq!(found.links[0].param, "s_vk__k_games");
    assert_eq!(
        (
            found.links[0].stats.registrations,
            found.links[0].stats.paid,
            found.links[0].stats.revenue
        ),
        (1, 1, 15_000)
    );
    assert_eq!(found.stray.len(), 1);
    assert_eq!(found.stray[0].source_key, "tiktok");
    assert_eq!(found.stray[0].registrations, 1);
    {
        let mut conn = db.store.conn().await.unwrap();
        sqlx::query("UPDATE utm_links SET archived_at = now() WHERE source = 'vk'")
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let after = crate::catalog::utm::links::page(&ctx, &q, now())
        .await
        .unwrap();
    assert!(after.links.is_empty());
    let vk = after
        .stray
        .iter()
        .find(|s| s.source_key == "vk" && s.campaign == "games")
        .unwrap();
    assert_eq!((vk.registrations, vk.paid, vk.revenue), (1, 1, 15_000));
}
