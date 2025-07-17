use log::{error, info};
use reqwest::{Client, multipart};
use std::path::Path;
use tokio_util::codec::{BytesCodec, FramedRead};

pub async fn send_audio_to_telegram(chat_id: i64, path: &str, bot_token: &str) {
    let client = Client::new();
    let url = format!("https://api.telegram.org/bot{}/sendAudio", bot_token);

    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Failed to open file {}: {}", path, e);
            return;
        }
    };

    let stream = FramedRead::new(file, BytesCodec::new());
    let file_body = reqwest::Body::wrap_stream(stream);

    let file_name = Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "audio.mp3".to_string());

    let form = multipart::Form::new()
        .text("chat_id", chat_id.to_string())
        .part(
            "audio",
            multipart::Part::stream(file_body).file_name(file_name),
        );

    match client.post(&url).multipart(form).send().await {
        Ok(res) if res.status().is_success() => {
            info!("Audio sent successfully to Telegram.");
            let _ = tokio::fs::remove_file(&path).await;
            info!("Deleted file: {}", path);
        }
        Ok(res) => {
            let status = res.status();
            let body = res.text().await.unwrap_or_else(|_| "Unknown error".into());
            error!("Telegram API error {}: {}", status, body);
        }
        Err(e) => {
            error!("Failed to send audio: {}", e);
        }
    }
}
