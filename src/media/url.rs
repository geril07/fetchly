use crate::error::{Error, Result};

/// Platform detected from a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Youtube,
    Tiktok,
    Instagram,
    Twitter,
}

impl Platform {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Youtube => "YouTube",
            Self::Tiktok => "TikTok",
            Self::Instagram => "Instagram",
            Self::Twitter => "X",
        }
    }
}

/// A URL that passed platform detection, ready for yt-dlp.
#[derive(Debug, Clone)]
pub struct MediaUrl {
    pub platform: Platform,
    /// Normalized URL string (scheme + host lowercased, tracking params stripped).
    pub url: String,
}

/// Detect platform from host. Returns `None` for unsupported hosts.
fn detect_platform(host: &str) -> Option<Platform> {
    let h = host.strip_prefix("www.").unwrap_or(host);
    match h {
        "youtube.com"
        | "youtu.be"
        | "music.youtube.com"
        | "m.youtube.com"
        | "youtube-nocookie.com" => Some(Platform::Youtube),
        h if h == "tiktok.com"
            || h.ends_with(".tiktok.com")
            || h == "vm.tiktok.com"
            || h == "vt.tiktok.com" =>
        {
            Some(Platform::Tiktok)
        }
        h if h == "instagram.com" || h.ends_with(".instagram.com") => Some(Platform::Instagram),
        h if h == "x.com"
            || h.ends_with(".x.com")
            || h == "twitter.com"
            || h.ends_with(".twitter.com")
            || h == "vxtwitter.com"
            || h == "fixvx.com" =>
        {
            Some(Platform::Twitter)
        }
        _ => None,
    }
}

/// Parse + normalize a user-supplied URL.
///
/// Strips common tracking params (`si`, `utm_*`, `igsh`, `fbclid`, …) so the
/// same content maps to the same cache key regardless of share-link noise.
pub fn parse(input: &str) -> Result<MediaUrl> {
    let input = input.trim();
    let mut parsed = url::Url::parse(input).map_err(|_| Error::UnsupportedUrl)?;

    // Accept bare `youtu.be/…` etc. pasted without scheme.
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(Error::UnsupportedUrl);
    }

    let host = parsed
        .host_str()
        .ok_or(Error::UnsupportedUrl)?
        .to_lowercase();
    let platform = detect_platform(&host).ok_or(Error::UnsupportedUrl)?;

    // Normalize host case.
    let _ = parsed.set_host(Some(&host));

    // Strip tracking query params (keep the rest, e.g. `v=`, `t=`).
    {
        let drop: Vec<String> = parsed
            .query_pairs()
            .filter_map(|(k, _)| {
                let k = k.as_ref();
                if k == "si"
                    || k == "igsh"
                    || k == "igshid"
                    || k == "fbclid"
                    || k == "gclid"
                    || k.starts_with("utm_")
                {
                    Some(k.to_owned())
                } else {
                    None
                }
            })
            .collect();
        if !drop.is_empty() {
            let keep: Vec<(String, String)> = parsed
                .query_pairs()
                .filter(|(k, _)| !drop.iter().any(|d| d == k.as_ref()))
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            parsed.query_pairs_mut().clear();
            if !keep.is_empty() {
                parsed.query_pairs_mut().extend_pairs(keep);
            }
        }
    }

    Ok(MediaUrl {
        platform,
        url: parsed.to_string(),
    })
}

/// Stable cache key for a URL: lowercase hex SHA-256.
#[must_use]
pub fn url_hash(url: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(url.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_all_platforms() {
        let cases = [
            (
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                Platform::Youtube,
            ),
            ("https://youtu.be/dQw4w9WgXcQ", Platform::Youtube),
            ("https://music.youtube.com/watch?v=abc", Platform::Youtube),
            ("https://www.tiktok.com/@user/video/123", Platform::Tiktok),
            ("https://vm.tiktok.com/abc123/", Platform::Tiktok),
            ("https://www.instagram.com/reel/abc/", Platform::Instagram),
            ("https://x.com/user/status/123", Platform::Twitter),
            ("https://twitter.com/user/status/123", Platform::Twitter),
        ];
        for (input, want) in cases {
            assert_eq!(parse(input).expect("parses").platform, want, "{input}");
        }
    }

    #[test]
    fn rejects_unsupported() {
        for input in [
            "not a url",
            "https://example.com/video",
            "ftp://youtube.com/watch?v=1",
        ] {
            assert!(parse(input).is_err(), "{input}");
        }
    }

    #[test]
    fn strips_tracking_params() {
        let m = parse("https://youtu.be/abc?si=XYZ&utm_source=share").expect("parses");
        assert!(!m.url.contains("si="), "{}", m.url);
        assert!(!m.url.contains("utm_"), "{}", m.url);
        let m = parse("https://www.youtube.com/watch?v=abc&t=42s&si=XYZ").expect("parses");
        assert!(m.url.contains("v=abc"), "{}", m.url);
        assert!(m.url.contains("t=42s"), "{}", m.url);
    }

    #[test]
    fn hash_is_stable_hex() {
        assert_eq!(url_hash("a"), url_hash("a"));
        assert_ne!(url_hash("a"), url_hash("b"));
        assert_eq!(url_hash("a").len(), 64);
    }
}
