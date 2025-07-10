use axum::{Json, Router, routing::get, routing::post};
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
        // Step 1: Get original video title
        let output = Command::new("yt-dlp")
            .arg("--get-title")
            .arg("-4")
            .arg("-v")
            .stderr(std::process::Stdio::null()) // suppress stderr
            .stdout(std::process::Stdio::null()) // optionally suppress stdout too
            .arg(&url)
            .output()
            .await;

        let Ok(output) = output else {
            eprintln!("Failed to get title from yt-dlp");
            return;
        };

        let title = String::from_utf8_lossy(&output.stdout)
            .trim()
            .replace('/', "_")
            .replace('\\', "_");

        let file_name = format!("{}.mp3", title);
        let output_file = format!("./downloads/{}", file_name);
        let status = Command::new("yt-dlp")
            .arg("-4")
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
