use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct TelegramWebhook {
    pub message: TelegramMessage,
}

#[derive(Debug, Deserialize)]
pub struct TelegramMessage {
    pub chat: TelegramChat,
    pub text: Option<String>, // Text might be missing (e.g., photo messages)
}

#[derive(Debug, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
}
