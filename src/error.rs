/// Unified error type for the whole bot.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("unsupported link. Send a YouTube, TikTok, Instagram, or X URL.")]
    UnsupportedUrl,

    #[error("failed to resolve URL: {0}")]
    Resolve(String),

    #[error("this content is private or restricted.")]
    PrivateOrRestricted,

    #[error("download failed. Try again later. ({0})")]
    Download(String),

    #[error("conversion failed. Try again later. ({0})")]
    Convert(String),

    #[error("file exceeds 2 GB limit. Try lower quality.")]
    TooLarge,

    #[error("telegram error: {0}")]
    Telegram(String),

    #[error("session expired. Send the link again.")]
    SessionExpired,

    #[error("rate limited")]
    RateLimited { retry_in_secs: u64, remaining: u32 },

    #[error("cache error: {0}")]
    Cache(String),

    #[error("session store error: {0}")]
    Session(String),

    #[error("rate limiter error: {0}")]
    Limiter(String),

    #[error("cancelled")]
    Cancelled,

    #[error("download timed out after {minutes} minutes")]
    TimedOut { minutes: u64 },

    #[error("too many concurrent downloads (max {max})")]
    TooManyConcurrent { max: usize },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    /// User-facing message (safe to send to chat).
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            // Explicit sentence-case copy (matches docs/v1-spec.md).
            // The `Display` strings stay lowercase for log lines.
            Self::UnsupportedUrl => {
                "Unsupported link. Send a YouTube, TikTok, Instagram, or X URL.".to_owned()
            }
            Self::PrivateOrRestricted => "This content is private or restricted.".to_owned(),
            Self::TooLarge => "File exceeds 2 GB limit. Try lower quality.".to_owned(),
            Self::SessionExpired => "Session expired. Send the link again.".to_owned(),
            Self::Cancelled => "Cancelled.".to_owned(),
            Self::TimedOut { minutes } => {
                format!(
                    "Download timed out after {minutes} minutes. Try again or pick a lower quality."
                )
            }
            Self::TooManyConcurrent { max } => {
                format!(
                    "You already have {max} downloads running. Wait for one to finish, then try again."
                )
            }
            Self::RateLimited {
                retry_in_secs,
                remaining: _,
            } => {
                format!("Too many requests. Try again in {retry_in_secs} seconds.")
            }
            Self::Download(_) | Self::Convert(_) => "Download failed. Try again later.".to_owned(),
            Self::Resolve(_) => "Could not fetch this link. Try again later.".to_owned(),
            _ => "Something went wrong. Try again later.".to_owned(),
        }
    }
}

impl From<teloxide::RequestError> for Error {
    fn from(e: teloxide::RequestError) -> Self {
        Self::Telegram(e.to_string())
    }
}

impl From<redis::RedisError> for Error {
    fn from(e: redis::RedisError) -> Self {
        // Callers map to Session/Limiter/Cache as appropriate; default to session.
        Self::Session(e.to_string())
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Cache(e.to_string())
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_messages_are_safe_to_send() {
        // Raw tool/DB details must never leak into chat.
        let cases: Vec<(Error, &str)> = vec![
            (
                Error::UnsupportedUrl,
                "Unsupported link. Send a YouTube, TikTok, Instagram, or X URL.",
            ),
            (
                Error::PrivateOrRestricted,
                "This content is private or restricted.",
            ),
            (
                Error::TooLarge,
                "File exceeds 2 GB limit. Try lower quality.",
            ),
            (
                Error::SessionExpired,
                "Session expired. Send the link again.",
            ),
            (Error::Cancelled, "Cancelled."),
            (
                Error::TimedOut { minutes: 15 },
                "Download timed out after 15 minutes. Try again or pick a lower quality.",
            ),
            (
                Error::TooManyConcurrent { max: 2 },
                "You already have 2 downloads running. Wait for one to finish, then try again.",
            ),
            (
                Error::RateLimited {
                    retry_in_secs: 42,
                    remaining: 0,
                },
                "Too many requests. Try again in 42 seconds.",
            ),
            (
                Error::Download("yt-dlp: exit 1, SIGNAL 9".to_owned()),
                "Download failed. Try again later.",
            ),
            (
                Error::Convert("ffmpeg: invalid data".to_owned()),
                "Download failed. Try again later.",
            ),
            (
                Error::Resolve("socket timeout".to_owned()),
                "Could not fetch this link. Try again later.",
            ),
            (
                Error::Telegram("Bad Request: file too big".to_owned()),
                "Something went wrong. Try again later.",
            ),
            (
                Error::Cache("sqlite: disk I/O error".to_owned()),
                "Something went wrong. Try again later.",
            ),
        ];
        for (err, want) in cases {
            assert_eq!(err.user_message(), want, "{err:?}");
        }
    }
}
