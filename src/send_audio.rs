use crate::chunk_audio::{
    ChunkCleanupGuard, ChunkPolicy, MediaTools, PRODUCTION_POLICY, split_mp3_with_options,
};
use log::{error, info};
use reqwest::{Client, multipart};
use std::path::Path;
use tokio::fs;
use tokio_util::codec::{BytesCodec, FramedRead};

struct AudioUploadContext {
    client: Client,
    api_base_url: String,
    bot_token: String,
}

async fn send_single_chunk(
    chat_id: i64,
    path: &Path,
    performer: &str,
    title: &str,
    context: &AudioUploadContext,
    delete_on_success: bool,
) {
    let file = match fs::File::open(path).await {
        Ok(file) => file,
        Err(e) => {
            error!("Failed to open audio {}: {}", path.display(), e);
            return;
        }
    };
    let stream = FramedRead::new(file, BytesCodec::new());
    let file_body = reqwest::Body::wrap_stream(stream);
    let file_name = path
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
    let url = format!(
        "{}/bot{}/sendAudio",
        context.api_base_url.trim_end_matches('/'),
        context.bot_token
    );
    match context.client.post(&url).multipart(form).send().await {
        Ok(res) if res.status().is_success() => {
            info!("Audio sent successfully to Telegram.");
            if delete_on_success {
                let _ = fs::remove_file(path).await;
                info!("Deleted file: {}", path.display());
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
    let context = AudioUploadContext {
        client: Client::new(),
        api_base_url: "https://api.telegram.org".to_string(),
        bot_token: bot_token.to_string(),
    };
    send_audio_with_options(
        chat_id,
        Path::new(path),
        performer,
        title,
        &context,
        PRODUCTION_POLICY,
        &MediaTools::default(),
    )
    .await;
}

async fn send_audio_with_options(
    chat_id: i64,
    path: &Path,
    performer: &str,
    title: &str,
    context: &AudioUploadContext,
    policy: ChunkPolicy,
    tools: &MediaTools,
) {
    let metadata = match fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(e) => {
            error!("Failed to get file metadata for {}: {}", path.display(), e);
            return;
        }
    };
    if metadata.len() <= policy.original_max_bytes {
        send_single_chunk(chat_id, path, performer, title, context, true).await;
        return;
    }
    info!(
        "File {} is {} bytes, splitting into chunks",
        path.display(),
        metadata.len()
    );
    let chunks = match split_mp3_with_options(path, policy, tools).await {
        Ok(chunks) => chunks,
        Err(e) => {
            error!("Failed to split file {}: {}", path.display(), e);
            return;
        }
    };
    // Arm ownership before the next await, including cancellation during an upload.
    let mut owned_chunks = ChunkCleanupGuard::new(chunks);
    let total = owned_chunks.as_slice().len();
    if total == 0 {
        // The source may have become small enough between the two metadata checks.
        send_single_chunk(chat_id, path, performer, title, context, true).await;
        return;
    }
    for chunk in owned_chunks.as_slice() {
        info!(
            "Sending chunk: {} ({} bytes)",
            chunk.path.display(),
            chunk.size
        );
        let chunk_title = format!("{} (Part {}/{})", title, chunk.index, total);
        send_single_chunk(
            chat_id,
            &chunk.path,
            performer,
            &chunk_title,
            context,
            false,
        )
        .await;
    }
    if let Err(e) = owned_chunks.cleanup().await {
        error!("Failed to clean up chunks: {}", e);
    }
    // Preserve the existing cleanup policy after all chunk upload attempts.
    let _ = fs::remove_file(path).await;
}

#[cfg(test)]
mod tests;
