use crate::DomainError;
use crate::ids::{ClientId, OrderId, PeriodId, SubscriptionId, TariffId};
use crate::module::ModuleId;
use crate::money::Money;
use crate::plan::FulfillmentPlan;
use crate::pricing::Quote;
use crate::tariff::TariffSnapshot;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderKind {
    New,
    Renewal,
    Change,
    TopUp,
    Trial,
    Module,
    AdminGrant,
}

impl OrderKind {
    pub const ALL: [OrderKind; 7] = [
        OrderKind::New,
        OrderKind::Renewal,
        OrderKind::Change,
        OrderKind::TopUp,
        OrderKind::Trial,
        OrderKind::Module,
        OrderKind::AdminGrant,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            OrderKind::New => "new",
            OrderKind::Renewal => "renewal",
            OrderKind::Change => "change",
            OrderKind::TopUp => "topup",
            OrderKind::Trial => "trial",
            OrderKind::Module => "module",
            OrderKind::AdminGrant => "admin_grant",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        OrderKind::ALL
            .into_iter()
            .find(|k| k.as_str() == value)
            .ok_or_else(|| DomainError::unknown("order_kind", value))
    }

    pub const fn needs_remnawave(self) -> bool {
        !matches!(self, OrderKind::TopUp | OrderKind::Module)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    Pending,
    Paid,
    Fulfilled,
    Review,
    Canceled,
    Refunded,
}

impl OrderStatus {
    pub const ALL: [OrderStatus; 6] = [
        OrderStatus::Pending,
        OrderStatus::Paid,
        OrderStatus::Fulfilled,
        OrderStatus::Review,
        OrderStatus::Canceled,
        OrderStatus::Refunded,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            OrderStatus::Pending => "pending",
            OrderStatus::Paid => "paid",
            OrderStatus::Fulfilled => "fulfilled",
            OrderStatus::Review => "review",
            OrderStatus::Canceled => "canceled",
            OrderStatus::Refunded => "refunded",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        OrderStatus::ALL
            .into_iter()
            .find(|s| s.as_str() == value)
            .ok_or_else(|| DomainError::unknown("order_status", value))
    }

    pub const fn can_become(self, to: OrderStatus) -> bool {
        use OrderStatus::*;
        matches!(
            (self, to),
            (Pending, Paid)
                | (Pending, Canceled)
                | (Canceled, Paid)
                | (Paid, Fulfilled)
                | (Paid, Review)
                | (Paid, Refunded)
                | (Review, Fulfilled)
                | (Review, Refunded)
                | (Fulfilled, Refunded)
        )
    }

    pub fn transition(self, to: OrderStatus) -> Result<bool, DomainError> {
        if self == to {
            Ok(false)
        } else if self.can_become(to) {
            Ok(true)
        } else {
            Err(DomainError::InvalidTransition {
                from: self.as_str(),
                to: to.as_str(),
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PurchaseIntent {
    Subscription {
        tariff_id: TariffId,
        period_id: PeriodId,
        extra_devices: u32,
        target: Option<SubscriptionId>,
    },
    TopUp {
        amount: Money,
    },
    Trial,
    Module {
        module: ModuleId,
        kind: String,
        payload: serde_json::Value,
    },
    AdminGrant {
        tariff_id: TariffId,
        period_id: Option<PeriodId>,
        days: u32,
        note: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    pub id: OrderId,
    pub public_id: Uuid,
    pub client_id: ClientId,
    pub subscription_id: Option<SubscriptionId>,
    pub kind: OrderKind,
    pub status: OrderStatus,
    pub intent: PurchaseIntent,
    pub quote: Quote,
    pub snapshot: Option<TariffSnapshot>,
    pub plan: Option<FulfillmentPlan>,
    pub created_at: DateTime<Utc>,
    pub paid_at: Option<DateTime<Utc>>,
    pub fulfilled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewOrder {
    pub client_id: ClientId,
    pub subscription_id: Option<SubscriptionId>,
    pub kind: OrderKind,
    pub intent: PurchaseIntent,
    pub quote: Quote,
    pub snapshot: Option<TariffSnapshot>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn поздняя_оплата_отменённого_заказа_принимается() {
        assert_eq!(
            OrderStatus::Canceled.transition(OrderStatus::Paid),
            Ok(true)
        );
        assert_eq!(OrderStatus::Paid.transition(OrderStatus::Paid), Ok(false));
        assert!(
            OrderStatus::Fulfilled
                .transition(OrderStatus::Pending)
                .is_err()
        );
        assert!(OrderStatus::Refunded.transition(OrderStatus::Paid).is_err());
    }
}
