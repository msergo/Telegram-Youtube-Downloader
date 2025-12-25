use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Metadata about a single chunk
#[derive(Debug, Clone)]
pub struct ChunkInfo {
    pub path: PathBuf,
    #[allow(dead_code)]
    pub index: u32,
    pub size: u64,
}

/// Custom error type for chunking operations
#[derive(Debug)]
pub enum ChunkError {
    Io(std::io::Error),
    Message(String),
}

impl std::fmt::Display for ChunkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChunkError::Io(e) => write!(f, "IO error: {}", e),
            ChunkError::Message(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for ChunkError {}

impl From<std::io::Error> for ChunkError {
    fn from(err: std::io::Error) -> Self {
        ChunkError::Io(err)
    }
}

impl From<String> for ChunkError {
    fn from(msg: String) -> Self {
        ChunkError::Message(msg)
    }
}

/// The target size for each chunk (49MB to stay under Telegram's 50MB limit)
const CHUNK_SIZE: u64 = 49 * 1024 * 1024; // 49MB

/// Check if a file needs to be split
pub fn needs_chunking(file_size: u64) -> bool {
    file_size > 50 * 1024 * 1024 // > 50MB
}

/// Split an MP3 file into chunks
///
/// # Arguments
/// * `file_path` - Path to the MP3 file to split
///
/// # Returns
/// A vector of ChunkInfo structs representing the created chunks
///
/// # Errors
/// Returns an error if file I/O fails
pub async fn split_mp3(file_path: &str) -> Result<Vec<ChunkInfo>, ChunkError> {
    let path = Path::new(file_path);

    // Verify file exists
    if !path.exists() {
        return Err(ChunkError::Message(format!(
            "File not found: {}",
            file_path
        )));
    }

    let metadata = fs::metadata(file_path).await?;
    let total_size = metadata.len();

    if !needs_chunking(total_size) {
        return Ok(vec![]); // No chunking needed
    }

    let mut input_file = fs::File::open(file_path).await?;
    let mut chunks = Vec::new();
    let mut chunk_index = 1u32;
    let mut bytes_read = 0u64;

    let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("audio");
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("mp3");
    let parent_dir = path.parent().unwrap_or_else(|| Path::new("."));

    while bytes_read < total_size {
        let chunk_path = parent_dir.join(format!("{}_{}.{}", chunk_index, file_stem, extension));

        let mut output_file = fs::File::create(&chunk_path).await?;
        let mut buffer = vec![0u8; CHUNK_SIZE as usize];
        let bytes_to_read = std::cmp::min(CHUNK_SIZE, total_size - bytes_read);

        let n = input_file
            .read_exact(&mut buffer[..bytes_to_read as usize])
            .await
            .or_else(|e| {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    Ok(bytes_to_read as usize)
                } else {
                    Err(e)
                }
            })?;

        output_file.write_all(&buffer[..n]).await?;
        output_file.sync_all().await?;

        let chunk_size = n as u64;
        chunks.push(ChunkInfo {
            path: chunk_path,
            index: chunk_index,
            size: chunk_size,
        });

        bytes_read += chunk_size;
        chunk_index += 1;
    }

    Ok(chunks)
}

/// Clean up chunk files
pub async fn cleanup_chunks(chunks: Vec<ChunkInfo>) -> Result<(), ChunkError> {
    for chunk in chunks {
        if let Err(e) = fs::remove_file(&chunk.path).await {
            eprintln!("Failed to remove chunk {}: {}", chunk.path.display(), e);
        }
    }
    Ok(())
}

/// Extract the original filename from a chunked filename
/// Example: "1_artist - title.mp3" -> "artist - title.mp3"
#[allow(dead_code)]
pub fn extract_original_filename(chunked_name: &str) -> String {
    if let Some(pos) = chunked_name.find('_') {
        chunked_name[pos + 1..].to_string()
    } else {
        chunked_name.to_string()
    }
}
