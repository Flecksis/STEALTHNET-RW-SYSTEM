use crate::money::{Currency, Money, Stars};
use crate::traffic::{GIB, TIB};

pub const NBSP: char = '\u{a0}';

pub fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3 * 2);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(NBSP);
        }
        out.push(ch);
    }
    out
}

pub fn plural_ru<'a>(n: u64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let last = n % 10;
    let last_two = n % 100;
    if last == 1 && last_two != 11 {
        one
    } else if (2..=4).contains(&last) && !(12..=14).contains(&last_two) {
        few
    } else {
        many
    }
}

pub fn money(value: Money) -> String {
    let sign = if value.minor < 0 { "-" } else { "" };
    let abs = value.minor.unsigned_abs();
    match value.currency {
        Currency::Usd => format!("{sign}${}.{:02}", group_digits(abs / 100), abs % 100),
        Currency::Rub => format!("{sign}{}{NBSP}₽", group_digits((abs + 50) / 100)),
    }
}

pub fn money_compact(value: Money) -> String {
    match value.currency {
        Currency::Usd if value.minor % 100 == 0 => {
            let sign = if value.minor < 0 { "-" } else { "" };
            format!("{sign}${}", group_digits(value.minor.unsigned_abs() / 100))
        }
        _ => money(value),
    }
}

pub fn stars(value: Stars) -> String {
    let sign = if value.0 < 0 { "-" } else { "" };
    let n = value.0.unsigned_abs();
    format!(
        "{sign}{}{NBSP}{}",
        group_digits(n),
        plural_ru(n, "звезда", "звезды", "звёзд")
    )
}

pub fn days(n: u32) -> String {
    format!(
        "{}{NBSP}{}",
        group_digits(n as u64),
        plural_ru(n as u64, "день", "дня", "дней")
    )
}

pub fn traffic(bytes: u64) -> String {
    let (unit_bytes, unit) = if bytes >= TIB {
        (TIB, "ТБ")
    } else {
        (GIB, "ГБ")
    };
    let tenths = (bytes as u128 * 20 + unit_bytes as u128) / (unit_bytes as u128 * 2);
    let whole = tenths / 10;
    let fraction = tenths % 10;
    if whole >= 100 || fraction == 0 {
        let rounded = (bytes as u128 * 2 + unit_bytes as u128) / (unit_bytes as u128 * 2);
        format!("{}{NBSP}{unit}", group_digits(rounded as u64))
    } else {
        format!("{whole},{fraction}{NBSP}{unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nb(s: &str) -> String {
        s.replace(' ', "\u{a0}")
    }

    #[test]
    fn суммы_как_в_макетах() {
        assert_eq!(money(Money::new(699, Currency::Usd)), "$6.99");
        assert_eq!(money(Money::new(150_100, Currency::Usd)), nb("$1 501.00"));
        assert_eq!(
            money_compact(Money::new(150_100, Currency::Usd)),
            nb("$1 501")
        );
        assert_eq!(money_compact(Money::new(23_670, Currency::Usd)), "$236.70");
        assert_eq!(money(Money::new(49_900, Currency::Rub)), nb("499 ₽"));
        assert_eq!(money(Money::new(169_000, Currency::Rub)), nb("1 690 ₽"));
        assert_eq!(money(Money::new(-420, Currency::Usd)), "-$4.20");
    }

    #[test]
    fn звёзды_склоняются() {
        assert_eq!(stars(Stars(150)), nb("150 звёзд"));
        assert_eq!(stars(Stars(1200)), nb("1 200 звёзд"));
        assert_eq!(stars(Stars(21)), nb("21 звезда"));
        assert_eq!(stars(Stars(2)), nb("2 звезды"));
        assert_eq!(stars(Stars(11)), nb("11 звёзд"));
        assert_eq!(stars(Stars(112)), nb("112 звёзд"));
    }

    #[test]
    fn дни_склоняются() {
        assert_eq!(days(1), nb("1 день"));
        assert_eq!(days(3), nb("3 дня"));
        assert_eq!(days(90), nb("90 дней"));
    }

    #[test]
    fn трафик_в_гигабайтах() {
        assert_eq!(traffic(300 * GIB), nb("300 ГБ"));
        assert_eq!(traffic(112 * GIB), nb("112 ГБ"));
        assert_eq!(traffic(46 * GIB + GIB / 5), nb("46,2 ГБ"));
        assert_eq!(traffic(GIB * 4 / 5), nb("0,8 ГБ"));
        assert_eq!(traffic(6 * TIB + TIB / 5), nb("6,2 ТБ"));
        assert_eq!(traffic(0), nb("0 ГБ"));
    }
}
