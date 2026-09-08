use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

/// Default per-download wall-clock budget (15 minutes).
const DEFAULT_DOWNLOAD_TIMEOUT_SECS: u64 = 900;

/// Runtime configuration, all from environment (see `.env.example`).
#[derive(Debug, Clone)]
pub struct Config {
    /// Telegram Bot API token (`TELEGRAM_BOT_TOKEN`, required).
    pub bot_token: String,
    /// Optional custom Bot API base URL (Local Bot API Server for >50 MB files).
    pub api_url: Option<String>,
    /// Max concurrent downloads (global semaphore).
    pub max_workers: usize,
    /// Max wall-clock seconds for one download pipeline (yt-dlp + convert).
    /// Queued time does not count; the clock starts at semaphore acquisition.
    pub download_timeout_secs: u64,
    /// Downloads allowed per user per hour.
    pub rate_limit: u32,
    /// `SQLite` file for the `file_id` cache.
    pub db_path: PathBuf,
    /// Directory for temp downloads (`/tmp/fetchly/{session_id}/`).
    pub temp_dir: PathBuf,
    /// Redis URL for sessions + rate limiting.
    pub redis_url: String,
}

impl Config {
    /// Load from environment. Missing `.env` file is fine (env may come from Docker).
    pub fn from_env() -> Result<Self, crate::error::Error> {
        let _ = dotenvy::dotenv();
        Self::from_map(&env::vars().collect())
    }

    /// Pure constructor over an explicit map (keeps tests hermetic —
    /// no global `env` mutation, so tests can run in parallel).
    fn from_map(vars: &HashMap<String, String>) -> Result<Self, crate::error::Error> {
        let bot_token = vars.get("TELEGRAM_BOT_TOKEN").cloned().unwrap_or_default();
        if bot_token.trim().is_empty() {
            return Err(crate::error::Error::Config(
                "TELEGRAM_BOT_TOKEN is not set. See .env.example.".to_owned(),
            ));
        }

        let positive = |key: &str| {
            vars.get(key)
                .and_then(|v| v.parse().ok())
                .filter(|&n: &usize| n > 0)
        };
        let positive_u32 = |key: &str| {
            vars.get(key)
                .and_then(|v| v.parse().ok())
                .filter(|&n: &u32| n > 0)
        };
        let positive_u64 = |key: &str| {
            vars.get(key)
                .and_then(|v| v.parse().ok())
                .filter(|&n: &u64| n > 0)
        };

        Ok(Self {
            bot_token,
            api_url: vars
                .get("TELEGRAM_API_URL")
                .filter(|s| !s.trim().is_empty())
                .cloned(),
            max_workers: positive("FETCHLY_MAX_WORKERS").unwrap_or(4),
            download_timeout_secs: positive_u64("FETCHLY_DOWNLOAD_TIMEOUT_SECS")
                .unwrap_or(DEFAULT_DOWNLOAD_TIMEOUT_SECS),
            rate_limit: positive_u32("FETCHLY_RATE_LIMIT").unwrap_or(20),
            db_path: vars
                .get("FETCHLY_DB_PATH")
                .map_or_else(|| PathBuf::from("./fetchly.db"), PathBuf::from),
            temp_dir: vars
                .get("FETCHLY_TEMP_DIR")
                .map_or_else(|| PathBuf::from("/tmp/fetchly"), PathBuf::from),
            redis_url: vars
                .get("REDIS_URL")
                .cloned()
                .unwrap_or_else(|| "redis://127.0.0.1:6379".to_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn defaults_apply() {
        let cfg =
            Config::from_map(&vars(&[("TELEGRAM_BOT_TOKEN", "test-token")])).expect("config loads");
        assert_eq!(cfg.max_workers, 4);
        assert_eq!(cfg.rate_limit, 20);
        assert_eq!(cfg.download_timeout_secs, 900);
        assert!(cfg.api_url.is_none());
        assert_eq!(cfg.redis_url, "redis://127.0.0.1:6379");
    }

    #[test]
    fn overrides_apply() {
        let cfg = Config::from_map(&vars(&[
            ("TELEGRAM_BOT_TOKEN", "t"),
            ("FETCHLY_MAX_WORKERS", "8"),
            ("FETCHLY_RATE_LIMIT", "5"),
            ("FETCHLY_DOWNLOAD_TIMEOUT_SECS", "300"),
            ("TELEGRAM_API_URL", "http://botapi:8081"),
        ]))
        .expect("config loads");
        assert_eq!(cfg.max_workers, 8);
        assert_eq!(cfg.rate_limit, 5);
        assert_eq!(cfg.download_timeout_secs, 300);
        assert_eq!(cfg.api_url.as_deref(), Some("http://botapi:8081"));
    }

    #[test]
    fn invalid_numbers_fall_back_to_defaults() {
        let cfg = Config::from_map(&vars(&[
            ("TELEGRAM_BOT_TOKEN", "t"),
            ("FETCHLY_MAX_WORKERS", "0"),
            ("FETCHLY_RATE_LIMIT", "banana"),
            ("FETCHLY_DOWNLOAD_TIMEOUT_SECS", "0"),
        ]))
        .expect("config loads");
        assert_eq!(cfg.max_workers, 4);
        assert_eq!(cfg.rate_limit, 20);
        assert_eq!(cfg.download_timeout_secs, 900);
    }

    #[test]
    fn missing_token_errors() {
        assert!(Config::from_map(&vars(&[])).is_err());
        assert!(Config::from_map(&vars(&[("TELEGRAM_BOT_TOKEN", "  ")])).is_err());
    }
}
