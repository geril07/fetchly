# Fetchly V1 — Decisions

Key decisions made during design, with rationale. Do not re-debate these.

## Rust over Go

Project is I/O-bound subprocess orchestration. Go ecosystem is slightly better for this but not "by far." User prefers Rust. teloxide, tokio, lofty, serde cover all needs.

## Long polling over webhook

Single server, single process. Long polling needs no domain, SSL, or reverse proxy. Webhook adds infrastructure for no benefit at this scale. Switch to webhook if multi-instance is ever needed.

## Redis for sessions + rate limiting, SQLite for file_id cache

Two stores, each plays to its strength:
- Redis: ephemeral data with TTL (sessions between button presses, rate limit counters). INCR + EXPIRE is native.
- SQLite: durable data (file_id cache). Must survive restarts. Telegram file_ids are persistent — no TTL, no eviction needed.

## No Redis for file_id cache

Was initially considered. Removed because file_ids are permanent (per Telegram FAQ) and must survive restarts. SQLite is a file on disk — zero risk of data loss, no persistence config needed.

## Semaphore concurrency, not job queue

tokio::sync::Semaphore with `FETCHLY_MAX_WORKERS` env var (default 4). No external job queue — downloads are tokio tasks waiting for a semaphore permit. Simple, in-process, sufficient for single-server.

## Graceful shutdown, not blue-green

120s stop_grace_period. Bot catches SIGTERM, stops accepting updates, waits for active downloads, exits. Telegram queues updates during the 2–3s gap. No messages lost, no downloads lost. Blue-green adds reverse proxy + webhook + health checks for a problem that barely exists.

## yt-dlp + ffmpeg as subprocesses, no library bindings

Both called via tokio::process::Command. No Rust bindings to libav or yt-dlp internals. Simpler, easier to update (just pip install / apt upgrade), same performance for this use case.

## GitHub Actions + GHCR + SSH deploy

CI builds image, pushes to GitHub Container Registry (free, bundled with repo), SSHs into server to pull + restart. No Kubernetes, no Ansible, no Terraform. Single server doesn't need more.

## File structure: low coupling, high cohesion, high colocation

- `media/` owns all extraction + processing. Zero outward coupling.
- `telegram/` owns all bot interaction.
- `cache.rs`, `session.rs`, `limiter.rs` are independent leaf modules.
- `telegram/handlers.rs` is the composition root.
- URL parsing lives inside `media/` (colocated with the domain it serves).
- yt-dlp metadata + download merged into one file (one subprocess, one owner).
