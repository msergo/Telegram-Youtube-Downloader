use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

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

/// Find the next MP3 frame sync marker (0xFFF) after a given position
/// Returns the byte offset of the frame header, or None if not found
/// Searches backwards from target position to find a safe split point
async fn find_next_frame_sync(
    file: &mut fs::File,
    target_pos: u64,
    max_search: u64,
) -> Result<Option<u64>, ChunkError> {
    // Search backwards from target to find the last valid frame before target
    let search_start = target_pos.saturating_sub(max_search);
    let mut buffer = vec![0u8; 4096];
    let mut last_frame_pos: Option<u64> = None;
    let mut current_pos = search_start;

    file.seek(std::io::SeekFrom::Start(search_start)).await?;

    while current_pos < target_pos {
        let remaining = (target_pos - current_pos).min(4096);
        let n = file.read(&mut buffer[..remaining as usize]).await?;
        if n == 0 {
            break;
        }

        // Look for MP3 frame sync marker (0xFFF with specific bits set)
        // Search through the buffer, allowing for boundary overlap
        for i in 0..n.saturating_sub(1) {
            // Check for valid MP3 frame header: 0xFF followed by byte with bits 7,6,5 set
            if buffer[i] == 0xFF && (buffer[i + 1] & 0xE0) == 0xE0 {
                // Additional validation: ensure this looks like a reasonable frame
                // Check that we have enough data for basic frame validation
                if i + 3 < n {
                    // MPEG version (bits 4-3 of second byte should not be 01 which is reserved)
                    let version_bits = (buffer[i + 1] >> 3) & 0x03;
                    if version_bits != 0x01 {
                        // Not reserved
                        last_frame_pos = Some(current_pos + i as u64);
                    }
                } else {
                    // If we can't validate further, still accept it but prefer validated ones
                    last_frame_pos = Some(current_pos + i as u64);
                }
            }
        }

        // Move back 1 byte to catch patterns spanning buffer boundaries
        current_pos += n.saturating_sub(1) as u64;
        if n > 1 {
            file.seek(std::io::SeekFrom::Start(current_pos)).await?;
        } else {
            current_pos += 1;
        }
    }

    Ok(last_frame_pos)
}

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
        // Calculate target end position for this chunk
        let target_end = std::cmp::min(bytes_read + CHUNK_SIZE, total_size);

        // If this is the last chunk, just use the target end (no need for frame boundary)
        let is_last_chunk = target_end == total_size;

        // Find the last valid MP3 frame before the target end position
        // Only search for frame boundaries if it's not the last chunk
        // This prevents creating tiny final chunks
        let actual_end = if is_last_chunk {
            target_end
        } else {
            match find_next_frame_sync(&mut input_file, target_end, 10 * 1024).await? {
                Some(frame_pos)
                    if frame_pos > bytes_read
                        && (target_end - frame_pos) < 2 * 1024 * 1024
                        && (frame_pos - bytes_read) > 1024 * 1024
                        && (target_end - frame_pos) >= 4 =>
                {
                    // Only use frame_pos if:
                    // 1. It doesn't move the split back more than 2MB
                    // 2. The resulting chunk would be at least 1MB
                    // 3. It doesn't cut off less than 4 bytes (avoids tiny adjustments)
                    frame_pos
                }
                _ => target_end,
            }
        };

        let chunk_size = actual_end - bytes_read;

        if chunk_size == 0 {
            // Avoid creating empty chunks
            break;
        }

        let chunk_path = parent_dir.join(format!("{}_{}.{}", chunk_index, file_stem, extension));
        let mut output_file = fs::File::create(&chunk_path).await?;

        // Read and write the chunk
        input_file
            .seek(std::io::SeekFrom::Start(bytes_read))
            .await?;
        let mut buffer = vec![0u8; chunk_size as usize];
        input_file.read_exact(&mut buffer).await?;
        output_file.write_all(&buffer).await?;
        output_file.sync_all().await?;

        chunks.push(ChunkInfo {
            path: chunk_path,
            index: chunk_index,
            size: chunk_size,
        });

        bytes_read = actual_end;
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
