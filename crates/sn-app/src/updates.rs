use sn_services::Services;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub async fn checks(services: Services, cancel: CancellationToken) {
    loop {
        let delay = match services.updates().next_check_in().await {
            Ok(Some(delay)) => delay,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(error = %err, "не удалось узнать, когда проверять новую версию");
                Duration::from_secs(600)
            }
        };
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(delay.max(Duration::from_secs(5))) => {}
        }
        if let Err(err) = services.updates().scheduled_check().await {
            tracing::warn!(error = %err, "проверка новой версии не удалась");
        }
    }
}
