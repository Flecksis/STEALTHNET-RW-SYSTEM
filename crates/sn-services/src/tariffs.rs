use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use serde::{Deserialize, Serialize};
use sn_domain::ids::{AdminId, CategoryId, PeriodId, SquadId, TariffId};
use sn_domain::money::{Currency, Stars};
use sn_domain::tariff::{Category, CurrencyPrices, DevicePolicy, Tariff, TariffPeriod};
use sn_domain::traffic::TrafficPolicy;
use sn_store::{Db, NewCategory, NewPeriod, NewTariff};

pub const MAX_NAME_CHARS: usize = 100;
pub const MAX_DESCRIPTION_CHARS: usize = 2_000;
pub const MAX_PERIOD_DAYS: u32 = 3_650;
pub const MAX_DEVICES: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PeriodInput {
    pub days: u32,
    #[serde(default)]
    pub prices: CurrencyPrices,
    #[serde(default)]
    pub stars: Option<Stars>,
    #[serde(default)]
    pub sort: i32,
    #[serde(default = "enabled")]
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TariffInput {
    pub category_id: CategoryId,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "enabled")]
    pub on_sale: bool,
    #[serde(default)]
    pub sort: i32,
    pub devices: DevicePolicy,
    #[serde(default)]
    pub traffic: TrafficPolicy,
    pub squads: Vec<SquadId>,
    #[serde(default)]
    pub external_squad: Option<SquadId>,
    pub periods: Vec<PeriodInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CategoryInput {
    pub name: String,
    #[serde(default)]
    pub sort: i32,
    #[serde(default = "enabled")]
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TariffsView {
    pub currency: Currency,
    pub categories: Vec<Category>,
    pub tariffs: Vec<Tariff>,
}

const fn enabled() -> bool {
    true
}

fn invalid(field: &str, reason: impl Into<String>) -> ServiceError {
    ServiceError::rejected("tariff.invalid", format!("{field}: {}", reason.into()))
}

fn check_prices(field: &str, prices: &CurrencyPrices) -> ServiceResult<()> {
    for (currency, value) in [(Currency::Usd, prices.usd), (Currency::Rub, prices.rub)] {
        let Some(value) = value else {
            continue;
        };
        if value < 0 {
            return Err(invalid(field, "цена не может быть отрицательной"));
        }
        if currency == Currency::Rub && value % Currency::Rub.rounding_step() != 0 {
            return Err(invalid(field, "цена в рублях задаётся целыми рублями"));
        }
    }
    Ok(())
}

fn check_stars(field: &str, stars: Option<Stars>) -> ServiceResult<()> {
    if stars.is_some_and(|s| s.0 <= 0) {
        return Err(invalid(field, "цена в звёздах должна быть больше нуля"));
    }
    Ok(())
}

impl TariffInput {
    pub fn validate(&self) -> ServiceResult<()> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
            return Err(invalid("Название", "от 1 до 100 символов"));
        }
        if self
            .description
            .as_deref()
            .is_some_and(|d| d.chars().count() > MAX_DESCRIPTION_CHARS)
        {
            return Err(invalid("Описание", "не длиннее 2000 символов"));
        }
        if self.periods.is_empty() {
            return Err(invalid("Сроки", "нужен хотя бы один срок"));
        }
        let mut days: Vec<u32> = Vec::with_capacity(self.periods.len());
        for period in &self.periods {
            if period.days == 0 || period.days > MAX_PERIOD_DAYS {
                return Err(invalid("Срок", "от 1 до 3650 дней"));
            }
            if days.contains(&period.days) {
                return Err(invalid(
                    "Срок",
                    format!("срок {} дней указан дважды", period.days),
                ));
            }
            days.push(period.days);
            check_prices("Цена", &period.prices)?;
            check_stars("Цена в звёздах", period.stars)?;
        }
        if self.devices.included == 0 || self.devices.included > MAX_DEVICES {
            return Err(invalid("Устройств в тарифе", "от 1 до 100"));
        }
        if let Some(extra) = &self.devices.extra {
            if extra.max == 0 || extra.max > MAX_DEVICES {
                return Err(invalid("Доп. устройств не больше", "от 1 до 100"));
            }
            check_prices("Цена за устройство в месяц", &extra.monthly)?;
            check_stars("Цена устройства в звёздах", extra.monthly_stars)?;
        }
        self.traffic.validate()?;
        if self.squads.is_empty() {
            return Err(invalid(
                "Сквады",
                "выберите хотя бы один сквад, иначе у клиентов не будет серверов",
            ));
        }
        let mut squads = self.squads.clone();
        squads.sort();
        squads.dedup();
        if squads.len() != self.squads.len() {
            return Err(invalid("Сквады", "сквад указан дважды"));
        }
        self.preview().validate()?;
        Ok(())
    }

    fn preview(&self) -> Tariff {
        Tariff {
            id: TariffId(0),
            category_id: self.category_id,
            name: self.name.trim().to_string(),
            description: self.description.clone(),
            on_sale: self.on_sale,
            sort: self.sort,
            periods: self
                .periods
                .iter()
                .map(|p| TariffPeriod {
                    id: PeriodId(0),
                    days: p.days,
                    prices: p.prices,
                    stars: p.stars,
                    sort: p.sort,
                    active: p.active,
                })
                .collect(),
            devices: self.devices.clone(),
            traffic: self.traffic,
            squads: self.squads.clone(),
            external_squad: self.external_squad,
        }
    }

    pub fn to_new(&self) -> NewTariff {
        NewTariff {
            category_id: self.category_id,
            name: self.name.trim().to_string(),
            description: self
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_string),
            on_sale: self.on_sale,
            sort: self.sort,
            devices: self.devices.clone(),
            traffic: self.traffic,
            squads: self.squads.clone(),
            external_squad: self.external_squad,
            periods: self
                .periods
                .iter()
                .map(|p| NewPeriod {
                    days: p.days,
                    prices: p.prices,
                    stars: p.stars,
                    sort: p.sort,
                    active: p.active,
                })
                .collect(),
        }
    }
}

impl CategoryInput {
    pub fn to_new(&self) -> ServiceResult<NewCategory> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            return Err(invalid("Категория", "название от 1 до 60 символов"));
        }
        Ok(NewCategory {
            name: name.to_string(),
            sort: self.sort,
            visible: self.visible,
        })
    }
}

pub struct TariffAdmin<'a> {
    services: &'a Services,
}

impl<'a> TariffAdmin<'a> {
    pub fn new(services: &'a Services) -> Self {
        TariffAdmin { services }
    }

    pub async fn list(&self) -> ServiceResult<TariffsView> {
        let mut conn = self.services.store().conn().await?;
        Ok(TariffsView {
            currency: self.services.settings().currency,
            categories: conn.tariffs().categories(true).await?,
            tariffs: conn.tariffs().list(true).await?,
        })
    }

    pub async fn get(&self, id: TariffId) -> ServiceResult<Tariff> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.tariffs().get(id).await?)
    }

    async fn check_references(&self, db: &mut impl Db, input: &TariffInput) -> ServiceResult<()> {
        if !db
            .tariffs()
            .categories(true)
            .await?
            .iter()
            .any(|c| c.id == input.category_id)
        {
            return Err(ServiceError::NotFound("категория"));
        }
        let Ok(remna) = self.services.remna() else {
            return Ok(());
        };
        match remna.internal_squads().await {
            Ok(known) => {
                let missing: Vec<String> = input
                    .squads
                    .iter()
                    .filter(|s| !known.internal_squads.iter().any(|k| k.uuid == s.0))
                    .map(|s| s.to_string())
                    .collect();
                if missing.is_empty() {
                    Ok(())
                } else {
                    Err(invalid(
                        "Сквады",
                        format!("в Remnawave нет сквадов {}", missing.join(", ")),
                    ))
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "сквады тарифа не сверены с Remnawave");
                Ok(())
            }
        }
    }

    pub async fn create(&self, input: &TariffInput, admin: AdminId) -> ServiceResult<Tariff> {
        input.validate()?;
        let mut tx = self.services.store().begin().await?;
        self.check_references(&mut tx, input).await?;
        let tariff = tx.tariffs().create(&input.to_new()).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "tariff.create",
                "tariff",
                Some(tariff.id.0),
                serde_json::json!({ "name": tariff.name }),
            ))
            .await?;
        tx.commit().await?;
        Ok(tariff)
    }

    pub async fn update(
        &self,
        id: TariffId,
        input: &TariffInput,
        admin: AdminId,
    ) -> ServiceResult<Tariff> {
        input.validate()?;
        let mut tx = self.services.store().begin().await?;
        tx.tariffs().get(id).await?;
        self.check_references(&mut tx, input).await?;
        let tariff = tx.tariffs().update(id, &input.to_new()).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "tariff.update",
                "tariff",
                Some(id.0),
                serde_json::json!({ "name": tariff.name }),
            ))
            .await?;
        tx.commit().await?;
        Ok(tariff)
    }

    pub async fn set_on_sale(
        &self,
        id: TariffId,
        on_sale: bool,
        admin: AdminId,
    ) -> ServiceResult<Tariff> {
        let mut tx = self.services.store().begin().await?;
        tx.tariffs().set_on_sale(id, on_sale).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "tariff.on_sale",
                "tariff",
                Some(id.0),
                serde_json::json!({ "on_sale": on_sale }),
            ))
            .await?;
        let tariff = tx.tariffs().get(id).await?;
        tx.commit().await?;
        Ok(tariff)
    }

    pub async fn create_category(
        &self,
        input: &CategoryInput,
        admin: AdminId,
    ) -> ServiceResult<Category> {
        let category = input.to_new()?;
        let mut tx = self.services.store().begin().await?;
        let created = tx.tariffs().create_category(&category).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "category.create",
                "category",
                Some(created.id.0),
                serde_json::json!({ "name": created.name }),
            ))
            .await?;
        tx.commit().await?;
        Ok(created)
    }

    pub async fn update_category(
        &self,
        id: CategoryId,
        input: &CategoryInput,
        admin: AdminId,
    ) -> ServiceResult<Category> {
        let category = input.to_new()?;
        let mut tx = self.services.store().begin().await?;
        let updated = tx.tariffs().update_category(id, &category).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "category.update",
                "category",
                Some(id.0),
                serde_json::json!({ "name": updated.name, "visible": updated.visible }),
            ))
            .await?;
        tx.commit().await?;
        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_domain::tariff::ExtraDevices;
    use uuid::Uuid;

    fn input() -> TariffInput {
        TariffInput {
            category_id: CategoryId(1),
            name: " Стандарт ".into(),
            description: None,
            on_sale: true,
            sort: 0,
            devices: DevicePolicy {
                included: 3,
                extra: Some(ExtraDevices {
                    max: 2,
                    monthly: CurrencyPrices {
                        usd: Some(99),
                        rub: Some(4_900),
                    },
                    monthly_stars: Some(Stars(50)),
                }),
            },
            traffic: TrafficPolicy::UNLIMITED,
            squads: vec![SquadId(Uuid::from_u128(1))],
            external_squad: None,
            periods: vec![PeriodInput {
                days: 30,
                prices: CurrencyPrices {
                    usd: Some(299),
                    rub: Some(19_900),
                },
                stars: Some(Stars(150)),
                sort: 0,
                active: true,
            }],
        }
    }

    #[test]
    fn тариф_из_формы_проверяется_до_записи() {
        assert!(input().validate().is_ok());
        assert_eq!(input().to_new().name, "Стандарт");
        let mut kopecks = input();
        kopecks.periods[0].prices.rub = Some(19_950);
        assert!(kopecks.validate().is_err());
        let mut twice = input();
        twice.periods.push(twice.periods[0].clone());
        assert!(twice.validate().is_err());
        let mut no_squads = input();
        no_squads.squads.clear();
        assert!(no_squads.validate().is_err());
        let mut no_devices = input();
        no_devices.devices.included = 0;
        assert!(no_devices.validate().is_err());
        let mut free_stars = input();
        free_stars.periods[0].stars = Some(Stars(0));
        assert!(free_stars.validate().is_err());
    }
}
