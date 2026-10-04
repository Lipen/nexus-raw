//! Retries: policy, backoff with jitter, error classification (protocol §7).

use std::hash::{Hash, Hasher};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Up to 4 attempts per request.
/// Backoff 0.5s × 2ⁿ + jitter ≤ 250ms.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Total attempts, including the first.
    pub attempts: u32,
    pub base: Duration,
    pub jitter: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 4,
            base: Duration::from_millis(500),
            jitter: Duration::from_millis(250),
        }
    }
}

impl RetryPolicy {
    /// Pause before attempt `attempt` (1-based): base × 2^(attempt−1) + jitter.
    /// Hash-based jitter, no external rand.
    #[must_use]
    pub fn delay(&self, attempt: u32) -> Duration {
        let factor = 2u32.saturating_pow(attempt.saturating_sub(1)).min(64);
        let exp = self
            .base
            .saturating_mul(factor)
            .min(Duration::from_secs(60));
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (
            attempt,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos()),
        )
            .hash(&mut h);
        let jitter = h.finish() % (self.jitter.as_millis().max(1) as u64 + 1);
        exp + Duration::from_millis(jitter)
    }
}

/// One attempt's failure: whether to retry, and an optional server-mandated pause.
#[derive(Debug)]
pub struct AttemptFailure {
    pub retryable: bool,
    /// A `Retry-After` pause from a 429: replaces the backoff for this retry.
    pub retry_after: Option<std::time::Duration>,
    pub error: crate::error::Error,
}

impl AttemptFailure {
    /// A failure that is never retried.
    pub fn stop(error: crate::error::Error) -> Self {
        Self {
            retryable: false,
            retry_after: None,
            error,
        }
    }

    /// A failure worth another attempt, on the regular backoff.
    pub fn again(error: crate::error::Error) -> Self {
        Self {
            retryable: true,
            retry_after: None,
            error,
        }
    }
}

/// Connect errors, timeouts, body breaks are retried.
/// 4xx are not (protocol §7).
/// Statuses are matched manually (`error_for_status` is never called), so
/// `is_status` never shows up here.
#[must_use]
pub fn is_retryable(e: &reqwest::Error) -> bool {
    !(e.is_builder() || e.is_redirect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_stays_bounded() {
        let p = RetryPolicy::default();
        let d1 = p.delay(1);
        assert!(d1 >= Duration::from_millis(500) && d1 <= Duration::from_millis(750));
        let d3 = p.delay(3);
        assert!(d3 >= Duration::from_secs(2) && d3 <= Duration::from_millis(2250));
        let d9 = p.delay(9);
        assert!(d9 <= Duration::from_secs(60) + Duration::from_millis(250));
    }
}
