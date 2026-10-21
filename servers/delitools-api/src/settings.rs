// SPDX-License-Identifier: FSL-1.1-ALv2
//! Operator execution settings. They are seeded once from the environment, stored in PostgreSQL
//! and afterwards changed only through the audited `UpdateSettings` RPC. The contract defines no
//! default values; the bounds below only reject nonsensical or unsafe values.
use anyhow::{Result, bail};
use delino_protocol::delitools::v1 as pb;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Records and audit rows expire 30 days after creation. Alert windows must stay below it.
pub const RETENTION_SECONDS: u32 = 30 * 24 * 60 * 60;
/// Hard ceiling for `maxInputBytes`. It also bounds the Connect message size accepted before
/// handlers run, so oversized bodies are rejected without buffering them.
pub const MAX_INPUT_BYTES_CEILING: u32 = 8 << 20;
/// Envelope slack added to the input ceiling for the Connect message limit.
pub const MESSAGE_OVERHEAD_BYTES: usize = 64 << 10;
/// Hard ceiling for `executionTimeoutSeconds`. Operators can change the live value up to it, so
/// anything sized for the longest execution (such as the connection drain) must use this ceiling
/// rather than the value read at startup.
pub const MAX_EXECUTION_TIMEOUT_SECONDS: u32 = 3600;
const MAX_MODELS: usize = 1000;
const MAX_MODEL_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub max_input_bytes: u32,
    pub max_translation_items: u32,
    pub execution_timeout_seconds: u32,
    pub tenant_requests_per_minute: u32,
    pub tenant_concurrency: u32,
    pub max_ai_retries: u32,
    pub price_sync_seconds: u32,
    pub delivery_retry_seconds: u32,
    pub max_backoff_seconds: u32,
    pub alert_backlog: u32,
    pub alert_expiry_seconds: u32,
    pub translation_models: Vec<String>,
}

/// Environment variable names for the initial settings, in the order of the settings fields.
pub const ENV_KEYS: [&str; 12] = [
    "MAX_INPUT_BYTES",
    "MAX_TRANSLATION_ITEMS",
    "EXECUTION_TIMEOUT_SECONDS",
    "TENANT_REQUESTS_PER_MINUTE",
    "TENANT_CONCURRENCY",
    "MAX_AI_RETRIES",
    "PRICE_SYNC_SECONDS",
    "DELIVERY_RETRY_SECONDS",
    "MAX_BACKOFF_SECONDS",
    "ALERT_BACKLOG",
    "ALERT_EXPIRY_SECONDS",
    "TRANSLATION_MODELS",
];

impl Settings {
    /// Parse the initial settings. Every value is required; errors name the variable only.
    pub fn from_getter(get: impl Fn(&str) -> String) -> Result<Self> {
        let number = |key: &str| -> Result<u32> {
            let raw = get(key);
            if raw.is_empty() {
                bail!("{key} is required");
            }
            raw.parse()
                .map_err(|_| anyhow::anyhow!("{key} must be a base-10 unsigned integer"))
        };
        let raw_models = get("TRANSLATION_MODELS");
        if raw_models.is_empty() {
            bail!("TRANSLATION_MODELS is required");
        }
        let settings = Self {
            max_input_bytes: number("MAX_INPUT_BYTES")?,
            max_translation_items: number("MAX_TRANSLATION_ITEMS")?,
            execution_timeout_seconds: number("EXECUTION_TIMEOUT_SECONDS")?,
            tenant_requests_per_minute: number("TENANT_REQUESTS_PER_MINUTE")?,
            tenant_concurrency: number("TENANT_CONCURRENCY")?,
            max_ai_retries: number("MAX_AI_RETRIES")?,
            price_sync_seconds: number("PRICE_SYNC_SECONDS")?,
            delivery_retry_seconds: number("DELIVERY_RETRY_SECONDS")?,
            max_backoff_seconds: number("MAX_BACKOFF_SECONDS")?,
            alert_backlog: number("ALERT_BACKLOG")?,
            alert_expiry_seconds: number("ALERT_EXPIRY_SECONDS")?,
            translation_models: serde_json::from_str(&raw_models).map_err(|_| {
                anyhow::anyhow!("TRANSLATION_MODELS must be a JSON array of strings")
            })?,
        };
        settings.validate()?;
        Ok(settings)
    }

    /// Validate bounds. Messages never include the rejected value.
    pub fn validate(&self) -> Result<()> {
        let range = |name: &str, value: u32, min: u32, max: u32| -> Result<()> {
            if !(min..=max).contains(&value) {
                bail!("{name} must be {min} to {max}");
            }
            Ok(())
        };
        range(
            "maxInputBytes",
            self.max_input_bytes,
            1,
            MAX_INPUT_BYTES_CEILING,
        )?;
        range("maxTranslationItems", self.max_translation_items, 1, 10_000)?;
        range(
            "executionTimeoutSeconds",
            self.execution_timeout_seconds,
            1,
            MAX_EXECUTION_TIMEOUT_SECONDS,
        )?;
        range(
            "tenantRequestsPerMinute",
            self.tenant_requests_per_minute,
            1,
            1_000_000,
        )?;
        range("tenantConcurrency", self.tenant_concurrency, 1, 10_000)?;
        range("maxAiRetries", self.max_ai_retries, 0, 10)?;
        range("priceSyncSeconds", self.price_sync_seconds, 1, 7 * 86_400)?;
        range(
            "deliveryRetrySeconds",
            self.delivery_retry_seconds,
            1,
            86_400,
        )?;
        range(
            "maxBackoffSeconds",
            self.max_backoff_seconds,
            1,
            RETENTION_SECONDS,
        )?;
        if self.max_backoff_seconds < self.delivery_retry_seconds {
            bail!("maxBackoffSeconds must be at least deliveryRetrySeconds");
        }
        range("alertBacklog", self.alert_backlog, 1, 1_000_000_000)?;
        range(
            "alertExpirySeconds",
            self.alert_expiry_seconds,
            1,
            RETENTION_SECONDS - 1,
        )?;
        if self.translation_models.is_empty() || self.translation_models.len() > MAX_MODELS {
            bail!("translationModels must contain 1 to {MAX_MODELS} models");
        }
        let mut seen = HashSet::new();
        for model in &self.translation_models {
            if !valid_model_id(model) {
                bail!("translationModels contains an invalid model identifier");
            }
            if !seen.insert(model.as_str()) {
                bail!("translationModels contains a duplicate model");
            }
        }
        Ok(())
    }

    pub fn allows_model(&self, model: &str) -> bool {
        self.translation_models.iter().any(|m| m == model)
    }

    /// Names of the fields whose values differ, for audit records.
    pub fn changed_fields(&self, other: &Self) -> Vec<&'static str> {
        let mut changed = Vec::new();
        macro_rules! diff {
            ($($field:ident => $name:literal),* $(,)?) => {
                $(if self.$field != other.$field { changed.push($name); })*
            };
        }
        diff!(
            max_input_bytes => "maxInputBytes",
            max_translation_items => "maxTranslationItems",
            execution_timeout_seconds => "executionTimeoutSeconds",
            tenant_requests_per_minute => "tenantRequestsPerMinute",
            tenant_concurrency => "tenantConcurrency",
            max_ai_retries => "maxAiRetries",
            price_sync_seconds => "priceSyncSeconds",
            delivery_retry_seconds => "deliveryRetrySeconds",
            max_backoff_seconds => "maxBackoffSeconds",
            alert_backlog => "alertBacklog",
            alert_expiry_seconds => "alertExpirySeconds",
            translation_models => "translationModels",
        );
        changed
    }

    /// Convert from the wire message. An absent `max_ai_retries` is rejected because zero is a
    /// valid explicit value and must be distinguishable from "not provided".
    pub fn from_proto(settings: &pb::Settings) -> Result<Self> {
        let Some(max_ai_retries) = settings.max_ai_retries else {
            bail!("maxAiRetries is required; zero is a valid value");
        };
        let parsed = Self {
            max_input_bytes: settings.max_input_bytes,
            max_translation_items: settings.max_translation_items,
            execution_timeout_seconds: settings.execution_timeout_seconds,
            tenant_requests_per_minute: settings.tenant_requests_per_minute,
            tenant_concurrency: settings.tenant_concurrency,
            max_ai_retries,
            price_sync_seconds: settings.price_sync_seconds,
            delivery_retry_seconds: settings.delivery_retry_seconds,
            max_backoff_seconds: settings.max_backoff_seconds,
            alert_backlog: settings.alert_backlog,
            alert_expiry_seconds: settings.alert_expiry_seconds,
            translation_models: settings.translation_models.clone(),
        };
        parsed.validate()?;
        Ok(parsed)
    }

    /// Camel-case JSON view used by audit records.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "maxInputBytes": self.max_input_bytes,
            "maxTranslationItems": self.max_translation_items,
            "executionTimeoutSeconds": self.execution_timeout_seconds,
            "tenantRequestsPerMinute": self.tenant_requests_per_minute,
            "tenantConcurrency": self.tenant_concurrency,
            "maxAiRetries": self.max_ai_retries,
            "priceSyncSeconds": self.price_sync_seconds,
            "deliveryRetrySeconds": self.delivery_retry_seconds,
            "maxBackoffSeconds": self.max_backoff_seconds,
            "alertBacklog": self.alert_backlog,
            "alertExpirySeconds": self.alert_expiry_seconds,
            "translationModels": self.translation_models,
        })
    }

    pub fn to_proto(&self) -> pb::Settings {
        pb::Settings {
            max_input_bytes: self.max_input_bytes,
            max_translation_items: self.max_translation_items,
            execution_timeout_seconds: self.execution_timeout_seconds,
            tenant_requests_per_minute: self.tenant_requests_per_minute,
            tenant_concurrency: self.tenant_concurrency,
            max_ai_retries: Some(self.max_ai_retries),
            price_sync_seconds: self.price_sync_seconds,
            delivery_retry_seconds: self.delivery_retry_seconds,
            max_backoff_seconds: self.max_backoff_seconds,
            alert_backlog: self.alert_backlog,
            alert_expiry_seconds: self.alert_expiry_seconds,
            translation_models: self.translation_models.clone(),
            ..Default::default()
        }
    }
}

/// Gateway model identifiers look like `provider/model-name`; allow only a conservative alphabet.
pub fn valid_model_id(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= MAX_MODEL_BYTES
        && model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/' | b':'))
}

#[cfg(test)]
pub(crate) fn fixture() -> Settings {
    Settings {
        max_input_bytes: 65_536,
        max_translation_items: 100,
        execution_timeout_seconds: 60,
        tenant_requests_per_minute: 120,
        tenant_concurrency: 4,
        max_ai_retries: 2,
        price_sync_seconds: 3600,
        delivery_retry_seconds: 30,
        max_backoff_seconds: 3600,
        alert_backlog: 100,
        alert_expiry_seconds: 86_400,
        translation_models: vec!["openai/gpt-6-luna".into(), "anthropic/claude-x".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env() -> HashMap<String, String> {
        [
            ("MAX_INPUT_BYTES", "65536"),
            ("MAX_TRANSLATION_ITEMS", "100"),
            ("EXECUTION_TIMEOUT_SECONDS", "60"),
            ("TENANT_REQUESTS_PER_MINUTE", "120"),
            ("TENANT_CONCURRENCY", "4"),
            ("MAX_AI_RETRIES", "0"),
            ("PRICE_SYNC_SECONDS", "3600"),
            ("DELIVERY_RETRY_SECONDS", "30"),
            ("MAX_BACKOFF_SECONDS", "3600"),
            ("ALERT_BACKLOG", "100"),
            ("ALERT_EXPIRY_SECONDS", "86400"),
            ("TRANSLATION_MODELS", "[\"openai/gpt-6-luna\"]"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }
    fn parse(values: &HashMap<String, String>) -> Result<Settings> {
        Settings::from_getter(|k| values.get(k).cloned().unwrap_or_default())
    }

    #[test]
    fn zero_retries_is_a_valid_explicit_value() {
        assert_eq!(parse(&env()).unwrap().max_ai_retries, 0);
    }

    #[test]
    fn every_missing_or_malformed_value_is_rejected_without_leaking_it() {
        for key in ENV_KEYS {
            let mut missing = env();
            missing.remove(key);
            let error = parse(&missing).unwrap_err().to_string();
            assert!(error.contains(key), "{error}");
            let mut bad = env();
            bad.insert(key.into(), "sensitive-invalid-value".into());
            let error = parse(&bad).unwrap_err().to_string();
            assert!(
                error.contains(key) || error.contains("translationModels"),
                "{error}"
            );
            assert!(!error.contains("sensitive-invalid-value"), "{error}");
        }
    }

    #[test]
    fn bounds_and_relationships_are_enforced() {
        let cases: [(&str, &str); 12] = [
            ("MAX_INPUT_BYTES", "0"),
            ("MAX_INPUT_BYTES", "8388609"),
            ("MAX_TRANSLATION_ITEMS", "0"),
            ("EXECUTION_TIMEOUT_SECONDS", "3601"),
            ("TENANT_REQUESTS_PER_MINUTE", "0"),
            ("TENANT_CONCURRENCY", "0"),
            ("MAX_AI_RETRIES", "11"),
            ("PRICE_SYNC_SECONDS", "0"),
            ("DELIVERY_RETRY_SECONDS", "0"),
            ("MAX_BACKOFF_SECONDS", "29"),
            ("ALERT_EXPIRY_SECONDS", "2592000"),
            ("TRANSLATION_MODELS", "[]"),
        ];
        for (key, value) in cases {
            let mut bad = env();
            bad.insert(key.into(), value.into());
            assert!(parse(&bad).is_err(), "{key}={value}");
        }
        for models in [
            "[\"a\",\"a\"]",
            "[\"has space\"]",
            "[\"\"]",
            "\"openai/x\"",
            "[1]",
        ] {
            let mut bad = env();
            bad.insert("TRANSLATION_MODELS".into(), models.into());
            assert!(parse(&bad).is_err(), "{models}");
        }
    }

    #[test]
    fn proto_round_trip_requires_explicit_retry_count() {
        let settings = fixture();
        let wire = settings.to_proto();
        assert_eq!(Settings::from_proto(&wire).unwrap(), settings);
        let mut absent = wire.clone();
        absent.max_ai_retries = None;
        assert!(Settings::from_proto(&absent).is_err());
        let mut zero = wire;
        zero.max_ai_retries = Some(0);
        assert_eq!(Settings::from_proto(&zero).unwrap().max_ai_retries, 0);
    }

    #[test]
    fn changed_fields_lists_only_differences() {
        let a = fixture();
        let mut b = a.clone();
        assert!(a.changed_fields(&b).is_empty());
        b.tenant_concurrency += 1;
        b.translation_models.push("x/y".into());
        assert_eq!(
            a.changed_fields(&b),
            vec!["tenantConcurrency", "translationModels"]
        );
    }

    #[test]
    fn model_allowlist_is_exact() {
        let s = fixture();
        assert!(s.allows_model("openai/gpt-6-luna"));
        assert!(!s.allows_model("openai/gpt-6"));
    }
}
