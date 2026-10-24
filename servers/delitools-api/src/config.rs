// SPDX-License-Identifier: FSL-1.1-ALv2
//! Startup configuration and its validation.
//!
//! Every setting is read once at startup and validated before the server listens, so a bad
//! value stops the process instead of surfacing under traffic. [`Config::from_lookup`] takes
//! the environment as a function, which keeps validation testable without mutating the
//! process environment. Secrets use [`Secret`], which never prints its value.

use crate::models::ModelCatalog;
use anyhow::{Context, Result, bail, ensure};
use server_common::config::safe_url;
use std::time::Duration;

/// Default OpenAI-compatible Vercel AI Gateway endpoint.
pub const DEFAULT_GATEWAY_URL: &str = "https://ai-gateway.vercel.sh/v1";
/// Fixed development listener; production sets `LISTEN`.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:46322";
/// Consumer records expire this long after their first creation, whatever their state.
pub const OUTBOX_RETENTION: chrono::Duration = chrono::Duration::days(30);

/// A credential that redacts itself in `Debug` output.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    /// Returns the secret for the single place that sends it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Delcore connection and this service's own identity.
#[derive(Clone, Debug)]
pub struct DelcoreSettings {
    pub url: String,
    pub token_url: String,
    pub resource: String,
    pub client_id: String,
    pub client_secret: Secret,
    /// Delcore service ID of DeliTools. Events carry it and catalog prices are matched to it.
    pub service_id: String,
}

/// Vercel AI Gateway connection.
#[derive(Clone, Debug)]
pub struct GatewaySettings {
    pub base_url: String,
    pub api_key: Secret,
    /// Upper bound on completion tokens requested per attempt.
    pub max_output_tokens: u32,
}

/// Request limits enforced before and during execution.
#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    /// Maximum UTF-8 bytes of all submitted text (and of the schema for extraction).
    pub max_input_bytes: usize,
    pub max_translation_items: usize,
    pub max_schema_bytes: usize,
    pub execution_timeout: Duration,
    pub tenant_rate: f64,
    pub tenant_burst: u32,
    pub tenant_concurrency: usize,
}

/// Margin added to the execution timeout before the HTTP layer and the connection writer give up.
pub const TRANSPORT_MARGIN: Duration = Duration::from_secs(5);

impl Limits {
    /// The execution timeout plus [`TRANSPORT_MARGIN`], or `None` when the sum is not
    /// representable as a `Duration`.
    pub fn transport_allowance(&self) -> Option<Duration> {
        self.execution_timeout.checked_add(TRANSPORT_MARGIN)
    }

    /// The monotonic deadline of an execution that starts at `start`, or `None` when it is
    /// beyond the platform's `Instant` range. Never panics, unlike `start + timeout`.
    pub fn deadline_from(&self, start: tokio::time::Instant) -> Option<tokio::time::Instant> {
        start.checked_add(self.execution_timeout)
    }

    /// The deadline of the transport allowance that starts at `start`, with the same
    /// guarantees as [`Limits::deadline_from`].
    pub fn transport_deadline_from(
        &self,
        start: tokio::time::Instant,
    ) -> Option<tokio::time::Instant> {
        start.checked_add(self.transport_allowance()?)
    }
}

/// Retry policy for transient Gateway failures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first attempt; zero disables retries.
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

/// Usage delivery worker settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxSettings {
    pub interval: Duration,
    pub batch: i64,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// Pending rows above which an alert is logged and flagged on the backlog gauge.
    pub alert_backlog: i64,
    /// Age of the oldest pending row above which an alert is logged.
    pub alert_age: Duration,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub listen: String,
    pub delcore: DelcoreSettings,
    pub gateway: GatewaySettings,
    pub limits: Limits,
    pub retry: RetryPolicy,
    pub outbox: OutboxSettings,
    pub dependency_timeout: Duration,
    /// How long a successful catalog price check is reused.
    pub price_cache_ttl: Duration,
    pub models: ModelCatalog,
}

const MAX_INPUT_BYTES_CEILING: usize = 4 << 20;
const MAX_ITEMS_CEILING: usize = 1000;
const MAX_RETRIES_CEILING: u32 = 5;

impl Config {
    /// Loads and validates the process environment.
    pub fn load() -> Result<Self> {
        Self::from_lookup(&|key| std::env::var(key).ok())
    }

    /// Loads and validates settings from an arbitrary variable source.
    pub fn from_lookup(get: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        let var = |key: &str| get(key).filter(|v| !v.is_empty());
        let required = |key: &str| var(key).with_context(|| format!("{key} is required"));
        let number = |key: &str, default: i64| -> Result<i64> {
            let n = var(key)
                .map(|v| {
                    v.parse::<i64>()
                        .with_context(|| format!("{key} must be an integer"))
                })
                .transpose()?
                .unwrap_or(default);
            ensure!(n > 0, "{key} must be positive");
            Ok(n)
        };
        let rate = |key: &str, default: f64| -> Result<f64> {
            let n = var(key)
                .map(|v| {
                    v.parse::<f64>()
                        .with_context(|| format!("{key} must be a number"))
                })
                .transpose()?
                .unwrap_or(default);
            ensure!(
                n > 0.0 && n.is_finite(),
                "{key} must be positive and finite"
            );
            Ok(n)
        };
        let time = |key: &str, default: Duration| -> Result<Duration> {
            match var(key) {
                Some(v) => parse_duration(key, &v),
                None => Ok(default),
            }
        };

        let delcore = DelcoreSettings {
            url: required("DELCORE_URL")?,
            token_url: required("LOGTO_TOKEN_URL")?,
            resource: required("DELCORE_RESOURCE")?,
            client_id: required("CLIENT_ID")?,
            client_secret: Secret::new(required("CLIENT_SECRET")?),
            service_id: required("DELITOOLS_SERVICE_ID")?,
        };
        ensure!(
            safe_url(&delcore.url) && safe_url(&delcore.token_url),
            "Delcore and token endpoints must be https or loopback http URLs"
        );
        ensure!(
            server_common::canonical_id(&delcore.service_id),
            "DELITOOLS_SERVICE_ID must be a canonical UUID v7"
        );
        let gateway = GatewaySettings {
            base_url: var("AI_GATEWAY_BASE_URL").unwrap_or_else(|| DEFAULT_GATEWAY_URL.into()),
            api_key: Secret::new(required("AI_GATEWAY_API_KEY")?),
            max_output_tokens: u32::try_from(number("DELITOOLS_MAX_OUTPUT_TOKENS", 16_384)?)
                .context("DELITOOLS_MAX_OUTPUT_TOKENS is too large")?,
        };
        ensure!(
            safe_url(&gateway.base_url),
            "AI_GATEWAY_BASE_URL must be an https or loopback http URL"
        );
        let limits = Limits {
            max_input_bytes: number("DELITOOLS_MAX_INPUT_BYTES", 256 << 10)? as usize,
            max_translation_items: number("DELITOOLS_MAX_TRANSLATION_ITEMS", 200)? as usize,
            max_schema_bytes: number("DELITOOLS_MAX_SCHEMA_BYTES", 32 << 10)? as usize,
            execution_timeout: time("DELITOOLS_EXECUTION_TIMEOUT", Duration::from_secs(120))?,
            tenant_rate: rate("DELITOOLS_TENANT_RATE_PER_SECOND", 5.0)?,
            tenant_burst: u32::try_from(number("DELITOOLS_TENANT_RATE_BURST", 10)?)
                .context("DELITOOLS_TENANT_RATE_BURST is too large")?,
            tenant_concurrency: number("DELITOOLS_TENANT_CONCURRENCY", 4)? as usize,
        };
        let retry = RetryPolicy {
            max_retries: var("DELITOOLS_MAX_RETRIES").map_or(Ok(2), |v| {
                v.parse::<u32>()
                    .context("DELITOOLS_MAX_RETRIES must be a non-negative integer")
            })?,
            base_delay: time("DELITOOLS_RETRY_BASE_DELAY", Duration::from_millis(250))?,
            max_delay: time("DELITOOLS_RETRY_MAX_DELAY", Duration::from_secs(5))?,
        };
        let outbox = OutboxSettings {
            interval: time("DELITOOLS_OUTBOX_INTERVAL", Duration::from_secs(5))?,
            batch: number("DELITOOLS_OUTBOX_BATCH", 50)?,
            backoff_min: time("DELITOOLS_OUTBOX_BACKOFF_MIN", Duration::from_secs(5))?,
            backoff_max: time("DELITOOLS_OUTBOX_BACKOFF_MAX", Duration::from_secs(900))?,
            alert_backlog: number("DELITOOLS_OUTBOX_ALERT_BACKLOG", 1000)?,
            alert_age: time("DELITOOLS_OUTBOX_ALERT_AGE", Duration::from_secs(900))?,
        };
        let models_path = required("DELITOOLS_MODELS")?;
        let text = std::fs::read_to_string(&models_path)
            .with_context(|| "DELITOOLS_MODELS could not be read".to_string())?;
        let models = ModelCatalog::parse(&text, models_path.ends_with(".json"))
            .context("DELITOOLS_MODELS is invalid")?;
        let config = Self {
            listen: var("LISTEN").unwrap_or_else(|| DEFAULT_LISTEN.into()),
            delcore,
            gateway,
            limits,
            retry,
            outbox,
            dependency_timeout: time("DEPENDENCY_TIMEOUT", Duration::from_secs(3))?,
            price_cache_ttl: time("DELITOOLS_PRICE_CACHE_TTL", Duration::from_secs(30))?,
            models,
        };
        config.validate()?;
        Ok(config)
    }

    /// Cross-field validation shared by loading and tests that build a `Config` directly.
    pub fn validate(&self) -> Result<()> {
        let l = &self.limits;
        ensure!(
            l.max_input_bytes <= MAX_INPUT_BYTES_CEILING,
            "DELITOOLS_MAX_INPUT_BYTES must not exceed {MAX_INPUT_BYTES_CEILING}"
        );
        ensure!(
            l.max_translation_items <= MAX_ITEMS_CEILING,
            "DELITOOLS_MAX_TRANSLATION_ITEMS must not exceed {MAX_ITEMS_CEILING}"
        );
        ensure!(
            l.max_schema_bytes <= l.max_input_bytes,
            "DELITOOLS_MAX_SCHEMA_BYTES must not exceed DELITOOLS_MAX_INPUT_BYTES"
        );
        ensure!(
            l.tenant_burst as f64 >= l.tenant_rate,
            "burst must not be smaller than the sustained tenant rate"
        );
        ensure!(
            l.tenant_burst >= 1 && l.tenant_concurrency >= 1,
            "tenant burst and concurrency must be at least 1"
        );
        // Deadlines are monotonic instants, so the timeout and its transport allowance must be
        // representable from now on this platform, not only as a `Duration`. The bound is
        // platform dependent. The message never echoes the configured value.
        ensure!(
            l.transport_deadline_from(tokio::time::Instant::now())
                .is_some(),
            "DELITOOLS_EXECUTION_TIMEOUT is too large for this platform's monotonic clock"
        );
        ensure!(
            self.dependency_timeout <= l.execution_timeout,
            "DEPENDENCY_TIMEOUT must not exceed DELITOOLS_EXECUTION_TIMEOUT"
        );
        ensure!(
            self.retry.max_retries <= MAX_RETRIES_CEILING,
            "DELITOOLS_MAX_RETRIES must not exceed {MAX_RETRIES_CEILING}"
        );
        ensure!(
            self.retry.base_delay <= self.retry.max_delay,
            "DELITOOLS_RETRY_BASE_DELAY must not exceed DELITOOLS_RETRY_MAX_DELAY"
        );
        ensure!(
            self.outbox.backoff_min <= self.outbox.backoff_max,
            "DELITOOLS_OUTBOX_BACKOFF_MIN must not exceed DELITOOLS_OUTBOX_BACKOFF_MAX"
        );
        // A bound that cannot form a calendar deadline would make every retry fail to schedule.
        ensure_schedulable("DELITOOLS_OUTBOX_BACKOFF_MIN", self.outbox.backoff_min)?;
        ensure_schedulable("DELITOOLS_OUTBOX_BACKOFF_MAX", self.outbox.backoff_max)?;
        ensure!(
            self.outbox.batch <= 500,
            "DELITOOLS_OUTBOX_BATCH must not exceed 500"
        );
        ensure!(
            self.gateway.max_output_tokens <= 1_000_000,
            "DELITOOLS_MAX_OUTPUT_TOKENS must not exceed 1000000"
        );
        self.models.require_extraction_model()?;
        Ok(())
    }
}

/// Rejects a backoff bound that cannot be added to the current time as a calendar deadline.
///
/// Both the conversion to a signed chrono delta and the checked calendar addition must succeed.
/// The error names the setting only and never prints its value.
fn ensure_schedulable(key: &str, bound: Duration) -> Result<()> {
    let schedulable = chrono::Duration::from_std(bound)
        .ok()
        .and_then(|delta| server_common::now().checked_add_signed(delta))
        .is_some();
    ensure!(
        schedulable,
        "{key} is too large to schedule a retry deadline"
    );
    Ok(())
}

/// Parses durations such as `250ms`, `5s`, `2m` or `1h30m`.
pub fn parse_duration(key: &str, value: &str) -> Result<Duration> {
    let mut rest = value;
    let mut seconds = 0.0;
    while !rest.is_empty() {
        let end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .with_context(|| format!("{key} must be a duration"))?;
        let amount: f64 = rest[..end]
            .parse()
            .with_context(|| format!("{key} must be a duration"))?;
        rest = &rest[end..];
        let (unit, scale) = [("ms", 1e-3), ("s", 1.0), ("m", 60.0), ("h", 3600.0)]
            .into_iter()
            .find(|(unit, _)| rest.starts_with(unit))
            .with_context(|| format!("{key} must be a duration"))?;
        rest = &rest[unit.len()..];
        seconds += amount * scale;
    }
    if !seconds.is_finite() || seconds <= 0.0 {
        bail!("{key} must be positive");
    }
    let parsed =
        Duration::try_from_secs_f64(seconds).with_context(|| format!("{key} is out of range"))?;
    // A positive input below 0.5ns passes the check above but rounds to Duration::ZERO. Consumers
    // such as tokio::time::interval panic on a zero period, so reject it at startup. The error
    // names the setting only and never prints its value.
    if parsed.is_zero() {
        bail!("{key} must be at least 1ns after conversion to nanosecond precision");
    }
    Ok(parsed)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    pub(crate) fn models_file(dir: &std::path::Path) -> String {
        let path = dir.join("models.json");
        std::fs::write(
            &path,
            r#"{"models":[
              {"model":"openai/gpt-6-luna","input_meter_key":"luna_in","output_meter_key":"luna_out"},
              {"model":"google/gemini-translate","input_meter_key":"gem_in","output_meter_key":"gem_out"}
            ]}"#,
        )
        .unwrap();
        path.to_string_lossy().into_owned()
    }

    pub(crate) fn base_env(dir: &std::path::Path) -> HashMap<String, String> {
        [
            ("DELCORE_URL", "http://127.0.0.1:8080"),
            ("LOGTO_TOKEN_URL", "http://127.0.0.1:9999/token"),
            ("DELCORE_RESOURCE", "https://delcore.example"),
            ("CLIENT_ID", "client"),
            ("CLIENT_SECRET", "client-secret"),
            (
                "DELITOOLS_SERVICE_ID",
                "019b0000-0000-7000-8000-000000000001",
            ),
            ("AI_GATEWAY_API_KEY", "gateway-secret"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .chain([("DELITOOLS_MODELS".to_owned(), models_file(dir))])
        .collect()
    }

    fn load(env: &HashMap<String, String>) -> Result<Config> {
        Config::from_lookup(&|key| env.get(key).cloned())
    }

    #[test]
    fn defaults_are_valid() {
        let dir = tempdir();
        let config = load(&base_env(&dir)).unwrap();
        assert_eq!(config.listen, DEFAULT_LISTEN);
        assert_eq!(config.gateway.base_url, DEFAULT_GATEWAY_URL);
        assert_eq!(config.retry.max_retries, 2);
        assert_eq!(config.limits.tenant_concurrency, 4);
        assert_eq!(config.models.len(), 2);
    }

    #[test]
    fn missing_required_settings_fail() {
        let dir = tempdir();
        for key in [
            "DELCORE_URL",
            "LOGTO_TOKEN_URL",
            "DELCORE_RESOURCE",
            "CLIENT_ID",
            "CLIENT_SECRET",
            "DELITOOLS_SERVICE_ID",
            "AI_GATEWAY_API_KEY",
            "DELITOOLS_MODELS",
        ] {
            let mut env = base_env(&dir);
            env.remove(key);
            let error = load(&env).unwrap_err().to_string();
            assert!(error.contains(key), "{key}: {error}");
            env.insert(key.into(), String::new());
            assert!(load(&env).is_err(), "{key} empty");
        }
    }

    #[test]
    fn invalid_values_fail() {
        let dir = tempdir();
        let cases = [
            ("DELITOOLS_MAX_INPUT_BYTES", "0"),
            ("DELITOOLS_MAX_INPUT_BYTES", "9999999999"),
            ("DELITOOLS_MAX_TRANSLATION_ITEMS", "100000"),
            ("DELITOOLS_MAX_SCHEMA_BYTES", "999999999"),
            ("DELITOOLS_EXECUTION_TIMEOUT", "soon"),
            ("DELITOOLS_EXECUTION_TIMEOUT", "0s"),
            ("DEPENDENCY_TIMEOUT", "10m"),
            ("DELITOOLS_TENANT_RATE_PER_SECOND", "-1"),
            ("DELITOOLS_TENANT_RATE_PER_SECOND", "NaN"),
            ("DELITOOLS_TENANT_RATE_BURST", "0"),
            ("DELITOOLS_TENANT_CONCURRENCY", "0"),
            ("DELITOOLS_MAX_RETRIES", "99"),
            ("DELITOOLS_MAX_RETRIES", "-1"),
            ("DELITOOLS_RETRY_BASE_DELAY", "1h"),
            ("DELITOOLS_OUTBOX_BACKOFF_MIN", "2h"),
            ("DELITOOLS_OUTBOX_BACKOFF_MIN", "10000000000000s"),
            ("DELITOOLS_OUTBOX_BACKOFF_MIN", "10000000000000000s"),
            ("DELITOOLS_OUTBOX_BACKOFF_MAX", "10000000000000s"),
            ("DELITOOLS_OUTBOX_BACKOFF_MAX", "10000000000000000s"),
            ("DELITOOLS_OUTBOX_BATCH", "1000"),
            ("AI_GATEWAY_BASE_URL", "http://gateway.example/v1"),
            ("DELCORE_URL", "ftp://delcore"),
            ("DELITOOLS_SERVICE_ID", "not-a-uuid"),
        ];
        for (key, value) in cases {
            let mut env = base_env(&dir);
            env.insert(key.into(), value.into());
            assert!(load(&env).is_err(), "{key}={value}");
        }
    }

    #[test]
    fn unschedulable_backoff_bounds_fail_loading_and_validation_without_panicking() {
        let dir = tempdir();
        for secs in ["10000000000000s", "10000000000000000s"] {
            let mut env = base_env(&dir);
            env.insert("DELITOOLS_OUTBOX_BACKOFF_MIN".into(), secs.into());
            env.insert("DELITOOLS_OUTBOX_BACKOFF_MAX".into(), secs.into());
            let message = format!("{:#}", load(&env).unwrap_err());
            assert!(
                message.contains("DELITOOLS_OUTBOX_BACKOFF_MIN"),
                "{message}"
            );
            assert!(!message.contains(secs), "{message}");

            let mut valid = load(&base_env(&dir)).unwrap();
            valid.outbox.backoff_min = Duration::from_secs(5);
            valid.outbox.backoff_max =
                Duration::from_secs(secs.trim_end_matches('s').parse().unwrap());
            let message = format!("{:#}", valid.validate().unwrap_err());
            assert!(
                message.contains("DELITOOLS_OUTBOX_BACKOFF_MAX"),
                "{message}"
            );
        }
    }

    #[test]
    fn large_representable_backoff_bounds_are_accepted() {
        let dir = tempdir();
        let mut env = base_env(&dir);
        env.insert("DELITOOLS_OUTBOX_BACKOFF_MIN".into(), "1440h".into());
        env.insert("DELITOOLS_OUTBOX_BACKOFF_MAX".into(), "5184000s".into());
        assert!(load(&env).is_ok());
    }

    #[test]
    fn burst_below_rate_is_rejected() {
        let dir = tempdir();
        let mut env = base_env(&dir);
        env.insert("DELITOOLS_TENANT_RATE_PER_SECOND".into(), "50".into());
        env.insert("DELITOOLS_TENANT_RATE_BURST".into(), "10".into());
        assert!(load(&env).is_err());
    }

    #[test]
    fn models_file_must_name_the_extraction_model() {
        let dir = tempdir();
        let mut env = base_env(&dir);
        let path = dir.join("only-translation.json");
        std::fs::write(
            &path,
            r#"{"models":[{"model":"google/gemini-translate","input_meter_key":"a","output_meter_key":"b"}]}"#,
        )
        .unwrap();
        env.insert(
            "DELITOOLS_MODELS".into(),
            path.to_string_lossy().into_owned(),
        );
        let error = load(&env).unwrap_err();
        assert!(format!("{error:#}").contains("openai/gpt-6-luna"));
    }

    #[test]
    fn unreadable_models_file_fails_without_leaking_the_path_contents() {
        let dir = tempdir();
        let mut env = base_env(&dir);
        env.insert(
            "DELITOOLS_MODELS".into(),
            dir.join("missing.json").to_string_lossy().into_owned(),
        );
        assert!(load(&env).is_err());
    }

    #[test]
    fn secrets_are_redacted_in_debug_output() {
        let dir = tempdir();
        let config = load(&base_env(&dir)).unwrap();
        let printed = format!("{config:?}");
        assert!(!printed.contains("gateway-secret"));
        assert!(!printed.contains("client-secret"));
        assert!(printed.contains("<redacted>"));
    }

    #[test]
    fn durations_parse_compound_units() {
        assert_eq!(
            parse_duration("K", "250ms").unwrap(),
            Duration::from_millis(250)
        );
        assert_eq!(
            parse_duration("K", "1m30s").unwrap(),
            Duration::from_secs(90)
        );
        assert!(parse_duration("K", "10").is_err());
        assert!(parse_duration("K", "5x").is_err());
        assert!(parse_duration("K", "0s").is_err());
    }

    #[test]
    fn durations_that_round_to_zero_are_rejected_naming_the_key() {
        for value in ["0.0000000001s", "0.0000001ms", "0.0000000004s"] {
            let error = parse_duration("DELITOOLS_OUTBOX_INTERVAL", value).unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains("DELITOOLS_OUTBOX_INTERVAL"), "{value}");
            assert!(!message.contains(value), "{value}");
        }
        assert!(parse_duration("K", "0s").is_err());
        assert!(parse_duration("K", "5x").is_err());
        assert!(parse_duration("K", "99999999999999999999999s").is_err());
    }

    #[test]
    fn nonzero_nanosecond_durations_keep_their_values() {
        assert_eq!(
            parse_duration("K", "0.000000001s").unwrap(),
            Duration::from_nanos(1)
        );
        assert_eq!(
            parse_duration("K", "0.0000000006s").unwrap(),
            Duration::from_nanos(1)
        );
        assert_eq!(
            parse_duration("K", "250ms").unwrap(),
            Duration::from_millis(250)
        );
        assert_eq!(
            parse_duration("K", "1m30s").unwrap(),
            Duration::from_secs(90)
        );
    }

    #[test]
    fn zero_rounding_durations_fail_loading_naming_the_setting() {
        let dir = tempdir();
        for key in ["DELITOOLS_OUTBOX_INTERVAL", "DELITOOLS_RETRY_BASE_DELAY"] {
            for value in ["0.0000000001s", "0.0000001ms"] {
                let mut env = base_env(&dir);
                env.insert(key.into(), value.into());
                let error = load(&env).unwrap_err();
                let message = format!("{error:#}");
                assert!(message.contains(key), "{key}={value}: {message}");
                assert!(!message.contains(value), "{key}={value}: {message}");
            }
        }
    }

    #[test]
    fn outbox_interval_defaults_to_five_seconds_and_accepts_one_nanosecond() {
        let dir = tempdir();
        let config = load(&base_env(&dir)).unwrap();
        assert_eq!(config.outbox.interval, Duration::from_secs(5));
        let mut env = base_env(&dir);
        env.insert("DELITOOLS_OUTBOX_INTERVAL".into(), "0.000000001s".into());
        assert_eq!(load(&env).unwrap().outbox.interval, Duration::from_nanos(1));
    }

    #[test]
    fn execution_timeouts_beyond_the_clock_range_are_rejected_without_panicking() {
        let dir = tempdir();
        for value in ["10000000000000000000s", "18446744073709551615s"] {
            let mut env = base_env(&dir);
            env.insert("DELITOOLS_EXECUTION_TIMEOUT".into(), value.into());
            let error = load(&env).unwrap_err().to_string();
            assert!(
                error.contains("DELITOOLS_EXECUTION_TIMEOUT"),
                "{value}: {error}"
            );
            assert!(
                !error.contains(value),
                "the value must not be echoed: {error}"
            );
        }
    }

    #[test]
    fn direct_validation_rejects_unrepresentable_deadlines() {
        let dir = tempdir();
        let mut config = load(&base_env(&dir)).unwrap();
        // Duration::MAX cannot take the transport margin; this used to panic in the router.
        config.limits.execution_timeout = Duration::MAX;
        config.dependency_timeout = Duration::from_secs(3);
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("DELITOOLS_EXECUTION_TIMEOUT"), "{error}");
        // The margin fits a Duration but the deadline does not fit the monotonic clock.
        config.limits.execution_timeout = Duration::MAX - TRANSPORT_MARGIN;
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("DELITOOLS_EXECUTION_TIMEOUT"), "{error}");
    }

    #[test]
    fn deadline_helpers_agree_with_the_clock_at_the_boundary() {
        let dir = tempdir();
        let mut config = load(&base_env(&dir)).unwrap();
        let start = tokio::time::Instant::now();
        assert_eq!(
            config.limits.transport_allowance(),
            Some(Duration::from_secs(125))
        );
        assert_eq!(
            config.limits.deadline_from(start),
            Some(start + Duration::from_secs(120))
        );
        // The largest whole-second offset the platform clock accepts from `start`.
        let (mut ok, mut bad) = (0u64, u64::MAX);
        while bad - ok > 1 {
            let mid = ok + (bad - ok) / 2;
            if start.checked_add(Duration::from_secs(mid)).is_some() {
                ok = mid;
            } else {
                bad = mid;
            }
        }
        let margin = TRANSPORT_MARGIN.as_secs();
        config.limits.execution_timeout = Duration::from_secs(ok - margin);
        assert!(config.limits.deadline_from(start).is_some());
        assert!(config.limits.transport_deadline_from(start).is_some());
        // Only the transport margin crosses the boundary: the execution deadline still fits.
        config.limits.execution_timeout = Duration::from_secs(ok - margin + 1);
        assert!(config.limits.deadline_from(start).is_some());
        assert!(config.limits.transport_deadline_from(start).is_none());
        config.limits.execution_timeout = Duration::from_secs(bad);
        assert!(config.limits.deadline_from(start).is_none());
    }

    #[test]
    fn defaults_compound_durations_and_larger_valid_timeouts_are_kept() {
        let dir = tempdir();
        let config = load(&base_env(&dir)).unwrap();
        assert_eq!(config.limits.execution_timeout, Duration::from_secs(120));
        let mut env = base_env(&dir);
        env.insert("DELITOOLS_EXECUTION_TIMEOUT".into(), "1h30m".into());
        assert_eq!(
            load(&env).unwrap().limits.execution_timeout,
            Duration::from_secs(5400)
        );
        env.insert("DELITOOLS_EXECUTION_TIMEOUT".into(), "720h".into());
        env.insert("DEPENDENCY_TIMEOUT".into(), "1h".into());
        assert_eq!(
            load(&env).unwrap().limits.execution_timeout,
            Duration::from_secs(720 * 3600)
        );
        env.insert("DELITOOLS_EXECUTION_TIMEOUT".into(), "10s".into());
        env.insert("DEPENDENCY_TIMEOUT".into(), "11s".into());
        let error = load(&env).unwrap_err().to_string();
        assert!(error.contains("DEPENDENCY_TIMEOUT"), "{error}");
    }

    pub(crate) fn tempdir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("delitools-config-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
