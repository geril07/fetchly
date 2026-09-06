use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};

/// Durable `{url_hash, format, quality} → telegram file_id` cache.
///
/// `rusqlite` is synchronous, so every method runs the query on a blocking
/// thread via `spawn_blocking`. Never call `rusqlite` directly on a tokio
/// worker thread.
#[derive(Debug, Clone)]
pub struct FileCache {
    inner: Arc<Mutex<rusqlite::Connection>>,
}

impl FileCache {
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
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=NORMAL;
                 CREATE TABLE IF NOT EXISTS file_cache (
                     url_hash   TEXT NOT NULL,
                     format     TEXT NOT NULL,
                     quality    TEXT NOT NULL,
                     file_id    TEXT NOT NULL,
                     created_at INTEGER NOT NULL,
                     PRIMARY KEY (url_hash, format, quality)
                 );",
            )
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
            "CREATE TABLE file_cache (
                 url_hash   TEXT NOT NULL,
                 format     TEXT NOT NULL,
                 quality    TEXT NOT NULL,
                 file_id    TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 PRIMARY KEY (url_hash, format, quality)
             );",
        )?;
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    pub async fn get(&self, url_hash: &str, format: &str, quality: &str) -> Result<Option<String>> {
        let inner = Arc::clone(&self.inner);
        let (url_hash, format, quality) =
            (url_hash.to_owned(), format.to_owned(), quality.to_owned());
        tokio::task::spawn_blocking(move || {
            let conn = inner.lock().map_err(|e| Error::Cache(e.to_string()))?;
            let mut stmt = conn
                .prepare(
                    "SELECT file_id FROM file_cache WHERE url_hash=?1 AND format=?2 AND quality=?3",
                )
                .map_err(|e| Error::Cache(e.to_string()))?;
            let mut rows = stmt
                .query([url_hash, format, quality])
                .map_err(|e| Error::Cache(e.to_string()))?;
            match rows.next().map_err(|e| Error::Cache(e.to_string()))? {
                Some(row) => row
                    .get::<_, String>(0)
                    .map(Some)
                    .map_err(|e| Error::Cache(e.to_string())),
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| Error::Cache(format!("cache task failed: {e}")))?
    }

    pub async fn set(
        &self,
        url_hash: &str,
        format: &str,
        quality: &str,
        file_id: &str,
    ) -> Result<()> {
        let inner = Arc::clone(&self.inner);
        let (url_hash, format, quality, file_id) = (
            url_hash.to_owned(),
            format.to_owned(),
            quality.to_owned(),
            file_id.to_owned(),
        );
        tokio::task::spawn_blocking(move || {
            let conn = inner.lock().map_err(|e| Error::Cache(e.to_string()))?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
                .unwrap_or(0);
            conn.execute(
                "INSERT OR REPLACE INTO file_cache
                     (url_hash, format, quality, file_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![url_hash, format, quality, file_id, now],
            )
            .map_err(|e| Error::Cache(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Cache(format!("cache task failed: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip() {
        let cache = FileCache::open_in_memory().expect("open");
        assert_eq!(cache.get("h", "video", "720").await.expect("get"), None);
        cache
            .set("h", "video", "720", "FILE123")
            .await
            .expect("set");
        assert_eq!(
            cache.get("h", "video", "720").await.expect("get"),
            Some("FILE123".to_owned())
        );
        // Different quality misses.
        assert_eq!(cache.get("h", "video", "1080").await.expect("get"), None);
    }
}
