use super::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TEST_POLICY: ChunkPolicy = ChunkPolicy {
    original_max_bytes: 65_000,
    generated_max_bytes: 64_000,
    output_bitrate: 192_000,
};

fn context(server: &MockServer) -> AudioUploadContext {
    AudioUploadContext {
        client: Client::new(),
        api_base_url: server.uri(),
        bot_token: "test-token".into(),
    }
}

fn missing_tools(dir: &Path) -> MediaTools {
    MediaTools {
        ffmpeg: dir.join("missing-ffmpeg"),
        ffprobe: dir.join("missing-ffprobe"),
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .position(|v| v == needle)
        .unwrap()
}

fn part_body<'a>(request: &'a Request, name: &str) -> &'a [u8] {
    let content_type = request
        .headers
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    let boundary = content_type.split("boundary=").nth(1).unwrap();
    let name = format!("name=\"{name}\"");
    let header = find_bytes(&request.body, name.as_bytes());
    let start = header + find_bytes(&request.body[header..], b"\r\n\r\n") + 4;
    let end = start
        + find_bytes(
            &request.body[start..],
            format!("\r\n--{boundary}").as_bytes(),
        );
    &request.body[start..end]
}

fn assert_no_workspace(dir: &Path) {
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

#[tokio::test]
async fn direct_original_at_limit_bypasses_tools_and_preserves_multipart_bytes() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("direct.mp3");
    let original = b"not MP3: exactly the direct limit\x00\xff";
    fs::write(&source, original).await.unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bottest-token/sendAudio"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let policy = ChunkPolicy {
        original_max_bytes: original.len() as u64,
        generated_max_bytes: original.len() as u64 - 1,
        ..TEST_POLICY
    };
    send_audio_with_options(
        42,
        &source,
        "Artist",
        "Title",
        &context(&server),
        policy,
        &missing_tools(dir.path()),
    )
    .await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(part_body(&requests[0], "chat_id"), b"42");
    assert_eq!(part_body(&requests[0], "performer"), b"Artist");
    assert_eq!(part_body(&requests[0], "title"), b"Title");
    assert_eq!(part_body(&requests[0], "audio"), original);
    assert!(String::from_utf8_lossy(&requests[0].body).contains("filename=\"direct.mp3\""));
    assert!(!source.exists());
    assert_no_workspace(dir.path());
}

#[tokio::test]
async fn direct_upload_failure_retains_original_and_success_honors_delete_option() {
    for (status, delete_on_success, retained) in
        [(500, true, true), (200, false, true), (200, true, false)]
    {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("direct.mp3");
        fs::write(&source, b"audio").await.unwrap();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status))
            .expect(1)
            .mount(&server)
            .await;
        send_single_chunk(1, &source, "a", "t", &context(&server), delete_on_success).await;
        assert_eq!(source.exists(), retained);
        if retained {
            assert_eq!(fs::read(&source).await.unwrap(), b"audio");
        }
    }
}

#[tokio::test]
async fn splitter_failure_never_falls_back_to_uploading_original() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("large.mp3");
    let original = vec![b'x'; 65_001];
    fs::write(&source, &original).await.unwrap();
    let server = MockServer::start().await;
    send_audio_with_options(
        1,
        &source,
        "a",
        "t",
        &context(&server),
        TEST_POLICY,
        &missing_tools(dir.path()),
    )
    .await;
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(fs::read(&source).await.unwrap(), original);
    assert_no_workspace(dir.path());
}

#[cfg(unix)]
async fn fake_source(dir: &Path) -> (PathBuf, MediaTools) {
    use std::os::unix::fs::PermissionsExt;
    let source = dir.join("source.mp3");
    fs::write(&source, vec![b'x'; 65_001]).await.unwrap();
    let probe = dir.join("fake-probe");
    std::fs::write(&probe, r#"#!/bin/sh
set -eu
for last do :; done
case "$last" in
  */source.mp3) duration=10.25;;
  *) duration=$(head -n 1 "$last");;
esac
printf '{"streams":[{"codec_name":"mp3","sample_rate":"44100","channels":1,"duration":"%s"}]}' "$duration"
"#).unwrap();
    let encoder = dir.join("fake-encoder");
    std::fs::write(
        &encoder,
        r#"#!/bin/sh
set -eu
for last do :; done
out=$(dirname "$last")
printf '2\n' > "$out/1.mp3"
printf '2.25\n' > "$out/2.mp3"
"#,
    )
    .unwrap();
    for script in [&probe, &encoder] {
        std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    (
        source,
        MediaTools {
            ffmpeg: encoder,
            ffprobe: probe,
        },
    )
}

#[cfg(unix)]
#[tokio::test]
async fn split_uploads_are_sequential_numbered_and_cleaned_after_attempts() {
    for status in [200, 500] {
        let dir = TempDir::new().unwrap();
        let (source, tools) = fake_source(dir.path()).await;
        let server = MockServer::start().await;
        let arrivals = Arc::new(Mutex::new(Vec::new()));
        let observed = arrivals.clone();
        Mock::given(method("POST"))
            .and(path("/bottest-token/sendAudio"))
            .respond_with(move |_: &Request| {
                observed.lock().unwrap().push(Instant::now());
                ResponseTemplate::new(status).set_delay(Duration::from_millis(150))
            })
            .mount(&server)
            .await;
        send_audio_with_options(
            42,
            &source,
            "Artist",
            "Title",
            &context(&server),
            TEST_POLICY,
            &tools,
        )
        .await;
        let requests = server.received_requests().await.unwrap();
        assert!(requests.len() > 1);
        for (position, request) in requests.iter().enumerate() {
            assert_eq!(
                part_body(request, "title"),
                format!("Title (Part {}/{})", position + 1, requests.len()).as_bytes()
            );
            assert_eq!(part_body(request, "performer"), b"Artist");
            assert_eq!(part_body(request, "chat_id"), b"42");
            assert!(
                String::from_utf8_lossy(&request.body)
                    .contains(&format!("filename=\"{}_source.mp3\"", position + 1))
            );
            let audio = part_body(request, "audio");
            assert!(!audio.is_empty() && audio.len() as u64 <= TEST_POLICY.generated_max_bytes);
        }
        for pair in arrivals.lock().unwrap().windows(2) {
            assert!(pair[1].duration_since(pair[0]) >= Duration::from_millis(100));
        }
        assert!(!source.exists());
        assert_no_workspace(dir.path());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_during_upload_cleans_owned_chunks_and_retains_original() {
    let dir = TempDir::new().unwrap();
    let (source, tools) = fake_source(dir.path()).await;
    let original = fs::read(&source).await.unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;
    let upload_source = source.clone();
    let context = context(&server);
    let task = tokio::spawn(async move {
        send_audio_with_options(1, &upload_source, "a", "t", &context, TEST_POLICY, &tools).await;
    });
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if !server.received_requests().await.unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("first upload should start after splitting");
    assert!(std::fs::read_dir(dir.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".yt-dl-chunks-")
    }));
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(fs::read(&source).await.unwrap(), original);
    assert_no_workspace(dir.path());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn later_candidate_validation_failure_prevents_all_uploads() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("source.mp3");
    let original = vec![b'x'; 65_001];
    fs::write(&source, &original).await.unwrap();
    let probe = dir.path().join("probe");
    std::fs::write(
        &probe,
        r#"#!/bin/sh
set -eu
for last do :; done
case "$last" in
  */2_*.mp3|*/2.mp3) printf 'malformed JSON';;
  *) printf '{"streams":[{"codec_name":"mp3","sample_rate":"44100","channels":1,"duration":"2"}]}';;
esac
"#,
    )
    .unwrap();
    let encoder = dir.path().join("encoder");
    std::fs::write(
        &encoder,
        r#"#!/bin/sh
set -eu
for last do :; done
out=$(dirname "$last")
printf 'candidate' > "$out/1.mp3"
printf 'candidate' > "$out/2.mp3"
"#,
    )
    .unwrap();
    for script in [&probe, &encoder] {
        std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let server = MockServer::start().await;
    send_audio_with_options(
        1,
        &source,
        "a",
        "t",
        &context(&server),
        TEST_POLICY,
        &MediaTools {
            ffmpeg: encoder,
            ffprobe: probe,
        },
    )
    .await;
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(fs::read(&source).await.unwrap(), original);
    assert_no_workspace(dir.path());
}
