use sn_services::Services;
use sn_services::settings::SETTINGS_CHANNEL;
use sqlx::postgres::PgListener;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const RECONNECT_DELAY: Duration = Duration::from_secs(5);

async fn reload(services: &Services) {
    if let Err(err) = services.reload().await {
        tracing::warn!(error = %err, "настройки не перечитаны");
    }
}

async fn listen(services: &Services, cancel: &CancellationToken) -> Result<(), sqlx::Error> {
    let mut listener = PgListener::connect_with(services.store().pool()).await?;
    listener.listen(SETTINGS_CHANNEL).await?;
    reload(services).await;
    let own = services.instance_id().to_string();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            message = listener.try_recv() => match message? {
                Some(notification) if notification.payload() == own => {}
                Some(_) => {
                    tracing::info!("настройки изменены в другом процессе, перечитываем");
                    reload(services).await;
                }
                None => {
                    tracing::warn!("связь с базой для уведомлений о настройках восстановлена");
                    reload(services).await;
                }
            }
        }
    }
}

pub async fn settings_changes(services: Services, cancel: CancellationToken) {
    loop {
        match listen(&services, &cancel).await {
            Ok(()) => return,
            Err(err) => {
                tracing::warn!(error = %err, "подписка на изменения настроек прервалась");
            }
        }
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(RECONNECT_DELAY) => {}
        }
    }
}
