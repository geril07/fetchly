use redis::AsyncCommands;

use crate::error::{Error, Result};

/// Per-user download rate limiting: N downloads per rolling hour.
///
/// Atomic check-then-consume via a Lua script on `fetchly:ratelimit:{user_id}`.
/// The key holds the count for the current window; TTL is the seconds left.
/// Rejected taps never increment the counter, so failed admission does not
/// burn quota. Call [`RateLimiter::refund`] to give back a unit consumed
/// before an admission failure further down the pipeline.
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

    /// Read-only view of a user's current window. Never consumes quota.
    /// Returns used count, remaining quota, and seconds until reset (0 = full quota).
    pub async fn usage(&self, user_id: u64) -> Result<Usage> {
        let mut conn = self.manager.clone();
        let key = Self::key(user_id);
        let count: Option<i64> = conn
            .get(&key)
            .await
            .map_err(|e| Error::Limiter(e.to_string()))?;
        let count = count.unwrap_or(0).max(0);
        let max = i64::from(self.max_per_hour);
        let used = u32::try_from(count.min(max)).unwrap_or(0);
        let remaining = u32::try_from(max - count.min(max)).unwrap_or(0);
        let reset_in_secs = if count > 0 {
            let ttl: i64 = conn
                .ttl(&key)
                .await
                .map_err(|e| Error::Limiter(e.to_string()))?;
            u64::try_from(ttl).unwrap_or(0)
        } else {
            0
        };
        Ok(Usage {
            used,
            remaining,
            reset_in_secs,
        })
    }

    /// Consume one unit. On success returns remaining quota in the window.
    /// On exhaustion returns [`Error::RateLimited`] with TTL-based retry hint.
    /// The check and the increment run atomically: a rejected tap leaves the
    /// counter untouched, so callers can try-then-give-up without a refund.
    pub async fn check_and_consume(&self, user_id: u64) -> Result<u32> {
        let max = i64::from(self.max_per_hour);
        let script = redis::Script::new(
            r"
            local count = tonumber(redis.call('GET', KEYS[1]) or '0')
            if count >= tonumber(ARGV[1]) then
                return {-1, redis.call('TTL', KEYS[1])}
            end
            count = redis.call('INCR', KEYS[1])
            if count == 1 then
                redis.call('EXPIRE', KEYS[1], 3600)
            end
            return {count, redis.call('TTL', KEYS[1])}
            ",
        );
        let (count, ttl): (i64, i64) = script
            .key(Self::key(user_id))
            .arg(max)
            .invoke_async(&mut self.manager.clone())
            .await
            .map_err(|e| Error::Limiter(e.to_string()))?;
        if count < 0 {
            return Err(Error::RateLimited {
                retry_in_secs: u64::try_from(ttl).unwrap_or(3600).max(1),
                remaining: 0,
            });
        }
        let remaining = u32::try_from(max - count).unwrap_or(0);
        Ok(remaining)
    }

    /// Give back one unit previously consumed by [`RateLimiter::check_and_consume`].
    /// Used when admission fails *after* the consume step (global queue full,
    /// progress message send failure). Never drops the counter below zero;
    /// deleting the key at zero restores the fresh-window state. TTL of a
    /// non-empty window is preserved.
    pub async fn refund(&self, user_id: u64) -> Result<()> {
        let script = redis::Script::new(
            r"
            local count = tonumber(redis.call('GET', KEYS[1]) or '0')
            if count <= 1 then
                redis.call('DEL', KEYS[1])
                return 0
            end
            return redis.call('DECR', KEYS[1])
            ",
        );
        let _: i64 = script
            .key(Self::key(user_id))
            .invoke_async(&mut self.manager.clone())
            .await
            .map_err(|e| Error::Limiter(e.to_string()))?;
        Ok(())
    }
}

/// Snapshot of one user's rate-limit window (see [`RateLimiter::usage`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub used: u32,
    pub remaining: u32,
    pub reset_in_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    #[test]
    fn key_format() {
        assert_eq!(RateLimiter::key(123), "fetchly:ratelimit:123");
    }

    #[tokio::test]
    async fn consumes_down_to_zero_then_rate_limits() {
        let Some(t) = crate::testutil::start_redis().await else {
            return;
        };
        let limiter = RateLimiter::new(t.manager.clone(), 3);
        assert_eq!(limiter.check_and_consume(1001).await.expect("1st"), 2);
        assert_eq!(limiter.check_and_consume(1001).await.expect("2nd"), 1);
        assert_eq!(limiter.check_and_consume(1001).await.expect("3rd"), 0);
        match limiter.check_and_consume(1001).await {
            Err(Error::RateLimited {
                retry_in_secs,
                remaining,
            }) => {
                assert!(retry_in_secs > 0 && retry_in_secs <= 3600);
                assert_eq!(remaining, 0);
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn usage_peeks_without_consuming() {
        let Some(t) = crate::testutil::start_redis().await else {
            return;
        };
        let limiter = RateLimiter::new(t.manager.clone(), 3);
        let fresh = limiter.usage(3001).await.expect("peek");
        assert_eq!(
            fresh,
            Usage {
                used: 0,
                remaining: 3,
                reset_in_secs: 0,
            }
        );
        limiter.check_and_consume(3001).await.expect("consume");
        limiter.check_and_consume(3001).await.expect("consume");
        let mid = limiter.usage(3001).await.expect("peek");
        assert_eq!(mid.used, 2);
        assert_eq!(mid.remaining, 1);
        assert!(mid.reset_in_secs > 0 && mid.reset_in_secs <= 3600);
        // Peek again: still 2 used, quota untouched.
        let again = limiter.usage(3001).await.expect("peek");
        assert_eq!(again.used, 2);
        assert_eq!(again.remaining, 1);
    }

    #[tokio::test]
    async fn users_are_isolated() {
        let Some(t) = crate::testutil::start_redis().await else {
            return;
        };
        let limiter = RateLimiter::new(t.manager.clone(), 2);
        assert_eq!(limiter.check_and_consume(2001).await.expect("A1"), 1);
        assert_eq!(limiter.check_and_consume(2001).await.expect("A2"), 0);
        assert!(limiter.check_and_consume(2001).await.is_err());
        // B is unaffected by A's exhaustion.
        assert_eq!(limiter.check_and_consume(2002).await.expect("B1"), 1);
    }

    #[tokio::test]
    async fn rejected_taps_do_not_inflate_counter() {
        let Some(t) = crate::testutil::start_redis().await else {
            return;
        };
        let limiter = RateLimiter::new(t.manager.clone(), 1);
        limiter.check_and_consume(4001).await.expect("1st");
        assert!(limiter.check_and_consume(4001).await.is_err());
        assert!(limiter.check_and_consume(4001).await.is_err());
        let snapshot = limiter.usage(4001).await.expect("peek");
        assert_eq!(snapshot.used, 1);
        assert_eq!(snapshot.remaining, 0);
    }

    #[tokio::test]
    async fn refund_gives_back_quota_and_restores_fresh_state() {
        let Some(t) = crate::testutil::start_redis().await else {
            return;
        };
        let limiter = RateLimiter::new(t.manager.clone(), 2);
        limiter.check_and_consume(5001).await.expect("1st");
        limiter.check_and_consume(5001).await.expect("2nd");
        assert!(limiter.check_and_consume(5001).await.is_err());
        limiter.refund(5001).await.expect("refund");
        let mid = limiter.usage(5001).await.expect("peek");
        assert_eq!(mid.used, 1);
        assert_eq!(mid.remaining, 1);
        limiter.refund(5001).await.expect("refund");
        limiter
            .refund(5001)
            .await
            .expect("refund is idempotent at zero");
        let fresh = limiter.usage(5001).await.expect("peek");
        assert_eq!(
            fresh,
            Usage {
                used: 0,
                remaining: 2,
                reset_in_secs: 0,
            }
        );
    }
}
