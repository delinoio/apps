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
    fn allow_at(&self, key: &str, now: Instant) -> std::result::Result<(), Duration> {
        let mut buckets = self.buckets.lock().unwrap();
        buckets.retain(|_, (tokens, at)| {
            let idle = now.duration_since(*at);
            idle < Duration::from_secs(600) || *tokens + idle.as_secs_f64() * self.rate < self.burst
        });
        let (tokens, at) = buckets.entry(key.into()).or_insert((self.burst, now));
        *tokens = (*tokens + now.duration_since(*at).as_secs_f64() * self.rate).min(self.burst);
        *at = now;
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
    fn exhaustion_reports_retry_delay_and_keeps_other_keys_usable() {
        let limiter = Limiter::new(0.001, 1);
        let start = Instant::now();
        assert!(limiter.allow_at("tenant", start).is_ok());
        assert_eq!(
            limiter.allow_at("tenant", start),
            Err(Duration::from_secs(1000))
        );
        assert!(limiter.allow_at("other", start).is_ok());
        assert!(
            limiter
                .allow_at("tenant", start + Duration::from_secs(1000))
                .is_ok()
        );
    }
}
