use crate::ids::{PeriodId, TariffId};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("суммы в разных валютах: {left} и {right}")]
    CurrencyMismatch {
        left: &'static str,
        right: &'static str,
    },
    #[error("сумма вне допустимого диапазона")]
    AmountOverflow,
    #[error("сумма не может быть отрицательной")]
    NegativeAmount,
    #[error("неизвестная валюта: {0}")]
    UnknownCurrency(String),
    #[error("процент должен быть от 0 до 100, получено {0}")]
    InvalidPercent(u8),
    #[error("у тарифа нет срока {0}")]
    PeriodNotFound(PeriodId),
    #[error("срок {0} не продаётся")]
    PeriodInactive(PeriodId),
    #[error("тариф {0} снят с продажи")]
    TariffNotOnSale(TariffId),
    #[error("у срока нет цены в валюте {0}")]
    NoPriceInCurrency(&'static str),
    #[error("у срока нет цены в звёздах")]
    NoStarsPrice,
    #[error("доп. устройства у этого тарифа не продаются")]
    ExtraDevicesNotSold,
    #[error("можно докупить не больше {max} устройств")]
    TooManyExtraDevices { max: u32 },
    #[error("сумма пополнения должна быть от {min} до {max}")]
    TopUpOutOfRange { min: String, max: String },
    #[error("переход {from} → {to} запрещён")]
    InvalidTransition {
        from: &'static str,
        to: &'static str,
    },
    #[error("неверное значение поля «{field}»: {reason}")]
    InvalidValue { field: &'static str, reason: String },
    #[error("неизвестное значение «{value}» для {kind}")]
    UnknownVariant { kind: &'static str, value: String },
    #[error("у подписки нет пользователя Remnawave")]
    NoRemnaUser,
    #[error("операция с балансом нарушает правило: {0}")]
    LedgerRule(&'static str),
}

impl DomainError {
    pub fn invalid(field: &'static str, reason: impl Into<String>) -> Self {
        DomainError::InvalidValue {
            field,
            reason: reason.into(),
        }
    }

    pub fn unknown(kind: &'static str, value: impl Into<String>) -> Self {
        DomainError::UnknownVariant {
            kind,
            value: value.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            DomainError::CurrencyMismatch { .. } => "money.currency_mismatch",
            DomainError::AmountOverflow => "money.overflow",
            DomainError::NegativeAmount => "money.negative",
            DomainError::UnknownCurrency(_) => "money.unknown_currency",
            DomainError::InvalidPercent(_) => "pricing.invalid_percent",
            DomainError::PeriodNotFound(_) => "tariff.period_not_found",
            DomainError::PeriodInactive(_) => "tariff.period_inactive",
            DomainError::TariffNotOnSale(_) => "tariff.not_on_sale",
            DomainError::NoPriceInCurrency(_) => "tariff.no_price",
            DomainError::NoStarsPrice => "tariff.no_stars_price",
            DomainError::ExtraDevicesNotSold => "tariff.extra_devices_not_sold",
            DomainError::TooManyExtraDevices { .. } => "tariff.too_many_devices",
            DomainError::TopUpOutOfRange { .. } => "balance.topup_out_of_range",
            DomainError::InvalidTransition { .. } => "state.invalid_transition",
            DomainError::InvalidValue { .. } => "validation.invalid_value",
            DomainError::UnknownVariant { .. } => "validation.unknown_variant",
            DomainError::NoRemnaUser => "subscription.no_remna_user",
            DomainError::LedgerRule(_) => "balance.rule",
        }
    }
}
