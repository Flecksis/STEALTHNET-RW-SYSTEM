use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! numeric_id {
    ($($name:ident),* $(,)?) => {
        $(
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
            #[serde(transparent)]
            pub struct $name(pub i64);

            impl $name {
                pub const fn get(self) -> i64 {
                    self.0
                }
            }

            impl fmt::Display for $name {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    write!(f, "{}", self.0)
                }
            }

            impl From<i64> for $name {
                fn from(value: i64) -> Self {
                    Self(value)
                }
            }
        )*
    };
}

numeric_id!(
    ClientId,
    AdminId,
    CategoryId,
    TariffId,
    PeriodId,
    SubscriptionId,
    OrderId,
    PaymentId,
    RefundId,
    LedgerEntryId,
    JobId,
    EventId,
    TicketId,
    TicketFileId,
    RemnaUserId,
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TelegramId(pub i64);

impl fmt::Display for TelegramId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SquadId(pub Uuid);

impl fmt::Display for SquadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for SquadId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}
