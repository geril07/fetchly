use redis::AsyncCommands;

use crate::error::{Error, Result};

/// Per-user download rate limiting: N downloads per rolling hour.
///
/// Implemented with `INCR` + `EXPIRE` on `fetchly:ratelimit:{user_id}`.
/// The key holds the count for the current window; TTL is the seconds left.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    manager: redis::aio::ConnectionManager,
    max_per_hour: u32,
}

impl RateLimiter {
    pub fn new(manager: redis::aio::ConnectionManager, max_per_hour: u32) -> Self {
        Self {
            manager,
            max_per_hour,
        }
    }

    fn key(user_id: u64) -> String {
        format!("fetchly:ratelimit:{user_id}")
    }

    /// Consume one unit. On success returns remaining quota in the window.
    /// On exhaustion returns [`Error::RateLimited`] with TTL-based retry hint.
    pub async fn check_and_consume(&self, user_id: u64) -> Result<u32> {
        let mut conn = self.manager.clone();
        let key = Self::key(user_id);
        let count: i64 = conn
            .incr(&key, 1)
            .await
            .map_err(|e| Error::Limiter(e.to_string()))?;
        if count == 1 {
            let _: bool = conn
                .expire(&key, 3600)
                .await
                .map_err(|e| Error::Limiter(e.to_string()))?;
        }
        let max = i64::from(self.max_per_hour);
        if count > max {
            let ttl: i64 = conn
                .ttl(&key)
                .await
                .map_err(|e| Error::Limiter(e.to_string()))?;
            return Err(Error::RateLimited {
                retry_in_secs: u64::try_from(ttl).unwrap_or(3600).max(1),
                remaining: 0,
            });
        }
        let remaining = u32::try_from(max - count).unwrap_or(0);
        Ok(remaining)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_format() {
        assert_eq!(RateLimiter::key(123), "fetchly:ratelimit:123");
    }
}
