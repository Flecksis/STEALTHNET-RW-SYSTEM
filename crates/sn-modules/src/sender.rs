use async_trait::async_trait;
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutButton {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Photo<'a> {
    Cached(&'a str),
    Upload {
        bytes: &'a [u8],
        content_type: &'a str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outgoing<'a> {
    pub html: &'a str,
    pub photo: Option<Photo<'a>>,
    pub buttons: &'a [OutButton],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    Sent { file_id: Option<String> },
    Blocked(String),
    Wait(Duration),
    StaleFile,
    Failed { error: String, retry: bool },
    NoBot(String),
}

impl Delivery {
    pub fn is_sent(&self) -> bool {
        matches!(self, Delivery::Sent { .. })
    }
}

#[async_trait]
pub trait BotSender: Send + Sync {
    fn bot_id(&self) -> Option<i64>;

    async fn send(&self, chat_id: i64, message: &Outgoing<'_>) -> Delivery;
}
