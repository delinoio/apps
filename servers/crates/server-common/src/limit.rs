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
        let now = Instant::now();
        let mut buckets = self.buckets.lock().unwrap();
        buckets.retain(|_, (_, at)| now.duration_since(*at) < Duration::from_secs(600));
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
