use super::*;

fn look() -> Look {
    Look {
        brand: "STEALTHNET".into(),
        base: Some("https://vpn.example.com".into()),
        logo: None,
        accent: true,
        support: Some("https://t.me/help_bot".into()),
    }
}

fn ready_values() -> Values {
    Values::new()
        .with("date", "14 февраля 2027")
        .with("tariff", "Стандарт <VPN> & ко")
        .with("brand", "STEALTHNET")
        .with("email", "alex@example.com")
}

#[test]
fn письмо_по_умолчанию_собирается_с_подстановками_кнопкой_и_подписью() {
    let spec = LetterId::SubscriptionReady.spec();
    let letter = compose(
        spec,
        Locale::Ru,
        &Saved::default(),
        None,
        &ready_values(),
        &look(),
    )
    .unwrap();
    assert_eq!(letter.subject, "Подписка готова");
    assert!(
        letter.html.contains(
            "Подписка готова и действует до 14 февраля 2027. Ссылка для подключения — в личном кабинете."
        ),
        "{}",
        letter.html
    );
    assert!(
        letter
            .html
            .contains("href=\"https://vpn.example.com/cabinet\"")
    );
    assert!(letter.html.contains(">Открыть кабинет</a>"));
    assert!(letter.html.contains("background:#ff4d79"));
    assert!(letter.html.contains("Команда STEALTHNET"));
    assert!(
        letter
            .html
            .contains("Это письмо о вашем аккаунте в STEALTHNET.")
    );
    assert!(letter.html.contains(">S</span>"), "без логотипа — буква");
    assert!(
        letter
            .text
            .starts_with("Подписка готова и действует до 14 февраля 2027.")
    );
    assert!(
        letter
            .text
            .contains("Открыть кабинет: https://vpn.example.com/cabinet")
    );
    assert!(letter.text.contains("\n\nКоманда STEALTHNET\n"));
    assert!(
        letter
            .text
            .ends_with("—\nЭто письмо о вашем аккаунте в STEALTHNET.\n")
    );
}

#[test]
fn шаблон_оператора_экранируется_в_html_и_остаётся_текстом_в_теме() {
    let spec = LetterId::SubscriptionReady.spec();
    let saved = Saved {
        subject: Some("Подписка «{tariff}» готова".into()),
        text: Some("<b>Готово!</b>\nТариф «{tariff}» до {date}.".into()),
        button: Some("В кабинет {brand}".into()),
    };
    let letter = compose(
        spec,
        Locale::Ru,
        &saved,
        Some("{brand}, поддержка: {support}"),
        &ready_values(),
        &look(),
    )
    .unwrap();
    assert_eq!(letter.subject, "Подписка «Стандарт <VPN> & ко» готова");
    assert!(
        letter.html.contains(
            "<b>Готово!</b><br>Тариф «Стандарт &lt;VPN&gt; &amp; ко» до 14 февраля 2027."
        ),
        "{}",
        letter.html
    );
    assert!(!letter.html.contains("<VPN>"));
    assert!(letter.html.contains(">В кабинет STEALTHNET</a>"));
    assert!(
        letter
            .html
            .contains("STEALTHNET, поддержка: https://t.me/help_bot")
    );
    assert!(
        letter
            .text
            .starts_with("Готово!\nТариф «Стандарт <VPN> & ко» до 14 февраля 2027.")
    );
    assert!(
        letter
            .html
            .contains("<title>Подписка «Стандарт &lt;VPN&gt; &amp; ко» готова</title>")
    );
}

#[test]
fn без_значения_подстановки_уходит_текст_по_умолчанию() {
    let spec = LetterId::SubscriptionRenewed.spec();
    let saved = Saved {
        subject: Some("Тариф «{tariff}» продлён".into()),
        text: Some("Тариф «{tariff}» продлён до {date}.".into()),
        button: None,
    };
    let values = Values::new()
        .with("date", "14 февраля 2027")
        .with("brand", "STEALTHNET")
        .with("email", "alex@example.com");
    let letter = compose(spec, Locale::Ru, &saved, None, &values, &look()).unwrap();
    assert_eq!(letter.subject, "Подписка продлена");
    assert!(
        letter
            .text
            .starts_with("Подписка продлена до 14 февраля 2027.")
    );
    assert!(!letter.html.contains("{tariff}"));
    let reply = LetterId::SupportReply.spec();
    let without_ticket = Values::new()
        .with("reply", "Обновите подписку")
        .with("brand", "STEALTHNET")
        .with("email", "alex@example.com");
    let letter = compose(
        reply,
        Locale::Ru,
        &Saved::default(),
        None,
        &without_ticket,
        &look(),
    )
    .unwrap();
    assert_eq!(letter.subject, "Ответ поддержки");
    assert!(
        letter
            .html
            .contains("href=\"https://vpn.example.com/cabinet/help\"")
    );
    let with_ticket = without_ticket.with("ticket", "41");
    let letter = compose(
        reply,
        Locale::Ru,
        &Saved::default(),
        None,
        &with_ticket,
        &look(),
    )
    .unwrap();
    assert_eq!(letter.subject, "Ответ поддержки по обращению №41");
    assert!(
        letter
            .html
            .contains("href=\"https://vpn.example.com/cabinet/help/41\"")
    );
    assert!(
        compose(
            LetterId::TopupDone.spec(),
            Locale::Ru,
            &Saved::default(),
            None,
            &Values::new().with("brand", "S"),
            &look()
        )
        .is_none()
    );
}

#[test]
fn английское_письмо_берёт_английские_тексты_и_подпись() {
    let spec = LetterId::TopupDone.spec();
    let values = Values::new()
        .with("amount", "$10.00")
        .with("brand", "STEALTHNET")
        .with("email", "alex@example.com");
    let letter = compose(spec, Locale::En, &Saved::default(), None, &values, &look()).unwrap();
    assert_eq!(letter.subject, "Balance topped up");
    assert!(
        letter
            .text
            .starts_with("Your balance is topped up by $10.00.")
    );
    assert!(letter.html.contains(">Payment history</a>"));
    assert!(letter.html.contains("The STEALTHNET team"));
    assert!(
        letter
            .html
            .contains("This email is about your account at STEALTHNET.")
    );
    assert!(letter.html.contains("<html lang=\"en\">"));
}

#[test]
fn код_на_отдельной_строке_крупный_и_без_кнопки() {
    let spec = LetterId::LoginCode.spec();
    let values = Values::new()
        .with("code", "482913")
        .with("minutes", "15 минут")
        .with("brand", "STEALTHNET")
        .with("email", "alex@example.com");
    let mono = Look {
        accent: false,
        base: None,
        ..look()
    };
    let letter = compose(spec, Locale::Ru, &Saved::default(), None, &values, &mono).unwrap();
    assert_eq!(letter.subject, "Код для входа в STEALTHNET: 482913");
    assert!(
        letter
            .html
            .contains("font-size:30px;font-weight:700;letter-spacing:0.08em;line-height:1.3;color:#151a22;\">482913</span>"),
        "{}",
        letter.html
    );
    assert!(!letter.html.contains("</a></td></tr>"));
    assert!(
        letter
            .text
            .contains("STEALTHNET:\n482913\n\nКод действует 15 минут.")
    );
    assert!(!letter.text.contains('\u{e000}'));
    let inline = Saved {
        text: Some("Ваш код {code}, он действует {minutes}.".into()),
        ..Saved::default()
    };
    let letter = compose(spec, Locale::Ru, &inline, None, &values, &mono).unwrap();
    assert!(
        letter
            .html
            .contains("Ваш код 482913, он действует 15 минут.")
    );
    assert!(!letter.html.contains("font-size:30px"));
}

#[test]
fn логотип_и_монохромные_кнопки_из_оформления() {
    let spec = LetterId::SubscriptionReady.spec();
    let styled = Look {
        logo: Some("https://vpn.example.com/api/public/logo/abc.png".into()),
        accent: false,
        ..look()
    };
    let letter = compose(
        spec,
        Locale::Ru,
        &Saved::default(),
        None,
        &ready_values(),
        &styled,
    )
    .unwrap();
    assert!(
        letter
            .html
            .contains("<img src=\"https://vpn.example.com/api/public/logo/abc.png\"")
    );
    assert!(letter.html.contains("background:#151a22;color:#ffffff"));
    let site_off = Look {
        base: None,
        ..look()
    };
    let letter = compose(
        spec,
        Locale::Ru,
        &Saved::default(),
        None,
        &ready_values(),
        &site_off,
    )
    .unwrap();
    assert!(!letter.html.contains("Открыть кабинет"));
    assert!(!letter.text.contains("Открыть кабинет"));
}

#[test]
fn черновик_проверяется_и_совпадение_с_текстом_по_умолчанию_не_хранится() {
    let spec = LetterId::SubscriptionReady.spec();
    let same = cleaned(
        spec,
        Locale::Ru,
        &DraftTexts {
            subject: "Подписка готова".into(),
            text: spec.ru.text[0].into(),
            button: " Открыть кабинет ".into(),
        },
    )
    .unwrap();
    assert!(same.is_empty());
    let own = cleaned(
        spec,
        Locale::En,
        &DraftTexts {
            subject: "Ready: {tariff}".into(),
            text: String::new(),
            button: String::new(),
        },
    )
    .unwrap();
    assert_eq!(own.subject.as_deref(), Some("Ready: {tariff}"));
    assert_eq!(own.text, None);
    let err = cleaned(
        spec,
        Locale::En,
        &DraftTexts {
            subject: String::new(),
            text: "Until {datе}".into(),
            button: String::new(),
        },
    )
    .unwrap_err();
    assert_eq!(err.code(), "mail_letters.invalid");
    assert!(
        err.to_string().starts_with(
            "«Подписка готова», английский, текст: подстановка {datе} записана неверно"
        ),
        "{err}"
    );
    assert_eq!(cleaned_sign(Locale::Ru, "Команда {brand}").unwrap(), None);
    assert!(cleaned_sign(Locale::Ru, "Команда {tariff}").is_err());
}
