// SPDX-License-Identifier: FSL-1.1-ALv2
//! Deployment configuration from the environment. An optional working-directory `.env` supplies
//! values that process variables do not define. Startup rejects missing or invalid values with
//! messages that name the variable but never repeat its value. No defaults are invented.
use crate::settings::{MAX_EXECUTION_TIMEOUT_SECONDS, Settings};
use anyhow::{Result, bail, ensure};
use server_common::config::safe_url;
use std::{collections::HashMap, time::Duration};

pub const MAX_TIMEOUT_SECONDS: u64 = 120;
const MAX_GATEWAY_RESPONSE_CEILING: usize = 64 << 20;

#[derive(Clone)]
pub struct Config {
    pub listen: String,
    pub database_url: String,
    pub issuer: String,
    pub audience: String,
    pub operator_role: String,
    pub delcore_url: String,
    pub delcore_resource: String,
    pub logto_token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub service_id: uuid::Uuid,
    pub gateway_url: String,
    pub gateway_key: String,
    pub timeout_seconds: u64,
    pub max_gateway_response_bytes: usize,
    /// Seeds the stored settings exactly once; afterwards only `UpdateSettings` changes them.
    pub initial_settings: Settings,
}

// Secrets and the database URL (which can embed a password) are never printed.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("listen", &self.listen)
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("service_id", &self.service_id)
            .field("secrets", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Read the optional working-directory `.env`. Process variables always win.
fn local_env() -> Result<HashMap<String, String>> {
    match dotenvy::from_path_iter(".env") {
        Ok(values) => values
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| anyhow::anyhow!("could not read .env configuration")),
        Err(dotenvy::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(HashMap::new())
        }
        Err(_) => bail!("could not read .env configuration"),
    }
}

/// Publish the working-directory `.env` to the process environment without overriding a variable
/// that is already set (an explicitly empty process value counts as set). Call it before the
/// logging and tracing setup: `server_common::telemetry::init` reads `LOG_FORMAT`, `RUST_LOG` and
/// the `OTEL_EXPORTER_OTLP_*` variables directly from the process environment, so values supplied
/// only through `.env` would otherwise never take effect. Configuration parsing keeps its own
/// process-first lookup, which this does not change. Only the exact working-directory file is read.
pub fn export_local_environment() -> Result<()> {
    match dotenvy::from_path(".env") {
        Ok(()) => Ok(()),
        Err(dotenvy::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => bail!("could not read .env configuration"),
    }
}

fn getter(local: HashMap<String, String>) -> impl Fn(&str) -> String {
    move |key| {
        std::env::var(key)
            .ok()
            .unwrap_or_else(|| local.get(key).cloned().unwrap_or_default())
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        Self::from_getter(getter(local_env()?))
    }

    /// The explicit `--migrate` and `--check-schema` commands need only the database URL.
    pub fn database_url_only() -> Result<String> {
        let get = getter(local_env()?);
        let url = get("DATABASE_URL");
        ensure!(!url.is_empty(), "DATABASE_URL is required");
        Ok(url)
    }

    pub fn from_getter(get: impl Fn(&str) -> String) -> Result<Self> {
        let required = |key: &str| -> Result<String> {
            let value = get(key);
            ensure!(!value.is_empty(), "{key} is required");
            ensure!(
                value.trim() == value && !value.contains('\0'),
                "{key} must not contain surrounding whitespace"
            );
            Ok(value)
        };
        let number = |key: &str| -> Result<u64> {
            required(key)?
                .parse()
                .map_err(|_| anyhow::anyhow!("{key} must be a base-10 unsigned integer"))
        };
        let url = |key: &str| -> Result<String> {
            let value = required(key)?;
            ensure!(
                safe_url(&value),
                "{key} must be an https URL (loopback http is allowed for fixtures)"
            );
            Ok(value)
        };
        let service_id = required("SERVICE_ID")?;
        ensure!(
            server_common::canonical_id(&service_id),
            "SERVICE_ID must be a canonical UUID v7"
        );
        let timeout_seconds = number("TIMEOUT_SECONDS")?;
        ensure!(
            (1..=MAX_TIMEOUT_SECONDS).contains(&timeout_seconds),
            "TIMEOUT_SECONDS must be 1 to {MAX_TIMEOUT_SECONDS}"
        );
        let max_gateway_response_bytes = number("MAX_GATEWAY_RESPONSE_BYTES")? as usize;
        ensure!(
            (1024..=MAX_GATEWAY_RESPONSE_CEILING).contains(&max_gateway_response_bytes),
            "MAX_GATEWAY_RESPONSE_BYTES must be 1024 to {MAX_GATEWAY_RESPONSE_CEILING}"
        );
        let config = Self {
            listen: required("LISTEN")?,
            database_url: required("DATABASE_URL")?,
            issuer: url("ISSUER")?,
            audience: required("AUDIENCE")?,
            operator_role: required("OPERATOR_ROLE")?,
            delcore_url: url("DELCORE_URL")?,
            delcore_resource: required("DELCORE_RESOURCE")?,
            logto_token_url: url("LOGTO_TOKEN_URL")?,
            client_id: required("CLIENT_ID")?,
            client_secret: required("CLIENT_SECRET")?,
            service_id: uuid::Uuid::parse_str(&service_id).expect("validated above"),
            gateway_url: url("GATEWAY_URL")?,
            gateway_key: required("GATEWAY_KEY")?,
            timeout_seconds,
            max_gateway_response_bytes,
            initial_settings: Settings::from_getter(&get)?,
        };
        Ok(config)
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }

    /// How long shutdown lets in-flight connections finish. Operators can raise
    /// `executionTimeoutSeconds` while the process runs and requests are admitted under the new
    /// value at once, so a value captured at startup could close a request before its deadline.
    /// The ceiling covers every supported live value; idle connections still close immediately
    /// and every request stays bounded by its own deadline.
    pub fn drain_ceiling(&self) -> Duration {
        self.timeout() + Duration::from_secs(u64::from(MAX_EXECUTION_TIMEOUT_SECONDS))
    }

    /// Socket limits for the shared transport. While a handler runs, the connection keeps a
    /// socket read pending without receiving bytes (to notice a disconnect), and the transport
    /// applies the idle limit to that read as an unconditional timeout. The idle limit therefore
    /// has to cover the longest request deadline, which is the live execution timeout plus the
    /// dependency timeout; a shorter value would close the connection of a long execution that
    /// an operator legitimately permitted. Each request stays bounded by its own deadline.
    pub fn transport_limits(&self) -> server_common::transport::Limits {
        server_common::transport::Limits {
            header: self.timeout(),
            idle: self.drain_ceiling(),
            write: self.timeout(),
            shutdown: self.drain_ceiling(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> HashMap<String, String> {
        dotenvy::from_read_iter(include_bytes!("../.env.example").as_slice())
            .collect::<std::result::Result<_, _>>()
            .unwrap()
    }
    fn load(values: &HashMap<String, String>) -> Result<Config> {
        Config::from_getter(|k| values.get(k).cloned().unwrap_or_default())
    }

    #[test]
    fn the_documented_example_is_a_valid_configuration() {
        let config = load(&example()).unwrap();
        assert_eq!(config.timeout(), Duration::from_secs(20));
        assert_eq!(config.initial_settings.max_ai_retries, 2);
    }

    #[test]
    fn missing_values_are_rejected_by_name() {
        let required = [
            "LISTEN",
            "DATABASE_URL",
            "ISSUER",
            "AUDIENCE",
            "OPERATOR_ROLE",
            "DELCORE_URL",
            "DELCORE_RESOURCE",
            "LOGTO_TOKEN_URL",
            "CLIENT_ID",
            "CLIENT_SECRET",
            "SERVICE_ID",
            "GATEWAY_URL",
            "GATEWAY_KEY",
            "TIMEOUT_SECONDS",
            "MAX_GATEWAY_RESPONSE_BYTES",
        ];
        for key in required {
            let mut values = example();
            values.remove(key);
            let error = load(&values).unwrap_err().to_string();
            assert!(error.contains(key), "{key}: {error}");
            let mut empty = example();
            empty.insert(key.into(), String::new());
            assert!(load(&empty).is_err(), "{key}");
        }
    }

    #[test]
    fn invalid_values_are_rejected_without_leaking_them() {
        let invalid = [
            ("ISSUER", "http://external.example.invalid/oidc"),
            ("DELCORE_URL", "ftp://delcore.example.invalid"),
            ("GATEWAY_URL", "https://user:pass@gateway.example.invalid"),
            ("LOGTO_TOKEN_URL", "not a url"),
            ("SERVICE_ID", "11111111-1111-4111-8111-111111111111"),
            ("TIMEOUT_SECONDS", "0"),
            ("TIMEOUT_SECONDS", "121"),
            ("TIMEOUT_SECONDS", "twenty"),
            ("MAX_GATEWAY_RESPONSE_BYTES", "10"),
            ("MAX_GATEWAY_RESPONSE_BYTES", "-1"),
            ("CLIENT_SECRET", " padded "),
            ("MAX_INPUT_BYTES", "0"),
            ("TRANSLATION_MODELS", "not json"),
        ];
        for (key, value) in invalid {
            let mut values = example();
            values.insert(key.into(), value.to_string());
            let error = load(&values).unwrap_err().to_string();
            assert!(!error.contains(value) || value.len() < 4, "{key}: {error}");
        }
        let mut secret = example();
        secret.insert("CLIENT_SECRET".into(), "sensitive-secret-value".into());
        secret.insert("TIMEOUT_SECONDS".into(), "sensitive-timeout".into());
        let error = load(&secret).unwrap_err().to_string();
        assert!(!error.contains("sensitive"), "{error}");
    }

    #[test]
    fn loopback_http_fixtures_are_allowed() {
        let mut values = example();
        for key in ["ISSUER", "DELCORE_URL", "LOGTO_TOKEN_URL", "GATEWAY_URL"] {
            values.insert(key.into(), "http://127.0.0.1:9/x".into());
        }
        assert!(load(&values).is_ok());
    }

    #[test]
    fn the_drain_ceiling_covers_every_live_execution_timeout() {
        let config = load(&example()).unwrap();
        let mut settings = config.initial_settings.clone();
        // An operator can raise the live timeout to the validated maximum after startup.
        settings.execution_timeout_seconds = MAX_EXECUTION_TIMEOUT_SECONDS;
        assert!(settings.validate().is_ok());
        assert!(
            config.drain_ceiling()
                >= config.timeout()
                    + Duration::from_secs(u64::from(settings.execution_timeout_seconds))
        );
        settings.execution_timeout_seconds = MAX_EXECUTION_TIMEOUT_SECONDS + 1;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn the_idle_read_limit_covers_the_longest_request_deadline() {
        let config = load(&example()).unwrap();
        let limits = config.transport_limits();
        let longest_request =
            config.timeout() + Duration::from_secs(u64::from(MAX_EXECUTION_TIMEOUT_SECONDS));
        assert!(
            limits.idle >= longest_request,
            "a quiet socket read must outlive a request that runs to its deadline"
        );
        assert!(limits.shutdown >= longest_request);
        assert_eq!(limits.header, config.timeout());
        assert_eq!(limits.write, config.timeout());
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let mut values = example();
        values.insert("CLIENT_SECRET".into(), "super-secret-client".into());
        values.insert("GATEWAY_KEY".into(), "super-secret-gateway".into());
        values.insert(
            "DATABASE_URL".into(),
            "postgres://u:super-secret-db@127.0.0.1/db".into(),
        );
        let text = format!("{:?}", load(&values).unwrap());
        assert!(!text.contains("super-secret"), "{text}");
    }
}
