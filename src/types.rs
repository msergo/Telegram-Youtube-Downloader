use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct TelegramWebhook {
    pub message: TelegramMessage,
}

#[derive(Debug, Deserialize)]
pub struct TelegramMessage {
    pub chat: TelegramChat,
    pub from: TelegramFrom,
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
}

#[derive(Debug, Deserialize)]
pub struct TelegramFrom {
    pub id: i64,
}
