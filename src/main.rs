use axum::{Json, Router, routing::get, routing::post};
use serde_json::Value;
use std::env;
use tokio::process::Command;

use crate::types::TelegramWebhook;
mod send_audio;
use send_audio::send_audio_to_telegram;

mod types;

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(|| async { "OK" }))
        .route("/webhook", post(download_handler));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn download_handler(Json(payload): Json<TelegramWebhook>) {
    // Check if the message is from the allowed user
    let allowed_user_id: i64 = env::var("ALLOWED_USER_ID")
        .expect("ALLOWED_USER_ID must be set")
        .parse()
        .expect("ALLOWED_USER_ID must be a valid integer");

    if payload.message.from.id != allowed_user_id {
        println!("Unauthorized user: {}", payload.message.from.id);
        return;
    }

    let Some(url) = payload.message.text else {
        return;
    };

    let bot_token = env::var("TELEGRAM_BOT_TOKEN").unwrap();

    tokio::spawn(async move {
        // Step 1: get metadata
        let output = Command::new("yt-dlp")
            .arg("-j")
            .arg("-6")
            .arg(&url)
            .output()
            .await;

        let metadata: Option<Value> = output
            .ok()
            .and_then(|out| serde_json::from_slice(&out.stdout).ok());

        let artist = metadata
            .as_ref()
            .and_then(|m| m.get("artist"))
            .and_then(|a| a.as_str())
            .unwrap_or("Unknown Artist")
            .to_string();

        let title = metadata
            .as_ref()
            .and_then(|m| m.get("title"))
            .and_then(|t| t.as_str())
            .unwrap_or("Untitled")
            .to_string();

        let file_name = format!("{} - {}.mp3", artist, title)
            .replace('/', "_") // replace slashes to avoid directory issues
            .replace('\\', "_"); // replace backslashes to avoid directory issues

        let output_file = format!("./downloads/{}", file_name);
        let status = Command::new("yt-dlp")
            .arg("-6")
            .arg("-v")
            .arg("-x") // extract audio
            .arg("--audio-format")
            .arg("mp3") // convert to mp3
            .arg("-o")
            .arg(&output_file)
            .arg(&url)
            .status()
            .await;

        match status {
            Ok(s) if s.success() => {
                send_audio_to_telegram(payload.message.chat.id, &output_file, &bot_token).await
            }
            Ok(s) => {
                println!("yt-dlp exited with status: {:?}", s);
            }
            Err(e) => {
                println!("Failed to spawn yt-dlp for job {}: {}", file_name, e);
            }
        }
    });
}
