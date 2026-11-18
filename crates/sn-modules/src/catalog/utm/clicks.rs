use crate::catalog::start_param::Utm;
use chrono::{DateTime, Utc};
use sn_store::{Db, StoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickChannel {
    Bot,
    Miniapp,
    Site,
}

impl ClickChannel {
    pub const fn as_str(self) -> &'static str {
        match self {
            ClickChannel::Bot => "bot",
            ClickChannel::Miniapp => "miniapp",
            ClickChannel::Site => "site",
        }
    }
}

pub async fn record_click(
    db: &mut impl Db,
    channel: ClickChannel,
    utm: &Utm,
    at: DateTime<Utc>,
) -> StoreResult<()> {
    let text = |value: &Option<String>| value.clone().unwrap_or_default();
    sqlx::query(
        "INSERT INTO utm_clicks (at, channel, source, medium, campaign, content, term, clicks) \
         VALUES (date_trunc('hour', $1), $2, $3, $4, $5, $6, $7, 1) \
         ON CONFLICT (at, channel, source, medium, campaign, content, term) \
         DO UPDATE SET clicks = utm_clicks.clicks + 1",
    )
    .bind(at)
    .bind(channel.as_str())
    .bind(text(&utm.source))
    .bind(text(&utm.medium))
    .bind(text(&utm.campaign))
    .bind(text(&utm.content))
    .bind(text(&utm.term))
    .execute(db.conn())
    .await?;
    Ok(())
}
