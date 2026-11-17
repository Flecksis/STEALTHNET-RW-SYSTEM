use serde::{Deserialize, Serialize};

pub const MAX_LEN: usize = 64;
pub const SEPARATOR: &str = "__";

fn valid_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_LEN
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn segments(raw: &str) -> impl Iterator<Item = &str> {
    raw.trim()
        .split(SEPARATOR)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

pub fn segment<'a>(raw: &'a str, prefix: &str) -> Option<&'a str> {
    segments(raw)
        .filter_map(|s| s.strip_prefix(prefix))
        .find(|value| valid_value(value))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Utm {
    pub source: Option<String>,
    pub medium: Option<String>,
    pub campaign: Option<String>,
    pub content: Option<String>,
    pub term: Option<String>,
}

impl Utm {
    pub fn parse(raw: &str) -> Option<Utm> {
        let pick = |prefix: &str| segment(raw, prefix).map(str::to_string);
        let utm = Utm {
            source: pick("s_"),
            medium: pick("m_"),
            campaign: pick("k_"),
            content: pick("n_"),
            term: pick("t_"),
        };
        (!utm.is_empty()).then_some(utm)
    }

    pub fn is_empty(&self) -> bool {
        self.source.is_none()
            && self.medium.is_none()
            && self.campaign.is_none()
            && self.content.is_none()
            && self.term.is_none()
    }
}

pub fn referral_code(raw: &str) -> Option<String> {
    segment(raw, "ref_")
        .map(str::to_ascii_uppercase)
        .filter(|code| code.bytes().all(|b| b.is_ascii_alphanumeric()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn метки_разбираются_через_двойное_подчёркивание() {
        let utm =
            Utm::parse("ref_ABC123__s_tiktok__m_cpc__k_winter-sale__n_video1__t_vpn").unwrap();
        assert_eq!(utm.source.as_deref(), Some("tiktok"));
        assert_eq!(utm.medium.as_deref(), Some("cpc"));
        assert_eq!(utm.campaign.as_deref(), Some("winter-sale"));
        assert_eq!(utm.content.as_deref(), Some("video1"));
        assert_eq!(utm.term.as_deref(), Some("vpn"));
        assert_eq!(
            referral_code("ref_abc123__s_tiktok").as_deref(),
            Some("ABC123")
        );
    }

    #[test]
    fn чужие_параметры_не_принимаются_за_метки() {
        assert_eq!(Utm::parse("auth_0123456789abcdef"), None);
        assert_eq!(Utm::parse("ref_ABC"), None);
        assert_eq!(Utm::parse(""), None);
        assert_eq!(Utm::parse("s_"), None);
        assert_eq!(Utm::parse("s_bad value"), None);
        assert_eq!(referral_code("auth_x"), None);
        assert_eq!(referral_code("ref_"), None);
        assert_eq!(referral_code("ref_A-B"), None);
    }

    #[test]
    fn берётся_первое_значение_метки() {
        let utm = Utm::parse("s_first__s_second").unwrap();
        assert_eq!(utm.source.as_deref(), Some("first"));
        assert_eq!(utm.medium, None);
    }
}
