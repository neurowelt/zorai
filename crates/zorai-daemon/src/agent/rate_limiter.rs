//! Per-tool rate limiter — token-bucket algorithm to prevent tool call flooding.

use std::collections::HashMap;

/// A single token bucket that refills at a constant rate.
pub struct TokenBucket {
    capacity: u32,
    tokens: f64,
    refill_rate: f64,
    last_refill_at: u64,
}

impl TokenBucket {
    /// Create a new bucket that starts full.
    pub fn new(capacity: u32, refill_rate: f64, now: u64) -> Self {
        Self {
            capacity,
            tokens: capacity as f64,
            refill_rate,
            last_refill_at: now,
        }
    }

    /// Refill based on elapsed time, then try to consume 1 token.
    /// Returns `true` if the token was acquired.
    pub fn try_acquire(&mut self, now: u64) -> bool {
        self.refill(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Number of whole tokens currently available (without refilling).
    pub fn tokens_available(&self) -> u32 {
        self.tokens as u32
    }

    /// Internal: add tokens based on elapsed time since last refill.
    fn refill(&mut self, now: u64) {
        if now > self.last_refill_at {
            let elapsed_secs = (now - self.last_refill_at) as f64;
            self.tokens = (self.tokens + elapsed_secs * self.refill_rate).min(self.capacity as f64);
            self.last_refill_at = now;
        }
    }
}

/// Per-tool rate limiter that lazily creates token buckets.
pub struct RateLimiter {
    buckets: HashMap<String, TokenBucket>,
    default_capacity: u32,
    default_refill_rate: f64,
}

impl RateLimiter {
    /// Create a new rate limiter with default limits (60/min, 1 token/sec).
    pub fn new() -> Self {
        Self {
            buckets: HashMap::new(),
            default_capacity: 60,
            default_refill_rate: 1.0,
        }
    }

    /// Set a custom limit for a specific tool.
    pub fn with_tool_limit(&mut self, tool_name: &str, capacity: u32, rate_per_sec: f64) {
        self.buckets.insert(
            tool_name.to_string(),
            TokenBucket::new(capacity, rate_per_sec, 0),
        );
    }

    /// Check whether a tool call is allowed right now.
    ///
    /// Lazily creates a bucket for unknown tools using the default limits.
    pub fn check(&mut self, tool_name: &str, now: u64) -> bool {
        let default_cap = self.default_capacity;
        let default_rate = self.default_refill_rate;
        let bucket = self
            .buckets
            .entry(tool_name.to_string())
            .or_insert_with(|| TokenBucket::new(default_cap, default_rate, now));
        bucket.try_acquire(now)
    }

    /// If the bucket for `tool_name` exists and is empty, return a human-readable
    /// denial reason. Returns `None` if the tool is unknown or has tokens left.
    pub fn deny_reason(&self, tool_name: &str) -> Option<String> {
        let bucket = self.buckets.get(tool_name)?;
        if bucket.tokens_available() == 0 {
            Some(format!(
                "Rate limit exceeded for tool '{}': 0/{} tokens available, refills at {:.1}/sec",
                tool_name, bucket.capacity, bucket.refill_rate,
            ))
        } else {
            None
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a [`RateLimiter`] pre-configured with sensible per-tool defaults.
///
/// | Tool group | Limit |
/// |---|---|
/// | `bash_command` | 30/min (0.5/sec) |
/// | `write_file`, `create_file`, `replace_in_file` | 40/min (~0.667/sec) |
/// | `web_search`, `fetch_url` | 20/min (~0.333/sec) |
/// | Everything else | 60/min (1.0/sec) |
pub fn build_default_limiter() -> RateLimiter {
    let mut limiter = RateLimiter::new();

    limiter.with_tool_limit(zorai_protocol::tool_names::BASH_COMMAND, 30, 0.5);
    limiter.with_tool_limit(zorai_protocol::tool_names::BASH_COMMAND_LEGACY, 30, 0.5);

    let file_rate = 40.0 / 60.0;
    limiter.with_tool_limit(zorai_protocol::tool_names::WRITE_FILE, 40, file_rate);
    limiter.with_tool_limit(zorai_protocol::tool_names::CREATE_FILE, 40, file_rate);
    limiter.with_tool_limit(zorai_protocol::tool_names::REPLACE_IN_FILE, 40, file_rate);

    let net_rate = 20.0 / 60.0;
    limiter.with_tool_limit(zorai_protocol::tool_names::WEB_SEARCH, 20, net_rate);
    limiter.with_tool_limit(zorai_protocol::tool_names::FETCH_URL, 20, net_rate);

    limiter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_bucket_starts_full() {
        let bucket = TokenBucket::new(10, 1.0, 0);
        assert_eq!(bucket.tokens_available(), 10);
    }

    #[test]
    fn acquire_succeeds_when_tokens_available() {
        let mut bucket = TokenBucket::new(5, 1.0, 0);
        assert!(bucket.try_acquire(0));
        assert_eq!(bucket.tokens_available(), 4);
    }

    #[test]
    fn acquire_fails_when_empty() {
        let mut bucket = TokenBucket::new(2, 1.0, 0);
        assert!(bucket.try_acquire(0));
        assert!(bucket.try_acquire(0));
        assert!(!bucket.try_acquire(0));
    }

    #[test]
    fn tokens_refill_over_time() {
        let mut bucket = TokenBucket::new(5, 1.0, 0);
        for _ in 0..5 {
            bucket.try_acquire(0);
        }
        assert_eq!(bucket.tokens_available(), 0);

        assert!(bucket.try_acquire(3));
        assert_eq!(bucket.tokens_available(), 2);
    }

    #[test]
    fn refill_does_not_exceed_capacity() {
        let mut bucket = TokenBucket::new(5, 1.0, 0);
        bucket.try_acquire(0);
        bucket.try_acquire(1000);
        assert_eq!(bucket.tokens_available(), 4);
    }

    #[test]
    fn default_limiter_allows_normal_usage() {
        let mut limiter = RateLimiter::new();
        assert!(limiter.check("some_tool", 100));
    }

    #[test]
    fn bash_command_limited_to_30_per_min() {
        let mut limiter = build_default_limiter();
        let now = 1000;
        for i in 0..30 {
            assert!(
                limiter.check(zorai_protocol::tool_names::BASH_COMMAND, now),
                "call {} should succeed",
                i
            );
        }
        assert!(!limiter.check(zorai_protocol::tool_names::BASH_COMMAND, now));
    }

    #[test]
    fn custom_tool_limits_work() {
        let mut limiter = RateLimiter::new();
        limiter.with_tool_limit("my_tool", 3, 0.5);
        let now = 500;
        assert!(limiter.check("my_tool", now));
        assert!(limiter.check("my_tool", now));
        assert!(limiter.check("my_tool", now));
        assert!(!limiter.check("my_tool", now));
    }

    #[test]
    fn lazy_bucket_creation() {
        let mut limiter = RateLimiter::new();
        assert!(limiter.deny_reason("unknown_tool").is_none());
        assert!(limiter.check("unknown_tool", 0));
        assert!(limiter.buckets.contains_key("unknown_tool"));
    }

    #[test]
    fn deny_reason_when_throttled() {
        let mut limiter = RateLimiter::new();
        limiter.with_tool_limit("tiny", 1, 0.1);
        let now = 1000;
        assert!(limiter.check("tiny", now));
        assert!(!limiter.check("tiny", now));
        let reason = limiter.deny_reason("tiny").expect("should have a reason");
        assert!(reason.contains("tiny"));
        assert!(reason.contains("0/1"));
    }

    #[test]
    fn multiple_tools_tracked_independently() {
        let mut limiter = RateLimiter::new();
        limiter.with_tool_limit("tool_a", 2, 1.0);
        limiter.with_tool_limit("tool_b", 2, 1.0);
        let now = 100;

        assert!(limiter.check("tool_a", now));
        assert!(limiter.check("tool_a", now));
        assert!(!limiter.check("tool_a", now));

        assert!(limiter.check("tool_b", now));
        assert!(limiter.check("tool_b", now));
        assert!(!limiter.check("tool_b", now));
    }

    #[test]
    fn build_default_limiter_has_expected_tools() {
        let limiter = build_default_limiter();
        assert!(limiter
            .buckets
            .contains_key(zorai_protocol::tool_names::BASH_COMMAND));
        assert!(limiter
            .buckets
            .contains_key(zorai_protocol::tool_names::WRITE_FILE));
        assert!(limiter
            .buckets
            .contains_key(zorai_protocol::tool_names::CREATE_FILE));
        assert!(limiter
            .buckets
            .contains_key(zorai_protocol::tool_names::REPLACE_IN_FILE));
        assert!(limiter
            .buckets
            .contains_key(zorai_protocol::tool_names::WEB_SEARCH));
        assert!(limiter
            .buckets
            .contains_key(zorai_protocol::tool_names::FETCH_URL));
    }

    #[test]
    fn web_search_limited_to_20_per_min() {
        let mut limiter = build_default_limiter();
        let now = 2000;
        for i in 0..20 {
            assert!(
                limiter.check(zorai_protocol::tool_names::WEB_SEARCH, now),
                "call {} should succeed",
                i
            );
        }
        assert!(!limiter.check(zorai_protocol::tool_names::WEB_SEARCH, now));
    }

    #[test]
    fn deny_reason_returns_none_when_tokens_available() {
        let mut limiter = RateLimiter::new();
        limiter.with_tool_limit("healthy", 10, 1.0);
        limiter.check("healthy", 100);
        assert!(limiter.deny_reason("healthy").is_none());
    }
}
