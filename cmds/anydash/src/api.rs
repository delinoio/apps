// SPDX-License-Identifier: Apache-2.0
//! AnyDash API client over the Connect protocol with binary Protobuf. Every request carries the
//! client kind and version headers; device tokens are never logged.
use crate::{
    connect,
    error::{Failure, Result},
    pb::ErrorReason,
    schedule,
};
use prost::Message;
use std::time::{Duration, Instant};
pub const CLIENT_KIND: &str = "CLIENT_KIND_CLI";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_RESPONSE: usize = 8 << 20;
#[derive(Clone, Debug)]
pub struct Api {
    base: String,
    http: reqwest::Client,
}
/// Plain HTTP is accepted only for loopback development and test servers.
#[must_use]
pub fn loopback(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| {
        matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
            || u.host_str().is_some_and(|h| h.ends_with(".localhost"))
    })
}
pub fn http_client(allow_http: bool) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(schedule::REQUEST_TIMEOUT)
        .https_only(!allow_http)
        .user_agent(concat!("anydash/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| Failure::internal("The HTTP client is unavailable."))
}
fn transport(message: &str) -> Failure {
    let mut failure = Failure::local(ErrorReason::DependencyUnavailable, message);
    failure.transport = true;
    failure
}
impl Api {
    pub fn new(base: &str) -> Result<Self> {
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
            http: http_client(loopback(base))?,
        })
    }
    /// Call `anydash.v1.<service>/<method>` with an optional device access token.
    pub async fn call<Req: Message, Resp: Message + Default>(
        &self,
        service: &str,
        method: &str,
        request: &Req,
        token: Option<&str>,
    ) -> Result<Resp> {
        let started = Instant::now();
        let mut builder = self
            .http
            .post(format!("{}/anydash.v1.{service}/{method}", self.base))
            .header("Content-Type", connect::CONTENT_TYPE)
            .header("Connect-Protocol-Version", "1")
            .header("X-AnyDash-Client-Kind", CLIENT_KIND)
            .header("X-AnyDash-Client-Version", VERSION)
            .body(request.encode_to_vec());
        if let Some(token) = token {
            builder = builder.bearer_auth(token);
        }
        let result = async {
            let mut response = builder
                .send()
                .await
                .map_err(|_| transport("The AnyDash API is unreachable."))?;
            let status = response.status().as_u16();
            let header = |name: &str| {
                response
                    .headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            };
            let correlation = header("x-anydash-correlation-id");
            let retry_after =
                header("retry-after").and_then(|v| schedule::retry_after(&v, crate::time::now()));
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| transport("The AnyDash API response was interrupted."))?
            {
                if body.len() + chunk.len() > MAX_RESPONSE {
                    return Err(transport("The AnyDash API response is too large."));
                }
                body.extend_from_slice(&chunk);
            }
            if status == 200 {
                return Resp::decode(body.as_slice())
                    .map_err(|_| transport("The AnyDash API response is unreadable."));
            }
            Err(connect::decode_error(
                status,
                &body,
                correlation.as_deref(),
                retry_after,
            ))
        }
        .await;
        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match &result {
            Ok(_) => tracing::debug!(
                event = "api_call",
                method,
                result = "ok",
                duration_ms = elapsed
            ),
            Err(e) => tracing::info!(
                event = "api_call",
                method,
                result = e.reason.as_str_name(),
                correlation_id = %e.correlation_id,
                duration_ms = elapsed
            ),
        }
        result
    }
}
/// Whether the failure should use the transient backoff.
#[must_use]
pub const fn transient(f: &Failure) -> bool {
    f.transport
        || matches!(
            f.reason,
            ErrorReason::DependencyUnavailable | ErrorReason::Internal
        )
}
#[must_use]
pub fn wait_for(f: &Failure, failures: u32) -> Duration {
    if f.reason == ErrorReason::RateLimited
        && let Some(retry) = f.retry_after
    {
        return retry;
    }
    schedule::backoff(failures, schedule::jitter())
}
