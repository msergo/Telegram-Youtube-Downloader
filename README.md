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

- External dependency on `yt-dlp` and `ffmpeg` available in the environment.
- Logs child process output with timestamps and severity levels.

---

## Build & Deployment

- Build with `cargo build --release`.
- Dockerized environment provided.
- Recommended to run as systemd service for production.

---

Enjoy coding in Rust! 🚀
