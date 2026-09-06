# ---- Stage 1: builder ----
FROM rust:1.98-trixie AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
# Cache deps first (dummy main to warm the registry).
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs && cargo build --release || true
COPY src ./src
# COPY preserves host mtimes, which predate the warmup artifacts above.
# Cargo trusts mtimes (older sources = "fresh") and would skip the rebuild,
# shipping the dummy binary — so bump mtimes to force a real rebuild.
RUN find src -name '*.rs' -exec touch {} + && cargo build --release

# ---- Stage 2: runtime ----
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
