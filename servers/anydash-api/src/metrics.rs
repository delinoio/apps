// SPDX-License-Identifier: FSL-1.1-ALv2
//! Prometheus metrics with bounded labels. Device, connection, user and space IDs never appear
//! in labels.
use prometheus::{
    Encoder, Gauge, GaugeVec, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge,
    Opts, Registry, TextEncoder,
};

pub struct Metrics {
    registry: Registry,
    pub requests: IntCounterVec,
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
        Self {
            registry,
            requests,
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
