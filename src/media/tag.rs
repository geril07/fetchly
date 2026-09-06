use std::path::Path;

use crate::error::{Error, Result};

/// Info written into the `MP3` (`ID3v2`) and shown by Telegram as track metadata.
#[derive(Debug, Clone, Default)]
pub struct TagInfo {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<u32>,
    pub cover_bytes: Option<Vec<u8>>,
    pub cover_mime: Option<String>,
}

/// Fetch thumbnail bytes for cover art. Returns `None` on any failure
/// (cover art is best-effort; the MP3 is still valid without it).
pub async fn fetch_cover(url: &str) -> Option<(Vec<u8>, Option<String>)> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let mime = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = resp.bytes().await.ok()?.to_vec();
    if bytes.is_empty() || bytes.len() > 10_000_000 {
        return None;
    }
    Some((bytes, mime))
}

/// Write `ID3v2` tags + embedded cover art into an `MP3` file.
///
/// Runs on a blocking thread (`lofty` is synchronous).
pub async fn tag_mp3(path: &Path, info: TagInfo) -> Result<()> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || tag_mp3_blocking(&path, &info))
        .await
        .map_err(|e| Error::Convert(format!("tagging task failed: {e}")))?
}

/// Synchronous lofty implementation.
fn tag_mp3_blocking(path: &Path, info: &TagInfo) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::probe::Probe;
    use lofty::tag::{Accessor, Tag, TagType};

    let mut tagged = Probe::open(path)
        .and_then(Probe::read)
        .map_err(|e| Error::Convert(format!("lofty read failed: {e}")))?;

    // Ensure an ID3v2 tag exists.
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(TagType::Id3v2));
    }
    let tag = tagged
        .primary_tag_mut()
        .ok_or_else(|| Error::Convert("lofty: no primary tag".to_owned()))?;
    // Normalize to ID3v2 so Telegram/music players see it.
    if tag.tag_type() != TagType::Id3v2 {
        *tag = Tag::new(TagType::Id3v2);
    }

    tag.set_title(info.title.clone());
    if let Some(artist) = &info.artist {
        tag.set_artist(artist.clone());
    }
    if let Some(album) = &info.album {
        tag.set_album(album.clone());
    }
    // `year` is kept in `TagInfo` for future use; lofty 0.25 exposes dates
    // via `Accessor::set_date(Timestamp)`, which needs a full date — the
    // bare year from yt-dlp metadata is not enough to build one reliably,
    // so we intentionally leave the date tag untouched.
    let _ = info.year;

    if let Some(bytes) = &info.cover_bytes {
        let mime_type = match info.cover_mime.as_deref() {
            Some(m) if m.contains("png") => MimeType::Png,
            Some(m) if m.contains("jpeg") || m.contains("jpg") => MimeType::Jpeg,
            _ => MimeType::Jpeg,
        };
        let picture = Picture::unchecked(bytes.clone())
            .mime_type(mime_type)
            .pic_type(PictureType::CoverFront)
            .build();
        tag.push_picture(picture);
    }

    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(|e| Error::Convert(format!("lofty write failed: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_info_default_is_empty() {
        let info = TagInfo::default();
        assert!(info.artist.is_none());
        assert!(info.cover_bytes.is_none());
    }
}
