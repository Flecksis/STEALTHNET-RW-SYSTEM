use super::fixtures::*;
use crate::testing::test_store;
use crate::*;
use chrono::{Duration, Utc};
use sn_domain::client::RegistrationSource;
use sn_domain::events::DomainEvent;
use sn_domain::ids::ClientId;
use sn_domain::ledger::{LedgerKind, NewLedgerEntry, Wallet};
use sn_domain::money::{Amount, Currency, Money, PayCurrency};
use sn_domain::paging::PageRequest;
use sn_domain::subscription::{SubscriptionKind, SubscriptionStatus};
use std::collections::HashSet;

async fn found(
    db: &mut impl Db,
    segment: Option<ClientSegment>,
    query: Option<&str>,
) -> Vec<ClientId> {
    db.clients()
        .search_at(
            &ClientFilter {
                segment,
                query: query.map(str::to_string),
            },
            PageRequest::default(),
            Utc::now(),
            Currency::Usd,
        )
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|item| item.client.id)
        .collect()
}

fn active(kind: SubscriptionKind, days: i64, connected: bool) -> Sub {
    Sub {
        kind,
        status: Some(SubscriptionStatus::Active),
        expire_at: Some(Utc::now() + Duration::days(days)),
        connected,
    }
}

#[tokio::test]
async fn клиенты_ищутся_по_сегментам_и_строке() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let now = Utc::now();
    let alexey = client(
        &mut conn,
        &Person {
            telegram: Some(5_093_118_422),
            username: Some("morozov_a"),
            first_name: Some("Алексей"),
            last_name: Some("Морозов"),
            email: Some("a.morozov@example.com"),
        },
    )
    .await;
    let alexey_sub =
        subscription(&mut conn, alexey, active(SubscriptionKind::Paid, 90, true)).await;
    let ksenia = client(
        &mut conn,
        &Person {
            telegram: Some(1_001),
            username: Some("ks_smirnova"),
            first_name: Some("Ксения"),
            last_name: Some("Смирнова"),
            email: None,
        },
    )
    .await;
    subscription(&mut conn, ksenia, active(SubscriptionKind::Paid, 2, true)).await;
    let maria = client(
        &mut conn,
        &Person {
            email: Some("m.lebedeva@example.com"),
            ..Person::NOBODY
        },
    )
    .await;
    subscription(&mut conn, maria, active(SubscriptionKind::Trial, 2, false)).await;
    let igor = client(
        &mut conn,
        &Person {
            telegram: Some(1_002),
            username: Some("igor_nv"),
            ..Person::NOBODY
        },
    )
    .await;
    subscription(
        &mut conn,
        igor,
        Sub {
            status: Some(SubscriptionStatus::Limited),
            ..active(SubscriptionKind::Paid, 60, true)
        },
    )
    .await;
    let artem = client(
        &mut conn,
        &Person {
            telegram: Some(1_003),
            username: Some("artem_p"),
            ..Person::NOBODY
        },
    )
    .await;
    subscription(
        &mut conn,
        artem,
        Sub {
            status: Some(SubscriptionStatus::Expired),
            ..active(SubscriptionKind::Paid, -5, true)
        },
    )
    .await;
    let oleg = client(
        &mut conn,
        &Person {
            telegram: Some(1_004),
            username: Some("oleg_s"),
            ..Person::NOBODY
        },
    )
    .await;
    subscription(
        &mut conn,
        oleg,
        Sub {
            status: Some(SubscriptionStatus::Disabled),
            ..active(SubscriptionKind::Paid, 30, true)
        },
    )
    .await;
    conn.clients()
        .set_status(
            oleg,
            sn_domain::client::ClientStatus::Blocked,
            Some("Злоупотребление трафиком"),
        )
        .await
        .unwrap();
    let bare = client(
        &mut conn,
        &Person {
            telegram: Some(7_412),
            ..Person::NOBODY
        },
    )
    .await;
    let alena = client(
        &mut conn,
        &Person {
            telegram: Some(1_005),
            username: Some("alena"),
            first_name: Some("Алёна"),
            last_name: Some("Ёлкина"),
            email: None,
        },
    )
    .await;
    subscription(
        &mut conn,
        alena,
        Sub {
            status: Some(SubscriptionStatus::Expired),
            ..active(SubscriptionKind::Paid, -40, true)
        },
    )
    .await;
    let alena_sub = subscription(&mut conn, alena, active(SubscriptionKind::Paid, 20, true)).await;
    let gone = client(
        &mut conn,
        &Person {
            telegram: Some(1_006),
            username: Some("gone"),
            ..Person::NOBODY
        },
    )
    .await;
    subscription(&mut conn, gone, active(SubscriptionKind::Paid, 20, true)).await;
    conn.clients().soft_delete(gone).await.unwrap();
    let pending = client(
        &mut conn,
        &Person {
            telegram: Some(1_007),
            ..Person::NOBODY
        },
    )
    .await;
    subscription(
        &mut conn,
        pending,
        Sub {
            status: None,
            expire_at: None,
            ..active(SubscriptionKind::Paid, 0, false)
        },
    )
    .await;
    let missing = client(
        &mut conn,
        &Person {
            telegram: Some(1_008),
            ..Person::NOBODY
        },
    )
    .await;
    let lost = subscription(&mut conn, missing, active(SubscriptionKind::Paid, 20, true)).await;
    conn.subscriptions().mark_missing(lost, true).await.unwrap();

    conn.ledger()
        .append(&NewLedgerEntry {
            client_id: alexey,
            wallet: Wallet::Main,
            amount: Money::new(420, Currency::Usd),
            kind: LedgerKind::TopUp,
            order_id: None,
            payment_id: None,
            admin_id: None,
            comment: None,
            idempotency_key: Some("topup:alexey".into()),
        })
        .await
        .unwrap();
    paid(&mut conn, alexey, "cryptopay", usd(699)).await;
    paid(&mut conn, alexey, "cryptopay", usd(640)).await;
    paid(&mut conn, alexey, "balance", usd(300)).await;
    paid(
        &mut conn,
        alexey,
        "stars",
        Amount::new(350, PayCurrency::Xtr),
    )
    .await;
    payment(&mut conn, alexey, "cryptopay", usd(999)).await;

    let counts = conn.clients().segment_counts(now).await.unwrap();
    assert_eq!(
        counts,
        SegmentCounts {
            total: 10,
            active: 3,
            trial: 1,
            no_subscription: 3,
            expiring_soon: 2,
            never_connected: 1,
            expired: 1,
            limited: 1,
            blocked: 1,
        }
    );
    assert_eq!(counts.get(Some(ClientSegment::Limited)), 1);

    assert_eq!(
        found(&mut conn, Some(ClientSegment::Active), None).await,
        [alena, ksenia, alexey]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::ExpiringSoon), None).await,
        [maria, ksenia]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::Trial), None).await,
        [maria]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::NeverConnected), None).await,
        [maria]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::Expired), None).await,
        [artem]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::Limited), None).await,
        [igor]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::Blocked), None).await,
        [oleg]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::NoSubscription), None).await,
        [missing, pending, bare]
    );
    assert_eq!(found(&mut conn, None, None).await.len(), 10);

    for (query, expected) in [
        ("морозов", vec![alexey]),
        ("МОРОЗОВ алексей", vec![alexey]),
        ("алена елкина", vec![alena]),
        ("@MOROZOV", vec![alexey]),
        ("@morozov_a", vec![alexey]),
        ("@%", vec![]),
        ("5093118422", vec![alexey]),
        ("7412", vec![bare]),
        ("a.morozov@example", vec![alexey]),
        ("M.Lebedeva@example.com", vec![maria]),
        ("нет такого", vec![]),
    ] {
        assert_eq!(
            found(&mut conn, None, Some(query)).await,
            expected,
            "{query}"
        );
    }
    assert_eq!(
        found(&mut conn, None, Some(&igor.0.to_string())).await,
        [igor]
    );
    let alexey_client = conn.clients().get(alexey).await.unwrap();
    assert_eq!(
        found(&mut conn, None, Some(&alexey_client.public_id.to_string())).await,
        [alexey]
    );
    let short = format!("https://sub.example.com/short{}", alexey_sub.0);
    assert_eq!(found(&mut conn, None, Some(&short)).await, [alexey]);
    assert_eq!(
        found(&mut conn, None, Some(&format!("short{}", alexey_sub.0))).await,
        [alexey]
    );
    assert_eq!(
        found(&mut conn, Some(ClientSegment::Active), Some("ксения")).await,
        [ksenia]
    );
    assert!(
        found(&mut conn, Some(ClientSegment::Expired), Some("ксения"))
            .await
            .is_empty()
    );

    let rows = conn
        .clients()
        .search_at(
            &ClientFilter {
                segment: Some(ClientSegment::Active),
                query: None,
            },
            PageRequest::default(),
            now,
            Currency::Usd,
        )
        .await
        .unwrap()
        .items;
    let first = rows.iter().find(|r| r.client.id == alexey).unwrap();
    assert_eq!(first.name.as_deref(), Some("Алексей Морозов"));
    assert_eq!(first.telegram_id.as_deref(), Some("5093118422"));
    assert_eq!(first.telegram_username.as_deref(), Some("morozov_a"));
    assert_eq!(first.email.as_deref(), Some("a.morozov@example.com"));
    assert_eq!(first.balance_minor, 420);
    assert_eq!(first.paid_total_minor, 1_339);
    assert_eq!(first.subscription.as_ref().map(|s| s.id), Some(alexey_sub));
    let two = rows.iter().find(|r| r.client.id == alena).unwrap();
    assert_eq!(two.subscription.as_ref().map(|s| s.id), Some(alena_sub));

    let mut seen = HashSet::new();
    let mut after = None;
    let mut pages = 0;
    loop {
        let page = conn
            .clients()
            .search_at(
                &ClientFilter::default(),
                PageRequest::new(Some(3), after),
                now,
                Currency::Usd,
            )
            .await
            .unwrap();
        pages += 1;
        for item in &page.items {
            assert!(seen.insert(item.client.id));
        }
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    assert_eq!((seen.len(), pages), (10, 4));
    assert!(!seen.contains(&gone));

    let by_default = conn
        .clients()
        .search(&ClientFilter::default(), PageRequest::default())
        .await
        .unwrap();
    assert_eq!(by_default.items.len(), 10);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn история_клиента_одной_лентой_без_потерь_на_границе_страниц() {
    let Some(db) = test_store().await else { return };
    let mut tx = db.store.begin().await.unwrap();
    let alexey = client(&mut tx, &Person::NOBODY).await;
    let other = client(&mut tx, &Person::NOBODY).await;
    let sub = subscription(&mut tx, alexey, active(SubscriptionKind::Paid, 90, true)).await;
    let payment_id = paid(&mut tx, alexey, "cryptopay", usd(699)).await;
    tx.events()
        .push(&DomainEvent::ClientRegistered {
            client_id: alexey,
            source: RegistrationSource::Bot,
            start_param: None,
        })
        .await
        .unwrap();
    tx.audit()
        .record(&AuditEntry {
            actor_kind: ActorKind::Admin,
            actor_id: None,
            action: "subscription.add_days".into(),
            entity_kind: Some(audit_entity::SUBSCRIPTION.into()),
            entity_id: Some(sub.0),
            details: serde_json::json!({"days": 7}),
            ip: None,
        })
        .await
        .unwrap();
    tx.audit()
        .record(&AuditEntry {
            actor_kind: ActorKind::Client,
            actor_id: Some(alexey.0),
            action: "cabinet.login".into(),
            entity_kind: None,
            entity_id: None,
            details: serde_json::json!({}),
            ip: None,
        })
        .await
        .unwrap();
    tx.audit()
        .record(&AuditEntry {
            actor_kind: ActorKind::Admin,
            actor_id: None,
            action: "client.note".into(),
            entity_kind: Some(audit_entity::CLIENT.into()),
            entity_id: Some(other.0),
            details: serde_json::json!({}),
            ip: None,
        })
        .await
        .unwrap();
    paid(&mut tx, other, "cryptopay", usd(100)).await;
    tx.commit().await.unwrap();

    let mut conn = db.store.conn().await.unwrap();
    let later = paid(&mut conn, alexey, "cryptopay", usd(640)).await;
    assert_eq!(conn.clients().history_count(alexey).await.unwrap(), 7);

    let first = conn
        .clients()
        .history(alexey, PageRequest::new(Some(2), None))
        .await
        .unwrap();
    assert_eq!(first.items.len(), 2);
    assert!(matches!(&first.items[0].entry, ClientHistoryEntry::Payment(p) if p.id == later));
    assert!(matches!(
        &first.items[1].entry,
        ClientHistoryEntry::Order(_)
    ));
    assert!(first.next.is_some());

    let second = conn
        .clients()
        .history(alexey, PageRequest::new(Some(2), first.next))
        .await
        .unwrap();
    assert_eq!(second.items.len(), 5);
    assert!(
        second
            .items
            .iter()
            .all(|item| item.at == second.items[0].at)
    );
    assert_eq!(second.next, None);
    let kinds: Vec<&str> = second
        .items
        .iter()
        .map(|item| match &item.entry {
            ClientHistoryEntry::Audit(_) => "audit",
            ClientHistoryEntry::Event(_) => "event",
            ClientHistoryEntry::Payment(_) => "payment",
            ClientHistoryEntry::Order(_) => "order",
        })
        .collect();
    assert_eq!(kinds, ["audit", "audit", "event", "payment", "order"]);
    assert!(
        second.items.iter().any(
            |item| matches!(&item.entry, ClientHistoryEntry::Payment(p) if p.id == payment_id)
        )
    );
    let json = serde_json::to_value(&second.items[0]).unwrap();
    assert_eq!(json["entry"]["type"], "audit");

    let empty = conn
        .clients()
        .history(
            alexey,
            PageRequest::new(Some(50), Some(second.items[0].at.timestamp_micros())),
        )
        .await
        .unwrap();
    assert!(empty.items.is_empty());
    drop(conn);
    db.cleanup().await;
}
