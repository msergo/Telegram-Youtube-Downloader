FROM rust:1.88 as builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Prebuild dependencies for caching
RUN cargo build --release

# 2️⃣ Create final runtime image with yt-dlp and ffmpeg
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ffmpeg \
    curl \
    ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Install yt-dlp (python-free binary version)
RUN curl -L https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp -o /usr/local/bin/yt-dlp \
 && chmod +x /usr/local/bin/yt-dlp

# Copy compiled Rust binary from builder
COPY --from=builder /app/target/release/yt_dl_service /usr/local/bin/yt_dl_service

WORKDIR /downloads

EXPOSE 3000

# Run the server
CMD ["yt_dl_service"]
