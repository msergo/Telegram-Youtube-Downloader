use super::*;
use std::process::Stdio;
use tempfile::TempDir;

const TEST_POLICY: ChunkPolicy = ChunkPolicy {
    original_max_bytes: 65_000,
    generated_max_bytes: 64_000,
    output_bitrate: 192_000,
};

#[test]
fn duration_policy_is_checked() {
    assert_eq!(initial_chunk_seconds(PRODUCTION_POLICY).unwrap(), 2038);
    assert_eq!(initial_chunk_seconds(TEST_POLICY).unwrap(), 2);
    for policy in [
        ChunkPolicy {
            output_bitrate: 0,
            ..TEST_POLICY
        },
        ChunkPolicy {
            generated_max_bytes: 65_001,
            ..TEST_POLICY
        },
        ChunkPolicy {
            generated_max_bytes: 1,
            ..TEST_POLICY
        },
        ChunkPolicy {
            original_max_bytes: u64::MAX,
            generated_max_bytes: u64::MAX,
            ..TEST_POLICY
        },
    ] {
        assert!(initial_chunk_seconds(policy).is_err());
    }
}

#[tokio::test]
async fn small_file_bypasses_missing_tools() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("small.mp3");
    let original = b"small input that is deliberately not valid MP3";
    fs::write(&source, original).await.unwrap();
    let tools = MediaTools {
        ffmpeg: dir.path().join("missing-ffmpeg"),
        ffprobe: dir.path().join("missing-ffprobe"),
    };
    let chunks = split_mp3_with_options(&source, PRODUCTION_POLICY, &tools)
        .await
        .unwrap();
    assert!(chunks.is_empty());
    assert_eq!(fs::read(&source).await.unwrap(), original);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[cfg(unix)]
mod fake_tools {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn executable(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn tools(dir: &Path, probe: &str, encoder: &str) -> MediaTools {
        MediaTools {
            ffprobe: executable(dir, "ffprobe", probe),
            ffmpeg: executable(dir, "ffmpeg", encoder),
        }
    }

    const VALID_PROBE: &str = r#"
for last do :; done
case "$last" in
  */.yt-dl-chunks-*/*) duration=$(head -n 1 "$last");;
  *) duration=10;;
esac
printf '{"streams":[{"codec_name":"mp3","sample_rate":"44100","channels":1,"duration":"%s"}]}' "$duration"
"#;

    async fn source(dir: &Path) -> PathBuf {
        let source = dir.join("source.mp3");
        fs::write(&source, vec![b'x'; 65_001]).await.unwrap();
        source
    }

    fn assert_preserved(dir: &Path, source: &Path) {
        assert_eq!(std::fs::read(source).unwrap(), vec![b'x'; 65_001]);
        for entry in std::fs::read_dir(dir).unwrap() {
            assert!(
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".yt-dl-chunks-")
            );
        }
    }

    async fn assert_recorded_child_reaped(dir: &Path) {
        let pid = std::fs::read_to_string(dir.join("pid")).unwrap();
        let pid = pid.trim().parse::<u32>().unwrap();
        let status = tokio::process::Command::new("/bin/sh")
            .args(["-c", "kill -0 \"$1\"", "check-child", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .unwrap();
        assert!(!status.success(), "timed-out child {pid} still exists");
    }

    #[tokio::test]
    async fn malformed_probe_and_invalid_metadata_preserve_source() {
        for response in [
            "not json",
            "{}",
            r#"{"streams":[]}"#,
            r#"{"streams":[{"codec_name":"aac","sample_rate":"44100","channels":1,"duration":"10"}]}"#,
            r#"{"streams":[{"codec_name":"mp3","sample_rate":"44100","channels":1,"duration":"NaN"}]}"#,
            r#"{"streams":[{"codec_name":"mp3","sample_rate":"0","channels":1,"duration":"10"}]}"#,
        ] {
            let dir = TempDir::new().unwrap();
            let source = source(dir.path()).await;
            let tools = tools(dir.path(), &format!("printf '%s' '{response}'"), "exit 99");
            assert!(
                split_mp3_with_options(&source, TEST_POLICY, &tools)
                    .await
                    .is_err()
            );
            assert_preserved(dir.path(), &source);
        }
    }

    #[tokio::test]
    async fn format_duration_fallback_is_used() {
        let dir = TempDir::new().unwrap();
        let source = source(dir.path()).await;
        let tools = tools(
            dir.path(),
            r#"printf '%s' '{"streams":[{"codec_name":"mp3","sample_rate":"48000","channels":2,"duration":"N/A"}],"format":{"duration":"5.25"}}'"#,
            "exit 99",
        );
        let metadata = probe_audio(&source, &tools, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(metadata.duration_seconds, 5.25);
        assert_eq!(metadata.sample_rate, 48_000);
        assert_eq!(metadata.channels, 2);
    }

    #[tokio::test]
    async fn encoder_failure_after_partial_write_removes_workspace() {
        let dir = TempDir::new().unwrap();
        let source = source(dir.path()).await;
        let tools = tools(
            dir.path(),
            VALID_PROBE,
            r#"for last do :; done
out=$(dirname "$last")
printf partial > "$out/1.mp3"
printf 'deliberate encoder failure' >&2
exit 7"#,
        );
        let error = split_mp3_with_options(&source, TEST_POLICY, &tools)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("deliberate encoder failure"));
        assert_preserved(dir.path(), &source);
    }

    #[tokio::test]
    async fn timeout_kills_child_before_removing_partial_output() {
        let dir = TempDir::new().unwrap();
        let source = source(dir.path()).await;
        let tools = tools(
            dir.path(),
            VALID_PROBE,
            r#"root=$(dirname "$0")
printf '%s\n' "$$" > "$root/pid"
for last do :; done
out=$(dirname "$last")
printf partial > "$out/1.mp3"
# Use the shell itself so there are no descendant processes holding pipes open.
while :; do :; done"#,
        );
        let deadlines = MediaDeadlines {
            probe: Duration::from_secs(2),
            processing: Duration::from_millis(500),
        };
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            split_mp3_with_deadlines(&source, TEST_POLICY, &tools, Some(deadlines)),
        )
        .await
        .unwrap();
        assert!(result.unwrap_err().to_string().contains("timed out"));
        assert_recorded_child_reaped(dir.path()).await;
        assert_preserved(dir.path(), &source);
    }

    #[tokio::test]
    async fn source_probe_timeout_reaps_child_without_creating_workspace() {
        let dir = TempDir::new().unwrap();
        let source = source(dir.path()).await;
        let tools = tools(
            dir.path(),
            r#"root=$(dirname "$0")
printf '%s\n' "$$" > "$root/pid"
while :; do :; done"#,
            "exit 99",
        );
        let deadlines = MediaDeadlines {
            probe: Duration::from_millis(500),
            processing: Duration::from_secs(2),
        };
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            split_mp3_with_deadlines(&source, TEST_POLICY, &tools, Some(deadlines)),
        )
        .await
        .unwrap();
        assert!(result.unwrap_err().to_string().contains("timed out"));
        assert_recorded_child_reaped(dir.path()).await;
        assert_preserved(dir.path(), &source);
    }

    #[tokio::test]
    async fn runner_drains_both_pipes_without_unbounded_capture() {
        let dir = TempDir::new().unwrap();
        let script = executable(
            dir.path(),
            "noisy",
            r#"
head -c 2000000 /dev/zero
head -c 2000000 /dev/zero >&2
"#,
        );
        let mut command = tokio::process::Command::new(script);
        command.stdout(Stdio::piped());
        let output = run_media_command(command, Duration::from_secs(5))
            .await
            .unwrap();
        assert!(output.stdout.len() < 2_000_000);
        assert!(output.stderr.len() < 2_000_000);
    }
}

#[tokio::test]
async fn cleanup_retains_unrelated_files_and_drop_retries_failure() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().join(".yt-dl-chunks-owned");
    std::fs::create_dir(&workspace).unwrap();
    let chunk = ChunkInfo {
        path: workspace.join("1_audio.mp3"),
        index: 1,
        size: 1,
    };
    let unrelated = workspace.join("unrelated");
    std::fs::write(&chunk.path, b"x").unwrap();
    std::fs::write(&unrelated, b"keep").unwrap();
    let mut guard = ChunkCleanupGuard::new(vec![chunk.clone()]);
    assert!(guard.cleanup().await.is_err());
    assert!(!chunk.path.exists());
    assert_eq!(std::fs::read(&unrelated).unwrap(), b"keep");
    std::fs::remove_file(&unrelated).unwrap();
    drop(guard);
    assert!(!workspace.exists());
    assert!(dir.path().exists());
}

#[test]
fn drop_guard_removes_only_known_files() {
    let dir = TempDir::new().unwrap();
    let chunk = ChunkInfo {
        path: dir.path().join("1_audio.mp3"),
        index: 1,
        size: 1,
    };
    let unrelated = dir.path().join("unrelated");
    std::fs::write(&chunk.path, b"x").unwrap();
    std::fs::write(&unrelated, b"keep").unwrap();
    drop(ChunkCleanupGuard::new(vec![chunk.clone()]));
    assert!(!chunk.path.exists());
    assert_eq!(std::fs::read(unrelated).unwrap(), b"keep");
    assert!(dir.path().exists());
}

#[tokio::test]
async fn cleanup_attempts_all_files_after_error_and_tolerates_missing_files() {
    let dir = TempDir::new().unwrap();
    let blocked = dir.path().join("blocked.mp3");
    std::fs::create_dir(&blocked).unwrap();
    let removable = dir.path().join("2_audio.mp3");
    std::fs::write(&removable, b"x").unwrap();
    let chunks = vec![
        ChunkInfo {
            path: blocked.clone(),
            index: 1,
            size: 1,
        },
        ChunkInfo {
            path: removable.clone(),
            index: 2,
            size: 1,
        },
        ChunkInfo {
            path: dir.path().join("already-missing.mp3"),
            index: 3,
            size: 1,
        },
    ];
    let mut guard = ChunkCleanupGuard::new(chunks);
    assert!(guard.cleanup().await.is_err());
    assert!(!removable.exists());
    assert!(blocked.is_dir());
    std::fs::remove_dir(blocked).unwrap();
    guard.cleanup().await.unwrap();
}
