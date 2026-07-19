# yt-dlp Telegram Audio Downloading Service

A simple Rust service that accepts YouTube video URLs via HTTP (e.g. Telegram webhook), downloads the audio using `yt-dlp`, converts it to MP3, and sends the audio file back to Telegram.

---

## Features

- Accepts video URLs via POST requests (Telegram webhook format).
- Downloads and converts YouTube videos to MP3 asynchronously.
- Sends the MP3 audio file to the specified Telegram chat via bot.

---

## Usage

1. Set your Telegram bot token and user allowed to use the bot in `.env`
2. Configure Telegram webhook to point to the service URL.

Can be run as a service, config example in `systemd_config` folder
---

## Development

The service shells out to command-line tools at runtime. When running directly on the host machine, install these first and make sure they are available on `PATH`:

- `yt-dlp`
- `ffmpeg`

The service writes downloaded audio files under `./downloads`, so create that directory before running locally:

```sh
mkdir -p downloads
```

Required environment variables:

- `TELEGRAM_BOT_TOKEN`
- `ALLOWED_USER_ID`

Optional environment variables:

- `USE_IPV6` controls whether `yt-dlp` is called with `-6`. Defaults to `true`. Set to `false`, `0`, `no`, or `off` to disable it.

Logs include child process output with timestamps and severity levels.

---

## Build & Deployment

- Build locally with `cargo build --locked --release`.
- Dockerized environment provided. The Docker image installs `yt-dlp` and `ffmpeg` inside the container, so host installs are only required for non-Docker runs.
- Recommended to run as a systemd service for production host deployments.

---

## Versioning

Releases use semantic versioning. The current version is defined in Rust package metadata in `Cargo.toml` and is currently `1.0.1`.

CI reads the package version via Cargo metadata and publishes releases with tags like `v1.0.1`. Release artifacts are named with the same package version, for example:

```text
yt_dl_service-1.0.1-x86_64-unknown-linux-musl
```

---

Enjoy coding in Rust! 🚀
