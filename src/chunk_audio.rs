use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

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

/// Inclusive byte limit for an original sent without processing.
pub const TELEGRAM_MAX_AUDIO_BYTES: u64 = 50_000_000;

/// Inclusive byte limit for generated chunks, reserving upload headroom.
pub const GENERATED_CHUNK_MAX_BYTES: u64 = 49_000_000;

#[derive(Clone, Copy)]
pub(crate) struct ChunkPolicy {
    pub original_max_bytes: u64,
    pub generated_max_bytes: u64,
    pub output_bitrate: u64,
}

pub(crate) const PRODUCTION_POLICY: ChunkPolicy = ChunkPolicy {
    original_max_bytes: TELEGRAM_MAX_AUDIO_BYTES,
    generated_max_bytes: GENERATED_CHUNK_MAX_BYTES,
    output_bitrate: 192_000,
};

pub(crate) struct MediaTools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Default for MediaTools {
    fn default() -> Self {
        Self {
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
        }
    }
}

struct AudioMetadata {
    duration_seconds: f64,
    sample_rate: u32,
    channels: u32,
}

#[derive(Clone, Copy)]
struct MediaDeadlines {
    probe: Duration,
    processing: Duration,
}

fn error(message: impl Into<String>) -> ChunkError {
    ChunkError::Message(message.into())
}

fn initial_chunk_seconds(policy: ChunkPolicy) -> Result<u64, ChunkError> {
    if policy.generated_max_bytes > policy.original_max_bytes {
        return Err(error("Invalid chunk size policy"));
    }
    let reserve = 65_536_u64.min(policy.generated_max_bytes / 100);
    policy
        .generated_max_bytes
        .checked_sub(reserve)
        .and_then(|bytes| bytes.checked_mul(8))
        .and_then(|bits| bits.checked_div(policy.output_bitrate))
        .filter(|seconds| *seconds >= 1)
        .ok_or_else(|| error("Invalid chunk duration policy"))
}

/// Check whether the original exceeds the inclusive upload limit.
#[allow(dead_code)] // Public library helper; the binary uses an injected size policy.
pub fn needs_chunking(file_size: u64) -> bool {
    file_size > TELEGRAM_MAX_AUDIO_BYTES
}

/// Encode the source once into duration-based chunks before returning any of them.
/// Originals within the upload limit return an empty vector without media processing.
/// On failure the source is preserved and operation-owned outputs are removed.
#[allow(dead_code)] // Public library entry point; the binary uses the options entry point.
pub async fn split_mp3(file_path: &str) -> Result<Vec<ChunkInfo>, ChunkError> {
    split_mp3_with_options(
        Path::new(file_path),
        PRODUCTION_POLICY,
        &MediaTools::default(),
    )
    .await
}

pub(crate) async fn split_mp3_with_options(
    source: &Path,
    policy: ChunkPolicy,
    tools: &MediaTools,
) -> Result<Vec<ChunkInfo>, ChunkError> {
    split_mp3_with_deadlines(source, policy, tools, None).await
}

async fn split_mp3_with_deadlines(
    source: &Path,
    policy: ChunkPolicy,
    tools: &MediaTools,
    deadlines: Option<MediaDeadlines>,
) -> Result<Vec<ChunkInfo>, ChunkError> {
    if fs::metadata(source).await?.len() <= policy.original_max_bytes {
        return Ok(Vec::new());
    }
    let span = initial_chunk_seconds(policy)?;
    let probe_deadline = deadlines.map_or(Duration::from_secs(30), |d| d.probe);
    let metadata = probe_audio(source, tools, probe_deadline).await?;
    let processing_deadline = match deadlines {
        Some(d) => d.processing,
        None => {
            Duration::try_from_secs_f64((2.0 * metadata.duration_seconds).ceil().max(780.0) + 120.0)
                .map_err(|_| error("Audio duration exceeds processing deadline range"))?
        }
    };
    log::info!(
        "Splitting {}: {} seconds, {} Hz, {} channels",
        source.display(),
        metadata.duration_seconds,
        metadata.sample_rate,
        metadata.channels
    );
    let parent = source
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let workspace = tempfile::Builder::new()
        .prefix(".yt-dl-chunks-")
        .tempdir_in(parent)?;
    let stem = source
        .file_stem()
        .unwrap_or_else(|| std::ffi::OsStr::new("audio"));
    run_media_command(
        build_segment_command(
            &std::path::absolute(source)?,
            workspace.path(),
            span,
            policy,
            tools,
        ),
        processing_deadline,
    )
    .await?;

    let mut outputs = Vec::new();
    let mut entries = fs::read_dir(workspace.path()).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let index = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|index| *index > 0)
            .ok_or_else(|| error("Invalid segment filename"))?;
        if path.extension().and_then(|s| s.to_str()) != Some("mp3") {
            return Err(error("Unexpected segment output"));
        }
        outputs.push((index, path));
    }
    outputs.sort_unstable_by_key(|(index, _)| *index);
    if outputs.is_empty() {
        return Err(error("FFmpeg produced no chunks"));
    }
    let mut chunks = Vec::with_capacity(outputs.len());
    for (position, (index, path)) in outputs.into_iter().enumerate() {
        if usize::try_from(index).ok() != position.checked_add(1) {
            return Err(error("Non-contiguous segment indexes"));
        }
        let size = fs::metadata(&path).await?.len();
        if size == 0 || size > policy.generated_max_bytes {
            return Err(error(format!(
                "Chunk {index} has invalid size: {size} bytes"
            )));
        }
        probe_audio(&path, tools, probe_deadline).await?;
        let mut name = std::ffi::OsString::from(format!("{index}_"));
        name.push(stem);
        name.push(".mp3");
        let destination = workspace.path().join(name);
        fs::rename(&path, &destination).await?;
        log::info!("Chunk {index} ready: {size} bytes");
        chunks.push(ChunkInfo {
            path: destination,
            index,
            size,
        });
    }
    // Publish the whole sequence only after all outputs pass the checks.
    let _ = workspace.keep();
    Ok(chunks)
}

#[derive(Deserialize)]
struct ProbeResponse {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeStream {
    codec_name: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u32>,
    duration: Option<String>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

fn positive_duration(value: Option<&str>) -> Option<f64> {
    value?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
}

async fn probe_audio(
    path: &Path,
    tools: &MediaTools,
    deadline: Duration,
) -> Result<AudioMetadata, ChunkError> {
    let mut command = Command::new(&tools.ffprobe);
    command.args([
        "-v",
        "error",
        "-select_streams",
        "a:0",
        "-show_entries",
        "stream=codec_name,sample_rate,channels,duration,bit_rate:format=duration",
        "-of",
        "json",
    ]);
    command.arg(path).stdout(Stdio::piped());
    let output = run_media_command(command, deadline).await?;
    let response: ProbeResponse = serde_json::from_slice(&output.stdout)
        .map_err(|e| error(format!("Invalid ffprobe JSON: {e}")))?;
    let stream = response
        .streams
        .first()
        .ok_or_else(|| error("No audio stream"))?;
    if stream.codec_name.as_deref() != Some("mp3") {
        return Err(error("Expected an MP3 audio stream"));
    }
    let duration_seconds = positive_duration(stream.duration.as_deref())
        .or_else(|| positive_duration(response.format.as_ref().and_then(|f| f.duration.as_deref())))
        .ok_or_else(|| error("No finite positive audio duration"))?;
    let sample_rate = stream
        .sample_rate
        .as_deref()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|v| *v > 0)
        .ok_or_else(|| error("Invalid audio sample rate"))?;
    let channels = stream
        .channels
        .filter(|v| *v > 0)
        .ok_or_else(|| error("Invalid audio channel count"))?;
    Ok(AudioMetadata {
        duration_seconds,
        sample_rate,
        channels,
    })
}

fn build_segment_command(
    source: &Path,
    workspace: &Path,
    seconds: u64,
    policy: ChunkPolicy,
    tools: &MediaTools,
) -> Command {
    let mut command = Command::new(&tools.ffmpeg);
    command.args(["-nostdin", "-hide_banner", "-v", "error", "-xerror", "-n"]);
    command.arg("-i").arg(source);
    command.args(["-map", "0:a:0", "-c:a", "libmp3lame", "-b:a"]);
    command.arg(policy.output_bitrate.to_string());
    // Prevent a chunk's MP3 frames from referring to data in the previous chunk.
    command.args([
        "-reservoir",
        "0",
        "-map_metadata",
        "-1",
        "-f",
        "segment",
        "-segment_format",
        "mp3",
        "-segment_time",
    ]);
    command.arg(seconds.to_string());
    command.args([
        "-segment_start_number",
        "1",
        "-reset_timestamps",
        "1",
        "%d.mp3",
    ]);
    // Keep the pattern literal, even when the source directory/stem contains '%'.
    command.current_dir(workspace).stdout(Stdio::null());
    command
}

// Drain both pipes completely while retaining only bounded diagnostics/JSON.
async fn drain_pipe<R: AsyncRead + Unpin>(
    pipe: Option<R>,
    limit: usize,
) -> std::io::Result<Vec<u8>> {
    let mut retained = Vec::new();
    if let Some(mut pipe) = pipe {
        let mut buffer = [0_u8; 8192];
        loop {
            let count = pipe.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            let keep = count.min(limit.saturating_sub(retained.len()));
            retained.extend_from_slice(&buffer[..keep]);
        }
    }
    Ok(retained)
}

async fn run_media_command(mut command: Command, deadline: Duration) -> Result<Output, ChunkError> {
    command
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let result = tokio::time::timeout(deadline, async {
        tokio::try_join!(
            child.wait(),
            drain_pipe(stdout, 1_048_576),
            drain_pipe(stderr, 16_384)
        )
    })
    .await;
    let (status, stdout, stderr) = match result {
        Ok(Ok(output)) => output,
        failure => {
            // Explicitly request termination and reap before the workspace is removed.
            let kill_result = child.kill().await;
            let wait_result = child.wait().await;
            if let Err(e) = kill_result {
                log::warn!("Failed to kill media child: {e}");
            }
            wait_result?;
            return Err(match failure {
                Err(_) => error("Media process timed out"),
                Ok(Err(e)) => e.into(),
                Ok(Ok(_)) => unreachable!(),
            });
        }
    };
    if !status.success() {
        return Err(error(format!(
            "Media process exited with {status}: {}",
            String::from_utf8_lossy(&stderr)
        )));
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Owns published chunk files across asynchronous work, with best-effort drop cleanup.
pub(crate) struct ChunkCleanupGuard {
    chunks: Vec<ChunkInfo>,
    armed: bool,
}

impl ChunkCleanupGuard {
    pub(crate) fn new(chunks: Vec<ChunkInfo>) -> Self {
        Self {
            chunks,
            armed: true,
        }
    }

    #[allow(dead_code)] // Used by the binary sender, absent from the library target.
    pub(crate) fn as_slice(&self) -> &[ChunkInfo] {
        &self.chunks
    }

    fn workspace_dirs(&self) -> HashSet<PathBuf> {
        self.chunks
            .iter()
            .filter_map(|chunk| chunk.path.parent())
            .filter(|parent| {
                parent
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(".yt-dl-chunks-"))
            })
            .map(Path::to_path_buf)
            .collect()
    }

    pub(crate) async fn cleanup(&mut self) -> Result<(), ChunkError> {
        let mut failure = None;
        for chunk in &self.chunks {
            remember_cleanup_error(
                fs::remove_file(&chunk.path).await,
                &chunk.path,
                &mut failure,
            );
        }
        for parent in self.workspace_dirs() {
            remember_cleanup_error(fs::remove_dir(&parent).await, &parent, &mut failure);
        }
        if let Some(failure) = failure {
            return Err(failure.into());
        }
        self.armed = false;
        Ok(())
    }
}

fn remember_cleanup_error(
    result: std::io::Result<()>,
    path: &Path,
    failure: &mut Option<std::io::Error>,
) {
    if let Err(e) = result
        && e.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("Failed to clean up {}: {e}", path.display());
        if failure.is_none() {
            *failure = Some(e);
        }
    }
}

impl Drop for ChunkCleanupGuard {
    fn drop(&mut self) {
        if self.armed {
            let mut failure = None;
            for chunk in &self.chunks {
                remember_cleanup_error(
                    std::fs::remove_file(&chunk.path),
                    &chunk.path,
                    &mut failure,
                );
            }
            for parent in self.workspace_dirs() {
                remember_cleanup_error(std::fs::remove_dir(&parent), &parent, &mut failure);
            }
        }
    }
}

/// Remove known chunks and their empty reserved workspaces, preserving all other files.
#[allow(dead_code)] // Public library helper; the binary owns a cleanup guard directly.
pub async fn cleanup_chunks(chunks: Vec<ChunkInfo>) -> Result<(), ChunkError> {
    ChunkCleanupGuard::new(chunks).cleanup().await
}

#[cfg(test)]
mod tests;

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
