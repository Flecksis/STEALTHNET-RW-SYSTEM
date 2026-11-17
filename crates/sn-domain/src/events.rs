use crate::client::RegistrationSource;
use crate::ids::{ClientId, OrderId, PaymentId, SubscriptionId, TariffId, TicketId};
use crate::ledger::{LedgerKind, Wallet};
use crate::money::{Amount, Money};
use crate::order::OrderKind;
use crate::payment::ProviderId;
use crate::subscription::{SubscriptionKind, SubscriptionStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomainEvent {
    ClientRegistered {
        client_id: ClientId,
        source: RegistrationSource,
        start_param: Option<String>,
    },
    ClientBlocked {
        client_id: ClientId,
        reason: Option<String>,
    },
    ClientUnblocked {
        client_id: ClientId,
    },
    PaymentPaid {
        payment_id: PaymentId,
        order_id: OrderId,
        client_id: ClientId,
        order_kind: OrderKind,
        amount: Amount,
        provider: ProviderId,
    },
    PaymentRefunded {
        payment_id: PaymentId,
        order_id: OrderId,
        client_id: ClientId,
        amount: Amount,
    },
    OrderFulfilled {
        order_id: OrderId,
        client_id: ClientId,
        order_kind: OrderKind,
        subscription_id: Option<SubscriptionId>,
    },
    SubscriptionCreated {
        subscription_id: SubscriptionId,
        client_id: ClientId,
        kind: SubscriptionKind,
    },
    SubscriptionRenewed {
        subscription_id: SubscriptionId,
        client_id: ClientId,
        expire_at: DateTime<Utc>,
    },
    TariffChanged {
        subscription_id: SubscriptionId,
        client_id: ClientId,
        from: Option<TariffId>,
        to: TariffId,
    },
    SubscriptionStatusChanged {
        subscription_id: SubscriptionId,
        client_id: ClientId,
        from: Option<SubscriptionStatus>,
        to: SubscriptionStatus,
    },
    BalanceChanged {
        client_id: ClientId,
        wallet: Wallet,
        kind: LedgerKind,
        delta: Money,
    },
    TicketOpened {
        ticket_id: TicketId,
        client_id: ClientId,
    },
    TicketAnswered {
        ticket_id: TicketId,
        client_id: ClientId,
    },
}

impl DomainEvent {
    pub const fn kind(&self) -> &'static str {
        match self {
            DomainEvent::ClientRegistered { .. } => "client_registered",
            DomainEvent::ClientBlocked { .. } => "client_blocked",
            DomainEvent::ClientUnblocked { .. } => "client_unblocked",
            DomainEvent::PaymentPaid { .. } => "payment_paid",
            DomainEvent::PaymentRefunded { .. } => "payment_refunded",
            DomainEvent::OrderFulfilled { .. } => "order_fulfilled",
            DomainEvent::SubscriptionCreated { .. } => "subscription_created",
            DomainEvent::SubscriptionRenewed { .. } => "subscription_renewed",
            DomainEvent::TariffChanged { .. } => "tariff_changed",
            DomainEvent::SubscriptionStatusChanged { .. } => "subscription_status_changed",
            DomainEvent::BalanceChanged { .. } => "balance_changed",
            DomainEvent::TicketOpened { .. } => "ticket_opened",
            DomainEvent::TicketAnswered { .. } => "ticket_answered",
        }
    }

    pub const fn client_id(&self) -> ClientId {
        match self {
            DomainEvent::ClientRegistered { client_id, .. }
            | DomainEvent::ClientBlocked { client_id, .. }
            | DomainEvent::ClientUnblocked { client_id }
            | DomainEvent::PaymentPaid { client_id, .. }
            | DomainEvent::PaymentRefunded { client_id, .. }
            | DomainEvent::OrderFulfilled { client_id, .. }
            | DomainEvent::SubscriptionCreated { client_id, .. }
            | DomainEvent::SubscriptionRenewed { client_id, .. }
            | DomainEvent::TariffChanged { client_id, .. }
            | DomainEvent::SubscriptionStatusChanged { client_id, .. }
            | DomainEvent::BalanceChanged { client_id, .. }
            | DomainEvent::TicketOpened { client_id, .. }
            | DomainEvent::TicketAnswered { client_id, .. } => *client_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::PayCurrency;

    #[test]
    fn событие_сериализуется_с_типом() {
        let event = DomainEvent::PaymentPaid {
            payment_id: PaymentId(5),
            order_id: OrderId(4),
            client_id: ClientId(3),
            order_kind: OrderKind::Renewal,
            amount: Amount::new(699, PayCurrency::Usd),
            provider: ProviderId::from("cryptopay"),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "payment_paid");
        assert_eq!(json["amount"]["currency"], "USD");
        assert_eq!(serde_json::from_value::<DomainEvent>(json).unwrap(), event);
        assert_eq!(event.kind(), "payment_paid");
        assert_eq!(event.client_id(), ClientId(3));
    }
}
