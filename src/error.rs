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
    /// User-facing message (safe to send to chat) in the user's language.
    #[must_use]
    pub fn user_message(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n as t;
        match self {
            // Explicit sentence-case copy (matches docs/v1-spec.md).
            // The `Display` strings stay lowercase for log lines.
            Self::UnsupportedUrl => t::error_unsupported(lang).to_owned(),
            Self::PrivateOrRestricted => t::error_private(lang).to_owned(),
            Self::TooLarge => t::error_too_large(lang).to_owned(),
            Self::SessionExpired => t::session_expired(lang).to_owned(),
            Self::Cancelled => t::cancelled(lang).to_owned(),
            Self::TimedOut { minutes } => t::error_timed_out(lang, *minutes),
            Self::TooManyConcurrent { max } => t::error_too_many_concurrent(lang, *max),
            Self::RateLimited {
                retry_in_secs,
                remaining: _,
            } => t::rate_limited(lang, *retry_in_secs),
            Self::Download(_) | Self::Convert(_) => t::error_download(lang).to_owned(),
            Self::Resolve(_) => t::error_resolve(lang).to_owned(),
            _ => t::error_generic(lang).to_owned(),
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
    use crate::i18n::Lang;

    #[test]
    fn user_messages_are_safe_to_send() {
        // Raw tool/DB details must never leak into chat.
        let cases: Vec<(Error, &str, &str)> = vec![
            (
                Error::UnsupportedUrl,
                "Unsupported link. Send a YouTube, TikTok, Instagram, or X URL.",
                "Неподдерживаемая ссылка. Пришли URL YouTube, TikTok, Instagram или X.",
            ),
            (
                Error::PrivateOrRestricted,
                "This content is private or restricted.",
                "Этот контент приватный или недоступен.",
            ),
            (
                Error::TooLarge,
                "File exceeds 2 GB limit. Try lower quality.",
                "Файл больше лимита 2 ГБ. Попробуй качество пониже.",
            ),
            (
                Error::SessionExpired,
                "Session expired. Send the link again.",
                "Сессия истекла. Пришли ссылку ещё раз.",
            ),
            (Error::Cancelled, "Cancelled.", "Отменено."),
            (
                Error::TimedOut { minutes: 15 },
                "Download timed out after 15 minutes. Try again or pick a lower quality.",
                "Загрузка заняла больше 15 мин. Попробуй ещё раз или выбери качество пониже.",
            ),
            (
                Error::TooManyConcurrent { max: 2 },
                "You already have 2 downloads running. Wait for one to finish, then try again.",
                "У тебя уже 2 активных загрузки. Дождись завершения и попробуй ещё раз.",
            ),
            (
                Error::RateLimited {
                    retry_in_secs: 42,
                    remaining: 0,
                },
                "Too many requests. Try again in 42 seconds.",
                "Слишком много запросов. Попробуй через 42 с.",
            ),
            (
                Error::Download("yt-dlp: exit 1, SIGNAL 9".to_owned()),
                "Download failed. Try again later.",
                "Не получилось скачать. Попробуй позже.",
            ),
            (
                Error::Convert("ffmpeg: invalid data".to_owned()),
                "Download failed. Try again later.",
                "Не получилось скачать. Попробуй позже.",
            ),
            (
                Error::Resolve("socket timeout".to_owned()),
                "Could not fetch this link. Try again later.",
                "Не получилось открыть ссылку. Попробуй позже.",
            ),
            (
                Error::Telegram("Bad Request: file too big".to_owned()),
                "Something went wrong. Try again later.",
                "Что-то пошло не так. Попробуй позже.",
            ),
            (
                Error::Cache("sqlite: disk I/O error".to_owned()),
                "Something went wrong. Try again later.",
                "Что-то пошло не так. Попробуй позже.",
            ),
        ];
        for (err, want_en, want_ru) in cases {
            assert_eq!(err.user_message(Lang::En), want_en, "{err:?}");
            assert_eq!(err.user_message(Lang::Ru), want_ru, "{err:?}");
        }
    }
}
