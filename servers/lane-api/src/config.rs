// SPDX-License-Identifier: FSL-1.1-ALv2
use server_common::config::required;

#[derive(Clone)]
pub struct Config {
    pub listen: String,
    pub public_url: String,
    pub web_origin: String,
    pub metrics_token: String,
    pub max_item_bytes: usize,
    pub max_webhook_bytes: usize,
    pub requests_per_minute: i64,
    pub webhook_requests_per_minute: i64,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::read(|name| required(name))
    }

    fn read(get: impl Fn(&str) -> anyhow::Result<String>) -> anyhow::Result<Self> {
        let positive = |name: &str| -> anyhow::Result<i64> {
            let text = get(name)?;
            anyhow::ensure!(
                text.bytes().all(|b| b.is_ascii_digit()),
                "{name} must be a positive integer"
            );
            let value = text.parse::<i64>()?;
            anyhow::ensure!(value > 0, "{name} must be positive");
            Ok(value)
        };
        let public_url = get("LANE_PUBLIC_URL")?;
        let web_origin = get("WEB_ORIGIN")?;
        for value in [&public_url, &web_origin] {
            let url = url::Url::parse(value)?;
            anyhow::ensure!(
                url.scheme() == "https"
                    || (url.scheme() == "http"
                        && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))),
                "HTTPS or loopback URL required"
            );
            anyhow::ensure!(
                url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none()
                    && url.path() == "/",
                "origin URL required"
            );
        }
        let metrics_token = get("LANE_METRICS_TOKEN")?;
        anyhow::ensure!(
            metrics_token.len() >= 32,
            "metrics token must contain at least 32 characters"
        );
        Ok(Self {
            listen: get("LISTEN").unwrap_or_else(|_| "127.0.0.1:46326".into()),
            public_url: public_url.trim_end_matches('/').into(),
            web_origin: web_origin.trim_end_matches('/').into(),
            metrics_token,
            max_item_bytes: positive("MAX_ITEM_BYTES")?.try_into()?,
            max_webhook_bytes: positive("MAX_WEBHOOK_BYTES")?.try_into()?,
            requests_per_minute: positive("REQUESTS_PER_MINUTE")?,
            webhook_requests_per_minute: positive("WEBHOOK_REQUESTS_PER_MINUTE")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settings(key: &str) -> anyhow::Result<String> {
        Ok(match key {
            "LANE_PUBLIC_URL" => "https://lane.api.delino.io",
            "WEB_ORIGIN" => "https://lane.delino.io",
            "LANE_METRICS_TOKEN" => "synthetic-metrics-token-at-least-32-characters",
            "LISTEN" => "127.0.0.1:46326",
            _ => "1",
        }
        .into())
    }
    #[test]
    fn all_input_and_rate_limits_are_mandatory_positive_integers() {
        for key in [
            "MAX_ITEM_BYTES",
            "MAX_WEBHOOK_BYTES",
            "REQUESTS_PER_MINUTE",
            "WEBHOOK_REQUESTS_PER_MINUTE",
        ] {
            for invalid in ["", "0", "-1", "1.5", " 1", "999999999999999999999999999"] {
                assert!(
                    Config::read(|name| if name == key {
                        Ok(invalid.into())
                    } else {
                        settings(name)
                    })
                    .is_err()
                );
            }
            assert!(
                Config::read(|name| if name == key {
                    anyhow::bail!("missing")
                } else {
                    settings(name)
                })
                .is_err()
            );
        }
        assert!(Config::read(settings).is_ok());
    }
}
