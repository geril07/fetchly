# Fetchly V1 — Spec

Telegram bot. User sends a link → gets video or audio file back.

## Supported platforms

| Platform  | Video | Audio | Photos/Carousels |
|-----------|-------|-------|------------------|
| YouTube   | ✅    | ✅    | —                |
| TikTok    | ✅    | ✅    | slideshows       |
| Instagram | ✅    | ✅    | carousels        |
| X/Twitter | ✅    | ✅    | photos           |

## Core flow

```
User sends URL
  → Bot resolves metadata (title, duration, thumbnail, platform)
  → Bot shows preview card with inline buttons:

    🎬 Video  🎵 Audio

  → User taps choice
  → Bot shows quality options:

    Video: 360p · 480p · 720p · 1080p · Best
    Audio: 128 kbps · 192 kbps · 320 kbps · Best

  → Bot shows progress:
    ⬇️ Downloading...
    🔄 Converting...
    ⬆️ Uploading...

  → Bot sends file
```

## Preview card

After URL resolution, bot sends a message:

```
{title}
{duration} · {platform} · {views if available}
[thumbnail]

🎬 Video    🎵 Audio
```

## Audio — MP3 with metadata

When user selects audio, the MP3 file includes:

- ID3 tags: title, artist, album (if available), year
- Embedded cover art from thumbnail
- Telegram audio metadata: title, performer, duration, thumbnail

The file appears in Telegram as a proper music track, not a generic document.

## Quality selection

### Video
Offer available resolutions. Show estimated file size next to each option.

```
360p  — ~15 MB
720p  — ~42 MB
1080p — ~95 MB
```

### Audio
```
128 kbps — ~3 MB
320 kbps — ~7 MB
Best     — ~8 MB
```

## Progress feedback

Show status updates during download:

```
⬇️ Downloading ████████░░ 82%
```

Support `❌ Cancel` button during download.

## File delivery

| Condition    | Method                        |
|-------------|-------------------------------|
| ≤ 50 MB     | Standard Bot API upload       |
| 50 MB–2 GB  | Local Bot API Server          |
| > 2 GB      | Not supported in V1           |

## Caching

After first download+upload, store `{url_hash, format, quality} → telegram_file_id`.

Second request for same URL+format+quality → instant `sendVideo`/`sendAudio` via `file_id`. No re-download.

## Error handling

| Condition              | Response                        |
|-----------------------|---------------------------------|
| Invalid/unsupported URL | "Unsupported link. Send a YouTube, TikTok, Instagram, or X URL." |
| Private/restricted content | "This content is private or restricted." |
| Download failure       | "Download failed. Try again later." |
| File too large         | "File exceeds 2 GB limit. Try lower quality." |
| Rate limit (platform)  | "Too many requests. Try again in {N} seconds." |

## Rate limiting

Per-user: **20 downloads per hour**. Show remaining quota on limit hit.

## Commands

| Command   | Action                          |
|-----------|--------------------------------|
| `/start`  | Welcome message + instructions |
| `/help`   | Usage guide                    |

## Tech stack

Versions pinned September 2026.

| Component        | Tool                                             | Version                          |
|-----------------|--------------------------------------------------|----------------------------------|
| Language         | Rust (2024 edition)                              | toolchain 1.98                   |
| Async runtime    | tokio                                            | 1                                |
| Async utilities  | tokio-util (`CancellationToken`)                 | 0.7                              |
| Bot framework    | teloxide                                         | 0.17                             |
| Extraction       | yt-dlp standalone binary (GitHub releases)       | latest at image build time       |
| Audio conversion | ffmpeg (Debian package)                          | trixie                           |
| MP3 tags         | lofty                                            | 0.25                             |
| HTTP client      | reqwest                                          | 0.13                             |
| Serialization    | serde + serde_json                               | 1                                |
| File-ID cache    | SQLite (rusqlite, `bundled`)                     | 0.40                             |
| Sessions + rate  | Redis (`redis` crate, `tokio-comp` feature)      | server 8 / crate 1               |
| Task queue       | None in V1 (tokio tasks)                         | —                                |
| Lints            | rustfmt + clippy (`all=deny`, `pedantic=warn`)   | enforced in CI                   |

## Non-goals for V1

- YouTube search by text query
- Playlist download
- Timestamp trimming
- Inline mode
- Group mode
- User settings / preferences
- Video/audio editing tools
- AI features (transcription, summary)
- Music recognition
- Mini App
- Premium / monetization
- Admin dashboard
- Multi-language support
- Shorts-specific handling (treated as normal URL)

## Deployment

Single server. Docker Compose:
- `bot` container (Rust binary + yt-dlp + ffmpeg)
- `redis` container (sessions + rate limiting)
- Optional: Telegram Local Bot API Server container (for >50 MB files)
