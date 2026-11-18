use crate::error::{PaymentError, PaymentResult};
use crate::provider::*;
use crate::util::{self, field};
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sn_domain::money::{Amount, Currency, PayCurrency};
use sn_domain::payment::providers;
use sn_domain::settings::SettingField;

const FORM: &str = "https://yoomoney.ru/quickpay/confirm";
const PAYMENT_TYPES: &[&str] = &["AC", "PC"];
const DEFAULT_PAYMENT_TYPE: &str = "AC";
const RUBLE_CODE: &str = "643";
const METHODS: [(PayMethod, &str); 2] = [
    (
        PayMethod::new("card", "Карта", "Банковская карта через ЮMoney"),
        "AC",
    ),
    (PayMethod::new("wallet", "ЮMoney", "Кошелёк ЮMoney"), "PC"),
];

pub struct YooMoney;

fn signature_valid(secret: &str, pairs: &[(String, String)]) -> bool {
    let Some(sign) = field(pairs, "sign") else {
        return false;
    };
    let mut signed: Vec<&(String, String)> = pairs.iter().filter(|(k, _)| k != "sign").collect();
    signed.sort_by(|a, b| a.0.cmp(&b.0));
    let canonical = signed
        .iter()
        .map(|(k, v)| format!("{k}={}", util::rfc3986(v)))
        .collect::<Vec<_>>()
        .join("&");
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(canonical.as_bytes());
    util::hex_digest_eq(&mac.finalize().into_bytes(), sign)
}

fn payment_type<'a>(config: &'a ProviderConfig, method: Option<&str>) -> PaymentResult<&'a str> {
    match method {
        None => Ok(config.get("payment_type").unwrap_or(DEFAULT_PAYMENT_TYPE)),
        Some(code) => METHODS
            .iter()
            .find(|(m, _)| m.code == code)
            .map(|(_, kind)| *kind)
            .ok_or_else(|| PaymentError::Rejected("этот способ оплаты недоступен".into())),
    }
}

#[async_trait]
impl PaymentProvider for YooMoney {
    fn id(&self) -> &'static str {
        providers::YOOMONEY
    }

    fn title(&self) -> &'static str {
        "ЮMoney"
    }

    fn currencies(&self) -> &'static [Currency] {
        &[Currency::Rub]
    }

    fn client_label(&self, _currency: Currency) -> ClientLabel {
        ClientLabel {
            title: "ЮMoney",
            hint: "Оплата на странице ЮMoney",
        }
    }

    fn methods(&self, currency: Currency) -> Vec<PayMethod> {
        if self.currencies().contains(&currency) {
            METHODS.iter().map(|(method, _)| *method).collect()
        } else {
            Vec::new()
        }
    }

    fn settings_schema(&self) -> Vec<SettingField> {
        vec![
            SettingField::text(
                "wallet",
                "Номер кошелька",
                "Кошелёк, на который приходят оплаты",
            ),
            SettingField::secret(
                "notification_secret",
                "Секрет уведомлений",
                "ЮMoney → HTTP-уведомления",
            ),
            SettingField::select(
                "payment_type",
                "Способ оплаты",
                "AC — банковская карта, PC — кошелёк ЮMoney",
                PAYMENT_TYPES,
            )
            .with_default(DEFAULT_PAYMENT_TYPE),
        ]
    }

    fn is_configured(&self, config: &ProviderConfig) -> bool {
        self.validate_settings(config).is_ok()
    }

    fn validate_settings(&self, config: &ProviderConfig) -> PaymentResult<()> {
        util::require_fields(&self.settings_schema(), config)?;
        let wallet_ok = config
            .get("wallet")
            .is_some_and(|w| (11..=20).contains(&w.len()) && w.bytes().all(|b| b.is_ascii_digit()));
        if !wallet_ok {
            return Err(PaymentError::NotConfigured(
                "номер кошелька ЮMoney — от 11 до 20 цифр".into(),
            ));
        }
        match config.get("payment_type") {
            Some(kind) if !PAYMENT_TYPES.contains(&kind) => Err(PaymentError::NotConfigured(
                "способ оплаты ЮMoney: AC или PC".into(),
            )),
            _ => Ok(()),
        }
    }

    fn enabled_by_default(&self) -> bool {
        true
    }

    fn accepts_http_webhooks(&self) -> bool {
        true
    }

    async fn create_invoice(
        &self,
        config: &ProviderConfig,
        request: &InvoiceRequest,
    ) -> PaymentResult<Invoice> {
        util::require_currency(request.amount, PayCurrency::Rub)?;
        let payment_type = payment_type(config, request.method.as_deref())?;
        let wallet = config.require("wallet")?;
        let label = request.payment_public_id.to_string();
        let mut url =
            reqwest::Url::parse(FORM).map_err(|e| PaymentError::NotConfigured(e.to_string()))?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("receiver", wallet)
                .append_pair("quickpay-form", "button")
                .append_pair("sum", &request.amount.units())
                .append_pair("paymentType", payment_type)
                .append_pair("label", &label);
            if let Some(back) = util::return_url(request) {
                query.append_pair("successURL", &back);
            }
        }
        Ok(Invoice {
            pay_url: url.to_string(),
            external_id: None,
            expires_at: None,
            payload: serde_json::json!({ "label": label }),
        })
    }

    async fn parse_webhook(
        &self,
        config: &ProviderConfig,
        request: &WebhookRequest,
    ) -> PaymentResult<WebhookOutcome> {
        let secret = config.require("notification_secret")?;
        let pairs = util::form_pairs(&request.body)?;
        if !signature_valid(secret, &pairs) {
            return Err(PaymentError::BadSignature);
        }
        let kind = field(&pairs, "notification_type").unwrap_or_default();
        let operation = field(&pairs, "operation_id")
            .filter(|op| !op.is_empty())
            .ok_or_else(|| util::bad("нет operation_id"))?;
        let event_key = format!("{kind}:{operation}");
        let flag = |key: &str| field(&pairs, key).is_some_and(|v| v.eq_ignore_ascii_case("true"));
        if flag("test_notification") || !matches!(kind, "p2p-incoming" | "card-incoming") {
            return Ok(util::skipped(event_key, None, None, WebhookReply::ok()));
        }
        let payment = util::public_ref(field(&pairs, "label"));
        if flag("codepro") || flag("unaccepted") {
            return Ok(util::skipped(
                event_key,
                payment,
                Some(operation.to_string()),
                WebhookReply::ok(),
            ));
        }
        if field(&pairs, "currency") != Some(RUBLE_CODE) {
            return Err(util::bad("валюта уведомления ЮMoney не рубли"));
        }
        let raw = field(&pairs, "withdraw_amount")
            .filter(|v| !v.is_empty())
            .or_else(|| field(&pairs, "amount"))
            .ok_or_else(|| util::bad("нет суммы"))?;
        Ok(WebhookOutcome {
            event_key,
            payment,
            provider_txid: Some(operation.to_string()),
            status: ProviderStatus::Paid,
            amount: Some(Amount::new(
                util::minor(raw, PayCurrency::Rub)?,
                PayCurrency::Rub,
            )),
            saved_method: None,
            reply: WebhookReply::ok(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use serde_json::json;
    use sha1::{Digest, Sha1};
    use sn_domain::payment::{AmountCheck, check_paid_amount};

    const SECRET: &str = "secret123";

    fn settings() -> ProviderConfig {
        config(&[
            ("wallet", json!("4100118412345678")),
            ("notification_secret", json!(SECRET)),
        ])
    }

    fn documented() -> Vec<(&'static str, String)> {
        vec![
            ("notification_type", "p2p-incoming".into()),
            ("operation_id", "441361714955017004".into()),
            ("amount", "98.00".into()),
            ("withdraw_amount", "100.00".into()),
            ("currency", "643".into()),
            ("datetime", "2013-12-26T08:28:34Z".into()),
            ("sender", "41000000000".into()),
            ("codepro", "false".into()),
            ("label", "ML23045".into()),
            ("unaccepted", "false".into()),
            (
                "sha1_hash",
                "ac13833bd6ba9eff1fa9e4bed76f3d6ebb57f6c0".into(),
            ),
        ]
    }

    fn form(fields: &[(&str, String)]) -> String {
        fields
            .iter()
            .map(|(k, v)| format!("{k}={}", util::rfc3986(v)))
            .collect::<Vec<_>>()
            .join("&")
    }

    fn with_sign(mut fields: Vec<(&'static str, String)>, secret: &str) -> String {
        fields.retain(|(k, _)| *k != "sign");
        let pairs: Vec<(String, String)> = fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        let mut sorted = pairs.clone();
        sorted.sort();
        let canonical = sorted
            .iter()
            .map(|(k, v)| format!("{k}={}", util::rfc3986(v)))
            .collect::<Vec<_>>()
            .join("&");
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(canonical.as_bytes());
        fields.push(("sign", hex::encode(mac.finalize().into_bytes())));
        form(&fields)
    }

    fn ours(amount: &str, withdraw: &str) -> Vec<(&'static str, String)> {
        let mut fields = documented();
        for (key, value) in fields.iter_mut() {
            match *key {
                "label" => *value = PAYMENT.to_string(),
                "amount" => *value = amount.into(),
                "withdraw_amount" => *value = withdraw.into(),
                "notification_type" => *value = "card-incoming".into(),
                _ => {}
            }
        }
        fields.retain(|(k, _)| *k != "sha1_hash");
        fields
    }

    async fn parse(body: String) -> PaymentResult<WebhookOutcome> {
        YooMoney.parse_webhook(&settings(), &hook(&[], body)).await
    }

    #[tokio::test]
    async fn подпись_sign_по_примеру_из_документации() {
        let mut fields = documented();
        fields.push((
            "sign",
            "a452af731650e2c5b39abcdc7c28dd27db7b3b654c2230ad2c386e64afb98605".into(),
        ));
        let outcome = parse(form(&fields)).await.unwrap();
        assert_eq!(outcome.status, ProviderStatus::Paid);
        assert_eq!(outcome.amount, Some(rub(10_000)));
        assert_eq!(outcome.payment, None);
        assert_eq!(outcome.provider_txid.as_deref(), Some("441361714955017004"));
        let mut upper = documented();
        upper.push((
            "sign",
            "A452AF731650E2C5B39ABCDC7C28DD27DB7B3B654C2230AD2C386E64AFB98605".into(),
        ));
        assert!(parse(form(&upper)).await.is_ok());
    }

    #[tokio::test]
    async fn неверная_и_отсутствующая_подпись() {
        let body = with_sign(ours("485.03", "499.00"), SECRET);
        assert_eq!(
            parse(body.replace("label=", "label=0")).await,
            Err(PaymentError::BadSignature)
        );
        assert_eq!(
            parse(with_sign(ours("485.03", "499.00"), "wrong")).await,
            Err(PaymentError::BadSignature)
        );
        assert_eq!(
            parse(form(&ours("485.03", "499.00"))).await,
            Err(PaymentError::BadSignature)
        );
        assert!(matches!(
            parse(format!("{body}&amount=1.00")).await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[tokio::test]
    async fn устаревший_sha1_hash_без_sign_не_принимается() {
        let mut fields = ours("485.03", "499.00");
        let value = |key: &str| {
            fields
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        let source = [
            value("notification_type"),
            value("operation_id"),
            value("amount"),
            value("currency"),
            value("datetime"),
            value("sender"),
            value("codepro"),
            SECRET.to_string(),
            value("label"),
        ]
        .join("&");
        let hash = hex::encode(Sha1::digest(source.as_bytes()));
        fields.push(("sha1_hash", hash));
        assert_eq!(parse(form(&fields)).await, Err(PaymentError::BadSignature));
        for (key, value) in fields.iter_mut() {
            if *key == "withdraw_amount" {
                *value = "4990.00".into();
            }
        }
        assert_eq!(parse(form(&fields)).await, Err(PaymentError::BadSignature));
        let signed = parse(with_sign(ours("485.03", "499.00"), SECRET))
            .await
            .unwrap();
        assert_eq!(signed.payment, Some(PaymentRef::PublicId(PAYMENT)));
        assert_eq!(signed.amount, Some(rub(49_900)));
    }

    #[tokio::test]
    async fn сверяется_списанная_сумма_а_не_зачисленная() {
        let expected = rub(49_900);
        let paid = parse(with_sign(ours("485.03", "499.00"), SECRET))
            .await
            .unwrap();
        assert_eq!(check_paid_amount(expected, paid.amount), AmountCheck::Ok);
        let under = parse(with_sign(ours("485.03", "498.00"), SECRET))
            .await
            .unwrap();
        assert!(matches!(
            check_paid_amount(expected, under.amount),
            AmountCheck::Underpaid { .. }
        ));
        let mut no_withdraw = ours("485.03", "499.00");
        no_withdraw.retain(|(k, _)| *k != "withdraw_amount");
        let net = parse(with_sign(no_withdraw, SECRET)).await.unwrap();
        assert_eq!(net.amount, Some(rub(48_503)));
        let mut dollars = ours("485.03", "499.00");
        for (key, value) in dollars.iter_mut() {
            if *key == "currency" {
                *value = "840".into();
            }
        }
        assert!(matches!(
            parse(with_sign(dollars, SECRET)).await,
            Err(PaymentError::BadPayload(_))
        ));
        assert!(matches!(
            parse(with_sign(ours("485.03", "499.001"), SECRET)).await,
            Err(PaymentError::BadPayload(_))
        ));
    }

    #[tokio::test]
    async fn codepro_unaccepted_и_тестовое_уведомление_пропускаются() {
        for (key, value) in [("codepro", "true"), ("unaccepted", "true")] {
            let mut fields = ours("485.03", "499.00");
            for (k, v) in fields.iter_mut() {
                if *k == key {
                    *v = value.into();
                }
            }
            let outcome = parse(with_sign(fields, SECRET)).await.unwrap();
            assert_eq!(outcome.status, ProviderStatus::Pending, "{key}");
            assert_eq!(outcome.payment, Some(PaymentRef::PublicId(PAYMENT)));
        }
        let mut test = ours("485.03", "499.00");
        test.push(("test_notification", "true".into()));
        let outcome = parse(with_sign(test, SECRET)).await.unwrap();
        assert_eq!(outcome.payment, None);
    }

    #[tokio::test]
    async fn ссылка_на_форму_с_суммой_с_сервера() {
        let created = YooMoney
            .create_invoice(&settings(), &invoice(rub(49_900), None))
            .await
            .unwrap();
        let url = reqwest::Url::parse(&created.pay_url).unwrap();
        assert_eq!(url.host_str(), Some("yoomoney.ru"));
        assert_eq!(url.path(), "/quickpay/confirm");
        let query: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        let get = |key: &str| {
            query
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("receiver"), Some("4100118412345678"));
        assert_eq!(get("quickpay-form"), Some("button"));
        assert_eq!(get("sum"), Some("499.00"));
        assert_eq!(get("paymentType"), Some("AC"));
        assert_eq!(get("label"), Some(PAYMENT.to_string().as_str()));
        assert_eq!(
            get("successURL"),
            Some("https://panel.example.com/cabinet/payment-wait")
        );
        assert_eq!(created.external_id, None);
        assert!(matches!(
            YooMoney
                .create_invoice(&settings(), &invoice(usd(699), None))
                .await,
            Err(PaymentError::UnsupportedCurrency(_))
        ));
    }

    async fn payment_type_in_link(config: &ProviderConfig, method: Option<&str>) -> String {
        let mut request = invoice(rub(49_900), None);
        request.method = method.map(str::to_string);
        let created = YooMoney.create_invoice(config, &request).await.unwrap();
        reqwest::Url::parse(&created.pay_url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "paymentType")
            .map(|(_, v)| v.into_owned())
            .unwrap()
    }

    #[tokio::test]
    async fn способ_оплаты_задаёт_paymenttype() {
        let methods = YooMoney.methods(Currency::Rub);
        let shown: Vec<(&str, &str)> = methods.iter().map(|m| (m.code, m.title)).collect();
        assert_eq!(shown, [("card", "Карта"), ("wallet", "ЮMoney")]);
        assert!(
            methods
                .iter()
                .all(|m| !m.hint.is_empty() && m.min_minor.is_none())
        );
        assert!(YooMoney.methods(Currency::Usd).is_empty());
        assert_eq!(payment_type_in_link(&settings(), Some("card")).await, "AC");
        assert_eq!(
            payment_type_in_link(&settings(), Some("wallet")).await,
            "PC"
        );
        assert_eq!(payment_type_in_link(&settings(), None).await, "AC");
        let mut wallet = settings();
        wallet.set("payment_type", json!("PC"));
        assert_eq!(payment_type_in_link(&wallet, None).await, "PC");
        assert_eq!(payment_type_in_link(&wallet, Some("card")).await, "AC");
        for code in ["page", "sbp", "AC"] {
            let mut request = invoice(rub(49_900), None);
            request.method = Some(code.into());
            assert_eq!(
                YooMoney.create_invoice(&settings(), &request).await,
                Err(PaymentError::Rejected(
                    "этот способ оплаты недоступен".into()
                )),
                "{code}"
            );
        }
    }

    #[test]
    fn подпись_кнопки_не_обещает_только_карты() {
        let label = YooMoney.client_label(Currency::Rub);
        assert_eq!(label.title, "ЮMoney");
        assert!(!label.hint.to_lowercase().contains("карт"));
        assert!(!label.hint.to_lowercase().contains("кошел"));
    }

    #[test]
    fn настройки_юmoney() {
        assert!(YooMoney.is_configured(&settings()));
        let mut short = settings();
        short.set("wallet", json!("4100"));
        assert!(!YooMoney.is_configured(&short));
        let mut kind = settings();
        kind.set("payment_type", json!("SB"));
        assert!(!YooMoney.is_configured(&kind));
    }
}
