// SPDX-License-Identifier: FSL-1.1-ALv2
use opentelemetry::trace::TracerProvider;
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, Opts, Registry, TextEncoder,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
#[derive(Clone, Copy)]
pub enum Service {
    DevBase,
    GitTracker,
}
pub struct Metrics {
    registry: Registry,
    pub requests: IntCounterVec,
    pub duration: HistogramVec,
    pub events: IntCounterVec,
    pub limits: IntCounterVec,
    pub cleanup_runs: IntCounter,
    pub cleanup_failures: IntCounter,
}
impl Metrics {
    pub fn new(service: Service) -> Self {
        let registry = Registry::new();
        let (name, labels) = match service {
            Service::DevBase => ("devbase_requests_total", vec!["procedure", "code"]),
            Service::GitTracker => ("git_tracker_http_requests_total", vec!["outcome"]),
        };
        let requests =
            IntCounterVec::new(Opts::new(name, "Requests by bounded outcome."), &labels).unwrap();
        let duration = HistogramVec::new(
            HistogramOpts::new(
                "devbase_request_duration_seconds",
                "Request latency by procedure.",
            ),
            &["procedure"],
        )
        .unwrap();
        let events = IntCounterVec::new(
            Opts::new("git_tracker_events_total", "Bounded lifecycle events."),
            &["kind", "category"],
        )
        .unwrap();
        let limits = IntCounterVec::new(
            Opts::new(
                "devbase_limit_rejections_total",
                "Rejections by limit kind.",
            ),
            &["kind"],
        )
        .unwrap();
        let cleanup_runs = IntCounter::new(
            "devbase_cleanup_runs_total",
            "Completed maintenance cycles.",
        )
        .unwrap();
        let cleanup_failures = IntCounter::new(
            "devbase_cleanup_failures_total",
            "Failed cleanup operations.",
        )
        .unwrap();
        registry.register(Box::new(requests.clone())).unwrap();
        match service {
            Service::DevBase => {
                registry.register(Box::new(duration.clone())).unwrap();
                registry.register(Box::new(limits.clone())).unwrap();
                registry.register(Box::new(cleanup_runs.clone())).unwrap();
                registry
                    .register(Box::new(cleanup_failures.clone()))
                    .unwrap();
            }
            Service::GitTracker => {
                registry.register(Box::new(events.clone())).unwrap();
            }
        }
        Self {
            registry,
            requests,
            duration,
            events,
            limits,
            cleanup_runs,
            cleanup_failures,
        }
    }
    pub fn encode(&self) -> String {
        let mut out = Vec::new();
        TextEncoder::new()
            .encode(&self.registry.gather(), &mut out)
            .expect("encode registered metrics");
        String::from_utf8(out).unwrap()
    }
}
pub fn init() -> anyhow::Result<Option<opentelemetry_sdk::trace::SdkTracerProvider>> {
    let provider = if std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_some()
        || std::env::var_os("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").is_some()
    {
        Some(
            opentelemetry_sdk::trace::SdkTracerProvider::builder()
                .with_batch_exporter(
                    opentelemetry_otlp::SpanExporter::builder()
                        .with_http()
                        .build()?,
                )
                .build(),
        )
    } else {
        None
    };
    let layer = provider
        .as_ref()
        .map(|p| tracing_opentelemetry::layer().with_tracer(p.tracer("delino-api")));
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    let registry = tracing_subscriber::registry().with(filter).with(layer);
    match std::env::var("LOG_FORMAT").unwrap_or_default().as_str() {
        "json" => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(std::io::stderr),
            )
            .init(),
        "" | "text" => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(std::env::var("NO_COLOR").unwrap_or_default().is_empty())
                    .with_writer(std::io::stderr),
            )
            .init(),
        _ => anyhow::bail!("LOG_FORMAT must be text or json"),
    }
    Ok(provider)
}
