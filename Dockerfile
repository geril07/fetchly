# ---- Stage 0: cargo-chef tool ----
FROM rust:1.98-trixie AS chef
RUN cargo install cargo-chef
WORKDIR /app

# ---- Stage 1: planner — dependency snapshot (cheap, reruns on manifest change) ----
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo chef prepare --recipe-path recipe.json

# ---- Stage 2: builder — cached deps, then the real build ----
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

# ---- Stage 3: runtime ----
FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl ffmpeg \
    && rm -rf /var/lib/apt/lists/*
# yt-dlp standalone binary (bundles its own Python; no pip/PEP 668 issues).
# Rebuild regularly — extractors rot within weeks on stale builds.
ARG YT_DLP_VERSION=latest
RUN curl -fsSL "https://github.com/yt-dlp/yt-dlp/releases/${YT_DLP_VERSION}/download/yt-dlp_linux" \
        -o /usr/local/bin/yt-dlp \
    && chmod +x /usr/local/bin/yt-dlp \
    && yt-dlp --version
COPY --from=builder /app/target/release/fetchly /usr/local/bin/fetchly
ENV FETCHLY_DB_PATH=/data/fetchly.db \
    FETCHLY_TEMP_DIR=/tmp/fetchly
ENTRYPOINT ["/usr/local/bin/fetchly"]
