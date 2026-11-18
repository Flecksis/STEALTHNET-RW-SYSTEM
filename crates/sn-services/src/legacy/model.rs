use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::client::Locale;
use sn_domain::money::Currency;
use sn_domain::subscription::SubscriptionKind;
use sn_domain::traffic::TrafficReset;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Stealthnet,
    Bedolaga,
    TelegramShop,
    Chill,
    Remnashop,
    Minishop,
}

impl Source {
    pub const fn as_str(self) -> &'static str {
        match self {
            Source::Stealthnet => "stealthnet",
            Source::Bedolaga => "bedolaga",
            Source::TelegramShop => "telegram_shop",
            Source::Chill => "chill",
            Source::Remnashop => "remnashop",
            Source::Minishop => "minishop",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stealthnet" | "sn" => Some(Source::Stealthnet),
            "bedolaga" | "bdl" => Some(Source::Bedolaga),
            "telegram_shop" | "remnawave-telegram-shop" => Some(Source::TelegramShop),
            "chill" | "chill-remna-bot" => Some(Source::Chill),
            "remnashop" => Some(Source::Remnashop),
            "minishop" | "remnawave-minishop" => Some(Source::Minishop),
            _ => None,
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Source::Stealthnet => "STEALTHNET",
            Source::Bedolaga => "Бот Бедолаги",
            Source::TelegramShop => "remnawave-telegram-shop",
            Source::Chill => "chill-remna-bot",
            Source::Remnashop => "Remnashop",
            Source::Minishop => "Minishop",
        }
    }

    pub fn entity(self, kind: &str) -> String {
        format!("{}:{kind}", self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TariffMode {
    Tariffs,
    Classic,
}

#[derive(Debug, Clone, Default)]
pub struct Utm {
    pub source: Option<String>,
    pub medium: Option<String>,
    pub campaign: Option<String>,
    pub content: Option<String>,
    pub term: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LClient {
    pub old_id: String,
    pub telegram_id: Option<i64>,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    pub email_raw: Option<String>,
    pub email_verified: bool,
    pub password_hash: Option<String>,
    pub google_id: Option<String>,
    pub apple_id: Option<String>,
    pub locale: Locale,
    pub blocked: bool,
    pub block_reason: Option<String>,
    pub bot_unreachable: bool,
    pub balance_minor: i64,
    pub balance_mixed: bool,
    pub referral_code: Option<String>,
    pub referrer_old_id: Option<String>,
    pub referral_percent: Option<i32>,
    pub utm: Option<Utm>,
    pub trial_used: bool,
    pub created_at: Option<DateTime<Utc>>,
}

impl LClient {
    pub fn who(&self) -> String {
        if let Some(username) = &self.username {
            return format!("@{username}");
        }
        if let Some(id) = self.telegram_id {
            return format!("Telegram {id}");
        }
        if let Some(email) = &self.email {
            return email.clone();
        }
        format!("id {}", self.old_id)
    }
}

#[derive(Debug, Clone)]
pub struct LCategory {
    pub old_id: String,
    pub name: String,
    pub sort: i32,
}

#[derive(Debug, Clone)]
pub struct LPeriod {
    pub old_id: Option<String>,
    pub days: u32,
    pub price_minor: Option<i64>,
    pub sort: i32,
}

#[derive(Debug, Clone)]
pub struct LTariff {
    pub old_id: String,
    pub category_old_id: String,
    pub name: String,
    pub description: Option<String>,
    pub sort: i32,
    pub on_sale: bool,
    pub currency: Currency,
    pub periods: Vec<LPeriod>,
    pub devices_included: u32,
    pub extra_devices_max: u32,
    pub extra_device_monthly_minor: Option<i64>,
    pub traffic_limit_bytes: Option<u64>,
    pub traffic_reset: TrafficReset,
    pub squads: Vec<Uuid>,
    pub external_squad: Option<Uuid>,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LSubscription {
    pub old_id: String,
    pub client_old_id: String,
    pub tariff_old_id: Option<String>,
    pub kind: SubscriptionKind,
    pub remna_id: Option<i64>,
    pub remna_uuid: Option<String>,
    pub short_uuid: Option<String>,
    pub expire_at: Option<DateTime<Utc>>,
    pub extra_devices: u32,
    pub autorenew: bool,
    pub autorenew_days: Option<u32>,
    pub autorenew_period_old_id: Option<String>,
    pub ordinal: u32,
    pub gifted: bool,
    pub created_at: Option<DateTime<Utc>>,
}

impl LSubscription {
    pub fn from_client_card(&self) -> bool {
        self.old_id.starts_with("client:")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentKind {
    TopUp,
    Subscription,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayCurrencyLegacy {
    Money(Currency),
    Stars,
}

#[derive(Debug, Clone)]
pub struct LPayment {
    pub old_id: String,
    pub client_old_id: String,
    pub kind: PaymentKind,
    pub refunded: bool,
    pub amount_minor: i64,
    pub currency: PayCurrencyLegacy,
    pub provider: String,
    pub external_id: Option<String>,
    pub tariff_old_id: Option<String>,
    pub days: Option<u32>,
    pub subscription_old_id: Option<String>,
    pub label: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub paid_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromoValue {
    Percent(u8),
    Fixed(i64),
    Days(u32),
}

#[derive(Debug, Clone)]
pub struct LPromo {
    pub old_id: String,
    pub code: String,
    pub value: PromoValue,
    pub max_uses: Option<u32>,
    pub per_client: u32,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub active: bool,
    pub first_purchase_only: bool,
    pub tariff_old_id: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LPromoUse {
    pub old_id: String,
    pub promo_old_id: String,
    pub client_old_id: String,
    pub used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct LMessage {
    pub old_id: String,
    pub from_support: bool,
    pub body: String,
    pub read: bool,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct LTicket {
    pub old_id: String,
    pub client_old_id: String,
    pub subject: Option<String>,
    pub open: bool,
    pub created_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub messages: Vec<LMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Skip {
    pub code: String,
    pub count: u64,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Problem {
    pub code: &'static str,
    pub entity: &'static str,
    pub old_id: Option<String>,
    pub who: Option<String>,
    pub details: String,
}

#[derive(Debug, Clone)]
pub struct LAdmin {
    pub old_id: String,
    pub email: String,
    pub password_hash: String,
    pub full_access: bool,
    pub sections: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawalState {
    Pending,
    Paid,
    Rejected,
}

#[derive(Debug, Clone)]
pub struct LWithdrawal {
    pub old_id: String,
    pub client_old_id: String,
    pub amount_minor: i64,
    pub details: String,
    pub state: WithdrawalState,
    pub comment: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub decided_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct LGift {
    pub old_id: String,
    pub code: String,
    pub giver_old_id: String,
    pub tariff_old_id: Option<String>,
    pub days: u32,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct LAutoRule {
    pub old_id: String,
    pub name: Option<String>,
    pub trigger: String,
    pub days: i64,
    pub channel: String,
    pub subject: Option<String>,
    pub text: String,
    pub buttons: Vec<(String, String)>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvoicePurpose {
    TopUp,
    Tariff {
        tariff_old_id: String,
        period_old_id: Option<String>,
        days: Option<u32>,
        subscription_old_id: Option<String>,
    },
    Other(String),
}

#[derive(Debug, Clone)]
pub struct LInvoice {
    pub old_id: String,
    pub client_old_id: String,
    pub provider: &'static str,
    pub old_provider: String,
    pub external_id: Option<String>,
    pub order_ref: Option<String>,
    pub amount_minor: i64,
    pub currency: Currency,
    pub stars: Option<i64>,
    pub purpose: InvoicePurpose,
    pub pay_url: Option<String>,
    pub recurring: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct Legacy {
    pub source: Source,
    pub dumped_at: Option<DateTime<Utc>>,
    pub currency: Currency,
    pub tariff_mode: TariffMode,
    pub categories: Vec<LCategory>,
    pub tariffs: Vec<LTariff>,
    pub clients: Vec<LClient>,
    pub subscriptions: Vec<LSubscription>,
    pub payments: Vec<LPayment>,
    pub promos: Vec<LPromo>,
    pub promo_uses: Vec<LPromoUse>,
    pub tickets: Vec<LTicket>,
    pub settings: std::collections::BTreeMap<String, String>,
    pub admins: Vec<LAdmin>,
    pub withdrawals: Vec<LWithdrawal>,
    pub gifts: Vec<LGift>,
    pub auto_rules: Vec<LAutoRule>,
    pub invoices: Vec<LInvoice>,
    pub category_limits: std::collections::HashMap<String, u32>,
    pub device_discounts: std::collections::HashMap<String, Vec<sn_domain::tariff::DeviceDiscount>>,
    pub skipped: Vec<Skip>,
    pub problems: Vec<Problem>,
}

impl Legacy {
    pub fn new(source: Source, currency: Currency) -> Self {
        Legacy {
            source,
            dumped_at: None,
            currency,
            tariff_mode: TariffMode::Tariffs,
            categories: Vec::new(),
            tariffs: Vec::new(),
            clients: Vec::new(),
            subscriptions: Vec::new(),
            payments: Vec::new(),
            promos: Vec::new(),
            promo_uses: Vec::new(),
            tickets: Vec::new(),
            settings: std::collections::BTreeMap::new(),
            admins: Vec::new(),
            withdrawals: Vec::new(),
            gifts: Vec::new(),
            auto_rules: Vec::new(),
            invoices: Vec::new(),
            category_limits: std::collections::HashMap::new(),
            device_discounts: std::collections::HashMap::new(),
            skipped: Vec::new(),
            problems: Vec::new(),
        }
    }

    pub fn skip(&mut self, code: &str, count: u64, text: impl Into<String>) {
        if count == 0 {
            return;
        }
        if let Some(found) = self.skipped.iter_mut().find(|s| s.code == code) {
            found.count += count;
            return;
        }
        self.skipped.push(Skip {
            code: code.to_string(),
            count,
            text: text.into(),
        });
    }

    pub fn problem(
        &mut self,
        code: &'static str,
        entity: &'static str,
        old_id: Option<String>,
        who: Option<String>,
        details: impl Into<String>,
    ) {
        self.problems.push(Problem {
            code,
            entity,
            old_id,
            who,
            details: details.into(),
        });
    }

    pub fn referrals(&self) -> usize {
        self.clients
            .iter()
            .filter(|c| c.referrer_old_id.is_some())
            .count()
    }
}

pub fn locale_of(raw: Option<&str>) -> Locale {
    match raw.map(|l| l.trim().to_ascii_lowercase()) {
        Some(l) if l.starts_with("en") => Locale::En,
        _ => Locale::Ru,
    }
}

pub fn gb_to_bytes(gb: i64) -> Option<u64> {
    u64::try_from(gb)
        .ok()
        .filter(|g| *g > 0)
        .and_then(|g| g.checked_mul(1024 * 1024 * 1024))
}

pub fn bad_email(legacy: &mut Legacy, raw: Option<&str>) {
    let Some(raw) = raw.map(str::trim).filter(|r| !r.is_empty()) else {
        return;
    };
    if clean_email(Some(raw)).is_some() {
        return;
    }
    let client = legacy.clients.last();
    let (old_id, who) = (client.map(|c| c.old_id.clone()), client.map(LClient::who));
    legacy.problem(
        "email_invalid",
        "client",
        old_id,
        who,
        format!("Почта «{raw}» записана с ошибкой — вход по почте не перенесён, клиент перенесён без неё"),
    );
}

pub fn clean_email(raw: Option<&str>) -> Option<String> {
    let value = raw?.trim().to_lowercase();
    sn_domain::client::normalize_identity(sn_domain::client::IdentityKind::Email, &value).ok()
}

pub fn clean_username(raw: Option<&str>) -> Option<String> {
    let value = raw?.trim().trim_start_matches('@').to_string();
    (!value.is_empty() && value.len() <= 64).then_some(value)
}

pub fn password_hash(raw: Option<&str>) -> Option<String> {
    let value = raw?.trim();
    let known = value.starts_with("$2a$")
        || value.starts_with("$2b$")
        || value.starts_with("$2y$")
        || value.starts_with("$argon2");
    known.then(|| value.to_string())
}

pub fn short_uuid_from_url(url: Option<&str>) -> Option<String> {
    let url = url?.trim().trim_end_matches('/');
    let last = url.rsplit('/').next()?;
    let candidate = last.split(['?', '#']).next()?;
    (candidate.len() >= 6
        && candidate.len() <= 64
        && candidate
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then(|| candidate.to_string())
}

pub fn parse_reset(raw: Option<&str>) -> Option<TrafficReset> {
    let value = raw?.trim().to_ascii_lowercase().replace('-', "_");
    match value.as_str() {
        "no_reset" | "none" | "never" | "carry_over" => Some(TrafficReset::NoReset),
        "day" | "daily" => Some(TrafficReset::Day),
        "week" | "weekly" => Some(TrafficReset::Week),
        "month" | "monthly" => Some(TrafficReset::Month),
        "month_rolling" | "monthly_rolling" => Some(TrafficReset::MonthRolling),
        "on_purchase" | "on_payment" => Some(TrafficReset::OnPayment),
        _ => None,
    }
}
