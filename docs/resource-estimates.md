# Fetchly V1 — Resource Estimates

## Per-download cost

Each download runs subprocesses sequentially:

| Stage            | CPU        | RAM         | Duration     |
|-----------------|------------|-------------|--------------|
| yt-dlp metadata  | low        | ~60 MB      | 2–5 s        |
| yt-dlp download  | low (I/O)  | ~80 MB      | 10–60 s      |
| ffmpeg convert   | 1 core     | ~50 MB      | 5–30 s       |
| Upload to TG     | low (I/O)  | ~10 MB      | 5–30 s       |

Peak RAM per concurrent download: **~80 MB** (yt-dlp Python process dominates).

Temp disk per download: **50–500 MB** (source + converted output simultaneously).

## Bot process

| Metric         | Value                    |
|---------------|--------------------------|
| Binary         | ~10–15 MB                |
| Idle RAM       | ~5–10 MB                 |
| Per tokio task  | negligible (~few KB)    |

The Rust process is trivial. Subprocesses dominate.

## Typical file sizes

| Content              | Size        |
|---------------------|------------|
| TikTok/Reel (video) | 5–30 MB    |
| YouTube 720p 5 min  | 40–80 MB   |
| YouTube 1080p 5 min | 80–150 MB  |
| YouTube 1080p 20 min| 300–600 MB |
| MP3 320 kbps 4 min  | ~8 MB      |

## Scenarios

### Small — personal / friends (~50 downloads/day)

| Resource      | Estimate                  |
|--------------|--------------------------|
| Concurrency   | 1–2 simultaneous         |
| CPU           | 1 vCPU                   |
| RAM           | 512 MB                   |
| Temp disk     | 1 GB                     |
| Network out   | ~5 GB/day                |
| Redis         | ~5 MB                    |
| **VPS**       | **$4–6/mo**              |

### Medium — public bot (~1K downloads/day)

| Resource      | Estimate                  |
|--------------|--------------------------|
| Concurrency   | 3–5 simultaneous         |
| CPU           | 2 vCPU                   |
| RAM           | 1 GB                     |
| Temp disk     | 5 GB                     |
| Network out   | ~100 GB/day              |
| Redis         | ~50 MB                   |
| **VPS**       | **$10–20/mo**            |

### Large — popular bot (~10K downloads/day)

| Resource      | Estimate                  |
|--------------|--------------------------|
| Concurrency   | 10–20 simultaneous       |
| CPU           | 4 vCPU                   |
| RAM           | 2–4 GB                   |
| Temp disk     | 20 GB (with cleanup)     |
| Network out   | ~1 TB/day                |
| Redis         | ~200 MB                  |
| **VPS**       | **$40–80/mo + bandwidth** |

## Main cost driver: bandwidth

Each file transfers **twice**: platform → server → Telegram.

1K downloads/day × 50 MB average = **100 GB/day** (50 down + 50 up).

Cache hits eliminate both transfers.

## Starting point

**1 vCPU, 1 GB RAM, 20 GB SSD, unmetered or 2 TB bandwidth.**
~$6–12/mo on Hetzner/Contabo/OVH. Handles up to ~1K downloads/day.
