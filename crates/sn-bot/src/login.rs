use sn_domain::client::TelegramProfile;
use sn_services::{ServiceError, Services};

pub const PREFIX: &str = "auth_";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginRequest {
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

pub fn token_from_param(param: &str) -> Option<&str> {
    let token = param.strip_prefix(PREFIX)?;
    let valid = (16..=64).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    valid.then_some(token)
}

pub async fn describe(
    services: &Services,
    token: &str,
    profile: &TelegramProfile,
) -> Result<LoginRequest, ServiceError> {
    let prompt = services
        .auth()
        .telegram_login_prompt(token, profile)
        .await?;
    Ok(LoginRequest {
        ip: prompt.ip,
        user_agent: prompt.device,
    })
}

pub async fn confirm(
    services: &Services,
    token: &str,
    profile: &TelegramProfile,
) -> Result<(), ServiceError> {
    services
        .auth()
        .confirm_telegram_login(token, profile)
        .await
        .map(|_| ())
}

pub async fn reject(services: &Services, token: &str) -> Result<(), ServiceError> {
    services.auth().reject_telegram_login(token).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn токен_входа_только_из_параметра_auth() {
        assert_eq!(
            token_from_param("auth_0123456789abcdef0123456789abcdef"),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert_eq!(token_from_param("auth_short"), None);
        assert_eq!(token_from_param("ref_0123456789abcdef0123"), None);
        assert_eq!(token_from_param("auth_0123456789abcdef!!"), None);
    }
}
