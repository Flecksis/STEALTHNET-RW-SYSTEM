use crate::services::Services;
use sn_domain::module::ModuleId;
use sn_modules::catalog::start_param::{Utm, with_referral};
use sn_modules::catalog::utm::clicks::{ClickChannel, record_click};

pub fn site_utm(raw: &serde_json::Value) -> Option<Utm> {
    let tag = |key: &str| raw.get(key).and_then(serde_json::Value::as_str);
    Utm::from_parts(
        tag("source"),
        tag("medium"),
        tag("campaign"),
        tag("content"),
        tag("term"),
    )
}

fn checked(utm: &Utm) -> Option<Utm> {
    Utm::from_parts(
        utm.source.as_deref(),
        utm.medium.as_deref(),
        utm.campaign.as_deref(),
        utm.content.as_deref(),
        utm.term.as_deref(),
    )
}

pub fn site_param(utm: Option<&Utm>) -> Option<String> {
    utm.and_then(checked).map(|utm| utm.to_param())
}

pub fn site_start_param(utm: Option<&Utm>, referral: Option<&str>) -> Option<String> {
    with_referral(site_param(utm).as_deref(), referral)
}

impl Services {
    pub async fn count_utm_click(&self, channel: ClickChannel, start_param: Option<&str>) -> bool {
        let Some(utm) = start_param.and_then(Utm::parse) else {
            return false;
        };
        if !self.modules().is_enabled(ModuleId::Utm) {
            return false;
        }
        let written = async {
            let mut conn = self.store().conn().await?;
            record_click(&mut conn, channel, &utm, self.now()).await
        }
        .await;
        match written {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(channel = channel.as_str(), error = %err, "переход по метке не записан");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn метка_сайта_собирается_из_допустимых_значений() {
        let utm = site_utm(&serde_json::json!({
            "source": "tiktok",
            "medium": "bad value",
            "campaign": "osen-shorts",
            "content": 5,
            "at": 1_700_000_000_000_i64,
        }))
        .unwrap();
        assert_eq!(utm.to_param(), "s_tiktok__k_osen-shorts");
        assert_eq!(site_utm(&serde_json::json!({ "source": "осень" })), None);
        assert_eq!(site_utm(&serde_json::json!("s_tiktok")), None);
        assert_eq!(
            site_start_param(Some(&utm), Some("FRIEND42")).as_deref(),
            Some("ref_FRIEND42__s_tiktok__k_osen-shorts")
        );
        assert_eq!(site_start_param(None, None), None);
        let forged = Utm {
            source: Some("a__s_vk".into()),
            term: Some("ok".into()),
            ..Utm::default()
        };
        assert_eq!(site_param(Some(&forged)).as_deref(), Some("t_ok"));
    }
}
