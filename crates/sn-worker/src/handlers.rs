use async_trait::async_trait;
use chrono::Duration;
use sn_domain::jobs::{JobKind, JobPayload, retry_delay};
use sn_services::autorenew::ChargeCheck;
use sn_services::fulfillment::FulfillmentOutcome;
use sn_services::inbox::PollResult;
use sn_services::{ServiceError, Services};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobResult {
    Done,
    Retry { after: Duration, error: String },
    Review(String),
    Fail(String),
}

impl JobResult {
    pub fn from_error(err: ServiceError, attempt: u32) -> Self {
        match err {
            ServiceError::NotImplemented(what) => {
                JobResult::Review(format!("не реализовано: {what}"))
            }
            err if err.is_not_implemented() => JobResult::Review(err.to_string()),
            err if err.is_retryable() => JobResult::Retry {
                after: retry_delay(attempt),
                error: err.to_string(),
            },
            ServiceError::NotFound(what) => JobResult::Fail(format!("не найдено: {what}")),
            err => JobResult::Retry {
                after: retry_delay(attempt),
                error: err.to_string(),
            },
        }
    }
}

#[async_trait]
pub trait JobHandler: Send + Sync {
    async fn handle(&self, services: &Services, payload: &JobPayload, attempt: u32) -> JobResult;
}

pub struct CoreHandlers;

#[async_trait]
impl JobHandler for CoreHandlers {
    async fn handle(&self, services: &Services, payload: &JobPayload, attempt: u32) -> JobResult {
        match payload {
            JobPayload::FulfillOrder { order_id } => {
                match services.fulfillment().run(*order_id).await {
                    Ok(FulfillmentOutcome::Done { .. }) => JobResult::Done,
                    Ok(FulfillmentOutcome::Retry {
                        after_seconds,
                        reason,
                    }) => JobResult::Retry {
                        after: Duration::seconds(i64::try_from(after_seconds).unwrap_or(i64::MAX))
                            .max(retry_delay(attempt)),
                        error: reason,
                    },
                    Ok(FulfillmentOutcome::Review { reason }) => JobResult::Review(reason),
                    Ok(FulfillmentOutcome::Refunded { .. }) => JobResult::Done,
                    Err(err) => JobResult::from_error(err, attempt),
                }
            }
            JobPayload::ProcessRemnaEvent { event_id } => {
                match services.remna_sync().process_event(*event_id).await {
                    Ok(()) => JobResult::Done,
                    Err(err) => JobResult::from_error(err, attempt),
                }
            }
            JobPayload::ModuleTask { module, task, .. } => {
                let ctx = services.module_context();
                if !services.modules().is_enabled(*module) {
                    return JobResult::Done;
                }
                match services.modules().get(*module).run_task(&ctx, task).await {
                    Ok(()) => JobResult::Done,
                    Err(err) => JobResult::Retry {
                        after: retry_delay(attempt),
                        error: err.to_string(),
                    },
                }
            }
            JobPayload::SyncSubscription { subscription_id } => {
                match services
                    .remna_sync()
                    .refresh_subscription(*subscription_id)
                    .await
                {
                    Ok(_) => JobResult::Done,
                    Err(err) => JobResult::from_error(err, attempt),
                }
            }
            JobPayload::PollPayment { payment_id } => {
                let last = attempt.saturating_add(1) >= JobKind::PollPayment.max_attempts();
                match services.autorenew().check_charge(*payment_id).await {
                    Ok(Some(ChargeCheck::Settled) | None) => {}
                    Ok(Some(ChargeCheck::Pending)) if last => {
                        return JobResult::Review(
                            "исход автосписания так и не выяснен: проверьте платёж в кабинете кассы"
                                .into(),
                        );
                    }
                    Ok(Some(ChargeCheck::Pending)) => {
                        return JobResult::Retry {
                            after: retry_delay(attempt),
                            error: "ждём ответа кассы по автосписанию".into(),
                        };
                    }
                    Ok(Some(ChargeCheck::Unclear(reason))) => return JobResult::Review(reason),
                    Err(err) => return JobResult::from_error(err, attempt),
                }
                match services.inbox().poll_payment(*payment_id).await {
                    Ok(PollResult::Settled | PollResult::Unsupported) => JobResult::Done,
                    Ok(PollResult::Pending) if last => JobResult::Done,
                    Ok(PollResult::Pending) => JobResult::Retry {
                        after: retry_delay(attempt),
                        error: "счёт ещё не оплачен".into(),
                    },
                    Err(err) => JobResult::from_error(err, attempt),
                }
            }
            JobPayload::Notify { .. } => {
                JobResult::from_error(ServiceError::NotImplemented("jobs.notify"), attempt)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_remnawave::RemnaError;

    #[test]
    fn нереализованное_уходит_на_ручную_проверку_а_сбой_сети_на_повтор() {
        assert!(matches!(
            JobResult::from_error(ServiceError::NotImplemented("x"), 1),
            JobResult::Review(_)
        ));
        assert!(matches!(
            JobResult::from_error(
                ServiceError::Store(sn_store::StoreError::NotImplemented("x")),
                1
            ),
            JobResult::Review(_)
        ));
        assert_eq!(
            JobResult::from_error(ServiceError::Remna(RemnaError::Timeout), 2),
            JobResult::Retry {
                after: Duration::seconds(30),
                error: RemnaError::Timeout.to_string()
            }
        );
        assert!(matches!(
            JobResult::from_error(ServiceError::NotFound("заказ"), 1),
            JobResult::Fail(_)
        ));
    }
}
