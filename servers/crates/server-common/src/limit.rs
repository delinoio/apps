// SPDX-License-Identifier: FSL-1.1-ALv2
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
pub struct Limiter {
    rate: f64,
    burst: f64,
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}
impl Limiter {
    pub fn new(rate: f64, burst: u32) -> Self {
        Self {
            rate,
            burst: burst.into(),
            buckets: Mutex::new(HashMap::new()),
        }
    }
    pub fn allow(&self, key: &str) -> std::result::Result<(), Duration> {
        self.allow_at(key, Instant::now())
    }
    /// Admits one request for `key` using `now` as the sampled arrival time.
    ///
    /// `Limiter::allow` samples the clock before taking the mutex, so a caller
    /// can process a sample that is older than the time already stored for its
    /// bucket. The stored time is therefore monotonic: an existing bucket uses
    /// `max(now, stored)` as its effective time for eviction, refill and
    /// assignment. A stale sample contributes no elapsed time, cannot move the
    /// stored time backwards, and cannot evict a bucket processed later, but it
    /// may still consume a token that is already available.
    fn allow_at(&self, key: &str, now: Instant) -> std::result::Result<(), Duration> {
        let mut buckets = self.buckets.lock().unwrap();
        buckets.retain(|_, (tokens, at)| {
            // `duration_since` yields zero for a stored time later than `now`.
            let idle = now.duration_since(*at);
            idle < Duration::from_secs(600) || *tokens + idle.as_secs_f64() * self.rate < self.burst
        });
        let (tokens, at) = buckets.entry(key.into()).or_insert((self.burst, now));
        let effective = now.max(*at);
        *tokens =
            (*tokens + effective.duration_since(*at).as_secs_f64() * self.rate).min(self.burst);
        *at = effective;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - *tokens) / self.rate))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_depleted_buckets_keep_their_balance() {
        let limiter = Limiter::new(0.001, 100);
        let start = Instant::now();
        for _ in 0..100 {
            assert!(limiter.allow_at("tenant", start).is_ok());
        }
        assert!(
            limiter
                .allow_at("tenant", start + Duration::from_secs(601))
                .is_err()
        );
        assert!(
            limiter
                .allow_at("tenant", start + Duration::from_secs(1000))
                .is_ok()
        );
        assert!(
            limiter
                .allow_at("tenant", start + Duration::from_secs(1000))
                .is_err()
        );
        assert!(
            limiter
                .allow_at("other", start + Duration::from_secs(101000))
                .is_ok()
        );
        assert!(!limiter.buckets.lock().unwrap().contains_key("tenant"));
    }

    #[test]
    fn exhausted_key_reports_retry_and_leaves_other_keys_usable() {
        let limiter = Limiter::new(0.001, 1);
        let start = Instant::now();
        assert!(limiter.allow_at("a", start).is_ok());
        assert_eq!(limiter.allow_at("a", start), Err(Duration::from_secs(1000)));
        assert!(limiter.allow_at("b", start).is_ok());
        assert!(
            limiter
                .allow_at("a", start + Duration::from_secs(1000))
                .is_ok()
        );
    }

    fn state(limiter: &Limiter, key: &str) -> (f64, Instant) {
        limiter.buckets.lock().unwrap()[key]
    }

    #[test]
    fn stale_sample_does_not_reduce_stored_time_or_refill_again() {
        let limiter = Limiter::new(1.0, 1);
        let base = Instant::now();
        let later = base + Duration::from_secs(2);
        assert!(limiter.allow_at("tenant", base).is_ok());
        assert!(limiter.allow_at("tenant", later).is_ok());
        assert_eq!(
            limiter.allow_at("tenant", base + Duration::from_secs(1)),
            Err(Duration::from_secs(1))
        );
        assert_eq!(state(&limiter, "tenant"), (0.0, later));
        assert_eq!(
            limiter.allow_at("tenant", later),
            Err(Duration::from_secs(1))
        );
        assert!(
            limiter
                .allow_at("tenant", base + Duration::from_secs(3))
                .is_ok()
        );
        assert!(
            limiter
                .allow_at("tenant", base + Duration::from_secs(3))
                .is_err()
        );
    }

    #[test]
    fn stale_sample_adds_no_admission_for_default_settings() {
        for (rate, burst, step) in [
            (1000.0, 2000, Duration::from_millis(1)),
            (50.0, 100, Duration::from_millis(20)),
        ] {
            let limiter = Limiter::new(rate, burst);
            let base = Instant::now();
            for _ in 0..burst {
                assert!(limiter.allow_at("tenant", base).is_ok());
            }
            let later = base + 2 * step;
            let refill = (2.0 * step.as_secs_f64() * rate).round() as u32;
            for _ in 0..refill {
                assert!(limiter.allow_at("tenant", later).is_ok());
            }
            assert!(limiter.allow_at("tenant", later).is_err());
            assert!(limiter.allow_at("tenant", base + step).is_err());
            assert_eq!(state(&limiter, "tenant").1, later);
            assert!(limiter.allow_at("tenant", later).is_err());
            assert_eq!(state(&limiter, "tenant").1, later);
        }
    }

    #[test]
    fn stale_sample_keeps_fractional_balance_and_retry() {
        let limiter = Limiter::new(1.0, 1);
        let base = Instant::now();
        let half = base + Duration::from_millis(500);
        assert!(limiter.allow_at("tenant", base).is_ok());
        assert_eq!(
            limiter.allow_at("tenant", half),
            Err(Duration::from_millis(500))
        );
        assert_eq!(
            limiter.allow_at("tenant", base + Duration::from_millis(250)),
            Err(Duration::from_millis(500))
        );
        assert_eq!(
            limiter.allow_at("tenant", half),
            Err(Duration::from_millis(500))
        );
        assert_eq!(state(&limiter, "tenant"), (0.5, half));
        let full = base + Duration::from_secs(1);
        assert!(limiter.allow_at("tenant", full).is_ok());
        assert!(limiter.allow_at("tenant", full).is_err());
    }

    #[test]
    fn stale_sample_consumes_available_token_without_refill() {
        let limiter = Limiter::new(1.0, 3);
        let base = Instant::now();
        let later = base + Duration::from_secs(10);
        assert!(limiter.allow_at("tenant", base).is_ok());
        assert!(limiter.allow_at("tenant", later).is_ok());
        assert_eq!(state(&limiter, "tenant"), (2.0, later));
        assert!(limiter.allow_at("tenant", base).is_ok());
        assert!(limiter.allow_at("tenant", base).is_ok());
        assert_eq!(state(&limiter, "tenant"), (0.0, later));
        assert!(limiter.allow_at("tenant", base).is_err());
        assert_eq!(state(&limiter, "tenant").1, later);
    }

    #[test]
    fn stale_sample_does_not_evict_later_bucket() {
        let limiter = Limiter::new(0.001, 100);
        let base = Instant::now();
        let later = base + Duration::from_secs(700);
        for _ in 0..100 {
            assert!(limiter.allow_at("tenant", later).is_ok());
        }
        // A sample more than 600 seconds older than the stored time must not
        // count as idle time, and other keys stay independent.
        assert!(limiter.allow_at("other", base).is_ok());
        assert!(limiter.allow_at("tenant", base).is_err());
        assert_eq!(state(&limiter, "tenant"), (0.0, later));
        assert!(limiter.buckets.lock().unwrap().contains_key("other"));
        // Ordered samples retain the existing idle-eviction boundary.
        assert!(
            limiter
                .allow_at("other", later + Duration::from_secs(599))
                .is_ok()
        );
        assert!(limiter.buckets.lock().unwrap().contains_key("tenant"));
    }
}
