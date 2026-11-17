use crate::*;
use chrono::{DateTime, Utc};
use sn_domain::client::{IdentityKind, Locale, NewIdentity, RegistrationSource};
use sn_domain::ids::{
    CategoryId, ClientId, OrderId, PaymentId, RemnaUserId, SquadId, SubscriptionId,
};
use sn_domain::money::{Amount, Currency, Money, PayCurrency, Stars};
use sn_domain::order::{NewOrder, OrderKind, PurchaseIntent};
use sn_domain::payment::{NewPayment, PaymentStatus, ProviderId};
use sn_domain::pricing::{Quote, StarsTotal};
use sn_domain::subscription::{
    AutoRenewState, RemnaLink, RemnaMirror, SubscriptionKind, SubscriptionStatus,
};
use sn_domain::tariff::{CurrencyPrices, DevicePolicy, ExtraDevices};
use sn_domain::traffic::{GIB, TrafficPolicy, TrafficReset};
use uuid::Uuid;

pub struct Person<'a> {
    pub telegram: Option<i64>,
    pub username: Option<&'a str>,
    pub first_name: Option<&'a str>,
    pub last_name: Option<&'a str>,
    pub email: Option<&'a str>,
}

impl Person<'_> {
    pub const NOBODY: Person<'static> = Person {
        telegram: None,
        username: None,
        first_name: None,
        last_name: None,
        email: None,
    };
}

pub async fn client(db: &mut impl Db, person: &Person<'_>) -> ClientId {
    let client = db
        .clients()
        .create(&NewClient {
            locale: Locale::Ru,
            registration_source: RegistrationSource::Bot,
            note: None,
        })
        .await
        .unwrap()
        .id;
    if let Some(telegram) = person.telegram {
        let mut identity =
            NewIdentity::new(IdentityKind::Telegram, &telegram.to_string(), true).unwrap();
        identity.meta = serde_json::json!({
            "username": person.username,
            "first_name": person.first_name,
            "last_name": person.last_name,
        });
        db.clients().add_identity(client, &identity).await.unwrap();
    }
    if let Some(email) = person.email {
        let identity = NewIdentity::new(IdentityKind::Email, email, true).unwrap();
        db.clients().add_identity(client, &identity).await.unwrap();
    }
    client
}

pub async fn category(db: &mut impl Db, name: &str) -> CategoryId {
    db.tariffs()
        .create_category(&NewCategory {
            name: name.into(),
            sort: 0,
            visible: true,
        })
        .await
        .unwrap()
        .id
}

pub fn period(days: u32, usd: i64, rub: i64, stars: i64) -> NewPeriod {
    NewPeriod {
        days,
        prices: CurrencyPrices {
            usd: Some(usd),
            rub: Some(rub),
        },
        stars: Some(Stars(stars)),
        sort: 0,
        active: true,
    }
}

pub fn tariff(category: CategoryId, periods: Vec<NewPeriod>, squads: &[u128]) -> NewTariff {
    NewTariff {
        category_id: category,
        name: "Unblock".into(),
        description: None,
        on_sale: true,
        sort: 0,
        devices: DevicePolicy {
            included: 3,
            extra: Some(ExtraDevices {
                max: 2,
                monthly: CurrencyPrices {
                    usd: Some(99),
                    rub: Some(4_900),
                },
                monthly_stars: Some(Stars(50)),
            }),
        },
        traffic: TrafficPolicy {
            limit_bytes: Some(300 * GIB),
            reset: TrafficReset::Month,
        },
        squads: squads
            .iter()
            .map(|n| SquadId(Uuid::from_u128(*n)))
            .collect(),
        external_squad: None,
        periods,
    }
}

pub fn quote(amount: Amount) -> Quote {
    let (money, stars) = match amount.as_money() {
        Some(money) => (money, None),
        None => (
            Money::zero(Currency::Usd),
            Some(StarsTotal {
                subtotal: Stars(amount.minor),
                discount: Stars::ZERO,
                total: Stars(amount.minor),
            }),
        ),
    };
    Quote {
        currency: money.currency,
        lines: Vec::new(),
        subtotal: money,
        discount: Money::zero(money.currency),
        total: money,
        stars,
        days: 30,
        extra_devices: 0,
    }
}

pub async fn order(db: &mut impl Db, client: ClientId, amount: Amount) -> OrderId {
    db.orders()
        .create(&NewOrder {
            client_id: client,
            subscription_id: None,
            kind: OrderKind::TopUp,
            intent: PurchaseIntent::TopUp {
                amount: amount.as_money().unwrap_or(Money::zero(Currency::Usd)),
            },
            quote: quote(amount),
            snapshot: None,
        })
        .await
        .unwrap()
        .id
}

pub async fn payment(
    db: &mut impl Db,
    client: ClientId,
    provider: &str,
    amount: Amount,
) -> PaymentId {
    let order = order(db, client, amount).await;
    db.payments()
        .create(&NewPayment {
            order_id: order,
            client_id: client,
            provider: ProviderId::from(provider),
            method: None,
            amount,
            expires_at: None,
        })
        .await
        .unwrap()
        .id
}

pub async fn paid(db: &mut impl Db, client: ClientId, provider: &str, amount: Amount) -> PaymentId {
    let id = payment(db, client, provider, amount).await;
    assert!(
        db.payments()
            .transition(id, PaymentStatus::Paid, None, None)
            .await
            .unwrap()
            .applied()
    );
    id
}

pub fn usd(minor: i64) -> Amount {
    Amount::new(minor, PayCurrency::Usd)
}

pub struct Sub {
    pub kind: SubscriptionKind,
    pub status: Option<SubscriptionStatus>,
    pub expire_at: Option<DateTime<Utc>>,
    pub connected: bool,
}

pub async fn subscription(db: &mut impl Db, client: ClientId, sub: Sub) -> SubscriptionId {
    let id = db
        .subscriptions()
        .create(&NewSubscription {
            client_id: client,
            tariff_id: None,
            kind: sub.kind,
            extra_devices: 0,
            autorenew: AutoRenewState::default(),
        })
        .await
        .unwrap()
        .id;
    if sub.status.is_none() && sub.expire_at.is_none() {
        return id;
    }
    db.subscriptions()
        .link_remna(
            id,
            &RemnaLink {
                user_id: RemnaUserId(10_000 + id.0),
                username: format!("sn_test_{}", id.0),
                short_uuid: Some(format!("short{}", id.0)),
            },
        )
        .await
        .unwrap();
    db.subscriptions()
        .apply_mirror(
            id,
            &RemnaMirror {
                status: sub.status,
                expire_at: sub.expire_at,
                first_connected_at: sub.connected.then(Utc::now),
                ..RemnaMirror::default()
            },
        )
        .await
        .unwrap();
    id
}
