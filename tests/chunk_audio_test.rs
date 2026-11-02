use std::fs;
use tempfile::TempDir;
use tokio::fs::File;
use yt_dl_service::chunk_audio::{
    ChunkInfo, GENERATED_CHUNK_MAX_BYTES, TELEGRAM_MAX_AUDIO_BYTES, cleanup_chunks,
    extract_original_filename, needs_chunking, split_mp3,
};

// ===== Basic Unit Tests =====

#[test]
fn test_needs_chunking() {
    assert_eq!(TELEGRAM_MAX_AUDIO_BYTES, 50_000_000);
    assert_eq!(GENERATED_CHUNK_MAX_BYTES, 49_000_000);
    for size in [0, 49_000_000, 49_500_000, 50_000_000] {
        assert!(!needs_chunking(size), "{size} bytes should bypass chunking");
    }
    for size in [50_000_001, 51_000_000, 100_000_000] {
        assert!(needs_chunking(size), "{size} bytes should require chunking");
    }
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
    assert!(!needs_chunking(50_000_000)); // Exactly 50 MB
    assert!(needs_chunking(50_000_001)); // 50 MB + 1 byte
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
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("small.mp3");
    let original = b"Deliberately invalid MP3: bypass must not inspect audio.";
    fs::write(&test_file, original).unwrap();

    let chunks = split_mp3(test_file.to_str().unwrap()).await.unwrap();
    assert!(chunks.is_empty());
    assert_eq!(fs::read(&test_file).unwrap(), original);
    assert_eq!(fs::read_dir(temp_dir.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn test_split_mp3_bypass_boundaries() {
    for size in [0, 49_000_000, 49_500_000, 50_000_000] {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("boundary.mp3");
        let file = File::create(&test_file).await.unwrap();
        file.set_len(size).await.unwrap();
        drop(file);

        let chunks = split_mp3(test_file.to_str().unwrap()).await.unwrap();

        assert!(chunks.is_empty(), "{size} bytes should bypass chunking");
        let contents = fs::read(&test_file).unwrap();
        assert_eq!(contents.len() as u64, size);
        assert!(contents.iter().all(|byte| *byte == 0));
        assert_eq!(fs::read_dir(temp_dir.path()).unwrap().count(), 1);
    }
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
    fs::write(&chunk1.path, [0u8; 100]).unwrap();
    fs::write(&chunk2.path, [0u8; 100]).unwrap();

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
