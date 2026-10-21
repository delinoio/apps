#![doc = include_str!("../README.md")]

mod types;
pub use types::*;

use reqwest::{
    StatusCode, Url,
    header::{AUTHORIZATION, HeaderValue},
};
use serde::Deserialize;
use std::time::Duration;

/// Cloudflare's public REST API base URL.
pub const API_BASE: &str = "https://api.cloudflare.com/client/v4/";

/// Structured Cloudflare error. Messages may contain sensitive upstream data.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiError {
    pub code: i64,
    pub message: String,
}

/// Client errors. Display intentionally excludes upstream response content.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid client configuration: {0}")]
    Configuration(&'static str),
    #[error("invalid request: {0}")]
    InvalidRequest(&'static str),
    #[error("HTTP transport failed")]
    Transport(#[source] reqwest::Error),
    #[error("Cloudflare request failed with HTTP {status}")]
    Api {
        status: StatusCode,
        errors: Vec<ApiError>,
    },
    #[error("invalid Cloudflare response")]
    Decode(#[source] serde_json::Error),
    #[error("Cloudflare success response has no result")]
    MissingResult,
}

#[derive(Deserialize)]
struct Envelope {
    success: bool,
    result: Option<Response>,
    #[serde(default)]
    errors: Vec<ApiError>,
}

/// Reusable asynchronous client. Clones share the HTTP connection pool.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    endpoint: Url,
    authorization: HeaderValue,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

impl Client {
    /// Create a client with a 120-second timeout and no redirects or retries.
    pub fn new(account_id: &str, token: &str) -> Result<Self, Error> {
        Self::with_options(account_id, token, API_BASE, Duration::from_secs(120))
    }

    /// Set a trusted API base and timeout. The base must include any API prefix.
    /// Custom bases receive your bearer token. HTTP is intended for local tests.
    pub fn with_options(
        account_id: &str,
        token: &str,
        api_base: &str,
        timeout: Duration,
    ) -> Result<Self, Error> {
        if account_id.is_empty()
            || !account_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(Error::Configuration("invalid account ID"));
        }
        if token.trim().is_empty() {
            return Err(Error::Configuration("empty token"));
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| Error::Configuration("invalid token header"))?;
        authorization.set_sensitive(true);
        let mut endpoint =
            Url::parse(api_base).map_err(|_| Error::Configuration("invalid API base"))?;
        if !matches!(endpoint.scheme(), "https" | "http")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(Error::Configuration(
                "API base must be an HTTP(S) URL without credentials, query or fragment",
            ));
        }
        endpoint
            .path_segments_mut()
            .map_err(|_| Error::Configuration("invalid API base path"))?
            .pop_if_empty()
            .extend([
                "accounts",
                account_id,
                "ai",
                "run",
                "@cf",
                "cloudflare",
                "clairic",
            ]);
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("cf-clairic/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(Error::Transport)?;
        Ok(Self {
            http,
            endpoint,
            authorization,
        })
    }

    /// Execute one inference request, with no automatic retries.
    pub async fn run(&self, request: &Request) -> Result<Response, Error> {
        if request.fields.is_empty() {
            return Err(Error::InvalidRequest("at least one field is required"));
        }
        if request.state.is_empty() && request.images.is_empty() {
            return Err(Error::InvalidRequest("state or images are required"));
        }
        let response = self
            .http
            .post(self.endpoint.clone())
            .header(AUTHORIZATION, self.authorization.clone())
            .json(request)
            .send()
            .await
            .map_err(Error::Transport)?;
        let status = response.status();
        tracing::debug!(status = status.as_u16(), "Clairic HTTP response received");
        let bytes = response.bytes().await.map_err(Error::Transport)?;
        // HTTP failures may contain HTML or an error-only envelope, not model output.
        if !status.is_success() {
            #[derive(Deserialize)]
            struct Failure {
                #[serde(default)]
                errors: Vec<ApiError>,
            }
            let errors = serde_json::from_slice::<Failure>(&bytes)
                .map(|v| v.errors)
                .unwrap_or_default();
            return Err(Error::Api { status, errors });
        }
        let envelope: Envelope = serde_json::from_slice(&bytes).map_err(Error::Decode)?;
        if !envelope.success {
            return Err(Error::Api {
                status,
                errors: envelope.errors,
            });
        }
        envelope.result.ok_or(Error::MissingResult)
    }
}
