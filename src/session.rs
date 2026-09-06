use redis::AsyncCommands;

use crate::error::{Error, Result};
use crate::media::ytdlp::Metadata;

/// Ephemeral per-preview state, kept between button presses.
///
/// Telegram callback data is limited to 64 bytes, so callbacks carry only
/// `v:720:<session_id>` / `a:320:<session_id>` and the full [`Metadata`]
/// lives here with a 10-minute TTL.
#[derive(Debug, Clone)]
pub struct SessionStore {
    manager: redis::aio::ConnectionManager,
}

impl SessionStore {
    pub fn new(manager: redis::aio::ConnectionManager) -> Self {
        Self { manager }
    }

    fn key(session_id: &str) -> String {
        format!("session:{session_id}")
    }

    /// Store metadata, returning the generated 8-char session ID.
    pub async fn create(&self, url_hash: &str, url: &str, metadata: &Metadata) -> Result<String> {
        let session_id = new_session_id();
        let payload = serde_json::json!({
            "url_hash": url_hash,
            "url": url,
            "metadata": metadata,
        });
        let mut conn = self.manager.clone();
        let _: () = conn
            .set_ex(
                Self::key(&session_id),
                serde_json::to_string(&payload).map_err(|e| Error::Session(e.to_string()))?,
                600,
            )
            .await
            .map_err(|e| Error::Session(e.to_string()))?;
        Ok(session_id)
    }

    pub async fn get(&self, session_id: &str) -> Result<StoredSession> {
        let mut conn = self.manager.clone();
        let raw: Option<String> = conn
            .get(Self::key(session_id))
            .await
            .map_err(|e| Error::Session(e.to_string()))?;
        let raw = raw.ok_or(Error::SessionExpired)?;
        serde_json::from_str(&raw).map_err(|_| Error::SessionExpired)
    }

    pub async fn delete(&self, session_id: &str) -> Result<()> {
        let mut conn = self.manager.clone();
        let _: i64 = conn
            .del(Self::key(session_id))
            .await
            .map_err(|e| Error::Session(e.to_string()))?;
        Ok(())
    }
}

/// What [`SessionStore::get`] returns.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredSession {
    pub url_hash: String,
    pub url: String,
    pub metadata: Metadata,
}

fn new_session_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_owned()
}

/// Parse callback data `v:720:<session_id>` / `a:320:<session_id>` / `cancel:<session_id>`.
///
/// Returns `(kind, quality_code, session_id)`.
pub fn parse_callback(data: &str) -> Option<(char, String, String)> {
    let mut parts = data.splitn(3, ':');
    let kind = parts.next()?;
    let quality = parts.next()?;
    let session = parts.next()?;
    if session.is_empty() || session.len() > 32 {
        return None;
    }
    match kind {
        "v" | "a" | "cancel" => {
            Some((kind.chars().next()?, quality.to_owned(), session.to_owned()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_parses() {
        assert_eq!(
            parse_callback("v:720:abc123xy"),
            Some(('v', "720".to_owned(), "abc123xy".to_owned()))
        );
        assert_eq!(
            parse_callback("a:best:abc123xy"),
            Some(('a', "best".to_owned(), "abc123xy".to_owned()))
        );
        assert_eq!(
            parse_callback("cancel:x:abc123xy"),
            Some(('c', "x".to_owned(), "abc123xy".to_owned()))
        );
        assert_eq!(parse_callback("x:720:abc"), None);
        assert_eq!(parse_callback("v:720:"), None);
        assert_eq!(parse_callback("garbage"), None);
    }

    #[test]
    fn callback_fits_telegram_limit() {
        // Telegram caps callback_data at 64 bytes.
        for data in ["v:720:abcdefgh", "a:best:abcdefgh", "cancel:x:abcdefgh"] {
            assert!(data.len() <= 64, "{data}");
        }
    }

    fn test_metadata() -> Metadata {
        use crate::media::url::Platform;
        Metadata {
            title: "T".to_owned(),
            duration_secs: Some(60),
            thumbnail_url: None,
            platform: Platform::Youtube,
            webpage_url: "https://youtu.be/x".to_owned(),
            view_count: None,
            uploader: None,
            video_options: vec![],
            audio_options: vec![],
        }
    }

    /// Each test spins its own Redis; skips (loudly) without Docker.
    /// The returned guard keeps the container alive for the whole test —
    /// dropping it kills Redis, so bind it in the test body.
    async fn test_redis() -> Option<crate::testutil::TestRedis> {
        crate::testutil::start_redis().await
    }

    #[tokio::test]
    async fn create_get_roundtrip() {
        let Some(t) = test_redis().await else { return };
        let store = SessionStore::new(t.manager.clone());
        let id = store
            .create("hash1", "https://youtu.be/x", &test_metadata())
            .await
            .expect("create");
        assert_eq!(id.len(), 8);
        let got = store.get(&id).await.expect("get");
        assert_eq!(got.url_hash, "hash1");
        assert_eq!(got.url, "https://youtu.be/x");
        assert_eq!(got.metadata.title, "T");
    }

    #[tokio::test]
    async fn get_missing_is_expired() {
        let Some(t) = test_redis().await else { return };
        let store = SessionStore::new(t.manager.clone());
        let err = store.get("doesnotexist").await.expect_err("missing");
        assert!(matches!(err, Error::SessionExpired));
    }

    #[tokio::test]
    async fn delete_removes_session() {
        let Some(t) = test_redis().await else { return };
        let store = SessionStore::new(t.manager.clone());
        let id = store
            .create("h", "https://youtu.be/x", &test_metadata())
            .await
            .expect("create");
        store.delete(&id).await.expect("delete");
        assert!(matches!(store.get(&id).await, Err(Error::SessionExpired)));
    }

    #[tokio::test]
    async fn session_key_has_10min_ttl() {
        let Some(t) = crate::testutil::start_redis().await else {
            return;
        };
        let store = SessionStore::new(t.manager.clone());
        let id = store
            .create("h", "https://youtu.be/x", &test_metadata())
            .await
            .expect("create");
        let mut conn = t
            .client
            .get_multiplexed_async_connection()
            .await
            .expect("redis conn");
        let ttl: i64 = redis::cmd("TTL")
            .arg(format!("session:{id}"))
            .query_async(&mut conn)
            .await
            .expect("TTL");
        assert!(ttl > 0 && ttl <= 600, "ttl={ttl}");
    }
}
