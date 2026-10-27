// SPDX-License-Identifier: FSL-1.1-ALv2
use axum::http::StatusCode;
use std::sync::Mutex;

const BUCKETS: [f64; 10] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0];
#[derive(Clone, Copy)]
pub enum Outcome {
    Success,
    ClientError,
    ServerError,
    Timeout,
}
/// A typed operation result for HTTP envelopes that do not expose the backend status.
#[derive(Clone, Copy)]
pub struct BackendOutcome(pub Outcome);

pub fn outcome_from_status(status: StatusCode) -> Outcome {
    if status == StatusCode::GATEWAY_TIMEOUT {
        Outcome::Timeout
    } else if status.is_server_error() {
        Outcome::ServerError
    } else if status.is_client_error() {
        Outcome::ClientError
    } else {
        Outcome::Success
    }
}
#[derive(Default)]
struct Samples {
    outcomes: [u64; 4],
    buckets: [u64; 10],
    count: u64,
    sum: f64,
}
#[derive(Default)]
pub struct Metrics {
    samples: Mutex<Samples>,
}
impl Metrics {
    pub fn observe(&self, outcome: Outcome, seconds: f64, subscription: bool) {
        let mut sample = self.samples.lock().unwrap_or_else(|e| e.into_inner());
        sample.outcomes[outcome as usize] += 1;
        // Streaming connection duration is not request latency.
        if subscription {
            return;
        }
        sample.count += 1;
        sample.sum += seconds;
        for (index, bound) in BUCKETS.iter().enumerate() {
            if seconds <= *bound {
                sample.buckets[index] += 1;
            }
        }
    }
    pub fn encode(&self) -> String {
        let sample = self.samples.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = String::from("# TYPE lane_api_requests_total counter\n");
        for (index, outcome) in ["success", "client_error", "server_error", "timeout"]
            .iter()
            .enumerate()
        {
            out.push_str(&format!(
                "lane_api_requests_total{{outcome=\"{outcome}\"}} {}\n",
                sample.outcomes[index]
            ));
        }
        out.push_str("# TYPE lane_api_request_duration_seconds histogram\n");
        for (index, bound) in BUCKETS.iter().enumerate() {
            out.push_str(&format!(
                "lane_api_request_duration_seconds_bucket{{le=\"{bound}\"}} {}\n",
                sample.buckets[index]
            ));
        }
        out.push_str(&format!("lane_api_request_duration_seconds_bucket{{le=\"+Inf\"}} {}\nlane_api_request_duration_seconds_sum {}\nlane_api_request_duration_seconds_count {}\n",sample.count,sample.sum,sample.count));
        out
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stream_lifetime_does_not_pollute_latency_and_buckets_are_cumulative() {
        let m = Metrics::default();
        m.observe(Outcome::Success, 1.5, false);
        m.observe(Outcome::Success, 600.0, true);
        let text = m.encode();
        assert!(text.contains("outcome=\"success\"} 2"));
        assert!(text.contains("le=\"2\"} 1"));
        assert!(text.contains("le=\"1\"} 0"));
        assert!(text.contains("_count 1\n"));
        assert!(text.contains("_sum 1.5\n"));
    }
}
