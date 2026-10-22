// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::telemetry::{Metrics, Service};
use axum::{
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::Instrument;
#[derive(Clone)]
pub struct Policy {
    pub origin: String,
    pub timeout: Duration,
    pub service: Service,
    pub metrics: Arc<Metrics>,
    /// Maps a request path to a bounded procedure label. The owning service supplies the
    /// allowlist so no raw path or identifier can become a metric label. `None` disables
    /// procedure-scoped timeout accounting, and unknown paths must resolve to `None`.
    pub procedure: Option<fn(&str) -> Option<&'static str>>,
}
impl Policy {
    /// Returns the bounded procedure label for a request path, if the service has one.
    pub fn procedure_for(&self, path: &str) -> Option<&'static str> {
        self.procedure.and_then(|resolve| resolve(path))
    }
    /// Records one `deadline_exceeded` request and one latency sample for a procedure whose
    /// own accounting was cancelled by a whole-request deadline. Call it once per completed
    /// response that the middleware synthesizes, with the instant taken before the request
    /// body was read. It does nothing without a procedure label.
    pub fn record_deadline_exceeded(&self, procedure: Option<&'static str>, started: Instant) {
        let Some(procedure) = procedure else {
            return;
        };
        self.metrics
            .requests
            .with_label_values(&[procedure, connectrpc::ErrorCode::DeadlineExceeded.as_str()])
            .inc();
        self.metrics
            .duration
            .with_label_values(&[procedure])
            .observe(started.elapsed().as_secs_f64());
    }
}
pub async fn middleware(State(policy): State<Policy>, request: Request, next: Next) -> Response {
    let origin = request.headers().get("origin").cloned();
    if let Some(origin) = &origin
        && (policy.origin.is_empty() || origin.as_bytes() != policy.origin.as_bytes())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let preflight = request.method() == Method::OPTIONS && origin.is_some();
    let mut response = if preflight {
        StatusCode::NO_CONTENT.into_response()
    } else {
        let span =
            tracing::info_span!("api.request",method=%request.method(),path=%request.uri().path());
        let procedure = policy.procedure_for(request.uri().path());
        let started = Instant::now();
        match tokio::time::timeout(policy.timeout, next.run(request).instrument(span)).await {
            Ok(response) => response,
            Err(_) => {
                policy.record_deadline_exceeded(procedure, started);
                timeout_response(policy.service)
            }
        }
    };
    if let Some(origin) = origin {
        let h = response.headers_mut();
        h.insert("Access-Control-Allow-Origin", origin);
        h.append("Vary", HeaderValue::from_static("Origin"));
        h.insert(
            "Access-Control-Allow-Methods",
            HeaderValue::from_static("POST, OPTIONS"),
        );
        h.insert(
            "Access-Control-Allow-Headers",
            HeaderValue::from_static(
                "Authorization, Content-Type, Connect-Protocol-Version, Connect-Timeout-Ms",
            ),
        );
        h.insert("Access-Control-Max-Age", HeaderValue::from_static("600"));
        h.insert(
            "Access-Control-Expose-Headers",
            HeaderValue::from_static(match policy.service {
                Service::DevBase => "X-Devbase-Trace-Id, Retry-After",
                Service::GitTracker => "X-Correlation-Id",
            }),
        );
    }
    if matches!(policy.service, Service::GitTracker) {
        response
            .headers_mut()
            .insert("Cache-Control", HeaderValue::from_static("no-store"));
        let code = response.status().as_u16();
        policy
            .metrics
            .requests
            .with_label_values(&[if code >= 500 {
                "server_error"
            } else if code >= 400 {
                "client_error"
            } else {
                "success"
            }])
            .inc();
    }
    response
}

fn timeout_response(service: Service) -> Response {
    let trace = crate::id();
    tracing::warn!(correlation_id = %trace, category = "timeout", "request deadline exceeded");
    let mut error = connectrpc::ConnectError::new(
        connectrpc::ErrorCode::DeadlineExceeded,
        "request deadline exceeded",
    );
    let header = match service {
        Service::DevBase => {
            let detail = delino_protocol::devbase::v1::ErrorDetail {
                trace_id: trace.clone(),
                reason:
                    delino_protocol::devbase::v1::ErrorReason::ERROR_REASON_DEPENDENCY_UNAVAILABLE
                        .into(),
                ..Default::default()
            };
            error = error.with_detail(connectrpc::ErrorDetail::from_message(
                "devbase.v1.ErrorDetail",
                &detail,
            ));
            "X-Devbase-Trace-Id"
        }
        Service::GitTracker => "X-Correlation-Id",
    };
    (
        error.http_status(),
        [
            ("Content-Type", "application/json"),
            (header, trace.as_str()),
        ],
        error.to_json(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Bytes, routing::post};
    use base64::Engine;
    use buffa::Message;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn resolve(path: &str) -> Option<&'static str> {
        (path == "/rpc").then_some("Rpc")
    }

    fn policy(procedure: Option<fn(&str) -> Option<&'static str>>) -> Policy {
        Policy {
            origin: String::new(),
            timeout: Duration::from_secs(1),
            service: Service::DevBase,
            metrics: Arc::new(Metrics::new(Service::DevBase)),
            procedure,
        }
    }

    #[test]
    fn deadline_accounting_records_one_labelled_observation() {
        let policy = policy(Some(resolve));
        assert_eq!(policy.procedure_for("/rpc"), Some("Rpc"));
        assert_eq!(policy.procedure_for("/other"), None);

        policy.record_deadline_exceeded(None, Instant::now());
        assert!(!policy.metrics.encode().contains("Rpc"));

        policy.record_deadline_exceeded(Some("Rpc"), Instant::now());
        let requests = policy
            .metrics
            .requests
            .with_label_values(&["Rpc", "deadline_exceeded"]);
        assert_eq!(requests.get(), 1);
        assert_eq!(
            policy
                .metrics
                .duration
                .with_label_values(&["Rpc"])
                .get_sample_count(),
            1
        );
    }

    #[test]
    fn requests_without_a_resolver_have_no_procedure() {
        assert_eq!(policy(None).procedure_for("/rpc"), None);
    }

    #[tokio::test]
    async fn timeouts_include_correlation_for_handlers_and_slow_bodies() {
        for service in [Service::DevBase, Service::GitTracker] {
            let app = Router::new()
                .route(
                    "/rpc",
                    post(|_: Bytes| async {
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }),
                )
                .layer(axum::middleware::from_fn_with_state(
                    Policy {
                        origin: "https://console.example".into(),
                        timeout: Duration::from_millis(20),
                        service,
                        metrics: Arc::new(Metrics::new(service)),
                        procedure: None,
                    },
                    middleware,
                ));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            for slow_body in [false, true] {
                let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
                let length = if slow_body { 1 } else { 0 };
                socket.write_all(format!("POST /rpc HTTP/1.1\r\nHost: localhost\r\nOrigin: https://console.example\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
                let mut raw = String::new();
                tokio::time::timeout(Duration::from_secs(2), socket.read_to_string(&mut raw))
                    .await
                    .unwrap()
                    .unwrap();
                let (headers, body) = raw.split_once("\r\n\r\n").unwrap();
                assert!(headers.starts_with("HTTP/1.1 504"), "{headers}");
                let name = match service {
                    Service::DevBase => "x-devbase-trace-id: ",
                    Service::GitTracker => "x-correlation-id: ",
                };
                let trace = headers
                    .lines()
                    .find_map(|line| line.strip_prefix(name))
                    .unwrap();
                assert!(uuid::Uuid::parse_str(trace).is_ok());
                let body: serde_json::Value = serde_json::from_str(body).unwrap();
                assert_eq!(body["code"], "deadline_exceeded");
                if matches!(service, Service::DevBase) {
                    let raw = base64::engine::general_purpose::STANDARD_NO_PAD
                        .decode(body["details"][0]["value"].as_str().unwrap())
                        .unwrap();
                    let detail =
                        delino_protocol::devbase::v1::ErrorDetail::decode_from_slice(&raw).unwrap();
                    assert_eq!(detail.trace_id, trace);
                } else {
                    assert!(headers.contains("cache-control: no-store"));
                }
            }
            task.abort();
        }
    }
}
