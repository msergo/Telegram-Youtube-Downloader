FROM rust:1.88 AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --locked --release

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        ffmpeg \
    && rm -rf /var/lib/apt/lists/*

RUN curl --fail --location --show-error \
        https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux \
        --output /usr/local/bin/yt-dlp \
    && chmod +x /usr/local/bin/yt-dlp

COPY --from=builder /app/target/release/yt_dl_service /usr/local/bin/yt_dl_service

WORKDIR /app
RUN mkdir -p downloads

EXPOSE 3000

CMD ["yt_dl_service"]
