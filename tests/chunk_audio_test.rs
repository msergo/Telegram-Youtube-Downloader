use std::fs;
use tempfile::TempDir;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use yt_dl_service::chunk_audio::{
    ChunkInfo, cleanup_chunks, extract_original_filename, needs_chunking, split_mp3,
};

// ===== Basic Unit Tests =====

#[test]
fn test_needs_chunking() {
    assert!(!needs_chunking(10 * 1024 * 1024)); // 10MB - no chunking
    assert!(!needs_chunking(50 * 1024 * 1024)); // 50MB - no chunking
    assert!(needs_chunking(51 * 1024 * 1024)); // 51MB - needs chunking
    assert!(needs_chunking(100 * 1024 * 1024)); // 100MB - needs chunking
}

#[test]
fn test_extract_original_filename() {
    assert_eq!(
        extract_original_filename("1_artist - title.mp3"),
        "artist - title.mp3"
    );
    assert_eq!(
        extract_original_filename("2_another song.mp3"),
        "another song.mp3"
    );
    assert_eq!(
        extract_original_filename("song-no-prefix.mp3"),
        "song-no-prefix.mp3"
    );
}

#[test]
fn test_needs_chunking_edge_cases() {
    // Test all boundaries
    assert!(!needs_chunking(0));
    assert!(!needs_chunking(1024)); // 1KB
    assert!(!needs_chunking(50 * 1024 * 1024)); // Exactly 50MB
    assert!(needs_chunking(50 * 1024 * 1024 + 1)); // 50MB + 1 byte
    assert!(needs_chunking(1024 * 1024 * 1024)); // 1GB
}

#[test]
fn test_extract_original_filename_edge_cases() {
    assert_eq!(extract_original_filename(""), "");
    assert_eq!(extract_original_filename("noprefix.mp3"), "noprefix.mp3");
    assert_eq!(extract_original_filename("10_filename.mp3"), "filename.mp3");
    assert_eq!(
        extract_original_filename("1_file with spaces.mp3"),
        "file with spaces.mp3"
    );
    assert_eq!(
        extract_original_filename("3_artist - title (remix).mp3"),
        "artist - title (remix).mp3"
    );
}

// ===== Async Integration Tests =====

#[tokio::test]
async fn test_split_mp3_small_file() {
    // Create temp directory and small test file
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("small.mp3");
    let mut file = File::create(&test_file).await.unwrap();

    // Write 10MB of data
    let data = vec![0u8; 10 * 1024 * 1024];
    file.write_all(&data).await.unwrap();
    file.sync_all().await.unwrap();
    drop(file);

    // Should return empty vec (no chunking needed)
    let chunks = split_mp3(test_file.to_str().unwrap()).await.unwrap();
    assert_eq!(chunks.len(), 0);
}

#[tokio::test]
async fn test_split_mp3_large_file() {
    // Create temp directory and large test file (110MB = needs 3 chunks)
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("large.mp3");
    let mut file = File::create(&test_file).await.unwrap();

    // Write 110MB of data
    let chunk_data = vec![0u8; 1024 * 1024]; // 1MB chunks
    for _ in 0..110 {
        file.write_all(&chunk_data).await.unwrap();
    }
    file.sync_all().await.unwrap();
    drop(file);

    // Split the file
    let chunks = split_mp3(test_file.to_str().unwrap()).await.unwrap();

    // Verify chunk count (110MB / 49MB = 3 chunks)
    assert_eq!(chunks.len(), 3);

    // Verify chunk sizes
    assert!(chunks[0].size <= 49 * 1024 * 1024);
    assert!(chunks[1].size <= 49 * 1024 * 1024);
    assert!(chunks[2].size <= 49 * 1024 * 1024);

    // Verify filenames have prefixes
    assert!(chunks[0].path.to_string_lossy().contains("1_"));
    assert!(chunks[1].path.to_string_lossy().contains("2_"));
    assert!(chunks[2].path.to_string_lossy().contains("3_"));
}

#[tokio::test]
async fn test_split_mp3_boundary_case() {
    // Test with file exactly at 51MB (just over threshold)
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("boundary.mp3");
    let mut file = File::create(&test_file).await.unwrap();

    let size = 51 * 1024 * 1024;
    let data = vec![0u8; size];
    file.write_all(&data).await.unwrap();
    file.sync_all().await.unwrap();
    drop(file);

    let chunks = split_mp3(test_file.to_str().unwrap()).await.unwrap();

    // Should create 2 chunks
    assert_eq!(chunks.len(), 2);
    assert!(chunks[0].size == 49 * 1024 * 1024);
    assert!(chunks[1].size == 2 * 1024 * 1024);
}

#[tokio::test]
async fn test_cleanup_chunks() {
    let temp_dir = TempDir::new().unwrap();
    let chunk1 = ChunkInfo {
        path: temp_dir.path().join("1_test.mp3"),
        index: 1,
        size: 49 * 1024 * 1024,
    };
    let chunk2 = ChunkInfo {
        path: temp_dir.path().join("2_test.mp3"),
        index: 2,
        size: 2 * 1024 * 1024,
    };

    // Create dummy files
    fs::write(&chunk1.path, &[0u8; 100]).unwrap();
    fs::write(&chunk2.path, &[0u8; 100]).unwrap();

    assert!(chunk1.path.exists());
    assert!(chunk2.path.exists());

    // Clean up
    cleanup_chunks(vec![chunk1.clone(), chunk2.clone()])
        .await
        .unwrap();

    assert!(!chunk1.path.exists());
    assert!(!chunk2.path.exists());
}

#[tokio::test]
async fn test_nonexistent_file() {
    let result = split_mp3("/nonexistent/file.mp3").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_exact_chunk_boundaries() {
    // Test file exactly 98MB (2x 49MB chunks)
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("exact.mp3");
    let mut file = File::create(&test_file).await.unwrap();

    let size = 98 * 1024 * 1024;
    file.write_all(&vec![0u8; size]).await.unwrap();
    file.sync_all().await.unwrap();
    drop(file);

    let chunks = split_mp3(test_file.to_str().unwrap()).await.unwrap();

    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].size, 49 * 1024 * 1024);
    assert_eq!(chunks[1].size, 49 * 1024 * 1024);
}
