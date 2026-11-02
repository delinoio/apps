// SPDX-License-Identifier: FSL-1.1-ALv2
//! Process configuration. Invalid or missing required values stop startup.
use buffa::Enumeration;
use delino_protocol::anydash::v1::ClientKind;
use std::{collections::HashMap, time::Duration};

pub const DEFAULT_PRICING_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

#[derive(Clone, Debug)]
pub struct Config {
    pub listen: String,
    pub public_url: String,
    pub web_origin: String,
    pub logto_issuer: String,
    pub resource: String,
    pub delcore_url: String,
    pub delcore_resource: String,
    pub logto_token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub min_versions: HashMap<i32, semver::Version>,
    pub latest_versions: HashMap<i32, semver::Version>,
    pub pricing_url: String,
    pub metrics_token: String,
    pub dependency_timeout: Duration,
    /// Optional header written by a trusted reverse proxy with the client IP address. When it is
    /// unset, the socket peer address keys the unauthenticated enrollment limit.
    pub client_ip_header: Option<String>,
}

/// Every `ClientKind` value except the unspecified zero value.
pub fn client_kinds() -> impl Iterator<Item = ClientKind> {
    ClientKind::values()
        .iter()
        .copied()
        .filter(|k| *k != ClientKind::Unspecified)
}

fn origin(name: &str, value: &str) -> anyhow::Result<String> {
    let trimmed = value.trim_end_matches('/');
    anyhow::ensure!(
        server_common::config::safe_url(trimmed) && server_common::config::valid_origin(trimmed),
        "{name} must be an https or loopback http origin"
    );
    Ok(trimmed.into())
}

fn versions(
    name: &str,
    text: &str,
    complete: bool,
) -> anyhow::Result<HashMap<i32, semver::Version>> {
    let map: HashMap<String, String> = serde_json::from_str(text)
        .map_err(|_| anyhow::anyhow!("{name} must be a JSON object of client kinds to versions"))?;
    let mut out = HashMap::new();
    for (kind, version) in map {
        let kind = ClientKind::from_proto_name(&kind)
            .filter(|k| *k != ClientKind::Unspecified)
            .ok_or_else(|| anyhow::anyhow!("{name} names an unknown client kind"))?;
        let version = semver::Version::parse(&version)
            .map_err(|_| anyhow::anyhow!("{name} must contain SemVer versions"))?;
        out.insert(kind.to_i32(), version);
    }
    if complete {
        for kind in client_kinds() {
            anyhow::ensure!(
                out.contains_key(&kind.to_i32()),
                "{name} must name every client kind"
            );
        }
    }
    Ok(out)
}

/// Parses `<integer><unit>` with the unit `ms`, `s` or `m`.
fn parse_duration(text: &str) -> Option<Duration> {
    let split = text.find(|c: char| !c.is_ascii_digit())?;
    let value: u64 = text[..split].parse().ok().filter(|v| *v > 0)?;
    match &text[split..] {
        "ms" => Some(Duration::from_millis(value)),
        "s" => Some(Duration::from_secs(value)),
        "m" => Some(Duration::from_secs(value.checked_mul(60)?)),
        _ => None,
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::read(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
    }

    pub fn read(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let required = |key: &str| get(key).ok_or_else(|| anyhow::anyhow!("{key} is required"));
        let url = |key: &str| -> anyhow::Result<String> {
            let value = required(key)?;
            anyhow::ensure!(
                server_common::config::safe_url(&value),
                "{key} must be an https or loopback http URL"
            );
            Ok(value.trim_end_matches('/').into())
        };
        let metrics_token = required("ANYDASH_METRICS_TOKEN")?;
        anyhow::ensure!(
            metrics_token.len() >= 32,
            "ANYDASH_METRICS_TOKEN must contain at least 32 characters"
        );
        let dependency_timeout = match get("DEPENDENCY_TIMEOUT") {
            None => Duration::from_secs(5),
            Some(text) => parse_duration(&text).ok_or_else(|| {
                anyhow::anyhow!("DEPENDENCY_TIMEOUT must be a positive duration such as 5s")
            })?,
        };
        let client_ip_header = get("ANYDASH_CLIENT_IP_HEADER")
            .map(|h| {
                axum::http::HeaderName::from_bytes(h.as_bytes())
                    .map(|n| n.as_str().to_owned())
                    .map_err(|_| anyhow::anyhow!("ANYDASH_CLIENT_IP_HEADER must be a header name"))
            })
            .transpose()?;
        let pricing_url = match get("ANYDASH_PRICING_URL") {
            Some(_) => url("ANYDASH_PRICING_URL")?,
            None => DEFAULT_PRICING_URL.into(),
        };
        Ok(Self {
            listen: required("LISTEN")?,
            public_url: origin("ANYDASH_PUBLIC_URL", &required("ANYDASH_PUBLIC_URL")?)?,
            web_origin: origin("WEB_ORIGIN", &required("WEB_ORIGIN")?)?,
            logto_issuer: url("LOGTO_ISSUER")?,
            resource: required("ANYDASH_RESOURCE")?,
            delcore_url: url("DELCORE_URL")?,
            delcore_resource: required("DELCORE_RESOURCE")?,
            logto_token_url: url("LOGTO_TOKEN_URL")?,
            client_id: required("CLIENT_ID")?,
            client_secret: required("CLIENT_SECRET")?,
            min_versions: versions(
                "ANYDASH_MIN_CLIENT_VERSIONS",
                &required("ANYDASH_MIN_CLIENT_VERSIONS")?,
                true,
            )?,
            latest_versions: match get("ANYDASH_LATEST_CLIENT_VERSIONS") {
                Some(text) => versions("ANYDASH_LATEST_CLIENT_VERSIONS", &text, false)?,
                None => HashMap::new(),
            },
            pricing_url,
            metrics_token,
            dependency_timeout,
            client_ip_header,
        })
    }

    /// Settings for tests and local fixtures.
    pub fn fixture(issuer: &str, delcore: &str, pricing: &str) -> Self {
        let all = |v: &str| {
            client_kinds()
                .map(|k| (k.to_i32(), semver::Version::parse(v).unwrap()))
                .collect()
        };
        Self {
            listen: "127.0.0.1:0".into(),
            public_url: "https://anydash.api.example.test".into(),
            web_origin: "https://anydash.example.test".into(),
            logto_issuer: issuer.trim_end_matches('/').into(),
            resource: "https://anydash.api.delino.io".into(),
            delcore_url: delcore.into(),
            delcore_resource: "https://delcore.api.delino.io".into(),
            logto_token_url: format!("{delcore}/token"),
            client_id: "anydash".into(),
            client_secret: "secret".into(),
            min_versions: all("1.0.0"),
            latest_versions: all("1.2.0"),
            pricing_url: pricing.into(),
            metrics_token: "synthetic-metrics-token-at-least-32-characters".into(),
            dependency_timeout: Duration::from_secs(5),
            client_ip_header: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settings(key: &str) -> Option<String> {
        Some(
            match key {
                "LISTEN" => "127.0.0.1:46331",
                "ANYDASH_PUBLIC_URL" => "https://anydash.api.delino.io",
                "WEB_ORIGIN" => "https://anydash.delino.io",
                "LOGTO_ISSUER" => "https://auth.example.test/oidc",
                "ANYDASH_RESOURCE" => "https://anydash.api.delino.io",
                "DELCORE_URL" => "https://delcore.api.delino.io",
                "DELCORE_RESOURCE" => "https://delcore.api.delino.io",
                "LOGTO_TOKEN_URL" => "https://auth.example.test/oidc/token",
                "CLIENT_ID" => "client",
                "CLIENT_SECRET" => "secret",
                "ANYDASH_METRICS_TOKEN" => "synthetic-metrics-token-at-least-32-characters",
                "ANYDASH_MIN_CLIENT_VERSIONS" => {
                    r#"{"CLIENT_KIND_CLI":"0.1.0","CLIENT_KIND_DESKTOP":"0.1.0","CLIENT_KIND_WEB":"0.1.0","CLIENT_KIND_IOS":"0.1.0","CLIENT_KIND_ANDROID":"0.1.0","CLIENT_KIND_NATIVE_WIDGET":"0.1.0"}"#
                }
                _ => return None,
            }
            .into(),
        )
    }
    #[test]
    fn required_values_and_version_maps_are_validated() {
        let config = Config::read(settings).unwrap();
        assert_eq!(config.pricing_url, DEFAULT_PRICING_URL);
        assert_eq!(config.min_versions.len(), 6);
        for key in [
            "LISTEN",
            "DATABASE_URL_IS_NOT_READ_HERE",
            "ANYDASH_PUBLIC_URL",
            "WEB_ORIGIN",
            "LOGTO_ISSUER",
            "ANYDASH_RESOURCE",
            "DELCORE_URL",
            "DELCORE_RESOURCE",
            "LOGTO_TOKEN_URL",
            "CLIENT_ID",
            "CLIENT_SECRET",
            "ANYDASH_METRICS_TOKEN",
            "ANYDASH_MIN_CLIENT_VERSIONS",
        ] {
            let result = Config::read(|k| if k == key { None } else { settings(k) });
            assert_eq!(
                result.is_err(),
                key != "DATABASE_URL_IS_NOT_READ_HERE",
                "{key}"
            );
        }
        for (key, value) in [
            (
                "ANYDASH_MIN_CLIENT_VERSIONS",
                r#"{"CLIENT_KIND_CLI":"0.1.0"}"#,
            ),
            (
                "ANYDASH_MIN_CLIENT_VERSIONS",
                r#"{"CLIENT_KIND_UNSPECIFIED":"0.1.0"}"#,
            ),
            ("ANYDASH_MIN_CLIENT_VERSIONS", "[]"),
            (
                "ANYDASH_LATEST_CLIENT_VERSIONS",
                r#"{"CLIENT_KIND_CLI":"one"}"#,
            ),
            ("ANYDASH_METRICS_TOKEN", "short"),
            ("WEB_ORIGIN", "http://anydash.delino.io"),
            ("WEB_ORIGIN", "https://anydash.delino.io/app"),
            ("ANYDASH_PRICING_URL", "ftp://example.test/prices.json"),
            ("ANYDASH_CLIENT_IP_HEADER", "bad header"),
            ("DEPENDENCY_TIMEOUT", "0s"),
            ("DEPENDENCY_TIMEOUT", "5"),
        ] {
            assert!(
                Config::read(|k| if k == key {
                    Some(value.into())
                } else {
                    settings(k)
                })
                .is_err(),
                "{key}={value}"
            );
        }
        let latest = Config::read(|k| {
            if k == "ANYDASH_LATEST_CLIENT_VERSIONS" {
                Some(r#"{"CLIENT_KIND_CLI":"1.2.3-beta.1"}"#.into())
            } else {
                settings(k)
            }
        })
        .unwrap();
        assert_eq!(latest.latest_versions.len(), 1);
    }
}
