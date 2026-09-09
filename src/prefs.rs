use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};
use crate::i18n::Lang;

/// Durable per-user language choice (`user_id → en|ru`).
///
/// Written only by an explicit `/language` tap, read on every update.
/// Same blocking discipline as [`crate::cache::FileCache`]: `rusqlite` is
/// synchronous, so every query runs on a blocking thread. Own connection to
/// the same DB file — safe under WAL, contention is near zero (one PK lookup
/// per update, writes only on language change).
#[derive(Debug, Clone)]
pub struct UserPrefs {
    inner: Arc<Mutex<rusqlite::Connection>>,
}

const MIGRATION: &str = "PRAGMA journal_mode=WAL;
                  PRAGMA synchronous=NORMAL;
                  CREATE TABLE IF NOT EXISTS user_lang (
                      user_id    INTEGER NOT NULL PRIMARY KEY,
                      lang       TEXT NOT NULL,
                      updated_at INTEGER NOT NULL
                  );";

impl UserPrefs {
    /// Open (creating parent dirs) and run migrations.
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }
        let path: PathBuf = path.to_owned();
        let conn = tokio::task::spawn_blocking(move || {
            let conn =
                rusqlite::Connection::open(&path).map_err(|e| Error::Cache(e.to_string()))?;
            conn.execute_batch(MIGRATION)
                .map_err(|e| Error::Cache(e.to_string()))?;
            Ok::<_, Error>(conn)
        })
        .await
        .map_err(|e| Error::Cache(format!("spawn failed: {e}")))?;
        let conn = conn?;

        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    /// Open an in-memory DB (tests).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE user_lang (
                 user_id    INTEGER NOT NULL PRIMARY KEY,
                 lang       TEXT NOT NULL,
                 updated_at INTEGER NOT NULL
             );",
        )?;
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    /// Explicit choice, if the user ever tapped `/language`.
    /// `Ok(None)` covers both never-set and garbage rows (caller falls back
    /// to the Telegram `language_code` guess).
    pub async fn get(&self, user_id: u64) -> Result<Option<Lang>> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            let conn = inner.lock().map_err(|e| Error::Cache(e.to_string()))?;
            let mut stmt = conn
                .prepare("SELECT lang FROM user_lang WHERE user_id=?1")
                .map_err(|e| Error::Cache(e.to_string()))?;
            let mut rows = stmt
                .query([sqlite_id(user_id)])
                .map_err(|e| Error::Cache(e.to_string()))?;
            match rows.next().map_err(|e| Error::Cache(e.to_string()))? {
                Some(row) => {
                    let raw: String = row.get(0).map_err(|e| Error::Cache(e.to_string()))?;
                    Ok(Lang::from_code(&raw))
                }
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| Error::Cache(format!("prefs task failed: {e}")))?
    }

    /// Record an explicit `/language` choice (insert or overwrite).
    pub async fn set(&self, user_id: u64, lang: Lang) -> Result<()> {
        let inner = Arc::clone(&self.inner);
        let code = lang.as_str().to_owned();
        tokio::task::spawn_blocking(move || {
            let conn = inner.lock().map_err(|e| Error::Cache(e.to_string()))?;
            let now = i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs()),
            )
            .unwrap_or(0);
            conn.execute(
                "INSERT OR REPLACE INTO user_lang (user_id, lang, updated_at)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![sqlite_id(user_id), code, now],
            )
            .map_err(|e| Error::Cache(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Cache(format!("prefs task failed: {e}")))?
    }
}

/// Telegram user ids fit `i64` by a wide margin; 0 is unused, so a saturating
/// fallback can never collide with a real user.
fn sqlite_id(user_id: u64) -> i64 {
    i64::try_from(user_id).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip_and_overwrite() {
        let prefs = UserPrefs::open_in_memory().expect("open");
        assert_eq!(prefs.get(7).await.expect("get"), None);
        prefs.set(7, Lang::Ru).await.expect("set");
        assert_eq!(prefs.get(7).await.expect("get"), Some(Lang::Ru));
        prefs.set(7, Lang::En).await.expect("set");
        assert_eq!(prefs.get(7).await.expect("get"), Some(Lang::En));
    }

    #[tokio::test]
    async fn users_are_isolated() {
        let prefs = UserPrefs::open_in_memory().expect("open");
        prefs.set(1, Lang::Ru).await.expect("set");
        assert_eq!(prefs.get(2).await.expect("get"), None);
        assert_eq!(prefs.get(1).await.expect("get"), Some(Lang::Ru));
    }
}
