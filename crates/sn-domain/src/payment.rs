use crate::DomainError;
use crate::access::ClientChannel;
use crate::ids::{AdminId, ClientId, OrderId, PaymentId, RefundId};
use crate::money::Amount;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

pub mod providers {
    pub const BALANCE: &str = "balance";
    pub const STARS: &str = "stars";
    pub const MANUAL: &str = "manual";
    pub const CRYPTOPAY: &str = "cryptopay";
    pub const HELEKET: &str = "heleket";
    pub const LAVA: &str = "lava";
    pub const YOOKASSA: &str = "yookassa";
    pub const PLATEGA: &str = "platega";
    pub const YOOMONEY: &str = "yoomoney";
    pub const CLOUDPAYMENTS: &str = "cloudpayments";
    pub const OVERPAY: &str = "overpay";
    pub const GRAM: &str = "gram";
}

const fn shown() -> bool {
    true
}

pub const METHOD_TITLE_MAX: usize = 40;
pub const LABEL_HINT_MAX: usize = 80;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ButtonLabel {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodChoice {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaymentChannels {
    #[serde(default = "shown")]
    pub cabinet: bool,
    #[serde(default = "shown")]
    pub bot: bool,
    #[serde(default = "shown")]
    pub miniapp: bool,
}

impl Default for PaymentChannels {
    fn default() -> Self {
        PaymentChannels::ALL
    }
}

impl PaymentChannels {
    pub const ALL: PaymentChannels = PaymentChannels {
        cabinet: true,
        bot: true,
        miniapp: true,
    };

    pub const fn allows(self, channel: ClientChannel) -> bool {
        match channel {
            ClientChannel::Cabinet => self.cabinet,
            ClientChannel::Bot => self.bot,
            ClientChannel::Miniapp => self.miniapp,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderId(pub String);

impl ProviderId {
    pub fn new(id: impl Into<String>) -> Self {
        ProviderId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_internal(&self) -> bool {
        matches!(self.0.as_str(), providers::BALANCE | providers::MANUAL)
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ProviderId {
    fn from(value: &str) -> Self {
        ProviderId(value.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    Pending,
    Paid,
    Failed,
    Canceled,
    Refunded,
    PartiallyRefunded,
}

impl PaymentStatus {
    pub const ALL: [PaymentStatus; 6] = [
        PaymentStatus::Pending,
        PaymentStatus::Paid,
        PaymentStatus::Failed,
        PaymentStatus::Canceled,
        PaymentStatus::Refunded,
        PaymentStatus::PartiallyRefunded,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            PaymentStatus::Pending => "pending",
            PaymentStatus::Paid => "paid",
            PaymentStatus::Failed => "failed",
            PaymentStatus::Canceled => "canceled",
            PaymentStatus::Refunded => "refunded",
            PaymentStatus::PartiallyRefunded => "partially_refunded",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        PaymentStatus::ALL
            .into_iter()
            .find(|s| s.as_str() == value)
            .ok_or_else(|| DomainError::unknown("payment_status", value))
    }

    pub const fn is_paid_like(self) -> bool {
        matches!(
            self,
            PaymentStatus::Paid | PaymentStatus::Refunded | PaymentStatus::PartiallyRefunded
        )
    }

    pub const fn can_become(self, to: PaymentStatus) -> bool {
        use PaymentStatus::*;
        matches!(
            (self, to),
            (Pending, Paid)
                | (Pending, Failed)
                | (Pending, Canceled)
                | (Failed, Paid)
                | (Canceled, Paid)
                | (Paid, Refunded)
                | (Paid, PartiallyRefunded)
                | (PartiallyRefunded, PartiallyRefunded)
                | (PartiallyRefunded, Refunded)
        )
    }

    pub fn transition(self, to: PaymentStatus) -> Result<bool, DomainError> {
        if self == to && to != PaymentStatus::PartiallyRefunded {
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
pub struct Payment {
    pub id: PaymentId,
    pub public_id: Uuid,
    pub order_id: OrderId,
    pub client_id: ClientId,
    pub provider: ProviderId,
    pub method: Option<String>,
    pub pay_method: Option<String>,
    pub amount: Amount,
    pub status: PaymentStatus,
    pub provider_txid: Option<String>,
    pub pay_url: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub paid_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewPayment {
    pub order_id: OrderId,
    pub client_id: ClientId,
    pub provider: ProviderId,
    pub method: Option<String>,
    pub amount: Amount,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AmountCheck {
    Ok,
    Overpaid { expected: Amount, received: Amount },
    Underpaid { expected: Amount, received: Amount },
    CurrencyMismatch { expected: Amount, received: Amount },
    Missing,
}

impl AmountCheck {
    pub const fn accepts(self) -> bool {
        matches!(self, AmountCheck::Ok | AmountCheck::Overpaid { .. })
    }
}

pub fn check_paid_amount(expected: Amount, received: Option<Amount>) -> AmountCheck {
    let Some(received) = received else {
        return AmountCheck::Missing;
    };
    if received.currency != expected.currency {
        AmountCheck::CurrencyMismatch { expected, received }
    } else if received.minor < expected.minor {
        AmountCheck::Underpaid { expected, received }
    } else if received.minor > expected.minor {
        AmountCheck::Overpaid { expected, received }
    } else {
        AmountCheck::Ok
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    Dispute,
    Chargeback,
    UnmatchedRefund,
    RefundReported,
    AmlCheck,
    ManualCredit,
}

impl NoticeKind {
    pub const ALL: [NoticeKind; 6] = [
        NoticeKind::Dispute,
        NoticeKind::Chargeback,
        NoticeKind::UnmatchedRefund,
        NoticeKind::RefundReported,
        NoticeKind::AmlCheck,
        NoticeKind::ManualCredit,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            NoticeKind::Dispute => "dispute",
            NoticeKind::Chargeback => "chargeback",
            NoticeKind::UnmatchedRefund => "unmatched_refund",
            NoticeKind::RefundReported => "refund_reported",
            NoticeKind::AmlCheck => "aml_check",
            NoticeKind::ManualCredit => "manual_credit",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        NoticeKind::ALL
            .into_iter()
            .find(|k| k.as_str() == value)
            .ok_or_else(|| DomainError::unknown("notice_kind", value))
    }

    pub const fn title_ru(self) -> &'static str {
        match self {
            NoticeKind::Dispute => "Спор по платежу",
            NoticeKind::Chargeback => "Чарджбэк",
            NoticeKind::UnmatchedRefund => "Возврат не привязан к оплате",
            NoticeKind::RefundReported => "Платёжная система сообщила о возврате",
            NoticeKind::AmlCheck => "Платёж задержан проверкой AML",
            NoticeKind::ManualCredit => "Деньги ждут ручного зачисления",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefundMethod {
    Balance,
    Provider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefundSubscriptionAction {
    Keep,
    Cut,
    Disable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refund {
    pub id: RefundId,
    pub payment_id: PaymentId,
    pub amount: Amount,
    pub method: RefundMethod,
    pub subscription_action: RefundSubscriptionAction,
    pub reason: Option<String>,
    pub admin_id: Option<AdminId>,
    pub provider_refund_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::PayCurrency;

    #[test]
    fn недоплата_никогда_не_считается_оплатой() {
        let expected = Amount::new(699, PayCurrency::Usd);
        assert!(!check_paid_amount(expected, Some(Amount::new(698, PayCurrency::Usd))).accepts());
        assert!(!check_paid_amount(expected, Some(Amount::new(699, PayCurrency::Rub))).accepts());
        assert!(!check_paid_amount(expected, None).accepts());
        assert!(check_paid_amount(expected, Some(expected)).accepts());
        assert!(check_paid_amount(expected, Some(Amount::new(700, PayCurrency::Usd))).accepts());
    }

    #[test]
    fn оплата_после_отмены_и_ошибки_засчитывается() {
        assert_eq!(
            PaymentStatus::Canceled.transition(PaymentStatus::Paid),
            Ok(true)
        );
        assert_eq!(
            PaymentStatus::Failed.transition(PaymentStatus::Paid),
            Ok(true)
        );
        assert_eq!(
            PaymentStatus::Paid.transition(PaymentStatus::Paid),
            Ok(false)
        );
        assert!(
            PaymentStatus::Refunded
                .transition(PaymentStatus::Paid)
                .is_err()
        );
    }
}

#[cfg(test)]
mod channel_tests {
    use super::*;

    #[test]
    fn каналы_оплаты_по_умолчанию_все_и_читаются_из_старых_настроек() {
        assert_eq!(PaymentChannels::default(), PaymentChannels::ALL);
        let old: PaymentChannels = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(old, PaymentChannels::ALL);
        let site_only: PaymentChannels =
            serde_json::from_value(serde_json::json!({"bot": false, "miniapp": false})).unwrap();
        assert!(site_only.allows(ClientChannel::Cabinet));
        assert!(!site_only.allows(ClientChannel::Bot));
        assert!(!site_only.allows(ClientChannel::Miniapp));
    }
}
