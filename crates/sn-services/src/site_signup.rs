use crate::auth::RequestMeta;
use crate::error::{ServiceError, ServiceResult};
use crate::limits::rules;
use crate::services::Services;
use sn_modules::catalog::antibot::signups::{self, Refusal};
use sn_modules::catalog::antibot::{self, SignupCheck};

const ASKED: &str = "asked";

pub(crate) fn email_taken() -> ServiceError {
    ServiceError::rejected(
        "auth.email_taken",
        "Эта почта уже зарегистрирована: войдите с паролем",
    )
}

pub(crate) fn captcha_error(check: SignupCheck) -> ServiceError {
    match check {
        SignupCheck::Unavailable => ServiceError::rejected(
            antibot::UNAVAILABLE,
            "Не удалось проверить, что вы не робот: сервис проверки не ответил. Попробуйте ещё раз через минуту",
        ),
        SignupCheck::Failed => {
            ServiceError::rejected(antibot::FAILED, "Проверка не пройдена. Попробуйте ещё раз")
        }
        SignupCheck::Off | SignupCheck::Passed | SignupCheck::Missing => {
            ServiceError::rejected(antibot::CAPTCHA_REQUIRED, "Пройдите проверку «Я не робот»")
        }
    }
}

pub(crate) async fn count(services: &Services, kind: &str) {
    let saved = async {
        let mut conn = services.store().conn().await?;
        signups::refused(&mut conn, kind).await
    }
    .await;
    if let Err(err) = saved {
        tracing::warn!(error = %err, "антибот: отказ в создании аккаунта не учтён");
    }
}

pub(crate) fn refusal(refusal: Refusal) -> ServiceError {
    ServiceError::rejected(refusal.code(), refusal.message())
}

async fn refused(services: &Services, why: Refusal) -> ServiceError {
    count(services, why.as_str()).await;
    refusal(why)
}

pub(crate) async fn over_limit(
    services: &Services,
    refusal: Refusal,
    captcha: Option<&str>,
    meta: &RequestMeta,
) -> ServiceResult<()> {
    let ctx = services.module_context();
    if refusal != Refusal::Minute || antibot::spare_captcha(&ctx).is_none() {
        return Err(refused(services, refusal).await);
    }
    if captcha.is_none_or(|token| token.trim().is_empty()) {
        count(services, ASKED).await;
        return Err(captcha_error(SignupCheck::Missing));
    }
    services
        .limits()
        .hit_ip(rules::CAPTCHA_IP, meta.ip.as_deref())
        .await?;
    match antibot::check_human(&ctx, captcha, meta.ip.as_deref()).await? {
        SignupCheck::Passed => Ok(()),
        check => Err(captcha_error(check)),
    }
}

pub(crate) fn asked_before(services: &Services) -> bool {
    antibot::signup_captcha(&services.module_context()).is_some()
}
