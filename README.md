# Fetchly

Telegram bot: send a link, get the video or audio file back. Rust + teloxide + yt-dlp.

## Supported platforms

| Platform  | Video | Audio | Extras           |
|-----------|-------|-------|------------------|
| YouTube   | ✅    | ✅    | —                |
| TikTok    | ✅    | ✅    | slideshows       |
| Instagram | ✅    | ✅    | carousels        |
| X/Twitter | ✅    | ✅    | photos           |

## How it works

1. Send a URL → the bot resolves metadata and shows a preview card with `🎬 Video` / `🎵 Audio` buttons.
2. Pick a quality — each option shows its estimated size (`720p — ~42 MB`).
3. Watch live progress (`⬇️ Downloading ████████░░ 82%`), cancel anytime with `❌`.
4. Ask for the same link again → instant delivery from the file cache, no re-download.

Audio arrives as a proper music track: MP3 with ID3 title/artist tags, embedded
cover art, and Telegram track metadata — not a generic file.

## Limits

| Limit                | Value                                              |
|----------------------|----------------------------------------------------|
| Rate                 | 20 downloads per user per hour                     |
| File size (standard) | 50 MB (Telegram Bot API cap)                       |
| File size (V1 max)   | 2 GB via Local Bot API Server (see below)          |
| Commands             | `/start`, `/help` — DMs only, no groups/inline     |

## Self-host in 10 minutes

No domain, SSL, or open ports needed — the bot uses long polling (outbound HTTPS only).

**Prerequisites:** a VPS with Docker (1 vCPU / 1 GB RAM handles ~1K downloads/day),
and a bot token from [@BotFather](https://t.me/BotFather).

```bash
mkdir -p /opt/fetchly && cd /opt/fetchly
# copy docker-compose.yml (server variant, image:) and .env there, then:
docker compose pull && docker compose up -d
docker compose logs -f bot
```

`.env` (see `.env.example`):

```env
TELEGRAM_BOT_TOKEN=your-token-here
FETCHLY_MAX_WORKERS=4
FETCHLY_RATE_LIMIT=20
FETCHLY_DB_PATH=/data/fetchly.db
FETCHLY_TEMP_DIR=/tmp/fetchly
REDIS_URL=redis://redis:6379
```

**Files over 50 MB:** run the optional Local Bot API Server (commented `botapi`
service in `docker-compose.yml`, needs API credentials from
[my.telegram.org](https://my.telegram.org)) and set
`TELEGRAM_API_URL=http://botapi:8081`. Unlocks uploads up to 2 GB.

**Auto-deploy (optional):** pushes to `main` build, test, and deploy via
`.github/workflows/deploy.yml` (GHCR + SSH). Needs repo secrets
`SERVER_HOST`, `SERVER_USER`, `SERVER_SSH_KEY` — use a dedicated deploy key.
The image is public, so the server pulls anonymously.

## Local development

```bash
cp .env.example .env        # fill in TELEGRAM_BOT_TOKEN, REDIS_URL=redis://127.0.0.1:6379
redis-server &              # sessions + rate limiting
cargo run                   # needs ffmpeg + yt-dlp on PATH
```

Quality gates (also enforced in CI):

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features   # Redis tests boot throwaway containers (Docker required)
```

## Configuration

| Variable              | Default                  | Purpose                                  |
|-----------------------|--------------------------|------------------------------------------|
| `TELEGRAM_BOT_TOKEN`  | — (required)             | Bot token from BotFather                 |
| `TELEGRAM_API_URL`    | standard API             | Local Bot API base URL for >50 MB files  |
| `FETCHLY_MAX_WORKERS` | `4`                      | Max concurrent downloads (global)        |
| `FETCHLY_RATE_LIMIT`  | `20`                     | Downloads per user per hour              |
| `FETCHLY_DB_PATH`     | `./fetchly.db`           | SQLite file-ID cache (durable)           |
| `FETCHLY_TEMP_DIR`    | `/tmp/fetchly`           | Temp downloads (auto-cleaned)            |
| `REDIS_URL`           | `redis://127.0.0.1:6379` | Sessions + rate counters (ephemeral)     |
| `RUST_LOG`            | `fetchly=info`           | Log filter                               |

## How it's built

```
Telegram update → handlers → media (yt-dlp/ffmpeg/lofty) → upload → cache file_id
                              Redis: 10-min sessions, hourly rate counters
                              SQLite: permanent {url, format, quality} → file_id
```

Concurrency is a semaphore (`FETCHLY_MAX_WORKERS`); cancel buttons flip a
`CancellationToken`; progress edits are throttled to one per 3 s. Design docs:
[`docs/v1-spec.md`](docs/v1-spec.md) · [`docs/architecture.md`](docs/architecture.md) ·
[`docs/decisions.md`](docs/decisions.md) · [`docs/resource-estimates.md`](docs/resource-estimates.md).

## License

MIT — see [LICENSE](LICENSE).
