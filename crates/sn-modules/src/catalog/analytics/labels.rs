use crate::context::PaymentTitle;
use std::collections::HashMap;

pub fn kind(kind: &str, item: &str) -> (&'static str, &'static str) {
    match (kind, item) {
        ("new", _) => ("new", "Новые продажи"),
        ("renewal", _) => ("renewal", "Продления"),
        ("change", _) => ("change", "Смена тарифа"),
        ("topup", _) => ("topup", "Пополнения баланса"),
        ("module", "options:devices") => ("devices", "Доп. устройства"),
        ("module", "options:traffic") => ("traffic", "Пакеты трафика"),
        ("module", "gifts:gift") => ("gift", "Подарки"),
        ("trial", _) => ("trial", "Пробные периоды"),
        _ => ("other", "Прочие покупки"),
    }
}

pub fn channel(code: &str) -> &'static str {
    match code {
        "bot" => "Бот",
        "miniapp" => "Мини-приложение",
        "cabinet" => "Кабинет",
        "autorenew" => "Автопродление",
        "admin" => "Админка",
        _ => "Не записан",
    }
}

pub fn registration(code: &str) -> &'static str {
    match code {
        "bot" => "Бот",
        "miniapp" => "Мини-приложение",
        "web" => "Сайт",
        "admin" => "Админка",
        "import" | "migration" => "Перенос",
        _ => "Другое",
    }
}

pub fn login(code: &str) -> &'static str {
    match code {
        "telegram" => "Telegram",
        "email" => "Почта",
        "google" => "Google",
        "apple" => "Apple",
        _ => "Без входа",
    }
}

pub fn source_type(code: &str) -> &'static str {
    match code {
        "utm" => "UTM-метки",
        "promo" => "Промо-ссылки",
        "ref" => "Рефералы",
        "gift" => "Подарки",
        _ => "Без источника",
    }
}

pub fn subscription_kind(code: &str) -> &'static str {
    match code {
        "paid" => "Платные",
        "trial" => "Пробные",
        "gift" => "Подарочные",
        "promo" => "По промо-ссылкам",
        "manual" => "Выданные вручную",
        "custom" => "Особые",
        _ => "Другие",
    }
}

fn plural(n: i64, one: &str, few: &str, many: &str) -> String {
    let n10 = n % 10;
    let n100 = n % 100;
    let word = if n10 == 1 && n100 != 11 {
        one
    } else if (2..=4).contains(&n10) && !(12..=14).contains(&n100) {
        few
    } else {
        many
    };
    format!("{n} {word}")
}

pub fn period(days: Option<i64>) -> String {
    match days {
        None | Some(0) => "Без срока".into(),
        Some(d) if d % 365 == 0 => plural(d / 365, "год", "года", "лет"),
        Some(d) if d % 30 == 0 => plural(d / 30, "месяц", "месяца", "месяцев"),
        Some(d) => plural(d, "день", "дня", "дней"),
    }
}

pub fn count(n: i64, one: &str, few: &str, many: &str) -> String {
    plural(n, one, few, many)
}

const GENITIVE: [&str; 12] = [
    "января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября",
    "ноября", "декабря",
];

pub fn date_text(day: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    format!("{} {} {}", day.day(), GENITIVE[day.month0() as usize], day.year())
}

pub fn money_short(minor: f64, currency: sn_domain::money::Currency) -> String {
    let major = minor / 100.0;
    let (head, tail) = match currency {
        sn_domain::money::Currency::Usd => ("$", ""),
        _ => ("", "\u{a0}₽"),
    };
    if major.abs() >= 1_000_000.0 {
        format!("{head}{}\u{a0}млн{tail}", format!("{:.1}", major / 1_000_000.0).replace('.', ","))
    } else if major.abs() >= 1_000.0 {
        format!("{head}{}\u{a0}тыс{tail}", (major / 1_000.0).round())
    } else {
        format!("{head}{}{tail}", major.round())
    }
}

pub fn duration(seconds: i64) -> String {
    let s = seconds.max(0);
    if s < 60 {
        format!("{s}\u{a0}с")
    } else if s < 3_600 {
        format!("{}\u{a0}мин", s / 60)
    } else if s < 86_400 {
        format!("{}\u{a0}ч {}\u{a0}мин", s / 3_600, (s % 3_600) / 60)
    } else {
        format!("{}\u{a0}дн {}\u{a0}ч", s / 86_400, (s % 86_400) / 3_600)
    }
}

pub fn outcome(code: &str) -> &'static str {
    match code {
        "rejected_signature" => "Неверная подпись",
        "rejected_payload" => "Не удалось разобрать",
        "rejected_business" => "Отказ по правилам",
        "not_accepted" => "Не принято",
        "payment_not_found" => "Платёж не найден",
        "amount_mismatch" => "Сумма не совпала",
        "error" => "Ошибка обработки",
        _ => "Другое",
    }
}

pub fn provider(titles: &[PaymentTitle], code: &str) -> String {
    if let Some(t) = titles.iter().find(|t| t.provider == code) {
        return t.title.clone();
    }
    match code {
        "manual" => "Вручную".into(),
        "stars" => "Звёзды Telegram".into(),
        "balance" => "С баланса".into(),
        other => other.into(),
    }
}

pub fn method(titles: &[PaymentTitle], provider: &str, code: &str) -> String {
    let t = titles.iter().find(|t| t.provider == provider);
    if code.is_empty() {
        return match t {
            Some(t) if !t.button.is_empty() => format!("Кнопка «{}»", t.button),
            _ => "Без выбора способа".into(),
        };
    }
    t.and_then(|t| t.methods.iter().find(|m| m.code == code))
        .map_or_else(|| code.to_string(), |m| m.title.clone())
}

#[derive(Debug, Clone, Default)]
pub struct Names {
    pub tariffs: HashMap<i64, String>,
    pub categories: HashMap<i64, String>,
    pub links: HashMap<String, String>,
}

impl Names {
    pub fn tariff(&self, id: Option<i64>) -> String {
        match id {
            None => "Без тарифа".into(),
            Some(id) => self
                .tariffs
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("Тариф №{id}")),
        }
    }

    pub fn category(&self, id: Option<i64>) -> String {
        match id {
            None => "Без категории".into(),
            Some(id) => self
                .categories
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("Категория №{id}")),
        }
    }

    pub fn source(&self, kind: &str, key: &str) -> String {
        match kind {
            "utm" if key.is_empty() => "Метка без источника".into(),
            "utm" => key.to_string(),
            "promo" => self
                .links
                .get(key)
                .cloned()
                .unwrap_or_else(|| format!("Промо-ссылка №{key}")),
            other => source_type(other).to_string(),
        }
    }
}

pub async fn names(db: &mut impl sn_store::Db) -> sn_store::StoreResult<Names> {
    let tariffs: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM tariffs")
        .fetch_all(db.conn())
        .await?;
    let categories: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM tariff_categories")
        .fetch_all(db.conn())
        .await?;
    let links: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM promo_links")
        .fetch_all(db.conn())
        .await?;
    Ok(Names {
        tariffs: tariffs.into_iter().collect(),
        categories: categories.into_iter().collect(),
        links: links.into_iter().map(|(id, n)| (id.to_string(), n)).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn сроки_называются_по_русски() {
        assert_eq!(period(Some(30)), "1 месяц");
        assert_eq!(period(Some(90)), "3 месяца");
        assert_eq!(period(Some(180)), "6 месяцев");
        assert_eq!(period(Some(365)), "1 год");
        assert_eq!(period(Some(7)), "7 дней");
        assert_eq!(period(Some(1)), "1 день");
        assert_eq!(period(None), "Без срока");
    }

    #[test]
    fn вид_оплаты_учитывает_покупки_модулей() {
        assert_eq!(kind("module", "options:devices").1, "Доп. устройства");
        assert_eq!(kind("module", "gifts:gift").0, "gift");
        assert_eq!(kind("renewal", "").1, "Продления");
        assert_eq!(kind("module", "x:y").0, "other");
    }
}
