use super::*;
use crate::checkout::testkit::*;
use sn_domain::ledger::{LedgerKind, Wallet};
use sn_domain::money::Currency;

fn contest(client_id: ClientId, minor: i64, key: &str) -> BalanceChange {
    BalanceChange {
        client_id,
        wallet: Wallet::Main,
        amount: Money::new(minor, Currency::Usd),
        kind: LedgerKind::Contest,
        comment: Some("Конкурс".into()),
        idempotency_key: key.into(),
        order_id: None,
        payment_id: None,
    }
}

#[tokio::test]
async fn модуль_меняет_баланс_через_ядро_один_раз_на_ключ() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 401).await;
    let services = &kit.services;
    let prize = contest(client_id, 500, "contest:1:winner");
    assert_eq!(
        services.change_balance(prize.clone()).await.unwrap(),
        Money::new(500, Currency::Usd)
    );
    assert_eq!(
        services.change_balance(prize).await.unwrap(),
        Money::new(500, Currency::Usd)
    );
    assert_eq!(balance(services, client_id).await, 500);
    assert!(matches!(
        services
            .change_balance(contest(client_id, -800, "contest:2:fine"))
            .await,
        Err(ModuleError::Core(_))
    ));
    assert_eq!(balance(services, client_id).await, 500);
    let rubles = BalanceChange {
        amount: Money::new(100, Currency::Rub),
        ..contest(client_id, 100, "contest:3")
    };
    assert!(matches!(
        services.change_balance(rubles).await,
        Err(ModuleError::Core(_))
    ));
    let referral = BalanceChange {
        wallet: Wallet::Referral,
        kind: LedgerKind::ReferralCredit,
        ..contest(client_id, 120, "referral:9:1")
    };
    assert_eq!(
        services.change_balance(referral).await.unwrap(),
        Money::new(120, Currency::Usd)
    );
    assert_eq!(balance(services, client_id).await, 500);
    let changes = events(services, client_id)
        .await
        .into_iter()
        .filter(|e| matches!(e, DomainEvent::BalanceChanged { .. }))
        .count();
    assert_eq!(changes, 2);
    kit.db.cleanup().await;
}

#[tokio::test]
async fn уведомление_и_задача_модуля_ставятся_в_очередь() {
    let Some(kit) = kit().await else { return };
    let client_id = client(&kit.services, 402).await;
    kit.services
        .notify(
            client_id,
            Notification {
                template: "promo.applied".into(),
                params: serde_json::json!({"code": "ZIMA"}),
            },
        )
        .await
        .unwrap();
    kit.services
        .enqueue(JobPayload::SyncSubscription {
            subscription_id: SubscriptionId(77),
        })
        .await
        .unwrap();
    assert_eq!(
        jobs(&kit.services).await,
        vec![
            JobPayload::Notify {
                client_id,
                template: "promo.applied".into(),
                params: serde_json::json!({"code": "ZIMA"}),
            },
            JobPayload::SyncSubscription {
                subscription_id: SubscriptionId(77)
            },
        ]
    );
    kit.db.cleanup().await;
}

#[tokio::test]
async fn ошибка_добавления_дней_возвращается_модулю_с_причиной() {
    let Some(kit) = kit().await else { return };
    let err = kit
        .services
        .add_days(SubscriptionId(999), 3, "промокод ZIMA")
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ModuleError::Core(message) if message.starts_with("промокод ZIMA")),
        "{err:?}"
    );
    kit.db.cleanup().await;
}
