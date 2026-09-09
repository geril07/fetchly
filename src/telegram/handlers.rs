use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use teloxide::prelude::*;
use teloxide::types::{
    BotCommandScope, CallbackQuery, ChatId, InlineKeyboardMarkup, InputFile, Message, MessageId,
    ParseMode,
};
use teloxide::utils::command::BotCommands;
use tokio_util::sync::CancellationToken;

use crate::cache::FileCache;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::i18n::{self, Lang};
use crate::limiter::RateLimiter;
use crate::media::url;
use crate::media::ytdlp::{self, AudioQuality, VideoQuality};
use crate::prefs::UserPrefs;
use crate::session::{SessionStore, StoredSession};
use crate::telegram::keyboard::{self, format_bytes};

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub cache: FileCache,
    pub prefs: UserPrefs,
    pub sessions: SessionStore,
    pub limiter: RateLimiter,
    pub semaphore: Arc<tokio::sync::Semaphore>,
    /// `session_id` → cancel token (wired to ❌ button).
    pub downloads: Arc<tokio::sync::Mutex<HashMap<String, CancellationToken>>>,
    /// Registered before semaphore acquisition so parked waiters get shutdown notices; removed on every pipeline exit.
    pub notify: Arc<tokio::sync::Mutex<HashMap<String, ChatId>>>,
    pub user_slots: Arc<tokio::sync::Mutex<HashMap<u64, usize>>>,
    /// Parked waiters + handoff; decremented right after a permit is granted.
    pub queued: Arc<AtomicUsize>,
    pub flights: Flights,
    pub http: reqwest::Client,
}

impl AppState {
    #[must_use]
    pub fn new(
        config: Config,
        cache: FileCache,
        prefs: UserPrefs,
        sessions: SessionStore,
        limiter: RateLimiter,
        semaphore: Arc<tokio::sync::Semaphore>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            config,
            cache,
            prefs,
            sessions,
            limiter,
            semaphore,
            downloads: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            notify: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            user_slots: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            queued: Arc::new(AtomicUsize::new(0)),
            flights: Flights::default(),
            http,
        }
    }
}

/// Same URL + format + quality tapped twice concurrently downloads once; latecomers attach.
pub type FlightKey = (String, String, String);

/// First tapper (leader) runs the pipeline; waiters get the finished `file_id`. Pure in-memory, no Redis.
/// Waiters carry their own language so fan-out matches their locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlightWaiter {
    pub chat: ChatId,
    pub lang: Lang,
}

#[derive(Clone, Default)]
pub struct Flights(Arc<tokio::sync::Mutex<HashMap<FlightKey, Vec<FlightWaiter>>>>);

impl Flights {
    /// `true` = leader (runs the pipeline), `false` = waiter. Simultaneous taps elect exactly one leader.
    pub async fn attach_or_lead(&self, key: &FlightKey, chat: ChatId, lang: Lang) -> bool {
        let mut flights = self.0.lock().await;
        if let Some(waiters) = flights.get_mut(key) {
            waiters.push(FlightWaiter { chat, lang });
            false
        } else {
            flights.insert(key.clone(), Vec::new());
            true
        }
    }

    /// A tap arriving after this normally hits the `file_id` cache instantly.
    pub async fn finish(&self, key: &FlightKey) -> Vec<FlightWaiter> {
        self.0.lock().await.remove(key).unwrap_or_default()
    }
}

#[derive(BotCommands, Clone)]
#[command(
    rename_rule = "lowercase",
    description = "Fetch videos and audio from links."
)]
pub enum Command {
    #[command(description = "Start the bot and show welcome.")]
    Start,
    #[command(description = "Show usage guide and limits.")]
    Help,
    #[command(description = "Show your current usage and limits.")]
    Usage,
    #[command(description = "Change interface language.")]
    Language,
}

/// Local Bot API unlocks 2 GB, else 50 MB. Units stay untranslated.
fn upload_limit_label(config: &Config) -> &'static str {
    if config.api_url.is_some() {
        "2 GB"
    } else {
        "50 MB"
    }
}

fn help_text(config: &Config, lang: Lang) -> String {
    i18n::help(
        lang,
        config.rate_limit,
        config.max_per_user,
        upload_limit_label(config),
    )
}

/// Explicit `/language` choice first; a prefs failure only logs and falls back to the Telegram guess.
async fn resolve_lang(state: &AppState, user_id: u64, tg_code: Option<&str>) -> Lang {
    match state.prefs.get(user_id).await {
        Ok(Some(lang)) => lang,
        Ok(None) => Lang::from_telegram_code(tg_code),
        Err(e) => {
            tracing::warn!("prefs lookup failed: {e}");
            Lang::from_telegram_code(tg_code)
        }
    }
}

fn format_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        if m > 0 {
            format!("{h}h {m}m")
        } else {
            format!("{h}h")
        }
    } else if m > 0 {
        if s > 0 {
            format!("{m}m {s}s")
        } else {
            format!("{m}m")
        }
    } else {
        format!("{s}s")
    }
}

/// Read-only: never consumes rate-limit quota.
async fn usage_text(state: &AppState, user_id: u64, lang: Lang) -> String {
    let slots = state
        .user_slots
        .lock()
        .await
        .get(&user_id)
        .copied()
        .unwrap_or(0);
    match state.limiter.usage(user_id).await {
        Ok(u) => {
            let reset = if u.reset_in_secs == 0 {
                i18n::reset_full(lang).to_owned()
            } else {
                format!("in {}", format_duration(u.reset_in_secs))
            };
            i18n::usage_body(
                lang,
                &i18n::UsageView {
                    used: u.used,
                    limit: state.config.rate_limit,
                    left: u.remaining,
                    reset: &reset,
                    slots,
                    max_slots: state.config.max_per_user,
                    upload: upload_limit_label(&state.config),
                },
            )
        }
        Err(e) => {
            tracing::warn!("usage lookup failed: {e}");
            i18n::usage_unavailable(lang).to_owned()
        }
    }
}

/// Idempotent, best-effort: logs a warning, never fails startup (e.g. no network).
/// Each item is set twice (default + `ru`); chat messages always follow `/language`, not the menu locale.
pub async fn init_telegram(bot: &Bot) {
    if let Err(e) = bot
        .set_my_commands(Command::bot_commands())
        .scope(BotCommandScope::AllPrivateChats)
        .await
    {
        tracing::warn!("setMyCommands failed: {e}");
    }
    if let Err(e) = bot
        .set_my_commands(ru_commands())
        .scope(BotCommandScope::AllPrivateChats)
        .language_code("ru")
        .await
    {
        tracing::warn!("setMyCommands (ru) failed: {e}");
    }
    for lang in [Lang::En, Lang::Ru] {
        let code = match lang {
            Lang::En => None,
            Lang::Ru => Some("ru"),
        };
        if let Err(e) = set_description(bot, lang, code).await {
            tracing::warn!("setMyDescription ({lang:?}) failed: {e}");
        }
        if let Err(e) = set_short_description(bot, lang, code).await {
            tracing::warn!("setMyShortDescription ({lang:?}) failed: {e}");
        }
    }
}

/// Names stay Latin (Telegram requires `[a-z0-9_]`); only descriptions are translated.
fn ru_commands() -> Vec<teloxide::types::BotCommand> {
    use teloxide::types::BotCommand;
    vec![
        BotCommand::new("start", i18n::cmd_start_desc(Lang::Ru)),
        BotCommand::new("help", i18n::cmd_help_desc(Lang::Ru)),
        BotCommand::new("usage", i18n::cmd_usage_desc(Lang::Ru)),
        BotCommand::new("language", i18n::cmd_language_desc(Lang::Ru)),
    ]
}

async fn set_description(bot: &Bot, lang: Lang, code: Option<&str>) -> Result<()> {
    let mut req = bot
        .set_my_description()
        .description(i18n::bot_description(lang));
    if let Some(code) = code {
        req = req.language_code(code);
    }
    req.await?;
    Ok(())
}

async fn set_short_description(bot: &Bot, lang: Lang, code: Option<&str>) -> Result<()> {
    let mut req = bot
        .set_my_short_description()
        .short_description(i18n::bot_short_description(lang));
    if let Some(code) = code {
        req = req.language_code(code);
    }
    req.await?;
    Ok(())
}

/// Never attributes quota to a shared id when Telegram omits the sender (e.g. channel posts).
async fn reply_usage(bot: &Bot, msg: &Message, state: &AppState, lang: Lang) -> Result<()> {
    let Some(user) = msg.from.as_ref() else {
        bot.send_message(msg.chat.id, i18n::usage_no_sender(lang))
            .await?;
        return Ok(());
    };
    bot.send_message(msg.chat.id, usage_text(state, user.id.0, lang).await)
        .await?;
    Ok(())
}

pub async fn handle_message(bot: Bot, msg: Message, state: AppState) -> Result<()> {
    let Some(text) = msg.text() else {
        return Ok(());
    };
    let text = text.to_owned();
    let tg_code = msg.from.as_ref().and_then(|u| u.language_code.as_deref());
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0);
    let lang = resolve_lang(&state, user_id, tg_code).await;

    if let Ok(cmd) = Command::parse(&text, "fetchly") {
        match cmd {
            Command::Start => {
                bot.send_message(msg.chat.id, i18n::welcome(lang)).await?;
            }
            Command::Help => {
                bot.send_message(msg.chat.id, help_text(&state.config, lang))
                    .await?;
            }
            Command::Usage => {
                reply_usage(&bot, &msg, &state, lang).await?;
            }
            Command::Language => {
                reply_language_picker(&bot, msg.chat.id).await?;
            }
        }
        return Ok(());
    }
    if text.starts_with("/start") {
        bot.send_message(msg.chat.id, i18n::welcome(lang)).await?;
        return Ok(());
    }
    if text.starts_with("/help") {
        bot.send_message(msg.chat.id, help_text(&state.config, lang))
            .await?;
        return Ok(());
    }
    if text.starts_with("/usage") {
        reply_usage(&bot, &msg, &state, lang).await?;
        return Ok(());
    }
    if text.starts_with("/language") {
        reply_language_picker(&bot, msg.chat.id).await?;
        return Ok(());
    }

    let Some(raw_url) = extract_url(&text) else {
        bot.send_message(msg.chat.id, i18n::send_link_hint(lang))
            .await?;
        return Ok(());
    };

    let media = match url::parse(&raw_url) {
        Ok(m) => m,
        Err(e) => {
            bot.send_message(msg.chat.id, e.user_message(lang)).await?;
            return Ok(());
        }
    };

    let status = bot.send_message(msg.chat.id, i18n::resolving(lang)).await?;
    let meta = match ytdlp::resolve(&media).await {
        Ok(m) => m,
        Err(e) => {
            bot.edit_message_text(msg.chat.id, status.id, e.user_message(lang))
                .await?;
            return Ok(());
        }
    };

    let hash = url::url_hash(&media.url);
    let session_id = match state.sessions.create(&hash, &media.url, &meta).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("session create failed: {e}");
            bot.edit_message_text(msg.chat.id, status.id, e.user_message(lang))
                .await?;
            return Ok(());
        }
    };

    let caption = keyboard::preview_text(&meta, lang);
    let markup = keyboard::preview_keyboard(&session_id, lang);
    let _ = bot.delete_message(msg.chat.id, status.id).await;
    send_preview(&bot, msg.chat.id, &meta, &caption, markup, &state.http).await?;
    Ok(())
}

async fn reply_language_picker(bot: &Bot, chat: ChatId) -> Result<()> {
    bot.send_message(chat, i18n::language_prompt())
        .reply_markup(keyboard::language_keyboard())
        .await?;
    Ok(())
}

async fn send_preview(
    bot: &Bot,
    chat: ChatId,
    meta: &ytdlp::Metadata,
    caption: &str,
    markup: InlineKeyboardMarkup,
    http: &reqwest::Client,
) -> Result<()> {
    if let Some(thumb) = &meta.thumbnail_url {
        if let Ok(bytes) = fetch_bytes(http, thumb).await {
            if !bytes.is_empty() && bytes.len() < 5_000_000 {
                let res = bot
                    .send_photo(chat, InputFile::memory(bytes))
                    .caption(caption)
                    .parse_mode(ParseMode::Html)
                    .reply_markup(markup.clone())
                    .await;
                if res.is_ok() {
                    return Ok(());
                }
            }
        }
    }
    bot.send_message(chat, caption)
        .parse_mode(ParseMode::Html)
        .reply_markup(markup)
        .await?;
    Ok(())
}

async fn fetch_bytes(
    http: &reqwest::Client,
    url: &str,
) -> std::result::Result<Vec<u8>, reqwest::Error> {
    Ok(http
        .get(url)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?
        .bytes()
        .await?
        .to_vec())
}

/// Some clients wrap links in `<…>`; strip that before the scheme check.
pub(crate) fn extract_url(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|t| t.trim().trim_matches(|c| c == '<' || c == '>'))
        .find(|t| t.starts_with("http://") || t.starts_with("https://"))
        .map(str::to_owned)
}

pub async fn handle_callback(bot: Bot, q: CallbackQuery, state: AppState) -> Result<()> {
    let Some(data) = q.data.clone() else {
        return Ok(());
    };
    let Some((chat, msg_id)) = callback_origin(&q) else {
        return Ok(());
    };
    let user_id = q.from.id.0;
    let tg_code = q.from.language_code.as_deref();

    // `lang:en` / `lang:ru` taps carry no session.
    if let Some(code) = data.strip_prefix("lang:") {
        handle_lang_tap(&bot, &state, &q, chat, msg_id, code, tg_code).await?;
        return Ok(());
    }

    let lang = resolve_lang(&state, user_id, tg_code).await;

    let Some((kind, quality, session_id)) = crate::session::parse_callback(&data) else {
        bot.answer_callback_query(q.id)
            .text(i18n::outdated_button(lang))
            .await?;
        return Ok(());
    };

    if data.starts_with("cancel:") {
        let token = state.downloads.lock().await.remove(&session_id);
        if let Some(t) = token {
            t.cancel();
            bot.answer_callback_query(q.id)
                .text(i18n::cancelling(lang))
                .await?;
        } else {
            bot.answer_callback_query(q.id)
                .text(i18n::nothing_to_cancel(lang))
                .await?;
            let _ = state.sessions.delete(&session_id).await;
            edit_preview_text(&bot, chat, msg_id, i18n::cancelled(lang)).await;
        }
        return Ok(());
    }

    let Ok(session) = state.sessions.get(&session_id).await else {
        bot.answer_callback_query(q.id)
            .text(i18n::session_expired(lang))
            .await?;
        edit_preview_text(&bot, chat, msg_id, i18n::session_expired(lang)).await;
        return Ok(());
    };

    match (kind, quality.as_str()) {
        ('v', "pick") => {
            bot.answer_callback_query(q.id).await?;
            bot.edit_message_reply_markup(chat, msg_id)
                .reply_markup(keyboard::video_quality_keyboard(
                    &session.metadata,
                    &session_id,
                    lang,
                ))
                .await?;
        }
        ('a', "pick") => {
            bot.answer_callback_query(q.id).await?;
            bot.edit_message_reply_markup(chat, msg_id)
                .reply_markup(keyboard::audio_quality_keyboard(
                    &session.metadata,
                    &session_id,
                    lang,
                ))
                .await?;
        }
        ('v', code) => {
            let Some(quality) = VideoQuality::parse_code(code) else {
                bot.answer_callback_query(q.id)
                    .text(i18n::unknown_quality(lang))
                    .await?;
                return Ok(());
            };
            bot.answer_callback_query(q.id).await?;
            // The update queue must stay live for ❌ taps while the pipeline runs.
            tokio::spawn(run_video(
                bot, chat, msg_id, user_id, session_id, session, quality, state, lang,
            ));
        }
        ('a', code) => {
            let Some(quality) = AudioQuality::parse_code(code) else {
                bot.answer_callback_query(q.id)
                    .text(i18n::unknown_quality(lang))
                    .await?;
                return Ok(());
            };
            bot.answer_callback_query(q.id).await?;
            tokio::spawn(run_audio(
                bot, chat, msg_id, user_id, session_id, session, quality, state, lang,
            ));
        }
        _ => {
            bot.answer_callback_query(q.id).await?;
        }
    }
    Ok(())
}

/// Unknown codes are stale buttons and ignored; a prefs failure answers with a generic error.
async fn handle_lang_tap(
    bot: &Bot,
    state: &AppState,
    q: &CallbackQuery,
    chat: ChatId,
    msg_id: MessageId,
    code: &str,
    tg_code: Option<&str>,
) -> Result<()> {
    let Some(choice) = Lang::from_code(code) else {
        return Ok(());
    };
    match state.prefs.set(q.from.id.0, choice).await {
        Ok(()) => {
            bot.answer_callback_query(q.id.clone()).await?;
            edit_preview_text(bot, chat, msg_id, i18n::language_set(choice)).await;
        }
        Err(e) => {
            tracing::warn!("prefs set failed: {e}");
            bot.answer_callback_query(q.id.clone())
                .text(i18n::error_generic(Lang::from_telegram_code(tg_code)))
                .await?;
        }
    }
    Ok(())
}

fn callback_origin(q: &CallbackQuery) -> Option<(ChatId, MessageId)> {
    match q.message.as_ref()? {
        teloxide::types::MaybeInaccessibleMessage::Regular(m) => Some((m.chat.id, m.id)),
        teloxide::types::MaybeInaccessibleMessage::Inaccessible(_) => None,
    }
}

/// `scale_to` reserves headroom: video 100, audio 80 (convert + upload take the last 20%).
fn spawn_progress_forwarder(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<u8>,
    bot: Bot,
    chat: ChatId,
    msg: MessageId,
    prefix: &'static str,
    scale_to: u8,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut progress = crate::telegram::progress::Progress::new(bot, chat, msg);
        while let Some(pct) = rx.recv().await {
            let scaled = u8::try_from(u16::from(pct) * u16::from(scale_to) / 100).unwrap_or(100);
            progress
                .update(&keyboard::progress_bar(prefix, scaled), false)
                .await;
        }
    })
}

/// Best-effort: tagging only logs on failure.
async fn tag_downloaded_audio(
    session: &StoredSession,
    mp3_path: &std::path::Path,
    quality_code: &str,
) {
    let cover = match &session.metadata.thumbnail_url {
        Some(thumb) => crate::media::tag::fetch_cover(thumb).await,
        None => None,
    };
    let (cover_bytes, cover_mime) = cover.map_or((None, None), |(b, m)| (Some(b), m));
    let info = crate::media::tag::TagInfo {
        title: session.metadata.title.clone(),
        artist: session.metadata.uploader.clone(),
        album: None,
        year: None,
        cover_bytes,
        cover_mime,
    };
    if let Err(e) = crate::media::tag::tag_mp3(mp3_path, info).await {
        tracing::warn!("tagging failed (non-fatal): {e}");
    }
    if let Ok(meta) = tokio::fs::metadata(mp3_path).await {
        tracing::info!(
            "audio ready: {} ({}), quality {quality_code}",
            format_bytes(meta.len()),
            session.metadata.title,
        );
    }
}

/// Checked before attempting a doomed upload.
const STANDARD_UPLOAD_LIMIT: u64 = 50_000_000;

async fn exceeds_standard_limit(path: &std::path::Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|m| m.len() > STANDARD_UPLOAD_LIMIT)
}

/// A 413 means the file passed the 2 GB guard but exceeds the 50 MB standard-API cap.
fn upload_error_message(raw: &str, lang: Lang) -> String {
    if raw.to_lowercase().contains("too large") {
        i18n::over_limit(lang).to_owned()
    } else {
        Error::Telegram(raw.to_owned()).user_message(lang)
    }
}

/// Telegram rejects `editMessageText` on photos, so fall back to `editMessageCaption`.
async fn edit_preview_text(bot: &Bot, chat: ChatId, msg: MessageId, text: &str) {
    if bot.edit_message_text(chat, msg, text).await.is_ok() {
        return;
    }
    let _ = bot.edit_message_caption(chat, msg).caption(text).await;
}

/// Over the waiter cap, reject with a busy notice instead of parking another task.
async fn acquire_permit(
    bot: &Bot,
    chat: ChatId,
    origin_msg: MessageId,
    state: &AppState,
    lang: Lang,
) -> Option<tokio::sync::OwnedSemaphorePermit> {
    let parked = state.queued.fetch_add(1, Ordering::SeqCst);
    if parked >= state.config.max_queued {
        state.queued.fetch_sub(1, Ordering::SeqCst);
        tracing::warn!("waiter cap hit ({parked} parked); rejecting tap");
        edit_preview_text(bot, chat, origin_msg, i18n::busy(lang)).await;
        return None;
    }
    if state.semaphore.available_permits() == 0 {
        tracing::info!("download queued (depth {})", parked + 1);
        edit_preview_text(bot, chat, origin_msg, i18n::queued(lang)).await;
    }
    let permit = state.semaphore.clone().acquire_owned().await.ok();
    state.queued.fetch_sub(1, Ordering::SeqCst);
    permit
}

/// On expiry the stage token is cancelled and the caller sees [`Error::TimedOut`].
async fn with_deadline<F, T>(
    deadline: tokio::time::Instant,
    timeout_secs: u64,
    cancel: &CancellationToken,
    fut: F,
) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    tokio::pin!(fut);
    let mut timed_out = false;
    let out = tokio::select! {
        biased;
        r = &mut fut => r,
        () = tokio::time::sleep_until(deadline) => {
            timed_out = true;
            cancel.cancel();
            fut.await
        }
    };
    if timed_out {
        let minutes = (timeout_secs / 60).max(1);
        tracing::warn!("pipeline stage hit {timeout_secs}s deadline");
        return Err(Error::TimedOut { minutes });
    }
    out
}

/// Bounded by `NOTIFY_TIMEOUT_SECS`; shutdown has only chat ids, so the notice is bilingual.
const NOTIFY_TIMEOUT_SECS: u64 = 15;

fn restart_bilingual() -> String {
    format!("{}\n{}", i18n::restart(Lang::En), i18n::restart(Lang::Ru))
}

pub async fn shutdown_notify(bot: Bot, state: &AppState) {
    let chats: HashSet<i64> = state.notify.lock().await.values().map(|c| c.0).collect();
    tracing::info!("shutdown: notifying {} chats", chats.len());
    let notice = restart_bilingual();
    let sends = async {
        let mut set = tokio::task::JoinSet::new();
        for chat_id in chats {
            let bot = bot.clone();
            let notice = notice.clone();
            set.spawn(async move {
                if let Err(e) = bot.send_message(ChatId(chat_id), notice).await {
                    tracing::warn!("restart notice to {chat_id} failed: {e}");
                }
            });
        }
        while set.join_next().await.is_some() {}
    };
    if tokio::time::timeout(std::time::Duration::from_secs(NOTIFY_TIMEOUT_SECS), sends)
        .await
        .is_err()
    {
        tracing::warn!("shutdown notices timed out");
    }
    let tokens: Vec<CancellationToken> = state
        .downloads
        .lock()
        .await
        .drain()
        .map(|(_, t)| t)
        .collect();
    tracing::info!("shutdown: cancelling {} downloads", tokens.len());
    for token in tokens {
        token.cancel();
    }
}

/// Returns `false` when the user is at their cap.
async fn acquire_user_slot(state: &AppState, user_id: u64) -> bool {
    let mut slots = state.user_slots.lock().await;
    let used = slots.get(&user_id).copied().unwrap_or(0);
    if used >= state.config.max_per_user {
        return false;
    }
    slots.insert(user_id, used + 1);
    true
}

/// Called on every pipeline exit after acquisition — audit these sites when adding new returns.
async fn release_user_slot(state: &AppState, user_id: u64) {
    let mut slots = state.user_slots.lock().await;
    if let Some(used) = slots.get_mut(&user_id) {
        *used = used.saturating_sub(1);
        if *used == 0 {
            slots.remove(&user_id);
        }
    }
}

/// Best-effort per chat; failures are logged.
async fn fan_out_file(
    bot: &Bot,
    state: &AppState,
    key: &FlightKey,
    format: &str,
    title: &str,
    file_id: &str,
) {
    for waiter in state.flights.finish(key).await {
        let chat = waiter.chat;
        let res = if format == "video" {
            bot.send_video(
                chat,
                InputFile::file_id(teloxide::types::FileId(file_id.to_owned())),
            )
            .caption(title.to_owned())
            .await
            .map(|_| ())
        } else {
            bot.send_audio(
                chat,
                InputFile::file_id(teloxide::types::FileId(file_id.to_owned())),
            )
            .title(title.to_owned())
            .await
            .map(|_| ())
        };
        if let Err(e) = res {
            tracing::warn!("singleflight fan-out to {chat} failed: {e}");
        }
    }
}

/// Each waiter gets the notice in their own language.
async fn fan_out_notice(
    bot: &Bot,
    state: &AppState,
    key: &FlightKey,
    notice: impl Fn(Lang) -> String,
) {
    for waiter in state.flights.finish(key).await {
        if let Err(e) = bot.send_message(waiter.chat, notice(waiter.lang)).await {
            tracing::warn!("singleflight notice to {} failed: {e}", waiter.chat);
        }
    }
}

/// Quota is consumed only after cache, per-user cap, and dedup pass; post-consume admission failures refund.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_video(
    bot: Bot,
    chat: ChatId,
    origin_msg: MessageId,
    user_id: u64,
    session_id: String,
    session: StoredSession,
    quality: VideoQuality,
    state: AppState,
    lang: Lang,
) {
    let quality_code = quality.as_str().to_owned();

    if let Ok(Some(file_id)) = state
        .cache
        .get(&session.url_hash, "video", &quality_code)
        .await
    {
        let sent = bot
            .send_video(chat, InputFile::file_id(teloxide::types::FileId(file_id)))
            .caption(format!(
                "{}\n{}",
                session.metadata.title,
                quality.label(lang)
            ))
            .await;
        // A stale file_id falls through to re-download.
        if sent.is_ok() {
            return;
        }
    }

    if !acquire_user_slot(&state, user_id).await {
        respond(
            bot,
            chat,
            Error::TooManyConcurrent {
                max: state.config.max_per_user,
            },
            lang,
        )
        .await;
        return;
    }

    let flight_key = (
        session.url_hash.clone(),
        "video".to_owned(),
        quality_code.clone(),
    );
    if !state.flights.attach_or_lead(&flight_key, chat, lang).await {
        release_user_slot(&state, user_id).await;
        let _ = bot.send_message(chat, i18n::flight_wait(lang)).await;
        return;
    }

    if let Err(e) = state.limiter.check_and_consume(user_id).await {
        release_user_slot(&state, user_id).await;
        fan_out_notice(&bot, &state, &flight_key, |l| {
            i18n::flight_retry(l).to_owned()
        })
        .await;
        respond(bot, chat, e, lang).await;
        return;
    }

    state.notify.lock().await.insert(session_id.clone(), chat);
    let Some(_permit) = acquire_permit(&bot, chat, origin_msg, &state, lang).await else {
        state.notify.lock().await.remove(&session_id);
        release_user_slot(&state, user_id).await;
        let _ = state.limiter.refund(user_id).await;
        fan_out_notice(&bot, &state, &flight_key, |l| {
            i18n::flight_retry(l).to_owned()
        })
        .await;
        return;
    };

    // Queued time does not count; the clock starts at semaphore acquisition.
    let timeout_secs = state.config.download_timeout_secs;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    let cancel = CancellationToken::new();
    state
        .downloads
        .lock()
        .await
        .insert(session_id.clone(), cancel.clone());

    let progress_msg = bot
        .send_message(chat, i18n::downloading_zero(lang))
        .reply_markup(keyboard::cancel_keyboard(&session_id, lang))
        .await;
    let Ok(progress_msg) = progress_msg else {
        state.downloads.lock().await.remove(&session_id);
        state.notify.lock().await.remove(&session_id);
        release_user_slot(&state, user_id).await;
        let _ = state.limiter.refund(user_id).await;
        fan_out_notice(&bot, &state, &flight_key, |l| {
            i18n::flight_retry(l).to_owned()
        })
        .await;
        return;
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<u8>();

    let work_dir = state.config.temp_dir.join(&session_id);
    let out_path = work_dir.join("video.mp4");
    let media = url::MediaUrl {
        platform: session.metadata.platform,
        url: session.url.clone(),
    };

    let progress_task = spawn_progress_forwarder(
        rx,
        bot.clone(),
        chat,
        progress_msg.id,
        i18n::downloading_prefix(lang),
        100,
    );

    let download = with_deadline(
        deadline,
        timeout_secs,
        &cancel,
        ytdlp::download_video(&media, quality, &out_path, &cancel, Some(&tx)),
    )
    .await;
    drop(tx);
    let _ = progress_task.await;
    state.downloads.lock().await.remove(&session_id);
    state.notify.lock().await.remove(&session_id);
    release_user_slot(&state, user_id).await;

    match download {
        Err(Error::Cancelled) => {
            let _ = bot
                .edit_message_text(chat, progress_msg.id, i18n::cancelled(lang))
                .await;
            cleanup(&work_dir).await;
            fan_out_notice(&bot, &state, &flight_key, |l| {
                i18n::flight_cancelled(l).to_owned()
            })
            .await;
        }
        Err(e) => {
            tracing::warn!("video download failed: {e}");
            let text = e.user_message(lang);
            let _ = bot.edit_message_text(chat, progress_msg.id, &text).await;
            cleanup(&work_dir).await;
            fan_out_notice(&bot, &state, &flight_key, |l| e.user_message(l)).await;
        }
        Ok(path) => {
            if exceeds_standard_limit(&path).await && state.config.api_url.is_none() {
                let _ = bot
                    .edit_message_text(chat, progress_msg.id, i18n::over_limit(lang))
                    .await;
                cleanup(&work_dir).await;
                fan_out_notice(&bot, &state, &flight_key, |l| {
                    i18n::over_limit(l).to_owned()
                })
                .await;
                return;
            }
            let file_id = upload_video(
                &bot,
                chat,
                progress_msg.id,
                &path,
                &session,
                quality,
                &state,
                lang,
            )
            .await;
            cleanup(&work_dir).await;
            match file_id {
                Some(fid) => {
                    fan_out_file(
                        &bot,
                        &state,
                        &flight_key,
                        "video",
                        &session.metadata.title,
                        &fid,
                    )
                    .await;
                }
                None => {
                    fan_out_notice(&bot, &state, &flight_key, |l| {
                        i18n::flight_failed(l).to_owned()
                    })
                    .await;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn upload_video(
    bot: &Bot,
    chat: ChatId,
    progress_msg: MessageId,
    path: &std::path::Path,
    session: &StoredSession,
    quality: VideoQuality,
    state: &AppState,
    lang: Lang,
) -> Option<String> {
    progress_msg_update(bot, chat, progress_msg, i18n::uploading(lang)).await;
    let caption = format!(
        "{}\n{} · {}",
        session.metadata.title,
        quality.label(lang),
        session.metadata.platform.as_str()
    );
    match bot
        .send_video(chat, InputFile::file(path.to_owned()))
        .caption(caption)
        .await
    {
        Ok(sent) => {
            let file_id = sent.video().map(|v| v.file.id.to_string());
            if let Some(fid) = &file_id {
                let _ = state
                    .cache
                    .set(&session.url_hash, "video", quality.as_str(), fid)
                    .await;
            }
            let _ = bot.delete_message(chat, progress_msg).await;
            file_id
        }
        Err(e) => {
            tracing::warn!("send_video failed: {e}");
            let _ = bot
                .edit_message_text(
                    chat,
                    progress_msg,
                    upload_error_message(&e.to_string(), lang),
                )
                .await;
            None
        }
    }
}

/// Same admission order as [`run_video`].
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_audio(
    bot: Bot,
    chat: ChatId,
    origin_msg: MessageId,
    user_id: u64,
    session_id: String,
    session: StoredSession,
    quality: AudioQuality,
    state: AppState,
    lang: Lang,
) {
    let quality_code = quality.as_str().to_owned();

    if let Ok(Some(file_id)) = state
        .cache
        .get(&session.url_hash, "audio", &quality_code)
        .await
    {
        let sent = bot
            .send_audio(chat, InputFile::file_id(teloxide::types::FileId(file_id)))
            .title(session.metadata.title.clone())
            .await;
        if sent.is_ok() {
            return;
        }
    }

    if !acquire_user_slot(&state, user_id).await {
        respond(
            bot,
            chat,
            Error::TooManyConcurrent {
                max: state.config.max_per_user,
            },
            lang,
        )
        .await;
        return;
    }

    let flight_key = (
        session.url_hash.clone(),
        "audio".to_owned(),
        quality_code.clone(),
    );
    if !state.flights.attach_or_lead(&flight_key, chat, lang).await {
        release_user_slot(&state, user_id).await;
        let _ = bot.send_message(chat, i18n::flight_wait(lang)).await;
        return;
    }

    if let Err(e) = state.limiter.check_and_consume(user_id).await {
        release_user_slot(&state, user_id).await;
        fan_out_notice(&bot, &state, &flight_key, |l| {
            i18n::flight_retry(l).to_owned()
        })
        .await;
        respond(bot, chat, e, lang).await;
        return;
    }

    state.notify.lock().await.insert(session_id.clone(), chat);
    let Some(_permit) = acquire_permit(&bot, chat, origin_msg, &state, lang).await else {
        state.notify.lock().await.remove(&session_id);
        release_user_slot(&state, user_id).await;
        let _ = state.limiter.refund(user_id).await;
        fan_out_notice(&bot, &state, &flight_key, |l| {
            i18n::flight_retry(l).to_owned()
        })
        .await;
        return;
    };

    // Queued time does not count; the clock starts at semaphore acquisition.
    let timeout_secs = state.config.download_timeout_secs;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    let cancel = CancellationToken::new();
    state
        .downloads
        .lock()
        .await
        .insert(session_id.clone(), cancel.clone());

    let progress_msg = bot
        .send_message(chat, i18n::downloading_zero(lang))
        .reply_markup(keyboard::cancel_keyboard(&session_id, lang))
        .await;
    let Ok(progress_msg) = progress_msg else {
        state.downloads.lock().await.remove(&session_id);
        state.notify.lock().await.remove(&session_id);
        release_user_slot(&state, user_id).await;
        let _ = state.limiter.refund(user_id).await;
        fan_out_notice(&bot, &state, &flight_key, |l| {
            i18n::flight_retry(l).to_owned()
        })
        .await;
        return;
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<u8>();

    let work_dir = state.config.temp_dir.join(&session_id);
    let src_path = work_dir.join("source");
    let mp3_path = work_dir.join("audio.mp3");
    let media = url::MediaUrl {
        platform: session.metadata.platform,
        url: session.url.clone(),
    };

    let progress_task = spawn_progress_forwarder(
        rx,
        bot.clone(),
        chat,
        progress_msg.id,
        i18n::downloading_prefix(lang),
        80,
    );

    let download = with_deadline(
        deadline,
        timeout_secs,
        &cancel,
        ytdlp::download_audio_source(&media, &src_path, &cancel, Some(&tx)),
    )
    .await;
    drop(tx);
    let _ = progress_task.await;

    let result: Result<std::path::PathBuf> = match download {
        Err(e) => Err(e),
        Ok(src) => {
            progress_msg_update(&bot, chat, progress_msg.id, i18n::converting(lang)).await;
            if let Err(e) = with_deadline(
                deadline,
                timeout_secs,
                &cancel,
                crate::media::ffmpeg::to_mp3(&src, &mp3_path, quality, &cancel),
            )
            .await
            {
                Err(e)
            } else {
                tag_downloaded_audio(&session, &mp3_path, &quality_code).await;
                Ok(mp3_path.clone())
            }
        }
    };
    state.downloads.lock().await.remove(&session_id);
    state.notify.lock().await.remove(&session_id);
    release_user_slot(&state, user_id).await;

    match result {
        Err(Error::Cancelled) => {
            let _ = bot
                .edit_message_text(chat, progress_msg.id, i18n::cancelled(lang))
                .await;
            cleanup(&work_dir).await;
            fan_out_notice(&bot, &state, &flight_key, |l| {
                i18n::flight_cancelled(l).to_owned()
            })
            .await;
        }
        Err(e) => {
            tracing::warn!("audio pipeline failed: {e}");
            let text = e.user_message(lang);
            let _ = bot.edit_message_text(chat, progress_msg.id, &text).await;
            cleanup(&work_dir).await;
            fan_out_notice(&bot, &state, &flight_key, |l| e.user_message(l)).await;
        }
        Ok(path) => {
            if exceeds_standard_limit(&path).await && state.config.api_url.is_none() {
                let _ = bot
                    .edit_message_text(chat, progress_msg.id, i18n::over_limit(lang))
                    .await;
                cleanup(&work_dir).await;
                fan_out_notice(&bot, &state, &flight_key, |l| {
                    i18n::over_limit(l).to_owned()
                })
                .await;
                return;
            }
            let file_id = upload_audio(
                &bot,
                chat,
                progress_msg.id,
                &path,
                &session,
                quality,
                &state,
                lang,
            )
            .await;
            cleanup(&work_dir).await;
            match file_id {
                Some(fid) => {
                    fan_out_file(
                        &bot,
                        &state,
                        &flight_key,
                        "audio",
                        &session.metadata.title,
                        &fid,
                    )
                    .await;
                }
                None => {
                    fan_out_notice(&bot, &state, &flight_key, |l| {
                        i18n::flight_failed(l).to_owned()
                    })
                    .await;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn upload_audio(
    bot: &Bot,
    chat: ChatId,
    progress_msg: MessageId,
    path: &std::path::Path,
    session: &StoredSession,
    quality: AudioQuality,
    state: &AppState,
    lang: Lang,
) -> Option<String> {
    progress_msg_update(bot, chat, progress_msg, i18n::uploading_audio(lang)).await;
    let mut req = bot
        .send_audio(chat, InputFile::file(path.to_owned()))
        .title(session.metadata.title.clone());
    if let Some(performer) = &session.metadata.uploader {
        req = req.performer(performer.clone());
    }
    if let Some(d) = session
        .metadata
        .duration_secs
        .and_then(|d| u32::try_from(d).ok())
    {
        req = req.duration(d);
    }
    match req.await {
        Ok(sent) => {
            let file_id = sent.audio().map(|a| a.file.id.to_string());
            if let Some(fid) = &file_id {
                let _ = state
                    .cache
                    .set(&session.url_hash, "audio", quality.as_str(), fid)
                    .await;
            }
            let _ = bot.delete_message(chat, progress_msg).await;
            file_id
        }
        Err(e) => {
            tracing::warn!("send_audio failed: {e}");
            let _ = bot
                .edit_message_text(
                    chat,
                    progress_msg,
                    upload_error_message(&e.to_string(), lang),
                )
                .await;
            None
        }
    }
}

async fn respond(bot: Bot, chat: ChatId, e: Error, lang: Lang) {
    match e {
        Error::RateLimited {
            retry_in_secs,
            remaining: _,
        } => {
            let _ = bot
                .send_message(chat, i18n::rate_limited(lang, retry_in_secs))
                .await;
        }
        other => {
            let _ = bot.send_message(chat, other.user_message(lang)).await;
        }
    }
}

async fn progress_msg_update(bot: &Bot, chat: ChatId, msg: MessageId, text: &str) {
    let _ = bot.edit_message_text(chat, msg, text).await;
}

async fn cleanup(dir: &std::path::Path) {
    if let Err(e) = tokio::fs::remove_dir_all(dir).await {
        tracing::debug!("cleanup {} failed: {e}", dir.display());
    }
}

#[must_use]
pub fn schema() -> teloxide::dispatching::UpdateHandler<Error> {
    use teloxide::dispatching::UpdateFilterExt;

    let msg_handler = Update::filter_message()
        .branch(dptree::filter(|msg: Message| msg.text().is_some()).endpoint(handle_message));
    let callback_handler = Update::filter_callback_query().endpoint(handle_callback);

    dptree::entry()
        .branch(msg_handler)
        .branch(callback_handler)
        .branch(dptree::endpoint(|upd: Update| async move {
            tracing::debug!("unhandled update: {:?}", upd.kind);
            Ok(())
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_first_url() {
        assert_eq!(
            extract_url("check this https://youtu.be/abc and https://x.com/1"),
            Some("https://youtu.be/abc".to_owned())
        );
        assert_eq!(
            extract_url("https://vm.tiktok.com/xyz/"),
            Some("https://vm.tiktok.com/xyz/".to_owned())
        );
    }

    #[test]
    fn extracts_wrapped_and_prefixed_urls() {
        assert_eq!(
            extract_url("watch <https://youtu.be/abc>"),
            Some("https://youtu.be/abc".to_owned())
        );
        assert_eq!(
            extract_url("hey,https://x.com/a"),
            None,
            "URL glued to text is not a token"
        );
    }

    #[test]
    fn no_url_yields_none() {
        assert_eq!(extract_url("just some words"), None);
        assert_eq!(extract_url(""), None);
        assert_eq!(extract_url("/start"), None);
    }

    #[test]
    fn upload_error_maps_413_to_limit_advice() {
        for lang in [Lang::En, Lang::Ru] {
            assert_eq!(
                upload_error_message("A Telegram's error: Request Entity Too Large", lang),
                i18n::over_limit(lang)
            );
            assert_eq!(
                upload_error_message("Bad Request: chat not found", lang),
                i18n::error_generic(lang)
            );
        }
    }

    #[tokio::test]
    async fn deadline_passes_through_completed_work() {
        let cancel = CancellationToken::new();
        let out = with_deadline(
            tokio::time::Instant::now() + std::time::Duration::from_secs(60),
            900,
            &cancel,
            async { Ok::<_, Error>(42) },
        )
        .await;
        assert_eq!(out.unwrap(), 42);
        assert!(!cancel.is_cancelled());
    }

    #[tokio::test]
    async fn expired_deadline_cancels_and_reports_timeout() {
        let cancel = CancellationToken::new();
        let fut = async {
            cancel.cancelled().await;
            Err::<(), Error>(Error::Cancelled)
        };
        let out = with_deadline(tokio::time::Instant::now(), 900, &cancel, fut).await;
        assert!(cancel.is_cancelled());
        match out {
            Err(Error::TimedOut { minutes }) => assert_eq!(minutes, 15),
            other => panic!("expected TimedOut, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn user_slots_cap_counts_and_releases() {
        let Some(t) = crate::testutil::start_redis().await else {
            return;
        };
        let config = Config {
            bot_token: "test".to_owned(),
            api_url: None,
            max_workers: 1,
            download_timeout_secs: 900,
            rate_limit: 20,
            max_per_user: 1,
            max_queued: 20,
            db_path: std::path::PathBuf::from(":memory:"),
            temp_dir: std::path::PathBuf::from("/tmp/fetchly-test"),
            redis_url: String::new(),
        };
        let state = AppState::new(
            config,
            FileCache::open_in_memory().expect("cache"),
            crate::prefs::UserPrefs::open_in_memory().expect("prefs"),
            SessionStore::new(t.manager.clone()),
            RateLimiter::new(t.manager.clone(), 20),
            Arc::new(tokio::sync::Semaphore::new(1)),
            reqwest::Client::new(),
        );
        assert!(acquire_user_slot(&state, 7).await);
        assert!(!acquire_user_slot(&state, 7).await, "cap is 1");
        assert!(acquire_user_slot(&state, 8).await, "other users unaffected");
        release_user_slot(&state, 7).await;
        assert!(acquire_user_slot(&state, 7).await, "release frees the slot");
        release_user_slot(&state, 7).await;
        release_user_slot(&state, 8).await;
        assert!(state.user_slots.lock().await.is_empty(), "no slot leaks");
    }

    #[tokio::test]
    async fn waiter_counter_returns_to_zero_after_admit() {
        let Some(t) = crate::testutil::start_redis().await else {
            return;
        };
        let config = Config {
            bot_token: "test".to_owned(),
            api_url: None,
            max_workers: 2,
            download_timeout_secs: 900,
            rate_limit: 20,
            max_per_user: 2,
            max_queued: 20,
            db_path: std::path::PathBuf::from(":memory:"),
            temp_dir: std::path::PathBuf::from("/tmp/fetchly-test"),
            redis_url: String::new(),
        };
        let state = AppState::new(
            config,
            FileCache::open_in_memory().expect("cache"),
            crate::prefs::UserPrefs::open_in_memory().expect("prefs"),
            SessionStore::new(t.manager.clone()),
            RateLimiter::new(t.manager.clone(), 20),
            Arc::new(tokio::sync::Semaphore::new(2)),
            reqwest::Client::new(),
        );
        let bot = Bot::new("test-token");
        let permit = acquire_permit(&bot, ChatId(1), MessageId(1), &state, Lang::En).await;
        assert!(permit.is_some());
        assert_eq!(state.queued.load(Ordering::SeqCst), 0);
        drop(permit);
    }

    #[test]
    fn menu_commands_match_handlers() {
        let cmds = Command::bot_commands();
        assert_eq!(cmds.len(), 4);
        assert_eq!(cmds[0].command.trim_start_matches('/'), "start");
        assert_eq!(cmds[1].command.trim_start_matches('/'), "help");
        assert_eq!(cmds[2].command.trim_start_matches('/'), "usage");
        assert_eq!(cmds[3].command.trim_start_matches('/'), "language");
        assert!(!cmds[0].description.is_empty());
        assert!(!cmds[1].description.is_empty());
        assert!(!cmds[2].description.is_empty());
        assert!(!cmds[3].description.is_empty());
        let text = Command::descriptions().to_string();
        assert!(text.contains("/start"));
        assert!(text.contains("/help"));
        assert!(text.contains("/usage"));
        assert!(text.contains("/language"));
    }

    #[test]
    fn menu_commands_match_i18n() {
        let cmds = Command::bot_commands();
        let want = [
            i18n::cmd_start_desc(Lang::En),
            i18n::cmd_help_desc(Lang::En),
            i18n::cmd_usage_desc(Lang::En),
            i18n::cmd_language_desc(Lang::En),
        ];
        for (cmd, desc) in cmds.iter().zip(want) {
            assert_eq!(cmd.description, desc);
        }
    }

    #[test]
    fn ru_menu_names_are_valid_commands() {
        for cmd in ru_commands() {
            assert!((1..=32).contains(&cmd.command.len()), "{}", cmd.command);
            assert!(
                cmd.command
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{}",
                cmd.command
            );
            assert!(!cmd.description.is_empty(), "{}", cmd.command);
        }
    }

    fn flight_key(hash: &str, format: &str, quality: &str) -> FlightKey {
        (hash.to_owned(), format.to_owned(), quality.to_owned())
    }

    #[tokio::test]
    async fn flight_first_tapper_leads_rest_wait() {
        let flights = Flights::default();
        let key = flight_key("hash", "video", "720");
        assert!(flights.attach_or_lead(&key, ChatId(1), Lang::En).await);
        assert!(!flights.attach_or_lead(&key, ChatId(2), Lang::Ru).await);
        let other = flight_key("hash", "video", "1080");
        assert!(flights.attach_or_lead(&other, ChatId(3), Lang::En).await);
    }

    #[tokio::test]
    async fn flight_finish_hands_over_waiters_and_releases_key() {
        let flights = Flights::default();
        let key = flight_key("hash", "audio", "320");
        assert!(flights.attach_or_lead(&key, ChatId(1), Lang::En).await);
        assert!(!flights.attach_or_lead(&key, ChatId(2), Lang::Ru).await);
        assert!(!flights.attach_or_lead(&key, ChatId(3), Lang::En).await);
        assert_eq!(
            flights.finish(&key).await,
            vec![
                FlightWaiter {
                    chat: ChatId(2),
                    lang: Lang::Ru
                },
                FlightWaiter {
                    chat: ChatId(3),
                    lang: Lang::En
                },
            ]
        );
        assert!(flights.attach_or_lead(&key, ChatId(4), Lang::En).await);
    }

    #[tokio::test]
    async fn flight_concurrent_taps_elect_exactly_one_leader() {
        let flights = Flights::default();
        let key = flight_key("hash", "video", "720");
        let mut set = tokio::task::JoinSet::new();
        for i in 0..10 {
            let registry = flights.clone();
            let tapped = key.clone();
            set.spawn(async move { registry.attach_or_lead(&tapped, ChatId(i), Lang::En).await });
        }
        let mut leaders = 0;
        while let Some(res) = set.join_next().await {
            if res.expect("tap task") {
                leaders += 1;
            }
        }
        assert_eq!(leaders, 1, "10 concurrent taps = 1 download");
        assert_eq!(flights.finish(&key).await.len(), 9);
    }

    fn test_config(rate_limit: u32, max_per_user: usize, api_url: Option<&str>) -> Config {
        Config {
            bot_token: "test".to_owned(),
            api_url: api_url.map(str::to_owned),
            max_workers: 4,
            download_timeout_secs: 900,
            rate_limit,
            max_per_user,
            max_queued: 20,
            db_path: std::path::PathBuf::from(":memory:"),
            temp_dir: std::path::PathBuf::from("/tmp/fetchly-test"),
            redis_url: String::new(),
        }
    }

    #[test]
    fn help_reflects_upload_cap() {
        let std = help_text(&test_config(20, 2, None), Lang::En);
        assert!(std.contains("20 downloads/hour"), "{std}");
        assert!(std.contains("2 at a time"), "{std}");
        assert!(std.contains("50 MB max file size"), "{std}");
        assert!(std.contains("/usage"), "{std}");
        assert!(std.contains("/language"), "{std}");

        let local = help_text(&test_config(5, 1, Some("http://botapi:8081")), Lang::En);
        assert!(local.contains("5 downloads/hour"), "{local}");
        assert!(local.contains("1 at a time"), "{local}");
        assert!(local.contains("2 GB max file size"), "{local}");

        let ru = help_text(&test_config(20, 2, None), Lang::Ru);
        assert!(ru.contains("20 загрузок/час"), "{ru}");
        assert!(ru.contains("/language"), "{ru}");
    }

    #[test]
    fn durations_are_human_readable() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(45), "45s");
        assert_eq!(format_duration(60), "1m");
        assert_eq!(format_duration(125), "2m 5s");
        assert_eq!(format_duration(3600), "1h");
        assert_eq!(format_duration(3720), "1h 2m");
    }

    #[tokio::test]
    async fn usage_shows_quota_slots_and_cap() {
        let Some(t) = crate::testutil::start_redis().await else {
            return;
        };
        let state = AppState::new(
            test_config(20, 2, None),
            FileCache::open_in_memory().expect("cache"),
            crate::prefs::UserPrefs::open_in_memory().expect("prefs"),
            SessionStore::new(t.manager.clone()),
            RateLimiter::new(t.manager.clone(), 20),
            Arc::new(tokio::sync::Semaphore::new(2)),
            reqwest::Client::new(),
        );
        let fresh = usage_text(&state, 9001, Lang::En).await;
        assert!(fresh.contains("0/20 used (20 left)"), "{fresh}");
        assert!(fresh.contains("0/2"), "{fresh}");
        assert!(fresh.contains("50 MB"), "{fresh}");

        state
            .limiter
            .check_and_consume(9001)
            .await
            .expect("consume");
        state.user_slots.lock().await.insert(9001, 1);
        let used = usage_text(&state, 9001, Lang::En).await;
        assert!(used.contains("1/20 used (19 left)"), "{used}");
        assert!(used.contains("1/2"), "{used}");
        assert!(used.contains("Reset: in "), "{used}");

        let ru = usage_text(&state, 9001, Lang::Ru).await;
        assert!(ru.contains("1/20"), "{ru}");
        assert!(ru.contains("Сброс:"), "{ru}");
    }

    #[tokio::test]
    async fn resolve_lang_prefers_explicit_choice() {
        let Some(t) = crate::testutil::start_redis().await else {
            return;
        };
        let state = AppState::new(
            test_config(20, 2, None),
            FileCache::open_in_memory().expect("cache"),
            crate::prefs::UserPrefs::open_in_memory().expect("prefs"),
            SessionStore::new(t.manager.clone()),
            RateLimiter::new(t.manager.clone(), 20),
            Arc::new(tokio::sync::Semaphore::new(2)),
            reqwest::Client::new(),
        );
        assert_eq!(resolve_lang(&state, 4242, Some("ru-RU")).await, Lang::Ru);
        assert_eq!(resolve_lang(&state, 4242, Some("en-US")).await, Lang::En);
        assert_eq!(resolve_lang(&state, 4242, None).await, Lang::En);
        state.prefs.set(4242, Lang::En).await.expect("set");
        assert_eq!(resolve_lang(&state, 4242, Some("ru-RU")).await, Lang::En);
        state.prefs.set(4242, Lang::Ru).await.expect("set");
        assert_eq!(resolve_lang(&state, 4242, Some("en-US")).await, Lang::Ru);
    }
}
