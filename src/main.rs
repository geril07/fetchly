mod cache;
mod config;
mod error;
mod limiter;
mod media;
mod session;
mod telegram;
#[cfg(test)]
mod testutil;

use std::sync::Arc;

use teloxide::dispatching::Dispatcher;
use teloxide::prelude::*;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::cache::FileCache;
use crate::config::Config;
use crate::limiter::RateLimiter;
use crate::session::SessionStore;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    let config = Config::from_env().map_err(|e| anyhow::anyhow!("{e}"))?;
    tracing::info!(
        "fetchly starting: workers={} rate={}/h db={} tmp={}",
        config.max_workers,
        config.rate_limit,
        config.db_path.display(),
        config.temp_dir.display()
    );
    tokio::fs::create_dir_all(&config.temp_dir).await?;

    // Remove stale temp dirs from previous runs (>10 min old safety net).
    spawn_temp_cleanup(config.temp_dir.clone());

    // Redis: sessions + rate limiting (ConnectionManager auto-reconnects).
    let redis_client = redis::Client::open(config.redis_url.as_str())?;
    let manager = redis_client.get_connection_manager().await?;
    let sessions = SessionStore::new(manager.clone());
    let limiter = RateLimiter::new(manager, config.rate_limit);

    let cache = FileCache::open(&config.db_path).await?;

    let mut bot = Bot::new(config.bot_token.clone());
    if let Some(api_url) = &config.api_url {
        match api_url.parse() {
            Ok(url) => {
                bot = bot.set_api_url(url);
                tracing::info!("using custom Bot API server: {api_url}");
            }
            Err(e) => tracing::warn!("invalid TELEGRAM_API_URL ({e}); using default"),
        }
    }

    let semaphore = Arc::new(tokio::sync::Semaphore::new(config.max_workers));
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let state = telegram::AppState::new(config, cache, sessions, limiter, semaphore, http);
    let notify_bot = bot.clone();

    // Register menu commands + profile texts (idempotent, best-effort).
    telegram::init_telegram(&bot).await;

    let mut dispatcher = Dispatcher::builder(bot, telegram::schema())
        .dependencies(dptree::deps![state.clone()])
        .enable_ctrlc_handler()
        .build();

    // Graceful shutdown: SIGINT/SIGTERM stops polling after waiters get a
    // restart notice and download tokens are cancelled (120s grace in
    // compose). Telegram queues updates during the gap — nothing is lost.
    tokio::select! {
        () = dispatcher.dispatch() => {},
        () = shutdown_signal() => {
            tracing::info!("shutdown signal received");
            telegram::shutdown_notify(notify_bot, &state).await;
        }
    }
    Ok(())
}

/// SIGINT (Ctrl-C) plus SIGTERM (`docker stop` sends TERM; without this the
/// process would die instantly with no notices sent).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(stream) => stream,
            Err(e) => {
                tracing::warn!("SIGTERM handler failed ({e}); watching SIGINT only");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn init_logging() {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "fetchly=info,teloxide=warn".into());
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(filter))
        .with(tracing_subscriber::fmt::layer())
        .init();
}

/// Background task: delete anything in the temp dir older than 10 minutes.
fn spawn_temp_cleanup(temp_dir: std::path::PathBuf) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
        loop {
            interval.tick().await;
            let Ok(mut entries) = tokio::fs::read_dir(&temp_dir).await else {
                continue;
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                let Ok(meta) = entry.metadata().await else {
                    continue;
                };
                let old = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|d| d.as_secs() > 600);
                if old {
                    let _ = tokio::fs::remove_dir_all(entry.path()).await;
                }
            }
        }
    });
}
