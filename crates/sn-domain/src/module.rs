use crate::DomainError;
use crate::access::Permission;
use crate::money::Currency;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleId {
    Promo,
    Gifts,
    Options,
    Broadcast,
    Referral,
    Promolinks,
    Utm,
    Contests,
    Roulette,
    Mail,
    Ai,
    Antibot,
    Limits,
    Analytics,
    Landing,
    Oauth,
    TicketFiles,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleGroup {
    Sales,
    Marketing,
    Support,
    Protection,
    Accounting,
    Site,
}

impl ModuleGroup {
    pub const ALL: [ModuleGroup; 6] = [
        ModuleGroup::Sales,
        ModuleGroup::Marketing,
        ModuleGroup::Support,
        ModuleGroup::Protection,
        ModuleGroup::Accounting,
        ModuleGroup::Site,
    ];

    pub const fn title_ru(self) -> &'static str {
        match self {
            ModuleGroup::Sales => "Продажи",
            ModuleGroup::Marketing => "Маркетинг",
            ModuleGroup::Support => "Поддержка и связь",
            ModuleGroup::Protection => "Защита",
            ModuleGroup::Accounting => "Учёт",
            ModuleGroup::Site => "Сайт и вход",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MenuEntry {
    pub label: &'static str,
    pub icon: &'static str,
    pub order: u8,
    pub need: Permission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModuleInfo {
    pub id: ModuleId,
    pub title: &'static str,
    pub description: &'static str,
    pub where_used: &'static str,
    pub group: ModuleGroup,
    pub enabled_by_default: bool,
    pub menu: Option<MenuEntry>,
    pub only_currency: Option<Currency>,
    pub needs_api_key: bool,
}

const fn info(
    id: ModuleId,
    group: ModuleGroup,
    title: &'static str,
    description: &'static str,
    where_used: &'static str,
    enabled_by_default: bool,
) -> ModuleInfo {
    ModuleInfo {
        id,
        title,
        description,
        where_used,
        group,
        enabled_by_default,
        menu: None,
        only_currency: None,
        needs_api_key: false,
    }
}

const fn with_menu(
    mut module: ModuleInfo,
    label: &'static str,
    icon: &'static str,
    order: u8,
    need: Permission,
) -> ModuleInfo {
    module.menu = Some(MenuEntry {
        label,
        icon,
        order,
        need,
    });
    module
}

pub static CATALOG: [ModuleInfo; 17] = [
    with_menu(
        info(
            ModuleId::Promo,
            ModuleGroup::Sales,
            "Промокоды",
            "Скидки, бесплатные дни и лимиты использований",
            "Админка · бот · кабинет",
            true,
        ),
        "Промокоды",
        "percent",
        2,
        Permission::Marketing,
    ),
    with_menu(
        info(
            ModuleId::Gifts,
            ModuleGroup::Sales,
            "Подарочные подписки",
            "Клиент покупает подписку другу и получает код",
            "Админка · бот · кабинет · мини-приложение",
            false,
        ),
        "Подарки",
        "gift",
        4,
        Permission::Marketing,
    ),
    info(
        ModuleId::Options,
        ModuleGroup::Sales,
        "Доп. опции",
        "Докупка устройств и трафика к подписке",
        "Тарифы · бот · кабинет · мини-приложение",
        true,
    ),
    with_menu(
        info(
            ModuleId::Broadcast,
            ModuleGroup::Marketing,
            "Рассылки",
            "Разовые и автоматические сообщения в бот и на почту",
            "Админка",
            true,
        ),
        "Рассылки",
        "send",
        1,
        Permission::Marketing,
    ),
    with_menu(
        info(
            ModuleId::Referral,
            ModuleGroup::Marketing,
            "Рефералы",
            "Бонусы за приглашённых друзей и вывод денег",
            "Админка · бот · кабинет",
            true,
        ),
        "Рефералы",
        "share",
        3,
        Permission::Money,
    ),
    info(
        ModuleId::Promolinks,
        ModuleGroup::Marketing,
        "Промо-ссылки",
        "Бесплатная подписка по ссылке через бота: сквад, срок, трафик, лимит активаций",
        "Админка · бот",
        false,
    ),
    with_menu(
        info(
            ModuleId::Utm,
            ModuleGroup::Marketing,
            "UTM-метки",
            "Ссылки с метками для рекламы и цифры по ним: переходы, регистрации, оплаты",
            "Админка · бот · сайт · мини-приложение",
            true,
        ),
        "UTM-метки",
        "flag",
        7,
        Permission::Marketing,
    ),
    with_menu(
        info(
            ModuleId::Contests,
            ModuleGroup::Marketing,
            "Конкурсы",
            "Розыгрыши среди клиентов с напоминаниями",
            "Админка · бот",
            false,
        ),
        "Конкурсы",
        "trophy",
        5,
        Permission::Marketing,
    ),
    info(
        ModuleId::Roulette,
        ModuleGroup::Marketing,
        "Рулетка",
        "Призы за вращение в мини-приложении",
        "Мини-приложение",
        false,
    ),
    info(
        ModuleId::Mail,
        ModuleGroup::Support,
        "Почта",
        "Письма о продлении, чеки и вход по email",
        "Бот · кабинет",
        true,
    ),
    ModuleInfo {
        needs_api_key: true,
        ..info(
            ModuleId::Ai,
            ModuleGroup::Support,
            "ИИ-чат поддержки",
            "Отвечает клиентам в чате на сайте и в мини-приложении через выбранную нейросеть: Groq, Gemini, DeepSeek, OpenRouter, OpenAI, Claude или свой адрес. Сложное передаёт оператору",
            "Поддержка · кабинет · мини-приложение",
            false,
        )
    },
    info(
        ModuleId::Antibot,
        ModuleGroup::Protection,
        "Антибот",
        "Отсекает фейковые регистрации и накрутки",
        "Клиенты",
        true,
    ),
    info(
        ModuleId::Limits,
        ModuleGroup::Protection,
        "Лимиты трафика",
        "Предупреждает или блокирует тех, кто качает слишком много",
        "Клиенты · инфраструктура",
        false,
    ),
    with_menu(
        info(
            ModuleId::Analytics,
            ModuleGroup::Accounting,
            "Бизнес-аналитика",
            "Деньги, клиенты, удержание, платёжки, источники, тарифы, экономика и поддержка",
            "Админка",
            false,
        ),
        "Аналитика",
        "chart",
        6,
        Permission::Money,
    ),
    info(
        ModuleId::Landing,
        ModuleGroup::Site,
        "Лендинг",
        "Страница сервиса на своём домене",
        "Сайт",
        true,
    ),
    info(
        ModuleId::Oauth,
        ModuleGroup::Site,
        "Вход через Google и Apple",
        "Кнопки входа в кабинет",
        "Кабинет",
        false,
    ),
    info(
        ModuleId::TicketFiles,
        ModuleGroup::Support,
        "Файлы в обращениях",
        "Клиенты и поддержка прикладывают к сообщениям фото, PDF и текстовые файлы",
        "Поддержка · бот · кабинет · мини-приложение",
        true,
    ),
];

impl ModuleId {
    pub const ALL: [ModuleId; 17] = [
        ModuleId::Promo,
        ModuleId::Gifts,
        ModuleId::Options,
        ModuleId::Broadcast,
        ModuleId::Referral,
        ModuleId::Promolinks,
        ModuleId::Utm,
        ModuleId::Contests,
        ModuleId::Roulette,
        ModuleId::Mail,
        ModuleId::Ai,
        ModuleId::Antibot,
        ModuleId::Limits,
        ModuleId::Analytics,
        ModuleId::Landing,
        ModuleId::Oauth,
        ModuleId::TicketFiles,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            ModuleId::Promo => "promo",
            ModuleId::Gifts => "gifts",
            ModuleId::Options => "options",
            ModuleId::Broadcast => "broadcast",
            ModuleId::Referral => "referral",
            ModuleId::Promolinks => "promolinks",
            ModuleId::Utm => "utm",
            ModuleId::Contests => "contests",
            ModuleId::Roulette => "roulette",
            ModuleId::Mail => "mail",
            ModuleId::Ai => "ai",
            ModuleId::Antibot => "antibot",
            ModuleId::Limits => "limits",
            ModuleId::Analytics => "analytics",
            ModuleId::Landing => "landing",
            ModuleId::Oauth => "oauth",
            ModuleId::TicketFiles => "ticket_files",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        ModuleId::ALL
            .into_iter()
            .find(|m| m.as_str() == value)
            .ok_or_else(|| DomainError::unknown("module", value))
    }

    pub fn info(self) -> &'static ModuleInfo {
        &CATALOG[self as usize]
    }

    pub fn available_in(self, currency: Currency) -> bool {
        self.info()
            .only_currency
            .is_none_or(|only| only == currency)
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreFeature {
    ClientsAndSubscriptions,
    TariffsAndTrial,
    PaymentsAndAutorenew,
    Support,
    NodesAndSquads,
    BotCabinetMiniapp,
    SettingsAndAccess,
}

impl CoreFeature {
    pub const ALL: [CoreFeature; 7] = [
        CoreFeature::ClientsAndSubscriptions,
        CoreFeature::TariffsAndTrial,
        CoreFeature::PaymentsAndAutorenew,
        CoreFeature::Support,
        CoreFeature::NodesAndSquads,
        CoreFeature::BotCabinetMiniapp,
        CoreFeature::SettingsAndAccess,
    ];

    pub const fn title_ru(self) -> &'static str {
        match self {
            CoreFeature::ClientsAndSubscriptions => "Клиенты и подписки",
            CoreFeature::TariffsAndTrial => "Тарифы и пробный период",
            CoreFeature::PaymentsAndAutorenew => "Платежи и автопродление",
            CoreFeature::Support => "Поддержка",
            CoreFeature::NodesAndSquads => "Ноды и сквады Remnawave",
            CoreFeature::BotCabinetMiniapp => "Бот, кабинет и мини-приложение",
            CoreFeature::SettingsAndAccess => "Настройки и доступы",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn каталог_совпадает_с_перечнем() {
        for (i, id) in ModuleId::ALL.iter().enumerate() {
            assert_eq!(CATALOG[i].id, *id);
            assert_eq!(id.info().id, *id);
            assert_eq!(ModuleId::parse(id.as_str()).unwrap(), *id);
        }
    }

    #[test]
    fn умолчания_как_в_макете_модулей() {
        let on: Vec<&str> = CATALOG
            .iter()
            .filter(|m| m.enabled_by_default)
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(
            on,
            [
                "promo",
                "options",
                "broadcast",
                "referral",
                "utm",
                "mail",
                "antibot",
                "landing",
                "ticket_files"
            ]
        );
    }

    #[test]
    fn меню_дают_только_семь_модулей_в_нужном_порядке() {
        let mut menu: Vec<(u8, &str)> = CATALOG
            .iter()
            .filter_map(|m| m.menu.map(|e| (e.order, e.label)))
            .collect();
        menu.sort();
        let labels: Vec<&str> = menu.into_iter().map(|(_, l)| l).collect();
        assert_eq!(
            labels,
            [
                "Рассылки",
                "Промокоды",
                "Рефералы",
                "Подарки",
                "Конкурсы",
                "Аналитика",
                "UTM-метки"
            ]
        );
    }
}
