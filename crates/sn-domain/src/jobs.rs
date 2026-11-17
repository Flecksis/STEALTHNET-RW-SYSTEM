use crate::DomainError;
use crate::ids::{ClientId, OrderId, PaymentId, SubscriptionId};
use crate::module::ModuleId;
use chrono::Duration;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    FulfillOrder,
    ProcessRemnaEvent,
    SyncSubscription,
    PollPayment,
    Notify,
    ModuleTask,
}

impl JobKind {
    pub const ALL: [JobKind; 6] = [
        JobKind::FulfillOrder,
        JobKind::ProcessRemnaEvent,
        JobKind::SyncSubscription,
        JobKind::PollPayment,
        JobKind::Notify,
        JobKind::ModuleTask,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            JobKind::FulfillOrder => "fulfill_order",
            JobKind::ProcessRemnaEvent => "process_remna_event",
            JobKind::SyncSubscription => "sync_subscription",
            JobKind::PollPayment => "poll_payment",
            JobKind::Notify => "notify",
            JobKind::ModuleTask => "module_task",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        JobKind::ALL
            .into_iter()
            .find(|k| k.as_str() == value)
            .ok_or_else(|| DomainError::unknown("job_kind", value))
    }

    pub const fn max_attempts(self) -> u32 {
        match self {
            JobKind::FulfillOrder => 12,
            JobKind::ProcessRemnaEvent | JobKind::SyncSubscription => 8,
            JobKind::PollPayment => 20,
            JobKind::Notify => 6,
            JobKind::ModuleTask => 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobPayload {
    FulfillOrder {
        order_id: OrderId,
    },
    ProcessRemnaEvent {
        event_id: i64,
    },
    SyncSubscription {
        subscription_id: SubscriptionId,
    },
    PollPayment {
        payment_id: PaymentId,
    },
    Notify {
        client_id: ClientId,
        template: String,
        params: serde_json::Value,
    },
    ModuleTask {
        module: ModuleId,
        task: String,
        payload: serde_json::Value,
    },
}

impl JobPayload {
    pub const fn kind(&self) -> JobKind {
        match self {
            JobPayload::FulfillOrder { .. } => JobKind::FulfillOrder,
            JobPayload::ProcessRemnaEvent { .. } => JobKind::ProcessRemnaEvent,
            JobPayload::SyncSubscription { .. } => JobKind::SyncSubscription,
            JobPayload::PollPayment { .. } => JobKind::PollPayment,
            JobPayload::Notify { .. } => JobKind::Notify,
            JobPayload::ModuleTask { .. } => JobKind::ModuleTask,
        }
    }

    pub fn dedupe_key(&self) -> Option<String> {
        match self {
            JobPayload::FulfillOrder { order_id } => Some(format!("fulfill:{order_id}")),
            JobPayload::ProcessRemnaEvent { event_id } => Some(format!("remna_event:{event_id}")),
            JobPayload::PollPayment { payment_id } => Some(format!("poll:{payment_id}")),
            JobPayload::SyncSubscription { .. }
            | JobPayload::Notify { .. }
            | JobPayload::ModuleTask { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed,
    Review,
}

impl JobStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Done => "done",
            JobStatus::Failed => "failed",
            JobStatus::Review => "review",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "queued" => Ok(JobStatus::Queued),
            "running" => Ok(JobStatus::Running),
            "done" => Ok(JobStatus::Done),
            "failed" => Ok(JobStatus::Failed),
            "review" => Ok(JobStatus::Review),
            other => Err(DomainError::unknown("job_status", other)),
        }
    }
}

pub fn retry_delay(attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(8);
    let seconds = (15_i64 << exponent).min(3_600);
    Duration::seconds(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn повторы_растут_и_упираются_в_час() {
        assert_eq!(retry_delay(1), Duration::seconds(15));
        assert_eq!(retry_delay(2), Duration::seconds(30));
        assert_eq!(retry_delay(5), Duration::seconds(240));
        assert_eq!(retry_delay(50), Duration::seconds(3_600));
    }

    #[test]
    fn выдача_заказа_дедуплицируется() {
        let payload = JobPayload::FulfillOrder {
            order_id: OrderId(7),
        };
        assert_eq!(payload.dedupe_key().as_deref(), Some("fulfill:7"));
        assert_eq!(payload.kind().as_str(), "fulfill_order");
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["kind"], "fulfill_order");
    }
}
