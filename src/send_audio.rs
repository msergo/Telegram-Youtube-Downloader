use crate::chunk_audio::{cleanup_chunks, needs_chunking, split_mp3};
use log::{error, info};
use reqwest::{Client, multipart};
use std::path::Path;
use tokio::fs;
use tokio_util::codec::{BytesCodec, FramedRead};

async fn send_single_chunk(
    chat_id: i64,
    path: &str,
    performer: &str,
    title: &str,
    bot_token: &str,
    should_delete: bool,
) {
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
        .text("performer", performer.to_string())
        .text("title", title.to_string())
        .part(
            "audio",
            multipart::Part::stream(file_body).file_name(file_name),
        );

    match client.post(&url).multipart(form).send().await {
        Ok(res) if res.status().is_success() => {
            info!("Audio sent successfully to Telegram.");
            // Only delete if this is a non-chunked file
            // Chunks are deleted by cleanup_chunks() function
            if should_delete {
                let _ = tokio::fs::remove_file(&path).await;
                info!("Deleted file: {}", path);
            }
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

pub async fn send_audio_to_telegram(
    chat_id: i64,
    path: &str,
    performer: &str,
    title: &str,
    bot_token: &str,
) {
    // Check file size and handle chunking transparently
    match fs::metadata(path).await {
        Ok(metadata) => {
            let file_size = metadata.len();

            if needs_chunking(file_size) {
                let file_name = Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("audio.mp3");

                info!(
                    "File {} is {}MB, splitting into chunks",
                    file_name,
                    file_size / 1024 / 1024
                );

                match split_mp3(path).await {
                    Ok(chunks) => {
                        let total_chunks = chunks.len();
                        // Send each chunk
                        for chunk in &chunks {
                            let chunk_filename = chunk
                                .path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(file_name);

                            info!(
                                "Sending chunk: {} ({}MB)",
                                chunk_filename,
                                chunk.size / 1024 / 1024
                            );

                            // Add chunk info to title: "Song Title (Part 1/3)"
                            let chunk_title =
                                format!("{} (Part {}/{})", title, chunk.index, total_chunks);

                            send_single_chunk(
                                chat_id,
                                chunk.path.to_str().unwrap(),
                                performer,
                                &chunk_title,
                                bot_token,
                                false, // Don't delete chunks here, cleanup_chunks() will handle it
                            )
                            .await;
                        }

                        // Clean up chunks
                        if let Err(e) = cleanup_chunks(chunks).await {
                            error!("Failed to clean up chunks: {}", e);
                        }

                        // Remove original file
                        let _ = fs::remove_file(path).await;
                    }
                    Err(e) => {
                        error!("Failed to split file {}: {}", file_name, e);
                        // Fallback: try to send original file as-is
                        send_single_chunk(chat_id, path, performer, title, bot_token, true).await;
                    }
                }
            } else {
                // File is under 50MB, send as-is
                send_single_chunk(chat_id, path, performer, title, bot_token, true).await;
            }
        }
        Err(e) => {
            error!("Failed to get file metadata for {}: {}", path, e);
        }
    }
}
