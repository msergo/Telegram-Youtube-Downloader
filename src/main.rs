use axum::{Json, Router, routing::get, routing::post};
use tokio::process::Command;
use uuid::Uuid;

use crate::types::TelegramWebhook;

mod types;

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(|| async { "OK" }))
        .route("/webhook", post(download_handler));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn download_handler(Json(payload): Json<TelegramWebhook>) -> Json<serde_json::Value> {
    let Some(url) = payload.message.text else {
        return Json(serde_json::json!({
            "status": "error",
            "message": "No URL provided"
        }));
    };

    let job_id = Uuid::new_v4();

    let output_file = format!("/downloads/{}.%(ext)s", job_id);

    tokio::spawn(async move {
        let status = Command::new("yt-dlp")
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
                println!("Download complete for job {}", job_id);
                // Later: send message to Telegram
            }
            Ok(s) => {
                println!("yt-dlp exited with status: {:?}", s);
            }
            Err(e) => {
                println!("Failed to spawn yt-dlp for job {}: {}", job_id, e);
            }
        }
    });
    Json(serde_json::json!({
        "job_id": job_id.to_string()
    }))
}
