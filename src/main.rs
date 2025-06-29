use axum::{Json, Router, routing::get, routing::post};
use serde::Deserialize;
use tokio::process::Command;
use uuid::Uuid;

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(|| async { "Hello, World!" }))
        .route("/download", post(download_handler));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[derive(Deserialize)]
struct DownloadRequest {
    url: String,
}
async fn download_handler(Json(payload): Json<DownloadRequest>) -> Json<serde_json::Value> {
    let job_id = Uuid::new_v4();
    let url = payload.url.clone();

    tokio::spawn(async move {
        let status = Command::new("yt-dlp")
            .arg("-x") // extract audio
            .arg("--audio-format")
            .arg("mp3") // convert to mp3
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
