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
use crate::limiter::RateLimiter;
use crate::media::url;
use crate::media::ytdlp::{self, AudioQuality, VideoQuality};
use crate::session::{SessionStore, StoredSession};
use crate::telegram::keyboard::{self, format_bytes};

/// Shared state injected into every handler via `dptree::deps!`.
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub cache: FileCache,
    pub sessions: SessionStore,
    pub limiter: RateLimiter,
    pub semaphore: Arc<tokio::sync::Semaphore>,
    /// In-flight downloads: `session_id` → cancel token (wired to ❌ button).
    pub downloads: Arc<tokio::sync::Mutex<HashMap<String, CancellationToken>>>,
    /// Chats with a queued or in-flight download: `session_id` → chat.
    /// Registered before semaphore acquisition (covers the queued state),
    /// removed on every pipeline exit. Read once at shutdown for notices.
    pub notify: Arc<tokio::sync::Mutex<HashMap<String, ChatId>>>,
    /// In-flight downloads per user (queued + running count toward the cap).
    pub user_slots: Arc<tokio::sync::Mutex<HashMap<u64, usize>>>,
    /// Tasks currently inside semaphore admission (parked waiters + handoff).
    /// Global backpressure bound; decremented right after a permit is granted.
    pub queued: Arc<AtomicUsize>,
    pub http: reqwest::Client,
}

impl AppState {
    #[must_use]
    pub fn new(
        config: Config,
        cache: FileCache,
        sessions: SessionStore,
        limiter: RateLimiter,
        semaphore: Arc<tokio::sync::Semaphore>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            config,
            cache,
            sessions,
            limiter,
            semaphore,
            downloads: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            notify: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            user_slots: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            queued: Arc::new(AtomicUsize::new(0)),
            http,
        }
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
}

pub const BOT_DESCRIPTION: &str =
    "Send me a YouTube, TikTok, Instagram, or X link and I'll fetch the video or audio for you.";
pub const BOT_SHORT_DESCRIPTION: &str =
    "Send a link, get video or audio back. YouTube, TikTok, Instagram, X.";

const WELCOME: &str =
    "Send me a YouTube, TikTok, Instagram, or X link and I'll fetch the video or audio for you.";
const HELP: &str = "Send a link → tap 🎬 Video or 🎵 Audio → pick quality.\n\nCommands:\n/start — welcome\n/help — this guide\n\nLimits: 20 downloads/hour, 2 at a time, 2 GB max file size.";

/// Register menu commands + profile texts. Idempotent, best-effort:
/// logs a warning on failure, never fails startup (e.g. no network).
pub async fn init_telegram(bot: &Bot) {
    if let Err(e) = bot
        .set_my_commands(Command::bot_commands())
        .scope(BotCommandScope::AllPrivateChats)
        .await
    {
        tracing::warn!("setMyCommands failed: {e}");
    }
    if let Err(e) = bot.set_my_description().description(BOT_DESCRIPTION).await {
        tracing::warn!("setMyDescription failed: {e}");
    }
    if let Err(e) = bot
        .set_my_short_description()
        .short_description(BOT_SHORT_DESCRIPTION)
        .await
    {
        tracing::warn!("setMyShortDescription failed: {e}");
    }
}

/// Entry point for text messages.
pub async fn handle_message(bot: Bot, msg: Message, state: AppState) -> Result<()> {
    let Some(text) = msg.text() else {
        return Ok(());
    };

    if let Ok(cmd) = Command::parse(text, "fetchly") {
        let reply = match cmd {
            Command::Start => WELCOME,
            Command::Help => HELP,
        };
        bot.send_message(msg.chat.id, reply).await?;
        return Ok(());
    }
    // Also handle bare `/start`/`/help` with bot username suffix.
    if text.starts_with("/start") {
        bot.send_message(msg.chat.id, WELCOME).await?;
        return Ok(());
    }
    if text.starts_with("/help") {
        bot.send_message(msg.chat.id, HELP).await?;
        return Ok(());
    }

    let Some(raw_url) = extract_url(text) else {
        bot.send_message(
            msg.chat.id,
            "Send a link and I'll fetch it. /help for details.",
        )
        .await?;
        return Ok(());
    };

    let media = match url::parse(&raw_url) {
        Ok(m) => m,
        Err(e) => {
            bot.send_message(msg.chat.id, e.user_message()).await?;
            return Ok(());
        }
    };

    let status = bot.send_message(msg.chat.id, "🔍 Resolving link…").await?;
    let meta = match ytdlp::resolve(&media).await {
        Ok(m) => m,
        Err(e) => {
            bot.edit_message_text(msg.chat.id, status.id, e.user_message())
                .await?;
            return Ok(());
        }
    };

    let hash = url::url_hash(&media.url);
    let session_id = match state.sessions.create(&hash, &media.url, &meta).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("session create failed: {e}");
            bot.edit_message_text(msg.chat.id, status.id, e.user_message())
                .await?;
            return Ok(());
        }
    };

    let caption = keyboard::preview_text(&meta);
    let markup = keyboard::preview_keyboard(&session_id);
    // Delete the "resolving" placeholder, then send the preview card.
    let _ = bot.delete_message(msg.chat.id, status.id).await;
    send_preview(&bot, msg.chat.id, &meta, &caption, markup, &state.http).await?;
    Ok(())
}

/// Send preview card: photo + caption when a thumbnail exists, else plain text.
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

/// First URL-like token in free text.
///
/// Leading `<` / trailing `>` wrapping (some clients autolink that way) is
/// stripped before the scheme check.
pub(crate) fn extract_url(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|t| t.trim().trim_matches(|c| c == '<' || c == '>'))
        .find(|t| t.starts_with("http://") || t.starts_with("https://"))
        .map(str::to_owned)
}

/// Entry point for all callback queries.
pub async fn handle_callback(bot: Bot, q: CallbackQuery, state: AppState) -> Result<()> {
    let Some(data) = q.data.clone() else {
        return Ok(());
    };
    let Some((chat, msg_id)) = callback_origin(&q) else {
        return Ok(());
    };
    let user_id = q.from.id.0;

    let Some((kind, quality, session_id)) = crate::session::parse_callback(&data) else {
        bot.answer_callback_query(q.id)
            .text("Outdated button. Send the link again.")
            .await?;
        return Ok(());
    };

    // ❌ Cancel: wired to the in-flight download token.
    if data.starts_with("cancel:") {
        let token = state.downloads.lock().await.remove(&session_id);
        if let Some(t) = token {
            t.cancel();
            bot.answer_callback_query(q.id).text("Cancelling…").await?;
        } else {
            bot.answer_callback_query(q.id)
                .text("Nothing to cancel.")
                .await?;
            let _ = state.sessions.delete(&session_id).await;
            edit_preview_text(&bot, chat, msg_id, "Cancelled.").await;
        }
        return Ok(());
    }

    let Ok(session) = state.sessions.get(&session_id).await else {
        bot.answer_callback_query(q.id)
            .text("Session expired. Send the link again.")
            .await?;
        edit_preview_text(&bot, chat, msg_id, "Session expired. Send the link again.").await;
        return Ok(());
    };

    match (kind, quality.as_str()) {
        ('v', "pick") => {
            bot.answer_callback_query(q.id).await?;
            bot.edit_message_reply_markup(chat, msg_id)
                .reply_markup(keyboard::video_quality_keyboard(
                    &session.metadata,
                    &session_id,
                ))
                .await?;
        }
        ('a', "pick") => {
            bot.answer_callback_query(q.id).await?;
            bot.edit_message_reply_markup(chat, msg_id)
                .reply_markup(keyboard::audio_quality_keyboard(
                    &session.metadata,
                    &session_id,
                ))
                .await?;
        }
        ('v', code) => {
            let Some(quality) = VideoQuality::parse_code(code) else {
                bot.answer_callback_query(q.id)
                    .text("Unknown quality.")
                    .await?;
                return Ok(());
            };
            bot.answer_callback_query(q.id).await?;
            // Detached: the chat's update queue must stay live for ❌ taps
            // while the multi-minute pipeline runs (see fix 3 notes).
            tokio::spawn(run_video(
                bot, chat, msg_id, user_id, session_id, session, quality, state,
            ));
        }
        ('a', code) => {
            let Some(quality) = AudioQuality::parse_code(code) else {
                bot.answer_callback_query(q.id)
                    .text("Unknown quality.")
                    .await?;
                return Ok(());
            };
            bot.answer_callback_query(q.id).await?;
            tokio::spawn(run_audio(
                bot, chat, msg_id, user_id, session_id, session, quality, state,
            ));
        }
        _ => {
            bot.answer_callback_query(q.id).await?;
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

/// Forward 0–100 download progress to a throttled Telegram progress message.
///
/// `scale_to` reserves headroom for later stages: video scales to 100,
/// audio to 80 (the last 20% of the bar is convert + upload).
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

/// Attach `ID3` tags + cover art to a converted `MP3`.
///
/// Best-effort: tagging never fails the download, it only logs.
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

/// Standard Bot API caps uploads at 50 MB; larger files need a Local Bot API
/// Server (see `TELEGRAM_API_URL`). Checked before attempting a doomed upload.
const STANDARD_UPLOAD_LIMIT: u64 = 50_000_000;

const OVER_LIMIT_MSG: &str = "This file is over 50 MB, which exceeds the standard Bot API upload limit. Pick a lower quality, or set up a Local Bot API Server for files up to 2 GB.";

async fn exceeds_standard_limit(path: &std::path::Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|m| m.len() > STANDARD_UPLOAD_LIMIT)
}

/// Map an upload failure to user text. A 413 from Telegram means the file
/// passed our 2 GB guard but exceeds the 50 MB standard-API cap (e.g. the
/// size estimate was off or the limit check was bypassed by cache racing).
fn upload_error_message(raw: &str) -> String {
    if raw.to_lowercase().contains("too large") {
        OVER_LIMIT_MSG.to_owned()
    } else {
        Error::Telegram(raw.to_owned()).user_message()
    }
}

/// Edit a preview card that may be a text message or a photo caption.
/// Telegram rejects `editMessageText` on photos ("no text in the message"),
/// so fall back to `editMessageCaption`.
async fn edit_preview_text(bot: &Bot, chat: ChatId, msg: MessageId, text: &str) {
    if bot.edit_message_text(chat, msg, text).await.is_ok() {
        return;
    }
    let _ = bot.edit_message_caption(chat, msg).caption(text).await;
}

/// Acquire a download slot, telling the user they are queued when busy.
/// Over the global waiter cap (`max_queued`), reject immediately with a busy
/// notice instead of parking another task — this is the backpressure bound
/// that keeps overload from growing memory and update-queue depth.
async fn acquire_permit(
    bot: &Bot,
    chat: ChatId,
    origin_msg: MessageId,
    state: &AppState,
) -> Option<tokio::sync::OwnedSemaphorePermit> {
    let parked = state.queued.fetch_add(1, Ordering::SeqCst);
    if parked >= state.config.max_queued {
        state.queued.fetch_sub(1, Ordering::SeqCst);
        tracing::warn!("waiter cap hit ({parked} parked); rejecting tap");
        edit_preview_text(
            bot,
            chat,
            origin_msg,
            "🔥 Fetchly is busy right now. Try again in a bit.",
        )
        .await;
        return None;
    }
    if state.semaphore.available_permits() == 0 {
        tracing::info!("download queued (depth {})", parked + 1);
        edit_preview_text(
            bot,
            chat,
            origin_msg,
            "⏳ Queued… your download starts automatically.",
        )
        .await;
    }
    let permit = state.semaphore.clone().acquire_owned().await.ok();
    state.queued.fetch_sub(1, Ordering::SeqCst);
    permit
}

/// Run a pipeline stage against an absolute deadline (whole-pipeline budget:
/// callers share one deadline across download + convert). On expiry the
/// stage's token is cancelled — subprocesses die via `kill_on_drop` plus the
/// explicit kill on cancel, so the awaited future resolves promptly — and the
/// caller sees [`Error::TimedOut`] instead of [`Error::Cancelled`].
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

/// Best-effort restart notice to every queued/in-flight chat, then cancel all
/// download tokens. Bounded by `NOTIFY_TIMEOUT_SECS`; send failures are
/// logged, never fatal — shutdown must not hang on a dead network.
const RESTART_MSG: &str =
    "🔄 Fetchly is restarting. Your download was stopped — please resend your link in a minute.";
const NOTIFY_TIMEOUT_SECS: u64 = 15;

pub async fn shutdown_notify(bot: Bot, state: &AppState) {
    let chats: HashSet<i64> = state.notify.lock().await.values().map(|c| c.0).collect();
    tracing::info!("shutdown: notifying {} chats", chats.len());
    let sends = async {
        let mut set = tokio::task::JoinSet::new();
        for chat_id in chats {
            let bot = bot.clone();
            set.spawn(async move {
                if let Err(e) = bot.send_message(ChatId(chat_id), RESTART_MSG).await {
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

/// Take one of the user's in-flight slots (queued + running count toward
/// `max_per_user`). Returns `false` when the user is at their cap.
async fn acquire_user_slot(state: &AppState, user_id: u64) -> bool {
    let mut slots = state.user_slots.lock().await;
    let used = slots.get(&user_id).copied().unwrap_or(0);
    if used >= state.config.max_per_user {
        return false;
    }
    slots.insert(user_id, used + 1);
    true
}

/// Release a slot taken by [`acquire_user_slot`]. Called on every pipeline
/// exit after acquisition — audit these sites when adding new returns.
async fn release_user_slot(state: &AppState, user_id: u64) {
    let mut slots = state.user_slots.lock().await;
    if let Some(used) = slots.get_mut(&user_id) {
        *used = used.saturating_sub(1);
        if *used == 0 {
            slots.remove(&user_id);
        }
    }
}

/// Video download pipeline: rate limit → cache → semaphore → yt-dlp → upload.
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
) {
    let quality_code = quality.as_str().to_owned();

    if let Err(e) = state.limiter.check_and_consume(user_id).await {
        respond(bot, chat, e).await;
        return;
    }

    // Instant path: same URL+format+quality seen before.
    if let Ok(Some(file_id)) = state
        .cache
        .get(&session.url_hash, "video", &quality_code)
        .await
    {
        let sent = bot
            .send_video(chat, InputFile::file_id(teloxide::types::FileId(file_id)))
            .caption(format!("{}\n{}", session.metadata.title, quality.label()))
            .await;
        // A stale file_id (message deleted upstream) falls through to re-download.
        if sent.is_ok() {
            return;
        }
    }

    // One user must not hold every worker: queued + running count toward the cap.
    if !acquire_user_slot(&state, user_id).await {
        respond(
            bot,
            chat,
            Error::TooManyConcurrent {
                max: state.config.max_per_user,
            },
        )
        .await;
        return;
    }

    // Queue when all workers are busy.
    // Register for shutdown notices first: parked waiters are invisible otherwise.
    state.notify.lock().await.insert(session_id.clone(), chat);
    let Some(_permit) = acquire_permit(&bot, chat, origin_msg, &state).await else {
        state.notify.lock().await.remove(&session_id);
        release_user_slot(&state, user_id).await;
        return;
    };

    // Whole-pipeline budget (download + upload); queued time does not count.
    let timeout_secs = state.config.download_timeout_secs;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    let cancel = CancellationToken::new();
    state
        .downloads
        .lock()
        .await
        .insert(session_id.clone(), cancel.clone());

    let progress_msg = bot
        .send_message(chat, "⬇️ Downloading ░░░░░░░░░░ 0%")
        .reply_markup(keyboard::cancel_keyboard(&session_id))
        .await;
    let Ok(progress_msg) = progress_msg else {
        state.downloads.lock().await.remove(&session_id);
        state.notify.lock().await.remove(&session_id);
        release_user_slot(&state, user_id).await;
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
        "⬇️ Downloading",
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
                .edit_message_text(chat, progress_msg.id, "Cancelled.")
                .await;
            cleanup(&work_dir).await;
        }
        Err(e) => {
            tracing::warn!("video download failed: {e}");
            let _ = bot
                .edit_message_text(chat, progress_msg.id, e.user_message())
                .await;
            cleanup(&work_dir).await;
        }
        Ok(path) => {
            if exceeds_standard_limit(&path).await && state.config.api_url.is_none() {
                let _ = bot
                    .edit_message_text(chat, progress_msg.id, OVER_LIMIT_MSG)
                    .await;
                cleanup(&work_dir).await;
                return;
            }
            upload_video(
                &bot,
                chat,
                progress_msg.id,
                &path,
                &session,
                quality,
                &state,
            )
            .await;
            cleanup(&work_dir).await;
        }
    }
}

/// Upload a finished video file, cache its `file_id`.
async fn upload_video(
    bot: &Bot,
    chat: ChatId,
    progress_msg: MessageId,
    path: &std::path::Path,
    session: &StoredSession,
    quality: VideoQuality,
    state: &AppState,
) {
    progress_msg_update(bot, chat, progress_msg, "🔄 Uploading…").await;
    let caption = format!(
        "{}\n{} · {}",
        session.metadata.title,
        quality.label(),
        session.metadata.platform.as_str()
    );
    match bot
        .send_video(chat, InputFile::file(path.to_owned()))
        .caption(caption)
        .await
    {
        Ok(sent) => {
            if let Some(video) = sent.video() {
                let _ = state
                    .cache
                    .set(
                        &session.url_hash,
                        "video",
                        quality.as_str(),
                        &video.file.id.to_string(),
                    )
                    .await;
            }
            let _ = bot.delete_message(chat, progress_msg).await;
        }
        Err(e) => {
            tracing::warn!("send_video failed: {e}");
            let _ = bot
                .edit_message_text(chat, progress_msg, upload_error_message(&e.to_string()))
                .await;
        }
    }
}

/// Audio pipeline: rate limit → cache → semaphore → yt-dlp → ffmpeg → tag → upload.
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
) {
    let quality_code = quality.as_str().to_owned();

    if let Err(e) = state.limiter.check_and_consume(user_id).await {
        respond(bot, chat, e).await;
        return;
    }

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

    // One user must not hold every worker: queued + running count toward the cap.
    if !acquire_user_slot(&state, user_id).await {
        respond(
            bot,
            chat,
            Error::TooManyConcurrent {
                max: state.config.max_per_user,
            },
        )
        .await;
        return;
    }

    // Register for shutdown notices first: parked waiters are invisible otherwise.
    state.notify.lock().await.insert(session_id.clone(), chat);
    let Some(_permit) = acquire_permit(&bot, chat, origin_msg, &state).await else {
        state.notify.lock().await.remove(&session_id);
        release_user_slot(&state, user_id).await;
        return;
    };

    // Whole-pipeline budget (download + convert + upload); queued time does not count.
    let timeout_secs = state.config.download_timeout_secs;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    let cancel = CancellationToken::new();
    state
        .downloads
        .lock()
        .await
        .insert(session_id.clone(), cancel.clone());

    let progress_msg = bot
        .send_message(chat, "⬇️ Downloading ░░░░░░░░░░ 0%")
        .reply_markup(keyboard::cancel_keyboard(&session_id))
        .await;
    let Ok(progress_msg) = progress_msg else {
        state.downloads.lock().await.remove(&session_id);
        state.notify.lock().await.remove(&session_id);
        release_user_slot(&state, user_id).await;
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

    let progress_task =
        spawn_progress_forwarder(rx, bot.clone(), chat, progress_msg.id, "⬇️ Downloading", 80);

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
            progress_msg_update(&bot, chat, progress_msg.id, "🔄 Converting…").await;
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
                .edit_message_text(chat, progress_msg.id, "Cancelled.")
                .await;
            cleanup(&work_dir).await;
        }
        Err(e) => {
            tracing::warn!("audio pipeline failed: {e}");
            let _ = bot
                .edit_message_text(chat, progress_msg.id, e.user_message())
                .await;
            cleanup(&work_dir).await;
        }
        Ok(path) => {
            if exceeds_standard_limit(&path).await && state.config.api_url.is_none() {
                let _ = bot
                    .edit_message_text(chat, progress_msg.id, OVER_LIMIT_MSG)
                    .await;
                cleanup(&work_dir).await;
                return;
            }
            upload_audio(
                &bot,
                chat,
                progress_msg.id,
                &path,
                &session,
                quality,
                &state,
            )
            .await;
            cleanup(&work_dir).await;
        }
    }
}

/// Upload a finished `MP3`, cache its `file_id`, and retire the progress message.
async fn upload_audio(
    bot: &Bot,
    chat: ChatId,
    progress_msg: MessageId,
    path: &std::path::Path,
    session: &StoredSession,
    quality: AudioQuality,
    state: &AppState,
) {
    progress_msg_update(bot, chat, progress_msg, "⬆️ Uploading…").await;
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
            if let Some(audio) = sent.audio() {
                let _ = state
                    .cache
                    .set(
                        &session.url_hash,
                        "audio",
                        quality.as_str(),
                        &audio.file.id.to_string(),
                    )
                    .await;
            }
            let _ = bot.delete_message(chat, progress_msg).await;
        }
        Err(e) => {
            tracing::warn!("send_audio failed: {e}");
            let _ = bot
                .edit_message_text(chat, progress_msg, upload_error_message(&e.to_string()))
                .await;
        }
    }
}

async fn respond(bot: Bot, chat: ChatId, e: Error) {
    match e {
        Error::RateLimited {
            retry_in_secs,
            remaining: _,
        } => {
            let _ = bot
                .send_message(
                    chat,
                    format!("Too many requests. Try again in {retry_in_secs} seconds."),
                )
                .await;
        }
        other => {
            let _ = bot.send_message(chat, other.user_message()).await;
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

/// Build the dispatcher schema: commands + URL messages + callbacks.
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
        assert_eq!(
            upload_error_message("A Telegram's error: Request Entity Too Large"),
            OVER_LIMIT_MSG
        );
        assert_eq!(
            upload_error_message("Bad Request: chat not found"),
            "Something went wrong. Try again later."
        );
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
        // Mimics the pipeline stages: resolve promptly once cancelled.
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
            SessionStore::new(t.manager.clone()),
            RateLimiter::new(t.manager.clone(), 20),
            Arc::new(tokio::sync::Semaphore::new(2)),
            reqwest::Client::new(),
        );
        // Free permits: no Telegram calls, pure admission accounting.
        let bot = Bot::new("test-token");
        let permit = acquire_permit(&bot, ChatId(1), MessageId(1), &state).await;
        assert!(permit.is_some());
        assert_eq!(state.queued.load(Ordering::SeqCst), 0);
        drop(permit);
    }

    #[test]
    fn menu_commands_match_handlers() {
        let cmds = Command::bot_commands();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].command.trim_start_matches('/'), "start");
        assert_eq!(cmds[1].command.trim_start_matches('/'), "help");
        assert!(!cmds[0].description.is_empty());
        assert!(!cmds[1].description.is_empty());
        let text = Command::descriptions().to_string();
        assert!(text.contains("/start"));
        assert!(text.contains("/help"));
    }

    #[test]
    fn profile_texts_fit_telegram_limits() {
        assert!(!BOT_DESCRIPTION.is_empty() && BOT_DESCRIPTION.len() <= 512);
        assert!(!BOT_SHORT_DESCRIPTION.is_empty() && BOT_SHORT_DESCRIPTION.len() <= 120);
    }
}
