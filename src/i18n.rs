//! User-facing strings in English and Russian.
//!
//! Every chat message, button label, and callback answer goes through here.
//! Log lines and tool/DB details stay English and never reach [`Lang`].
//!
//! Russian copy avoids plural inflection where possible
//! (`Загрузок за час: X/Y` instead of inflected nouns). Numbers, units
//! (`MB`, `720p`, `320 kbps`), and duration ticks (`45s`, `2m 5s`) stay
//! untranslated — the surrounding labels carry the locale.

/// Chat language. Explicit `/language` choice beats this; Telegram's
/// `language_code` is only the first-run guess (see `resolve` users).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Ru,
}

impl Lang {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ru => "ru",
        }
    }

    /// Parse a stored/callback code. Unknown codes are `None` (caller falls back).
    #[must_use]
    pub fn from_code(s: &str) -> Option<Self> {
        match s {
            "en" => Some(Self::En),
            "ru" => Some(Self::Ru),
            _ => None,
        }
    }

    /// First-run guess from Telegram's `language_code` (`ru`, `ru-RU`, `uk`, …).
    /// Prefix match: anything starting with `ru` is Russian, the rest is English.
    #[must_use]
    pub fn from_telegram_code(code: Option<&str>) -> Self {
        match code {
            Some(c) if c.to_lowercase().starts_with("ru") => Self::Ru,
            _ => Self::En,
        }
    }
}

#[must_use]
pub fn welcome(lang: Lang) -> &'static str {
    match lang {
        Lang::En => {
            "Send me a YouTube, TikTok, Instagram, or X link and I'll fetch the video or audio for you."
        }
        Lang::Ru => {
            "Пришли мне ссылку на YouTube, TikTok, Instagram или X — верну видео или аудио."
        }
    }
}

#[must_use]
pub fn help(lang: Lang, rate_limit: u32, max_per_user: usize, upload: &str) -> String {
    match lang {
        Lang::En => format!(
            "Send a link → tap 🎬 Video or 🎵 Audio → pick quality.\n\nCommands:\n/start — welcome\n/help — this guide\n/usage — your current usage\n/language — interface language\n\nLimits: {rate_limit} downloads/hour, {max_per_user} at a time, {upload} max file size."
        ),
        Lang::Ru => format!(
            "Пришли ссылку → нажми 🎬 Видео или 🎵 Аудио → выбери качество.\n\nКоманды:\n/start — приветствие\n/help — эта справка\n/usage — твоё использование\n/language — язык интерфейса\n\nЛимиты: {rate_limit} загрузок/час, {max_per_user} одновременно, макс. размер {upload}."
        ),
    }
}

#[must_use]
pub fn usage_title(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "📊 Usage",
        Lang::Ru => "📊 Использование",
    }
}

pub struct UsageView<'a> {
    pub used: u32,
    pub limit: u32,
    pub left: u32,
    pub reset: &'a str,
    pub slots: usize,
    pub max_slots: usize,
    pub upload: &'a str,
}

#[must_use]
pub fn usage_body(lang: Lang, v: &UsageView<'_>) -> String {
    match lang {
        Lang::En => format!(
            "{}\n\nDownloads this hour: {used}/{limit} used ({left} left)\nReset: {reset}\nConcurrent downloads: {slots}/{max_slots}\nMax file size: {upload}",
            usage_title(lang),
            used = v.used,
            limit = v.limit,
            left = v.left,
            reset = v.reset,
            slots = v.slots,
            max_slots = v.max_slots,
            upload = v.upload,
        ),
        Lang::Ru => format!(
            "{}\n\nЗагрузок за час: {used}/{limit} (осталось {left})\nСброс: {reset}\nОдновременных загрузок: {slots}/{max_slots}\nМакс. размер: {upload}",
            usage_title(lang),
            used = v.used,
            limit = v.limit,
            left = v.left,
            reset = v.reset,
            slots = v.slots,
            max_slots = v.max_slots,
            upload = v.upload,
        ),
    }
}

#[must_use]
pub fn reset_full(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "full quota",
        Lang::Ru => "лимит полон",
    }
}

#[must_use]
pub fn usage_unavailable(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Could not load usage. Try again later.",
        Lang::Ru => "Не получилось загрузить использование. Попробуй позже.",
    }
}

#[must_use]
pub fn usage_no_sender(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Can't tell who sent that. Usage is available from your own account.",
        Lang::Ru => "Не вижу, кто это прислал. Использование доступно только со своего аккаунта.",
    }
}

#[must_use]
pub fn send_link_hint(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Send a link and I'll fetch it. /help for details.",
        Lang::Ru => "Пришли ссылку, и я её скачаю. Подробности — /help.",
    }
}

#[must_use]
pub fn resolving(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "🔍 Resolving link…",
        Lang::Ru => "🔍 Открываю ссылку…",
    }
}

#[must_use]
pub fn outdated_button(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Outdated button. Send the link again.",
        Lang::Ru => "Кнопка устарела. Пришли ссылку ещё раз.",
    }
}

#[must_use]
pub fn cancelling(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Cancelling…",
        Lang::Ru => "Отменяю…",
    }
}

#[must_use]
pub fn nothing_to_cancel(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Nothing to cancel.",
        Lang::Ru => "Нечего отменять.",
    }
}

#[must_use]
pub fn cancelled(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Cancelled.",
        Lang::Ru => "Отменено.",
    }
}

#[must_use]
pub fn session_expired(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Session expired. Send the link again.",
        Lang::Ru => "Сессия истекла. Пришли ссылку ещё раз.",
    }
}

#[must_use]
pub fn unknown_quality(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Unknown quality.",
        Lang::Ru => "Неизвестное качество.",
    }
}

#[must_use]
pub fn queued(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "⏳ Queued… your download starts automatically.",
        Lang::Ru => "⏳ В очереди… загрузка начнётся автоматически.",
    }
}

#[must_use]
pub fn busy(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "🔥 Fetchly is busy right now. Try again in a bit.",
        Lang::Ru => "🔥 Сейчас много загрузок. Попробуй чуть позже.",
    }
}

#[must_use]
pub fn restart(lang: Lang) -> &'static str {
    match lang {
        Lang::En => {
            "🔄 Fetchly is restarting. Your download was stopped — please resend your link in a minute."
        }
        Lang::Ru => {
            "🔄 Fetchly перезапускается. Загрузка остановлена — пришли ссылку ещё раз через минуту."
        }
    }
}

#[must_use]
pub fn flight_wait(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "⏳ Already downloading — I'll send it here when it's ready.",
        Lang::Ru => "⏳ Уже качаю — пришлю сюда, как будет готово.",
    }
}

#[must_use]
pub fn flight_retry(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "The download didn't start. Please tap the quality again.",
        Lang::Ru => "Загрузка не началась. Нажми на качество ещё раз.",
    }
}

#[must_use]
pub fn flight_cancelled(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "The download was cancelled.",
        Lang::Ru => "Загрузка была отменена.",
    }
}

#[must_use]
pub fn flight_failed(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Something went wrong. Try again later.",
        Lang::Ru => "Что-то пошло не так. Попробуй позже.",
    }
}

#[must_use]
pub fn downloading_zero(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "⬇️ Downloading ░░░░░░░░░░ 0%",
        Lang::Ru => "⬇️ Загрузка ░░░░░░░░░░ 0%",
    }
}

#[must_use]
pub fn downloading_prefix(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "⬇️ Downloading",
        Lang::Ru => "⬇️ Загрузка",
    }
}

#[must_use]
pub fn converting(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "🔄 Converting…",
        Lang::Ru => "🔄 Конвертация…",
    }
}

#[must_use]
pub fn uploading(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "🔄 Uploading…",
        Lang::Ru => "🔄 Загрузка в Telegram…",
    }
}

#[must_use]
pub fn uploading_audio(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "⬆️ Uploading…",
        Lang::Ru => "⬆️ Загрузка в Telegram…",
    }
}

/// File over the 50 MB standard-API cap without a Local Bot API Server.
#[must_use]
pub fn over_limit(lang: Lang) -> &'static str {
    match lang {
        Lang::En => {
            "This file is over 50 MB, which exceeds the standard Bot API upload limit. Pick a lower quality, or set up a Local Bot API Server for files up to 2 GB."
        }
        Lang::Ru => {
            "Файл больше 50 МБ — это лимит стандартного Bot API. Выбери качество пониже или подними Local Bot API Server для файлов до 2 ГБ."
        }
    }
}

#[must_use]
pub fn rate_limited(lang: Lang, retry_in_secs: u64) -> String {
    match lang {
        Lang::En => format!("Too many requests. Try again in {retry_in_secs} seconds."),
        Lang::Ru => format!("Слишком много запросов. Попробуй через {retry_in_secs} с."),
    }
}

#[must_use]
pub fn video_button(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "🎬 Video",
        Lang::Ru => "🎬 Видео",
    }
}

#[must_use]
pub fn audio_button(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "🎵 Audio",
        Lang::Ru => "🎵 Аудио",
    }
}

#[must_use]
pub fn cancel_button(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "❌ Cancel",
        Lang::Ru => "❌ Отмена",
    }
}

#[must_use]
pub fn best_label(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Best",
        Lang::Ru => "Лучшее",
    }
}

#[must_use]
pub fn live_unknown(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "live/unknown",
        Lang::Ru => "эфир/неизвестно",
    }
}

/// `/language` picker prompt (bilingual on purpose: the user has no lang yet).
#[must_use]
pub fn language_prompt() -> &'static str {
    "Choose language / Выбери язык:"
}

/// `/language` confirmation, in the language just chosen.
#[must_use]
pub fn language_set(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Language: English. I'll reply in English from now on.",
        Lang::Ru => "Язык: русский. Дальше буду отвечать по-русски.",
    }
}

/// View-count suffix with Russian plural rules.
#[must_use]
pub fn views(lang: Lang, n: u64) -> String {
    match lang {
        Lang::En => {
            if n >= 1_000_000 {
                format!("{}.{}M views", n / 1_000_000, (n % 1_000_000) / 100_000)
            } else if n >= 1_000 {
                format!("{}.{}K views", n / 1_000, (n % 1_000) / 100)
            } else {
                format!("{n} views")
            }
        }
        Lang::Ru => {
            let word = match n % 10 {
                1 if n % 100 != 11 => "просмотр",
                2..=4 if !(12..=14).contains(&(n % 100)) => "просмотра",
                _ => "просмотров",
            };
            if n >= 1_000_000 {
                format!("{}.{} млн {word}", n / 1_000_000, (n % 1_000_000) / 100_000)
            } else if n >= 1_000 {
                format!("{}.{} тыс. {word}", n / 1_000, (n % 1_000) / 100)
            } else {
                format!("{n} {word}")
            }
        }
    }
}

#[must_use]
pub fn error_unsupported(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Unsupported link. Send a YouTube, TikTok, Instagram, or X URL.",
        Lang::Ru => "Неподдерживаемая ссылка. Пришли URL YouTube, TikTok, Instagram или X.",
    }
}

#[must_use]
pub fn error_private(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "This content is private or restricted.",
        Lang::Ru => "Этот контент приватный или недоступен.",
    }
}

#[must_use]
pub fn error_too_large(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "File exceeds 2 GB limit. Try lower quality.",
        Lang::Ru => "Файл больше лимита 2 ГБ. Попробуй качество пониже.",
    }
}

#[must_use]
pub fn error_download(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Download failed. Try again later.",
        Lang::Ru => "Не получилось скачать. Попробуй позже.",
    }
}

#[must_use]
pub fn error_resolve(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Could not fetch this link. Try again later.",
        Lang::Ru => "Не получилось открыть ссылку. Попробуй позже.",
    }
}

#[must_use]
pub fn error_timed_out(lang: Lang, minutes: u64) -> String {
    match lang {
        Lang::En => {
            format!(
                "Download timed out after {minutes} minutes. Try again or pick a lower quality."
            )
        }
        Lang::Ru => {
            format!(
                "Загрузка заняла больше {minutes} мин. Попробуй ещё раз или выбери качество пониже."
            )
        }
    }
}

#[must_use]
pub fn error_too_many_concurrent(lang: Lang, max: usize) -> String {
    match lang {
        Lang::En => format!(
            "You already have {max} downloads running. Wait for one to finish, then try again."
        ),
        Lang::Ru => {
            format!("У тебя уже {max} активных загрузки. Дождись завершения и попробуй ещё раз.")
        }
    }
}

#[must_use]
pub fn error_generic(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Something went wrong. Try again later.",
        Lang::Ru => "Что-то пошло не так. Попробуй позже.",
    }
}

/// Bot profile description (`setMyDescription`, ≤512 chars).
#[must_use]
pub fn bot_description(lang: Lang) -> &'static str {
    match lang {
        Lang::En => {
            "Send me a YouTube, TikTok, Instagram, or X link and I'll fetch the video or audio for you."
        }
        Lang::Ru => {
            "Пришли мне ссылку на YouTube, TikTok, Instagram или X — верну видео или аудио."
        }
    }
}

/// Bot profile short description (`setMyShortDescription`, ≤120 chars).
#[must_use]
pub fn bot_short_description(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Send a link, get video or audio back. YouTube, TikTok, Instagram, X.",
        Lang::Ru => "Ссылка → видео или аудио. YouTube, TikTok, Instagram, X.",
    }
}

/// Menu (`setMyCommands`) description for `/start` (3–256 chars).
#[must_use]
pub fn cmd_start_desc(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Start the bot and show welcome.",
        Lang::Ru => "Запустить бота и показать приветствие.",
    }
}

/// Menu (`setMyCommands`) description for `/help` (3–256 chars).
#[must_use]
pub fn cmd_help_desc(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Show usage guide and limits.",
        Lang::Ru => "Показать справку и лимиты.",
    }
}

/// Menu (`setMyCommands`) description for `/usage` (3–256 chars).
#[must_use]
pub fn cmd_usage_desc(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Show your current usage and limits.",
        Lang::Ru => "Показать использование и лимиты.",
    }
}

/// Menu (`setMyCommands`) description for `/language` (3–256 chars).
#[must_use]
pub fn cmd_language_desc(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Change interface language.",
        Lang::Ru => "Сменить язык интерфейса.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_code_prefix_match() {
        assert_eq!(Lang::from_telegram_code(None), Lang::En);
        assert_eq!(Lang::from_telegram_code(Some("en")), Lang::En);
        assert_eq!(Lang::from_telegram_code(Some("en-US")), Lang::En);
        assert_eq!(Lang::from_telegram_code(Some("ru")), Lang::Ru);
        assert_eq!(Lang::from_telegram_code(Some("ru-RU")), Lang::Ru);
        assert_eq!(Lang::from_telegram_code(Some("RU")), Lang::Ru);
        assert_eq!(Lang::from_telegram_code(Some("uk")), Lang::En);
    }

    #[test]
    fn codes_roundtrip() {
        assert_eq!(Lang::from_code("en"), Some(Lang::En));
        assert_eq!(Lang::from_code("ru"), Some(Lang::Ru));
        assert_eq!(Lang::from_code("de"), None);
        assert_eq!(Lang::En.as_str(), "en");
        assert_eq!(Lang::Ru.as_str(), "ru");
    }

    #[test]
    fn russian_view_plurals() {
        assert_eq!(views(Lang::Ru, 1), "1 просмотр");
        assert_eq!(views(Lang::Ru, 2), "2 просмотра");
        assert_eq!(views(Lang::Ru, 5), "5 просмотров");
        assert_eq!(views(Lang::Ru, 11), "11 просмотров");
        assert_eq!(views(Lang::Ru, 12), "12 просмотров");
        assert_eq!(views(Lang::Ru, 21), "21 просмотр");
        assert_eq!(views(Lang::Ru, 111), "111 просмотров");
        assert_eq!(views(Lang::Ru, 1_500_000), "1.5 млн просмотров");
    }

    #[test]
    fn english_views_unchanged() {
        assert_eq!(views(Lang::En, 999), "999 views");
        assert_eq!(views(Lang::En, 1_000), "1.0K views");
        assert_eq!(views(Lang::En, 1_500_000), "1.5M views");
    }

    #[test]
    fn both_langs_cover_every_key() {
        let cfgs = [(20u32, 2usize, "50 MB"), (5u32, 1usize, "2 GB")];
        for (rate, per_user, upload) in cfgs {
            for lang in [Lang::En, Lang::Ru] {
                let view = UsageView {
                    used: 1,
                    limit: rate,
                    left: 19,
                    reset: "x",
                    slots: 0,
                    max_slots: per_user,
                    upload,
                };
                assert!(!welcome(lang).is_empty());
                assert!(!help(lang, rate, per_user, upload).is_empty());
                assert!(!usage_body(lang, &view).is_empty());
                assert!(!reset_full(lang).is_empty());
                assert!(!usage_unavailable(lang).is_empty());
                assert!(!usage_no_sender(lang).is_empty());
                assert!(!send_link_hint(lang).is_empty());
                assert!(!resolving(lang).is_empty());
                assert!(!outdated_button(lang).is_empty());
                assert!(!cancelling(lang).is_empty());
                assert!(!nothing_to_cancel(lang).is_empty());
                assert!(!cancelled(lang).is_empty());
                assert!(!session_expired(lang).is_empty());
                assert!(!unknown_quality(lang).is_empty());
                assert!(!queued(lang).is_empty());
                assert!(!busy(lang).is_empty());
                assert!(!restart(lang).is_empty());
                assert!(!flight_wait(lang).is_empty());
                assert!(!flight_retry(lang).is_empty());
                assert!(!flight_cancelled(lang).is_empty());
                assert!(!flight_failed(lang).is_empty());
                assert!(!downloading_zero(lang).is_empty());
                assert!(!downloading_prefix(lang).is_empty());
                assert!(!converting(lang).is_empty());
                assert!(!uploading(lang).is_empty());
                assert!(!uploading_audio(lang).is_empty());
                assert!(!over_limit(lang).is_empty());
                assert!(!rate_limited(lang, 42).is_empty());
                assert!(!video_button(lang).is_empty());
                assert!(!audio_button(lang).is_empty());
                assert!(!cancel_button(lang).is_empty());
                assert!(!best_label(lang).is_empty());
                assert!(!live_unknown(lang).is_empty());
                assert!(!language_set(lang).is_empty());
                assert!(!error_unsupported(lang).is_empty());
                assert!(!error_private(lang).is_empty());
                assert!(!error_too_large(lang).is_empty());
                assert!(!error_download(lang).is_empty());
                assert!(!error_resolve(lang).is_empty());
                assert!(!error_timed_out(lang, 15).is_empty());
                assert!(!error_too_many_concurrent(lang, 2).is_empty());
                assert!(!error_generic(lang).is_empty());
                assert!(!bot_description(lang).is_empty());
                assert!(!bot_short_description(lang).is_empty());
                assert!(!cmd_start_desc(lang).is_empty());
                assert!(!cmd_help_desc(lang).is_empty());
                assert!(!cmd_usage_desc(lang).is_empty());
                assert!(!cmd_language_desc(lang).is_empty());
            }
        }
        assert!(!language_prompt().is_empty());
    }

    #[test]
    fn profile_texts_fit_telegram_limits() {
        // Telegram counts characters, not bytes — Cyrillic is 2 bytes in UTF-8.
        for lang in [Lang::En, Lang::Ru] {
            let desc = bot_description(lang);
            assert!(!desc.is_empty() && desc.chars().count() <= 512, "{desc}");
            let short = bot_short_description(lang);
            assert!(!short.is_empty() && short.chars().count() <= 120, "{short}");
            for cmd in [
                cmd_start_desc(lang),
                cmd_help_desc(lang),
                cmd_usage_desc(lang),
                cmd_language_desc(lang),
            ] {
                let n = cmd.chars().count();
                assert!((3..=256).contains(&n), "{cmd}");
            }
        }
    }
}
