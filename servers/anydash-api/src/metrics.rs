// SPDX-License-Identifier: FSL-1.1-ALv2
//! Prometheus metrics with bounded labels. Device, connection, user and space IDs never appear
//! in labels.
use buffa::Enumeration;
use delino_protocol::anydash::v1::{CollectionStatus, Provider};
use prometheus::{
    Encoder, Gauge, GaugeVec, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge,
    Opts, Registry, TextEncoder,
};

/// Outcome label values of `anydash_worker_job_runs_total`.
pub const JOB_OUTCOMES: [&str; 2] = ["success", "failure"];

pub struct Metrics {
    registry: Registry,
    pub requests: IntCounterVec,
    pub timeouts: IntCounter,
    pub gauge_refresh_failures: IntCounter,
    pub duration: HistogramVec,
    pub job_runs: IntCounterVec,
    pub job_last_success: GaugeVec,
    pub deletion_pending: IntGauge,
    pub deletion_oldest_age: Gauge,
    pub price_last_success: Gauge,
    pub price_validation_failures: IntCounter,
    pub snapshots: IntCounterVec,
    pub parser_devices: GaugeVec,
    pub leases_active: IntGauge,
    pub lease_failovers: IntCounterVec,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let requests = IntCounterVec::new(
            Opts::new(
                "anydash_api_requests_total",
                "API requests by bounded outcome.",
            ),
            &["service", "method", "outcome"],
        )
        .unwrap();
        // Requests that hit the deadline never reach `requests`, because the handler future that
        // records them is dropped. This counter is the only sample of such requests and has no
        // method label: the timeout happens in the HTTP layer, before the method is known to be
        // meaningful.
        let timeouts = IntCounter::new(
            "anydash_api_timeouts_total",
            "API requests that exceeded the request deadline and returned 504.",
        )
        .unwrap();
        // A scrape still succeeds when the database-backed gauges cannot be refreshed, so this
        // counter is how an operator sees the stale gauges.
        let gauge_refresh_failures = IntCounter::new(
            "anydash_gauge_refresh_failures_total",
            "Scrapes that could not refresh the database-backed gauges.",
        )
        .unwrap();
        let duration = HistogramVec::new(
            HistogramOpts::new(
                "anydash_api_request_duration_seconds",
                "API request latency.",
            )
            .buckets(vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0,
            ]),
            &["service", "method"],
        )
        .unwrap();
        let job_runs = IntCounterVec::new(
            Opts::new(
                "anydash_worker_job_runs_total",
                "Worker job runs by outcome.",
            ),
            // `job` is the reserved Prometheus target label, so the worker job uses its own name.
            &["worker_job", "outcome"],
        )
        .unwrap();
        let job_last_success = GaugeVec::new(
            Opts::new(
                "anydash_worker_job_last_success_timestamp_seconds",
                "Unix time of the last successful run of each worker job.",
            ),
            &["worker_job"],
        )
        .unwrap();
        let deletion_pending = IntGauge::new(
            "anydash_deletion_jobs_pending",
            "Deletion jobs that are not complete.",
        )
        .unwrap();
        let deletion_oldest_age = Gauge::new(
            "anydash_deletion_oldest_pending_age_seconds",
            "Age of the oldest pending deletion job.",
        )
        .unwrap();
        let price_last_success = Gauge::new(
            "anydash_price_sync_last_success_timestamp_seconds",
            "Unix time of the last successful LiteLLM price sync.",
        )
        .unwrap();
        let price_validation_failures = IntCounter::new(
            "anydash_price_sync_validation_failures_total",
            "LiteLLM price documents rejected by validation.",
        )
        .unwrap();
        let snapshots = IntCounterVec::new(
            Opts::new(
                "anydash_collection_snapshots_total",
                "Accepted collection snapshots.",
            ),
            &["provider", "status"],
        )
        .unwrap();
        let parser_devices = GaugeVec::new(
            Opts::new(
                "anydash_parser_devices",
                "Devices that uploaded in the last hour by latest parser state.",
            ),
            &["tool", "state"],
        )
        .unwrap();
        let leases_active =
            IntGauge::new("anydash_leases_active", "Valid collection leases.").unwrap();
        let lease_failovers = IntCounterVec::new(
            Opts::new(
                "anydash_lease_failovers_total",
                "Leases that expired without a release.",
            ),
            &["provider"],
        )
        .unwrap();
        for c in [
            Box::new(requests.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(timeouts.clone()),
            Box::new(gauge_refresh_failures.clone()),
            Box::new(duration.clone()),
            Box::new(job_runs.clone()),
            Box::new(job_last_success.clone()),
            Box::new(deletion_pending.clone()),
            Box::new(deletion_oldest_age.clone()),
            Box::new(price_last_success.clone()),
            Box::new(price_validation_failures.clone()),
            Box::new(snapshots.clone()),
            Box::new(parser_devices.clone()),
            Box::new(leases_active.clone()),
            Box::new(lease_failovers.clone()),
        ] {
            registry.register(c).unwrap();
        }
        let metrics = Self {
            registry,
            requests,
            timeouts,
            gauge_refresh_failures,
            duration,
            job_runs,
            job_last_success,
            deletion_pending,
            deletion_oldest_age,
            price_last_success,
            price_validation_failures,
            snapshots,
            parser_devices,
            leases_active,
            lease_failovers,
        };
        metrics.create_known_series();
        metrics
    }

    /// Creates every series whose label values are a known bounded set at zero.
    ///
    /// Prometheus `increase()` needs a sample before the first increment: a counter that first
    /// appears with value 1 makes the 0 -> 1 step invisible, so the first failure of a worker job
    /// or the first snapshot of a provider would never fire an alert or change a ratio. Creating
    /// the children at startup makes `/metrics` expose them at 0 before anything happens.
    fn create_known_series(&self) {
        for job in crate::worker::Job::ALL {
            for outcome in JOB_OUTCOMES {
                self.job_runs.with_label_values(&[job.name(), outcome]);
            }
        }
        for provider in Provider::values()
            .iter()
            .filter(|p| **p != Provider::Unspecified)
        {
            for status in CollectionStatus::values()
                .iter()
                .filter(|s| **s != CollectionStatus::Unspecified)
            {
                self.snapshots.with_label_values(&[
                    &crate::worker::label(*provider, "PROVIDER_"),
                    &crate::worker::label(*status, "COLLECTION_STATUS_"),
                ]);
            }
        }
    }

    pub fn encode(&self) -> String {
        let mut out = Vec::new();
        TextEncoder::new()
            .encode(&self.registry.gather(), &mut out)
            .expect("encode registered metrics");
        String::from_utf8(out).unwrap_or_default()
    }
}

/// Bounded outcome label: `success`, `client_error` or `server_error`.
pub fn outcome(error: Option<&crate::ApiError>) -> &'static str {
    use connectrpc::ErrorCode::*;
    match error.map(|e| e.code()) {
        None => "success",
        Some(Internal | Unavailable | Unknown | DataLoss | DeadlineExceeded) => "server_error",
        Some(_) => "client_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_series_exist_at_zero_before_any_event() {
        let text = Metrics::new().encode();
        for job in crate::worker::Job::ALL {
            for outcome in JOB_OUTCOMES {
                let line = format!(
                    "anydash_worker_job_runs_total{{outcome=\"{outcome}\",worker_job=\"{}\"}} 0",
                    job.name()
                );
                assert!(text.contains(&line), "missing {line}");
            }
        }
        let providers = Provider::values()
            .iter()
            .filter(|p| **p != Provider::Unspecified)
            .count();
        let statuses = CollectionStatus::values()
            .iter()
            .filter(|s| **s != CollectionStatus::Unspecified)
            .count();
        assert!(
            providers >= 6 && statuses >= 6,
            "enum values must be listed"
        );
        let snapshot_series = text
            .lines()
            .filter(|l| l.starts_with("anydash_collection_snapshots_total{") && l.ends_with(" 0"))
            .count();
        assert_eq!(snapshot_series, providers * statuses);
        assert!(text.contains(
            "anydash_collection_snapshots_total{provider=\"claude_code\",status=\"rate_limited\"} 0"
        ));
    }

    #[test]
    fn timeout_counter_is_exposed_at_zero() {
        let m = Metrics::new();
        assert!(m.encode().contains("anydash_api_timeouts_total 0"));
        m.timeouts.inc();
        assert!(m.encode().contains("anydash_api_timeouts_total 1"));
    }

    #[test]
    fn gauge_refresh_failure_counter_is_exposed_at_zero() {
        let m = Metrics::new();
        assert!(
            m.encode()
                .contains("anydash_gauge_refresh_failures_total 0")
        );
    }

    #[test]
    fn runtime_labels_match_the_created_series() {
        let m = Metrics::new();
        m.snapshots
            .with_label_values(&[
                &crate::worker::label(Provider::ClaudeCode, "PROVIDER_"),
                &crate::worker::label(CollectionStatus::Failed, "COLLECTION_STATUS_"),
            ])
            .inc();
        let text = m.encode();
        assert!(text.contains(
            "anydash_collection_snapshots_total{provider=\"claude_code\",status=\"failed\"} 1"
        ));
        assert_eq!(
            text.matches(
                "anydash_collection_snapshots_total{provider=\"claude_code\",status=\"failed\"}"
            )
            .count(),
            1
        );
    }
}
