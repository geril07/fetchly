# Fetchly V1 — Architecture

## Update delivery

Long polling (`getUpdates`). No domain, SSL, or reverse proxy required.

## Flow

```
User sends URL
  │
  ├─ parse URL → detect platform, normalize
  ├─ yt-dlp --dump-json → metadata (title, formats, thumbnail, duration)
  ├─ send preview message + [🎬 Video] [🎵 Audio] buttons
  │
User taps Video/Audio
  │
  ├─ show quality picker (with estimated sizes from metadata)
  │
User taps quality
  │
  ├─ rate limit check (Redis)
  ├─ cache lookup: url+format+quality → file_id? (SQLite)
  │   ├─ hit → sendVideo/sendAudio instantly, done
  │   └─ miss ↓
  ├─ acquire semaphore (or show queue position)
  ├─ yt-dlp download → temp file
  ├─ ffmpeg convert (audio only)
  ├─ lofty tag MP3 (audio only)
  ├─ upload to Telegram
  ├─ cache file_id (SQLite)
  └─ cleanup temp files
```

## Project structure

```
src/
  main.rs              — tokio + teloxide dispatcher setup
  config.rs            — env-based config (bot token, max workers, limits, paths)
  error.rs             — unified error type

  telegram/
    mod.rs
    handlers.rs        — message + callback handlers (orchestrator)
    keyboard.rs        — inline keyboard builders
    progress.rs        — throttled editMessageText

  media/
    mod.rs             — public API: resolve(url) → Metadata, download(...) → File
    url.rs             — detect platform, normalize URL
    ytdlp.rs           — yt-dlp subprocess (metadata + download)
    ffmpeg.rs          — ffmpeg subprocess wrapper
    tag.rs             — lofty MP3 tagging + cover art

  cache.rs             — SQLite file_id cache
  session.rs           — Redis session state
  limiter.rs           — Redis rate limiting
```

### Why this grouping

| Module | Cohesion | Changes when... |
|--------|----------|-----------------|
| `telegram/` | All Telegram Bot API interaction | Bot UX changes (buttons, messages, progress) |
| `media/` | All media extraction + processing | Platform support, format handling, yt-dlp/ffmpeg behavior |
| `cache.rs` | file_id persistence | Cache schema, eviction policy |
| `session.rs` | Callback state between button presses | Session data shape, TTL |
| `limiter.rs` | Per-user rate enforcement | Rate policy, counting logic |

### Coupling map

```
telegram/handlers.rs  →  media, cache, session, limiter, keyboard, progress
media/*               →  nothing (subprocess + file I/O only)
cache.rs              →  rusqlite
session.rs            →  redis
limiter.rs            →  redis
```

`media/` is a leaf — zero outward coupling.
`telegram/handlers.rs` is the composition root — couples to everything through narrow public APIs.

## State management

| Data | Store | Why |
|------|-------|-----|
| Session (URL, metadata between button presses) | Redis | Ephemeral, TTL-native, survives restart |
| Rate limit counters | Redis | INCR + EXPIRE, survives restart |
| file_id cache | SQLite | Durable, must survive restart, never re-download for free |

### Session → callback data contract

Telegram callback data limit: **64 bytes**.

Format: `v:720:{session_id}` or `a:320:{session_id}` — action + quality + 8-char session ID.

Full state (URL, metadata, formats) stored in Redis under `session:{session_id}`, TTL 10 minutes.

### SQLite schema

```sql
CREATE TABLE file_cache (
    url_hash   TEXT NOT NULL,
    format     TEXT NOT NULL,  -- "video" | "audio"
    quality    TEXT NOT NULL,  -- "720" | "320" | "best"
    file_id    TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (url_hash, format, quality)
);
```

## Concurrency control

```
tokio::sync::Semaphore  — max concurrent downloads (global)
tokio_util::sync::CancellationToken  — per-download, wired to ❌ button
```

Semaphore size set via `FETCHLY_MAX_WORKERS` env var (default: **4**).

Tuning reference: `(available_ram - 100 MB) / 80 MB per yt-dlp process`.

| RAM available | Suggested `MAX_WORKERS` |
|--------------|-------------------------|
| 512 MB        | 3                       |
| 1 GB          | 8                       |
| 2 GB          | 15                      |
| 4 GB          | 30                      |

When all permits are held, user sees queue position. Download starts automatically when a permit frees.

## Progress updates

yt-dlp writes progress to stderr. Parse percentage, throttle `editMessageText` to **once per 3 seconds** (Telegram rate limit).

```
⬇️ Downloading ████████░░ 82%
```

## Temp file management

Each download gets its own directory: `/tmp/fetchly/{session_id}/`.

Cleanup:
1. **Normal**: `rm -rf` the directory after upload or on error.
2. **Safety net**: tokio background task deletes anything in `/tmp/fetchly/` older than 10 minutes.

## Telegram Bot API limits

| Limit                        | Value  |
|-----------------------------|--------|
| File upload (standard)       | 50 MB  |
| File upload (Local Bot API)  | 2 GB   |
| Messages per second (global) | 30     |
| Messages per chat per second | 1      |

## Deployment

Docker Compose, single server.

### Project root layout

```
fetchly/
  src/
  docs/
  Cargo.toml
  Cargo.lock
  Dockerfile
  docker-compose.yml
  .dockerignore
  .env.example
```

### Dockerfile

Multi-stage build:

```
Stage 1 — builder
  rust:1.98-trixie
  cargo build --release

Stage 2 — runtime
  debian:trixie-slim
  apt install: ca-certificates, curl, ffmpeg
  yt-dlp standalone binary from GitHub releases
  copy binary from builder
  ENTRYPOINT ["./fetchly"]
```

Pinned September 2026. `bookworm` is oldstable (Debian 13 `trixie` is stable).
yt-dlp comes as the standalone `yt-dlp_linux` binary (bundles its own Python):
`pip install` is blocked by PEP 668 on trixie and lags on extractor fixes.
Rebuild the image regularly — extractors rot within weeks on stale builds.

Rust compiles to a single static binary. Runtime image needs ffmpeg plus
curl/ca-certificates to fetch yt-dlp — no Python toolchain.

### docker-compose.yml

```yaml
services:
  bot:
    build: .
    env_file: .env
    volumes:
      - db:/data              # SQLite file
      - tmp:/tmp/fetchly      # temp downloads
    depends_on:
      - redis
    restart: unless-stopped
    stop_grace_period: 120s   # let in-flight downloads finish on deploy

  redis:
    image: redis:8-alpine
    volumes:
      - redis:/data

volumes:
  db:
  tmp:
  redis:
```

Pinned September 2026 (`redis:7-alpine` is outdated; latest is 8.x).

Optional: Telegram Local Bot API Server for files >50 MB (up to 2 GB).
Uncomment to enable, and set `TELEGRAM_API_URL=http://botapi:8081` in `.env`:

```yaml
  # botapi:
  #   image: aiogram/telegram-bot-api:latest
  #   environment:
  #     TELEGRAM_API_ID: ${TELEGRAM_API_ID}
  #     TELEGRAM_API_HASH: ${TELEGRAM_API_HASH}
  #     TELEGRAM_LOCAL: "1"
  #   ports:
  #     - "8081:8081"
  #   volumes:
  #     - botapi:/var/lib/telegram-bot-api
```

### .env.example

```env
TELEGRAM_BOT_TOKEN=
# Optional: Local Bot API Server base URL (for >50 MB uploads).
# Example with the commented `botapi` service above:
# TELEGRAM_API_URL=http://botapi:8081
TELEGRAM_API_URL=
FETCHLY_MAX_WORKERS=4
FETCHLY_RATE_LIMIT=20           # downloads per user per hour
FETCHLY_DB_PATH=/data/fetchly.db
FETCHLY_TEMP_DIR=/tmp/fetchly
REDIS_URL=redis://redis:6379
RUST_LOG=fetchly=info,teloxide=warn
```

### Volumes

| Volume | Path in container | Purpose |
|--------|------------------|---------|
| `db`   | `/data`          | SQLite file — persists across restarts |
| `tmp`  | `/tmp/fetchly`   | Temp downloads — survives container restart, cleaned by bot |
| `redis`| `/data` (redis)  | Redis persistence — optional, sessions/counters are ephemeral |

## Continuous deployment

GitHub Actions → GHCR → SSH deploy.

### Flow

```
push to main
  → GitHub Actions
    → cargo test
    → docker build
    → push to ghcr.io/geril/fetchly:latest
    → SSH into server
    → docker compose pull && docker compose up -d
```

### Project root layout (updated)

```
fetchly/
  .github/
    workflows/
      deploy.yml
  src/
  docs/
  ...
```

### deploy.yml outline

```yaml
name: Deploy

on:
  push:
    branches: [main]

env:
  REGISTRY: ghcr.io
  IMAGE: ghcr.io/${{ github.repository }}

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all -- --check
      - run: cargo clippy --all-targets --all-features -- -D warnings
      - run: cargo test --all-features

  deploy:
    needs: test
    runs-on: ubuntu-latest
    permissions:
      packages: write
    steps:
      - uses: actions/checkout@v6

      - uses: docker/login-action@v4
        with:
          registry: ${{ env.REGISTRY }}
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - uses: docker/build-push-action@v7
        with:
          push: true
          tags: ${{ env.IMAGE }}:latest

      - name: Deploy to server
        uses: appleboy/ssh-action@v1
        with:
          host: ${{ secrets.SERVER_HOST }}
          username: ${{ secrets.SERVER_USER }}
          key: ${{ secrets.SERVER_SSH_KEY }}
          script: |
            cd /opt/fetchly
            docker compose pull
            docker compose up -d
```

### Server setup (one-time)

1. Install Docker + Docker Compose on server.
2. `mkdir /opt/fetchly`, place `docker-compose.yml` and `.env` there.
3. `docker login ghcr.io` with a PAT (read:packages scope).
4. Add GitHub repo secrets: `SERVER_HOST`, `SERVER_USER`, `SERVER_SSH_KEY`.

### docker-compose.yml on server

Server uses `image:` instead of `build:`:

```yaml
services:
  bot:
    image: ghcr.io/geril/fetchly:latest
    env_file: .env
    volumes:
      - db:/data
      - tmp:/tmp/fetchly
    depends_on:
      - redis
    restart: unless-stopped

  redis:
    image: redis:8-alpine
    volumes:
      - redis:/data

volumes:
  db:
  tmp:
  redis:
```

### Graceful shutdown

On deploy, active downloads finish before the old container exits.

```
docker compose up -d
  → SIGTERM to old container
  → bot stops accepting new updates
  → bot waits for in-flight downloads to complete (up to 120s)
  → bot exits
  → new container starts, begins polling
  → Telegram delivers queued updates
```

No messages lost — Telegram queues updates while bot is between containers.
No downloads lost — graceful shutdown waits for active tasks.

Server-side compose sets the grace period:

```yaml
services:
  bot:
    image: ghcr.io/geril/fetchly:latest
    stop_grace_period: 120s
    ...
```

In code:

1. Catch `SIGTERM` via `tokio::signal`.
2. Stop teloxide dispatcher (no new updates).
3. `join` all active download tasks.
4. Exit.
