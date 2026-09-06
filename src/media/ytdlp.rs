use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::media::url::{MediaUrl, Platform};

/// Channel for download progress (0–100).
pub type ProgressTx = tokio::sync::mpsc::UnboundedSender<u8>;

/// Video quality offered to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VideoQuality {
    P360,
    P480,
    P720,
    P1080,
    Best,
}

impl VideoQuality {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::P360 => "360",
            Self::P480 => "480",
            Self::P720 => "720",
            Self::P1080 => "1080",
            Self::Best => "best",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::P360 => "360p",
            Self::P480 => "480p",
            Self::P720 => "720p",
            Self::P1080 => "1080p",
            Self::Best => "Best",
        }
    }

    /// Parse the short callback code (`360`, `480`, `720`, `1080`, `best`).
    pub fn parse_code(s: &str) -> Option<Self> {
        match s {
            "360" => Some(Self::P360),
            "480" => Some(Self::P480),
            "720" => Some(Self::P720),
            "1080" => Some(Self::P1080),
            "best" => Some(Self::Best),
            _ => None,
        }
    }

    /// yt-dlp `-f` selector that caps height at the requested quality.
    #[must_use]
    pub fn format_selector(self) -> &'static str {
        match self {
            Self::P360 => "bestvideo[height<=360]+bestaudio/best[height<=360]/best",
            Self::P480 => "bestvideo[height<=480]+bestaudio/best[height<=480]/best",
            Self::P720 => "bestvideo[height<=720]+bestaudio/best[height<=720]/best",
            Self::P1080 => "bestvideo[height<=1080]+bestaudio/best[height<=1080]/best",
            Self::Best => "bestvideo+bestaudio/best",
        }
    }
}

/// Audio quality offered to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AudioQuality {
    K128,
    K192,
    K320,
    Best,
}

impl AudioQuality {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::K128 => "128",
            Self::K192 => "192",
            Self::K320 => "320",
            Self::Best => "best",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::K128 => "128 kbps",
            Self::K192 => "192 kbps",
            Self::K320 => "320 kbps",
            Self::Best => "Best",
        }
    }

    pub fn parse_code(s: &str) -> Option<Self> {
        match s {
            "128" => Some(Self::K128),
            "192" => Some(Self::K192),
            "320" => Some(Self::K320),
            "best" => Some(Self::Best),
            _ => None,
        }
    }

    /// ffmpeg `-b:a` value for the final MP3.
    #[must_use]
    pub fn mp3_bitrate(self) -> &'static str {
        match self {
            Self::K128 => "128k",
            Self::K192 => "192k",
            Self::K320 | Self::Best => "320k",
        }
    }
}

/// One video quality option with an estimated size (from metadata, if known).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VideoOption {
    pub quality: VideoQuality,
    pub estimated_bytes: Option<u64>,
}

/// One audio quality option with an estimated size.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AudioOption {
    pub quality: AudioQuality,
    pub estimated_bytes: Option<u64>,
}

/// Metadata resolved from a URL via `yt-dlp --dump-single-json`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Metadata {
    pub title: String,
    pub duration_secs: Option<u64>,
    pub thumbnail_url: Option<String>,
    pub platform: Platform,
    pub webpage_url: String,
    pub view_count: Option<u64>,
    pub uploader: Option<String>,
    pub video_options: Vec<VideoOption>,
    pub audio_options: Vec<AudioOption>,
}

impl Metadata {
    #[must_use]
    pub fn duration_label(&self) -> String {
        match self.duration_secs {
            None => "live/unknown".to_owned(),
            Some(s) => format!("{}:{:02}", s / 60, s % 60),
        }
    }
}

/// Resolve metadata with yt-dlp. No file is downloaded.
pub async fn resolve(media: &MediaUrl) -> Result<Metadata> {
    let out = tokio::process::Command::new("yt-dlp")
        .args([
            "--dump-single-json",
            "--no-playlist",
            "--no-warnings",
            "--no-colors",
            "--socket-timeout",
            "15",
            &media.url,
        ])
        .output()
        .await
        .map_err(|e| Error::Resolve(format!("yt-dlp not available: {e}")))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(classify_ytdlp_error(&stderr));
    }

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| Error::Resolve(e.to_string()))?;
    Ok(metadata_from_json(&json, media))
}

/// Largest observed file size per video height, plus the largest pure-audio
/// size (used to scale audio bitrate estimates). Pure helper over yt-dlp JSON.
fn parse_format_sizes(v: &serde_json::Value) -> (std::collections::HashMap<u32, u64>, Option<u64>) {
    let mut best_for_height: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
    let mut best_audio: Option<u64> = None;
    let Some(formats) = v.get("formats").and_then(|f| f.as_array()) else {
        return (best_for_height, best_audio);
    };
    for f in formats {
        let filesize = f
            .get("filesize")
            .and_then(serde_json::Value::as_u64)
            .or_else(|| f.get("filesize_approx").and_then(serde_json::Value::as_u64));
        let height = f.get("height").and_then(serde_json::Value::as_u64);
        let acodec = f.get("acodec").and_then(serde_json::Value::as_str);
        let vcodec = f.get("vcodec").and_then(serde_json::Value::as_str);
        if let (Some(h), Some(size)) = (height, filesize) {
            if vcodec != Some("none") {
                let h = u32::try_from(h).unwrap_or(u32::MAX);
                best_for_height
                    .entry(h)
                    .and_modify(|e| *e = (*e).max(size))
                    .or_insert(size);
            }
        }
        // Pure-audio formats contribute to the audio estimate.
        if vcodec == Some("none") && acodec != Some("none") {
            if let Some(size) = filesize {
                best_audio = Some(best_audio.map_or(size, |b| b.max(size)));
            }
        }
    }
    (best_for_height, best_audio)
}

/// yt-dlp durations are non-negative finite seconds. The saturating `as`
/// cast maps `NaN`/negatives to 0 and huge values to `u64::MAX`,
/// which is the desired behavior for display purposes.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn duration_to_secs(f: f64) -> u64 {
    f.round().max(0.0) as u64
}

/// Input is clamped to 0–100 first, so the conversion is exact.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn percent_to_u8(f: f64) -> u8 {
    f.clamp(0.0, 100.0).round() as u8
}

/// Build [`Metadata`] from yt-dlp JSON. Pure (testable without the binary).
pub fn metadata_from_json(v: &serde_json::Value, media: &MediaUrl) -> Metadata {
    let title = v
        .get("title")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Untitled")
        .to_owned();
    // yt-dlp reports duration as (possibly fractional) seconds.
    let duration_secs = v
        .get("duration")
        .and_then(serde_json::Value::as_f64)
        .map(duration_to_secs);
    let thumbnail_url = v
        .get("thumbnail")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let webpage_url = v
        .get("webpage_url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&media.url)
        .to_owned();
    let view_count = v.get("view_count").and_then(serde_json::Value::as_u64);
    let uploader = v
        .get("uploader")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);

    // Estimate per-height sizes from the formats list.
    let (best_for_height, best_audio) = parse_format_sizes(v);

    // Fall back to duration × bitrate when formats carry no sizes.
    let audio_estimate = |kbps: u64| -> Option<u64> {
        if let Some(b) = best_audio {
            // Scale the observed best-audio size to the requested bitrate
            // (assume the observed file is ~192 kbps when unknown).
            Some(b * kbps / 192)
        } else {
            duration_secs.map(|d| d * kbps * 1000 / 8)
        }
    };

    let video_for = |cap: u32| -> Option<u64> {
        best_for_height
            .iter()
            .filter(|(h, _)| **h <= cap)
            .map(|(_, s)| *s)
            .max()
    };

    Metadata {
        title,
        duration_secs,
        thumbnail_url,
        platform: media.platform,
        webpage_url,
        view_count,
        uploader,
        video_options: [
            VideoQuality::P360,
            VideoQuality::P480,
            VideoQuality::P720,
            VideoQuality::P1080,
            VideoQuality::Best,
        ]
        .into_iter()
        .map(|q| {
            let estimated_bytes = match q {
                VideoQuality::P360 => video_for(360),
                VideoQuality::P480 => video_for(480),
                VideoQuality::P720 => video_for(720),
                VideoQuality::P1080 => video_for(1080),
                VideoQuality::Best => {
                    best_for_height.values().copied().max().or_else(|| {
                        duration_secs.map(|d| d * 1_500_000 / 8) // ~1.5 Mbps guess
                    })
                }
            };
            VideoOption {
                quality: q,
                estimated_bytes,
            }
        })
        .collect(),
        audio_options: [
            AudioQuality::K128,
            AudioQuality::K192,
            AudioQuality::K320,
            AudioQuality::Best,
        ]
        .into_iter()
        .map(|q| {
            let estimated_bytes = match q {
                AudioQuality::K128 => audio_estimate(128),
                AudioQuality::K192 => audio_estimate(192),
                AudioQuality::K320 | AudioQuality::Best => audio_estimate(320),
            };
            AudioOption {
                quality: q,
                estimated_bytes,
            }
        })
        .collect(),
    }
}

/// Map yt-dlp stderr to a user-facing error.
pub fn classify_ytdlp_error(stderr: &str) -> Error {
    let lower = stderr.to_lowercase();
    if lower.contains("private")
        || lower.contains("login required")
        || lower.contains("log in")
        || lower.contains("restricted")
        || lower.contains("unavailable")
        || lower.contains("not available")
    {
        Error::PrivateOrRestricted
    } else if lower.contains("unsupported url") || lower.contains("no video formats found") {
        Error::UnsupportedUrl
    } else if lower.contains("file is larger than") || lower.contains("larger than the maximum") {
        Error::TooLarge
    } else {
        // Truncate raw tool output; the user message stays generic.
        let snippet: String = stderr.chars().take(300).collect();
        Error::Download(snippet)
    }
}

/// Parse a yt-dlp `--progress-template` line like `download: 82.3%`.
/// Returns 0–100 or `None` when the line carries no progress.
#[must_use]
pub fn parse_progress_line(line: &str) -> Option<u8> {
    let (_, pct) = line.split_once(':')?;
    let num: String = pct
        .trim()
        .strip_suffix('%')
        .unwrap_or(pct.trim())
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let f: f64 = num.parse().ok()?;
    Some(percent_to_u8(f))
}

/// Download video with yt-dlp, merging to MP4. Returns the final file path.
pub async fn download_video(
    media: &MediaUrl,
    quality: VideoQuality,
    out_path: &Path,
    cancel: &CancellationToken,
    progress: Option<&ProgressTx>,
) -> Result<PathBuf> {
    if let Some(parent) = out_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // yt-dlp appends the extension when merging; use a template without one.
    let template = out_path.with_extension("").to_string_lossy().into_owned();

    let mut child = tokio::process::Command::new("yt-dlp")
        .args([
            "-f",
            quality.format_selector(),
            "--merge-output-format",
            "mp4",
            "--no-playlist",
            "--no-warnings",
            "--no-colors",
            "--newline",
            "--progress-template",
            "download:%(progress._percent_str)s",
            "-o",
            &format!("{template}.%(ext)s"),
            &media.url,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| Error::Download(format!("failed to start yt-dlp: {e}")))?;

    let stdout = child.stdout.take();
    let progress_task = stdout.map(|stdout| {
        let progress = progress.cloned();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if let Some(pct) = parse_progress_line(&line) {
                    if let Some(tx) = &progress {
                        let _ = tx.send(pct);
                    }
                }
            }
        })
    });

    let status = tokio::select! {
        status = child.wait() => status.map_err(|e| Error::Download(e.to_string()))?,
        () = cancel.cancelled() => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            if let Some(t) = progress_task { t.abort(); }
            return Err(Error::Cancelled);
        }
    };
    if let Some(t) = progress_task {
        let _ = t.await;
    }

    if !status.success() {
        // Re-run is wasteful; surface a generic failure (stderr already consumed
        // for progress, so we cannot reliably classify here).
        return Err(Error::Download("yt-dlp exited with an error".to_owned()));
    }

    // Find the merged output (`.mp4` preferred).
    for ext in ["mp4", "mkv", "webm"] {
        let candidate = out_path.with_extension(ext);
        if tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
            enforce_size_limit(&candidate).await?;
            return Ok(candidate);
        }
    }
    Err(Error::Download("yt-dlp produced no file".to_owned()))
}

/// Download best-audio source (no conversion). Caller runs ffmpeg next.
pub async fn download_audio_source(
    media: &MediaUrl,
    out_path: &Path,
    cancel: &CancellationToken,
    progress: Option<&ProgressTx>,
) -> Result<PathBuf> {
    if let Some(parent) = out_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let template = out_path.with_extension("").to_string_lossy().into_owned();

    let mut child = tokio::process::Command::new("yt-dlp")
        .args([
            "-f",
            "bestaudio/best",
            "--no-playlist",
            "--no-warnings",
            "--no-colors",
            "--newline",
            "--progress-template",
            "download:%(progress._percent_str)s",
            "-o",
            &format!("{template}.%(ext)s"),
            &media.url,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| Error::Download(format!("failed to start yt-dlp: {e}")))?;

    let stdout = child.stdout.take();
    let progress_task = stdout.map(|stdout| {
        let progress = progress.cloned();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if let Some(pct) = parse_progress_line(&line) {
                    if let Some(tx) = &progress {
                        let _ = tx.send(pct);
                    }
                }
            }
        })
    });

    let status = tokio::select! {
        status = child.wait() => status.map_err(|e| Error::Download(e.to_string()))?,
        () = cancel.cancelled() => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            if let Some(t) = progress_task { t.abort(); }
            return Err(Error::Cancelled);
        }
    };
    if let Some(t) = progress_task {
        let _ = t.await;
    }
    if !status.success() {
        return Err(Error::Download("yt-dlp exited with an error".to_owned()));
    }

    // Pick whatever audio container yt-dlp produced.
    let mut found: Option<PathBuf> = None;
    let mut dir = tokio::fs::read_dir(out_path.parent().unwrap_or(Path::new("."))).await?;
    let stem = out_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("audio");
    while let Ok(Some(entry)) = dir.next_entry().await {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(stem) {
            found = Some(entry.path());
            break;
        }
    }
    match found {
        Some(p) => {
            enforce_size_limit(&p).await?;
            Ok(p)
        }
        None => Err(Error::Download("yt-dlp produced no file".to_owned())),
    }
}

/// Telegram caps Bot API uploads at 2 GB (Local Server). Reject larger files.
async fn enforce_size_limit(path: &Path) -> Result<()> {
    let meta = tokio::fs::metadata(path).await?;
    if meta.len() > 2_000_000_000 {
        let _ = tokio::fs::remove_file(path).await;
        return Err(Error::TooLarge);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media() -> MediaUrl {
        MediaUrl {
            platform: Platform::Youtube,
            url: "https://www.youtube.com/watch?v=x".to_owned(),
        }
    }

    #[test]
    fn progress_parses() {
        assert_eq!(parse_progress_line("download: 82.3%"), Some(82));
        assert_eq!(parse_progress_line("download:100%"), Some(100));
        assert_eq!(parse_progress_line("download: NA"), None);
        assert_eq!(parse_progress_line("noise"), None);
    }

    #[test]
    fn selectors_cover_all_qualities() {
        for q in [
            VideoQuality::P360,
            VideoQuality::P480,
            VideoQuality::P720,
            VideoQuality::P1080,
            VideoQuality::Best,
        ] {
            assert!(!q.format_selector().is_empty());
            assert!(VideoQuality::parse_code(q.as_str()) == Some(q));
        }
        for q in [
            AudioQuality::K128,
            AudioQuality::K192,
            AudioQuality::K320,
            AudioQuality::Best,
        ] {
            assert!(!q.mp3_bitrate().is_empty());
            assert!(AudioQuality::parse_code(q.as_str()) == Some(q));
        }
    }

    #[test]
    fn classifies_errors() {
        assert!(matches!(
            classify_ytdlp_error("ERROR: Private video"),
            Error::PrivateOrRestricted
        ));
        assert!(matches!(
            classify_ytdlp_error("ERROR: Unsupported URL"),
            Error::UnsupportedUrl
        ));
        assert!(matches!(
            classify_ytdlp_error("ERROR: boom"),
            Error::Download(_)
        ));
    }

    #[test]
    fn metadata_from_minimal_json() {
        let v: serde_json::Value = serde_json::json!({
            "title": "Test",
            "duration": 240.0,
            "thumbnail": "https://img/x.jpg",
            "webpage_url": "https://youtu.be/x",
            "formats": []
        });
        let m = metadata_from_json(&v, &media());
        assert_eq!(m.title, "Test");
        assert_eq!(m.duration_secs, Some(240));
        assert_eq!(m.video_options.len(), 5);
        assert_eq!(m.audio_options.len(), 4);
        // Duration-based fallback: 240 s × 320 kbps / 8 = 9.6 MB.
        let best = m
            .audio_options
            .iter()
            .find(|o| o.quality == AudioQuality::Best)
            .expect("best audio");
        assert_eq!(best.estimated_bytes, Some(9_600_000));
    }
}
