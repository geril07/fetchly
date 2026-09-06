use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

use crate::media::ytdlp::Metadata;

/// Preview card buttons: `[🎬 Video] [🎵 Audio]`.
#[must_use]
pub fn preview_keyboard(session_id: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new([[
        InlineKeyboardButton::callback("🎬 Video", format!("v:pick:{session_id}")),
        InlineKeyboardButton::callback("🎵 Audio", format!("a:pick:{session_id}")),
    ]])
}

/// Quality picker for video. Each button shows `720p — ~42 MB` when known.
#[must_use]
pub fn video_quality_keyboard(meta: &Metadata, session_id: &str) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = meta
        .video_options
        .iter()
        .map(|opt| {
            let label = match opt.estimated_bytes {
                Some(b) => format!("{} — ~{}", opt.quality.label(), format_bytes(b)),
                None => opt.quality.label().to_owned(),
            };
            vec![InlineKeyboardButton::callback(
                label,
                format!("v:{}:{session_id}", opt.quality.as_str()),
            )]
        })
        .collect();
    rows.push(vec![InlineKeyboardButton::callback(
        "❌ Cancel",
        format!("cancel:x:{session_id}"),
    )]);
    InlineKeyboardMarkup::new(rows)
}

/// Quality picker for audio.
#[must_use]
pub fn audio_quality_keyboard(meta: &Metadata, session_id: &str) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = meta
        .audio_options
        .iter()
        .map(|opt| {
            let label = match opt.estimated_bytes {
                Some(b) => format!("{} — ~{}", opt.quality.label(), format_bytes(b)),
                None => opt.quality.label().to_owned(),
            };
            vec![InlineKeyboardButton::callback(
                label,
                format!("a:{}:{session_id}", opt.quality.as_str()),
            )]
        })
        .collect();
    rows.push(vec![InlineKeyboardButton::callback(
        "❌ Cancel",
        format!("cancel:x:{session_id}"),
    )]);
    InlineKeyboardMarkup::new(rows)
}

/// Single cancel button shown under the progress message during download.
#[must_use]
pub fn cancel_keyboard(session_id: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new([[InlineKeyboardButton::callback(
        "❌ Cancel",
        format!("cancel:x:{session_id}"),
    )]])
}

/// Human-readable byte count: `42 MB`, `3.1 GB`, `512 KB`.
///
/// Integer math only — no float precision loss even for huge files.
#[must_use]
pub fn format_bytes(bytes: u64) -> String {
    const GB: u64 = 1_000_000_000;
    const MB: u64 = 1_000_000;
    const KB: u64 = 1_000;
    if bytes >= GB {
        format!("{}.{} GB", bytes / GB, (bytes % GB) / 100_000_000)
    } else if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{} KB", bytes / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Preview card text: `{title}\n{duration} · {platform} · {views}`.
#[must_use]
pub fn preview_text(meta: &Metadata) -> String {
    use std::fmt::Write as _;
    let mut line2 = format!("{} · {}", meta.duration_label(), meta.platform.as_str());
    if let Some(views) = meta.view_count {
        let _ = write!(line2, " · {}", format_views(views));
    }
    if let Some(uploader) = &meta.uploader {
        if !uploader.is_empty() {
            let _ = write!(line2, " · {uploader}");
        }
    }
    format!("{}\n{line2}", escape_caption(&meta.title))
}

fn format_views(views: u64) -> String {
    if views >= 1_000_000 {
        format!(
            "{}.{}M views",
            views / 1_000_000,
            (views % 1_000_000) / 100_000
        )
    } else if views >= 1_000 {
        format!("{}.{}K views", views / 1_000, (views % 1_000) / 100)
    } else {
        format!("{views} views")
    }
}

/// Escape text for Telegram HTML parse mode (we send captions as HTML).
fn escape_caption(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Progress bar: `⬇️ Downloading ████████░░ 82%`.
#[must_use]
pub fn progress_bar(prefix: &str, pct: u8) -> String {
    let filled = usize::from(pct) * 10 / 100;
    let empty = 10 - filled;
    format!(
        "{prefix} {}{} {pct}%",
        "█".repeat(filled),
        "░".repeat(empty)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::url::Platform;
    use crate::media::ytdlp::{AudioOption, AudioQuality, VideoOption, VideoQuality};

    fn meta() -> Metadata {
        Metadata {
            title: "T".to_owned(),
            duration_secs: Some(60),
            thumbnail_url: None,
            platform: Platform::Youtube,
            webpage_url: "https://x".to_owned(),
            view_count: Some(1_500_000),
            uploader: Some("u".to_owned()),
            video_options: vec![VideoOption {
                quality: VideoQuality::P720,
                estimated_bytes: Some(42_000_000),
            }],
            audio_options: vec![AudioOption {
                quality: AudioQuality::K320,
                estimated_bytes: Some(7_000_000),
            }],
        }
    }

    #[test]
    fn callbacks_fit_limit() {
        let sid = "abcdefgh";
        for markup in [
            preview_keyboard(sid),
            video_quality_keyboard(&meta(), sid),
            audio_quality_keyboard(&meta(), sid),
            cancel_keyboard(sid),
        ] {
            for row in markup.inline_keyboard {
                for btn in row {
                    if let teloxide::types::InlineKeyboardButtonKind::CallbackData(data) = &btn.kind
                    {
                        assert!(data.len() <= 64, "{data}");
                    }
                }
            }
        }
    }

    #[test]
    fn bytes_format() {
        assert_eq!(format_bytes(42_000_000), "42 MB");
        assert_eq!(format_bytes(512), "512 B");
    }

    #[test]
    fn bar_renders() {
        assert_eq!(
            progress_bar("⬇️ Downloading", 80),
            "⬇️ Downloading ████████░░ 80%"
        );
    }
}
