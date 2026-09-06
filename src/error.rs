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

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    /// User-facing message (safe to send to chat).
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::UnsupportedUrl
            | Self::PrivateOrRestricted
            | Self::TooLarge
            | Self::SessionExpired => self.to_string(),
            Self::Cancelled => "Cancelled.".to_owned(),
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
