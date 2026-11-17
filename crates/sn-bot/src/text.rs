use chrono::{DateTime, Datelike, FixedOffset, Utc};
use sn_domain::format;
use sn_domain::money::Amount;
use sn_domain::subscription::{Subscription, SubscriptionKind, SubscriptionStatus};
use sn_modules::ModuleError;
use sn_services::ServiceError;
use sn_store::StoreError;

const MONTHS: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];

pub const NB: char = format::NBSP;

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

pub fn date(at: DateTime<Utc>, offset: FixedOffset) -> String {
    let local = at.with_timezone(&offset);
    let month = MONTHS[local.month0() as usize];
    format!("{}{NB}{month}{NB}{}", local.day(), local.year())
}

pub fn day_month(at: DateTime<Utc>, offset: FixedOffset) -> String {
    let local = at.with_timezone(&offset);
    format!("{}{NB}{}", local.day(), MONTHS[local.month0() as usize])
}

pub fn days_left(expire_at: DateTime<Utc>, now: DateTime<Utc>) -> u32 {
    let seconds = (expire_at - now).num_seconds();
    if seconds <= 0 {
        0
    } else {
        u32::try_from((seconds + 86_399) / 86_400).unwrap_or(u32::MAX)
    }
}

pub fn amount(value: Amount) -> String {
    value.to_string()
}

pub fn status_title(subscription: &Subscription, now: DateTime<Utc>) -> &'static str {
    let status = subscription.mirror.status.or_else(|| {
        subscription.mirror.expire_at.map(|at| {
            if at <= now {
                SubscriptionStatus::Expired
            } else {
                SubscriptionStatus::Active
            }
        })
    });
    match status {
        Some(SubscriptionStatus::Active) if subscription.kind == SubscriptionKind::Trial => {
            "Пробный период"
        }
        Some(SubscriptionStatus::Active) => "Активна",
        Some(SubscriptionStatus::Limited) => "Трафик закончился",
        Some(SubscriptionStatus::Expired) => "Истекла",
        Some(SubscriptionStatus::Disabled) => "Заблокирована",
        None => "Оформляется",
    }
}

pub fn plural_days(n: u32) -> String {
    format::days(n)
}

pub fn user_error(err: &ServiceError) -> String {
    match err {
        ServiceError::Rejected { message, .. } => capitalize(message),
        ServiceError::Domain(e) => capitalize(&e.to_string()),
        ServiceError::Module(ModuleError::Invalid(message)) => capitalize(message),
        ServiceError::Module(ModuleError::Disabled(_)) => "Эта функция сейчас выключена.".into(),
        ServiceError::NotImplemented(_)
        | ServiceError::Module(ModuleError::NotImplemented(_))
        | ServiceError::Payment(sn_payments::PaymentError::NotImplemented(_)) => {
            "Эта функция пока недоступна. Попробуйте позже или напишите в поддержку.".into()
        }
        ServiceError::Forbidden => "Аккаунт заблокирован. Напишите в поддержку.".into(),
        ServiceError::InsufficientFunds | ServiceError::Store(StoreError::InsufficientFunds) => {
            "На балансе не хватает средств.".into()
        }
        ServiceError::NotFound(what) | ServiceError::Store(StoreError::NotFound(what)) => {
            format!("Не найдено: {what}.")
        }
        ServiceError::RemnaNotConfigured(_) | ServiceError::Remna(_) => {
            "Сервис VPN сейчас недоступен. Попробуйте через несколько минут.".into()
        }
        ServiceError::Payment(sn_payments::PaymentError::EmailRequired) => {
            "Для этого способа оплаты нужна почта. Выберите другой способ оплаты.".into()
        }
        ServiceError::Payment(_) => {
            "Платёжная система не ответила. Попробуйте другой способ оплаты или повторите позже."
                .into()
        }
        other => {
            tracing::error!(error = %other, "ошибка в боте");
            "Что-то пошло не так. Попробуйте ещё раз.".into()
        }
    }
}

pub fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn даты_по_русски_с_неразрывными_пробелами() {
        let at = Utc.with_ymd_and_hms(2027, 2, 14, 10, 0, 0).unwrap();
        let utc = FixedOffset::east_opt(0).unwrap();
        assert_eq!(date(at, utc), "14\u{a0}февраля\u{a0}2027");
        assert_eq!(day_month(at, utc), "14\u{a0}февраля");
        let late = Utc.with_ymd_and_hms(2027, 2, 14, 22, 0, 0).unwrap();
        let msk = FixedOffset::east_opt(3 * 3600).unwrap();
        assert_eq!(day_month(late, msk), "15\u{a0}февраля");
    }

    #[test]
    fn экранирование_html() {
        assert_eq!(
            escape("<b>A&B</b> \"x\""),
            "&lt;b&gt;A&amp;B&lt;/b&gt; &quot;x&quot;"
        );
    }

    #[test]
    fn понятные_ошибки_для_клиента() {
        assert_eq!(
            user_error(&ServiceError::rejected("x", "Счёт устарел")),
            "Счёт устарел"
        );
        assert_eq!(
            user_error(&ServiceError::Module(ModuleError::Invalid(
                "промокод закончился".into()
            ))),
            "Промокод закончился"
        );
        assert!(user_error(&ServiceError::NotImplemented("x")).starts_with("Эта функция пока"));
    }
}
