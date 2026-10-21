// SPDX-License-Identifier: FSL-1.1-ALv2
use anyhow::{Result, ensure};
use std::{collections::HashMap, time::Duration};

pub const MAX_CARDINALITY: i64 = 1_000_000;
#[derive(Clone, Default)]
pub struct Config {
    pub paddle_credit_product_id: String,
    pub listen: String,
    pub database_url: String,
    pub issuer: String,
    pub audience: String,
    pub operator_role: String,
    pub identity_key: String,
    pub paddle_url: String,
    pub paddle_token: String,
    pub paddle_webhook_secret: String,
    pub email_url: String,
    pub email_token: String,
    pub email_account: String,
    pub email_sender: String,
    pub return_urls: Vec<String>,
    pub timeout_seconds: i64,
    pub retry_limit: i64,
    pub retry_seconds: i64,
    pub max_payload_bytes: usize,
    pub requests_per_minute: i64,
    pub invitation_seconds: i64,
    pub usage_lateness_seconds: i64,
    pub reporting_grace_seconds: i64,
    pub deletion_grace_seconds: i64,
    pub raw_usage_retention_seconds: i64,
    pub accounting_retention_seconds: i64,
    pub audit_retention_seconds: i64,
    pub webhook_retention_seconds: i64,
    pub backlog_alert: i64,
    pub settlement_alert_seconds: i64,
    pub max_meter_groups: i64,
    pub max_meter_group_values: i64,
    pub development: bool,
    pub catalog_dir: String,
    /// Explicit switch for serving the DeliTools consumer. It requires 30-day
    /// reporting windows because DeliTools keeps unconfirmed usage that long.
    pub delitools_enabled: bool,
}
impl Config {
    pub fn load() -> Result<Self> {
        let local: HashMap<String, String> = match dotenvy::from_path_iter(".env") {
            Ok(values) => values
                .collect::<std::result::Result<_, _>>()
                .map_err(|_| anyhow::anyhow!("could not read .env configuration"))?,
            Err(dotenvy::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                HashMap::new()
            }
            Err(_) => anyhow::bail!("could not read .env configuration"),
        };
        Self::from_getter(|key| {
            std::env::var(key)
                .ok()
                .unwrap_or_else(|| local.get(key).cloned().unwrap_or_default())
        })
    }
    pub fn from_getter(get: impl Fn(&str) -> String) -> Result<Self> {
        let number = |key: &str| -> Result<i64> {
            get(key)
                .parse()
                .map_err(|_| anyhow::anyhow!("{key} must be a base-10 integer"))
        };
        let boolean = |key: &str| -> Result<bool> {
            match get(key).as_str() {
                "" | "0" | "f" | "F" | "false" | "False" | "FALSE" => Ok(false),
                "1" | "t" | "T" | "true" | "True" | "TRUE" => Ok(true),
                _ => anyhow::bail!("{key} has an invalid bool value"),
            }
        };
        let mut c = Self {
            paddle_credit_product_id: get("PADDLE_CREDIT_PRODUCT_ID"),
            listen: get("LISTEN"),
            database_url: get("DATABASE_URL"),
            issuer: get("ISSUER"),
            audience: get("AUDIENCE"),
            operator_role: get("OPERATOR_ROLE"),
            identity_key: get("IDENTITY_KEY"),
            paddle_url: get("PADDLE_URL"),
            paddle_token: get("PADDLE_TOKEN"),
            paddle_webhook_secret: get("PADDLE_WEBHOOK_SECRET"),
            email_url: get("EMAIL_URL"),
            email_token: get("EMAIL_TOKEN"),
            email_account: get("EMAIL_ACCOUNT"),
            email_sender: get("EMAIL_SENDER"),
            catalog_dir: get("CATALOG_DIR"),
            timeout_seconds: number("TIMEOUT_SECONDS")?,
            retry_limit: number("RETRY_LIMIT")?,
            retry_seconds: number("RETRY_SECONDS")?,
            requests_per_minute: number("REQUESTS_PER_MINUTE")?,
            invitation_seconds: number("INVITATION_SECONDS")?,
            usage_lateness_seconds: number("USAGE_LATENESS_SECONDS")?,
            reporting_grace_seconds: number("REPORTING_GRACE_SECONDS")?,
            deletion_grace_seconds: number("DELETION_GRACE_SECONDS")?,
            raw_usage_retention_seconds: number("RAW_USAGE_RETENTION_SECONDS")?,
            accounting_retention_seconds: number("ACCOUNTING_RETENTION_SECONDS")?,
            audit_retention_seconds: number("AUDIT_RETENTION_SECONDS")?,
            webhook_retention_seconds: number("WEBHOOK_RETENTION_SECONDS")?,
            backlog_alert: number("BACKLOG_ALERT")?,
            settlement_alert_seconds: number("SETTLEMENT_ALERT_SECONDS")?,
            max_meter_groups: number("MAX_METER_GROUPS")?,
            max_meter_group_values: number("MAX_METER_GROUP_VALUES")?,
            max_payload_bytes: get("MAX_PAYLOAD_BYTES")
                .parse()
                .map_err(|_| anyhow::anyhow!("MAX_PAYLOAD_BYTES must be a base-10 integer"))?,
            return_urls: serde_json::from_str(&get("RETURN_URLS"))
                .map_err(|_| anyhow::anyhow!("RETURN_URLS must be a JSON array of strings"))?,
            development: boolean("DEVELOPMENT")?,
            delitools_enabled: boolean("DELITOOLS_ENABLED")?,
        };
        if c.catalog_dir.is_empty() {
            c.catalog_dir = "servers/delcore-api/catalog".into();
        } else {
            ensure!(
                c.catalog_dir.trim() == c.catalog_dir && !c.catalog_dir.contains('\0'),
                "CATALOG_DIR must name a directory"
            );
            c.catalog_dir = clean_path(&c.catalog_dir);
            ensure!(c.catalog_dir != ".", "CATALOG_DIR must name a directory");
        }
        c.validate()?;
        Ok(c)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.listen.is_empty()
                && !self.database_url.is_empty()
                && !self.audience.is_empty()
                && !self.operator_role.is_empty()
                && self.identity_key.len() >= 32
                && self.paddle_webhook_secret.len() >= 16
                && !self.paddle_credit_product_id.is_empty()
                && !self.paddle_token.is_empty()
                && !self.email_token.is_empty()
                && !self.email_account.is_empty()
                && !self.email_sender.is_empty(),
            "required configuration or credentials missing"
        );
        for n in [
            self.timeout_seconds,
            self.retry_limit,
            self.retry_seconds,
            self.requests_per_minute,
            self.invitation_seconds,
            self.usage_lateness_seconds,
            self.reporting_grace_seconds,
            self.deletion_grace_seconds,
            self.raw_usage_retention_seconds,
            self.accounting_retention_seconds,
            self.audit_retention_seconds,
            self.webhook_retention_seconds,
            self.backlog_alert,
            self.settlement_alert_seconds,
        ] {
            ensure!(
                n > 0 && n <= 100 * 365 * 24 * 60 * 60,
                "policy limits must be explicitly positive and no greater than 100 years in seconds"
            );
        }
        ensure!(
            (1..=MAX_CARDINALITY).contains(&self.max_meter_groups)
                && (1..=MAX_CARDINALITY).contains(&self.max_meter_group_values),
            "MAX_METER_GROUPS and MAX_METER_GROUP_VALUES must be 1 to 1000000"
        );
        ensure!(
            self.timeout_seconds <= 120
                && (1024..=16 << 20).contains(&self.max_payload_bytes)
                && self.retry_limit <= 100,
            "request or retry bounds exceed safety limits"
        );
        ensure!(
            self.reporting_grace_seconds >= self.usage_lateness_seconds
                && self.raw_usage_retention_seconds >= self.usage_lateness_seconds
                && self.accounting_retention_seconds >= self.raw_usage_retention_seconds
                && self.webhook_retention_seconds >= self.reporting_grace_seconds,
            "contradictory retention or reporting policies"
        );
        // DeliTools keeps unconfirmed usage reports for 30 days and may deliver them
        // at the end of that period; shorter windows would reject them as late.
        // Raw usage and webhook retention must cover the same windows.
        if self.delitools_enabled {
            ensure!(
                self.usage_lateness_seconds >= crate::delitools::MIN_REPORTING_WINDOW_SECONDS
                    && self.reporting_grace_seconds
                        >= crate::delitools::MIN_REPORTING_WINDOW_SECONDS
                    && self.raw_usage_retention_seconds >= self.usage_lateness_seconds
                    && self.webhook_retention_seconds >= self.reporting_grace_seconds,
                "DELITOOLS_ENABLED requires USAGE_LATENESS_SECONDS and REPORTING_GRACE_SECONDS of at least 2592000 with retention at least as long"
            );
        }
        for url in [&self.issuer, &self.paddle_url, &self.email_url]
            .into_iter()
            .chain(self.return_urls.iter())
        {
            ensure!(
                safe_url(url, self.development),
                "invalid endpoint or return URL"
            );
        }
        ensure!(
            !self.return_urls.is_empty(),
            "return URL allowlist is required"
        );
        Ok(())
    }
    pub fn allows_return(&self, value: &str) -> bool {
        self.return_urls.iter().any(|v| v == value)
    }
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds as u64)
    }
}
fn clean_path(value: &str) -> String {
    let mut path = std::path::PathBuf::new();
    for component in std::path::Path::new(value).components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if path.file_name().is_some_and(|part| part != "..") {
                    path.pop();
                } else if !path.has_root() {
                    path.push("..");
                }
            }
            other => path.push(other.as_os_str()),
        }
    }
    if path.as_os_str().is_empty() {
        ".".into()
    } else {
        path.to_string_lossy().into_owned()
    }
}
fn safe_url(value: &str, development: bool) -> bool {
    reqwest::Url::parse(value).is_ok_and(|u| {
        u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.fragment().is_none()
            && (u.scheme() == "https"
                || development
                    && u.scheme() == "http"
                    && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn values() -> HashMap<String, String> {
        dotenvy::from_read_iter(include_bytes!("../.env.example").as_slice())
            .collect::<std::result::Result<_, _>>()
            .unwrap()
    }
    #[test]
    fn strict_configuration_and_safe_errors() {
        let base = values();
        let config = Config::from_getter(|k| base.get(k).cloned().unwrap_or_default()).unwrap();
        assert_eq!(config.max_meter_groups, 1000);
        assert!(!config.allows_return("https://consumer.example.invalid/delcore/return?extra=1"));
        for key in [
            "TIMEOUT_SECONDS",
            "RETRY_LIMIT",
            "MAX_PAYLOAD_BYTES",
            "MAX_METER_GROUPS",
            "MAX_METER_GROUP_VALUES",
            "DEVELOPMENT",
            "RETURN_URLS",
        ] {
            let mut bad = base.clone();
            bad.insert(key.into(), "sensitive-invalid-value".into());
            let err = Config::from_getter(|k| bad.get(k).cloned().unwrap_or_default())
                .err()
                .unwrap()
                .to_string();
            assert!(err.contains(key));
            assert!(!err.contains("sensitive-invalid-value"));
        }
        for (key, value) in [
            ("MAX_METER_GROUPS", "0"),
            ("MAX_METER_GROUP_VALUES", "1000001"),
            ("IDENTITY_KEY", ""),
            ("REPORTING_GRACE_SECONDS", "1"),
            ("ACCOUNTING_RETENTION_SECONDS", "1"),
            ("CATALOG_DIR", "."),
            ("CATALOG_DIR", "catalog/.."),
            ("PADDLE_URL", "http://external.example"),
            ("RETURN_URLS", "[\"https://user:pass@example.com\"]"),
        ] {
            let mut bad = base.clone();
            bad.insert(key.into(), value.into());
            assert!(
                Config::from_getter(|k| bad.get(k).cloned().unwrap_or_default()).is_err(),
                "{key}"
            );
        }
    }
    fn with(overrides: &[(&str, &str)]) -> Result<Config> {
        let mut base = values();
        for (key, value) in overrides {
            base.insert((*key).into(), (*value).into());
        }
        Config::from_getter(|k| base.get(k).cloned().unwrap_or_default())
    }
    #[test]
    fn serving_delitools_requires_thirty_day_reporting_windows() {
        let thirty_days = "2592000";
        assert!(with(&[]).unwrap().delitools_enabled);
        for (key, value) in [
            ("USAGE_LATENESS_SECONDS", "2591999"),
            ("REPORTING_GRACE_SECONDS", "2591999"),
            ("USAGE_LATENESS_SECONDS", "604800"),
            ("REPORTING_GRACE_SECONDS", "604800"),
            // Raw usage and webhook retention must cover the windows.
            ("RAW_USAGE_RETENTION_SECONDS", "2591999"),
            ("WEBHOOK_RETENTION_SECONDS", "2591999"),
        ] {
            assert!(with(&[(key, value)]).is_err(), "{key}={value}");
        }
        // Exactly 30 days and longer windows are accepted when retention covers them.
        assert!(
            with(&[
                ("USAGE_LATENESS_SECONDS", thirty_days),
                ("REPORTING_GRACE_SECONDS", thirty_days)
            ])
            .is_ok()
        );
        assert!(
            with(&[
                ("USAGE_LATENESS_SECONDS", "5184000"),
                ("REPORTING_GRACE_SECONDS", "7776000")
            ])
            .is_ok()
        );
        // The rule applies only when the explicit switch is on.
        let short = [
            ("USAGE_LATENESS_SECONDS", "604800"),
            ("REPORTING_GRACE_SECONDS", "604800"),
        ];
        let mut off = vec![("DELITOOLS_ENABLED", "false")];
        off.extend(short);
        let config = with(&off).unwrap();
        assert!(!config.delitools_enabled);
        let omitted = {
            let mut base = values();
            base.remove("DELITOOLS_ENABLED");
            base.insert("USAGE_LATENESS_SECONDS".into(), "604800".into());
            base.insert("REPORTING_GRACE_SECONDS".into(), "604800".into());
            Config::from_getter(|k| base.get(k).cloned().unwrap_or_default()).unwrap()
        };
        assert!(!omitted.delitools_enabled);
        // The switch accepts only boolean spellings and never echoes the value.
        let error = with(&[("DELITOOLS_ENABLED", "sensitive-invalid-value")])
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("DELITOOLS_ENABLED") && !error.contains("sensitive"));
        // The committed example is a valid DeliTools installation.
        assert!(config_example_serves_delitools());
    }
    fn config_example_serves_delitools() -> bool {
        let c = with(&[]).unwrap();
        c.delitools_enabled
            && c.usage_lateness_seconds >= crate::delitools::MIN_REPORTING_WINDOW_SECONDS
            && c.reporting_grace_seconds >= crate::delitools::MIN_REPORTING_WINDOW_SECONDS
            && c.raw_usage_retention_seconds >= c.usage_lateness_seconds
            && c.webhook_retention_seconds >= c.reporting_grace_seconds
    }
    #[test]
    fn development_urls_are_loopback_only() {
        for value in ["http://localhost/x", "http://127.0.0.1/x", "http://[::1]/x"] {
            assert!(safe_url(value, true));
            assert!(!safe_url(value, false));
        }
        for value in [
            "http://example.com",
            "https://user@example.com",
            "https://example.com/#x",
        ] {
            assert!(!safe_url(value, true));
        }
    }
}
