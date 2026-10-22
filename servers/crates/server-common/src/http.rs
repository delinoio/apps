// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::telemetry::{Metrics, Service};
use axum::{
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tracing::Instrument;

/// Maps a request path to a bounded procedure label. It must return `Some` only for an exact
/// allowlisted path so that no request-controlled text ever becomes a metric label.
pub type ProcedureResolver = fn(&str) -> Option<&'static str>;

#[derive(Clone)]
pub struct Policy {
    pub origin: String,
    pub timeout: Duration,
    pub service: Service,
    pub metrics: Arc<Metrics>,
    /// Resolver supplied by the service adapter that owns the path allowlist. `None` disables
    /// per-procedure accounting of outer timeouts (Git Tracker counts one outcome per response).
    pub procedure: Option<ProcedureResolver>,
}

/// Exactly-once marker for the request metrics of a known DevBase procedure. The middleware
/// stores one in the request extensions; the first party that calls [`Accounting::claim`] (the
/// service handler when it finishes, or the outer timeout) records the request, so a handler
/// that finishes just before the outer deadline is not counted a second time as a timeout.
#[derive(Clone, Default)]
pub struct Accounting(Arc<AtomicBool>);
impl Accounting {
    /// Returns `true` for the first caller only.
    pub fn claim(&self) -> bool {
        !self.0.swap(true, Ordering::AcqRel)
    }
}

pub async fn middleware(
    State(policy): State<Policy>,
    mut request: Request,
    next: Next,
) -> Response {
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
        // Capture the procedure and start time before the request is moved so that the elapsed
        // time covers body reading as well as handler execution.
        let procedure = policy
            .procedure
            .and_then(|resolve| resolve(request.uri().path()));
        let accounting = Accounting::default();
        if procedure.is_some() {
            request.extensions_mut().insert(accounting.clone());
        }
        let started = Instant::now();
        match tokio::time::timeout(policy.timeout, next.run(request).instrument(span)).await {
            Ok(response) => {
                if is_untyped_client_deadline(policy.service, &response) {
                    record_timeout(&policy, procedure, started);
                    timeout_response(policy.service)
                } else {
                    response
                }
            }
            Err(_) => {
                // A handler that already recorded its own outcome owns the request's metrics.
                if accounting.claim() {
                    record_timeout(&policy, procedure, started);
                }
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

/// Records one request and one latency sample for an outer timeout of a known DevBase procedure.
/// A cancelled handler never reaches its own accounting; callers claim [`Accounting`] first so a
/// handler that finished earlier is not counted twice.
pub fn record_timeout(policy: &Policy, procedure: Option<&'static str>, started: Instant) {
    let (Service::DevBase, Some(procedure)) = (policy.service, procedure) else {
        return;
    };
    policy
        .metrics
        .requests
        .with_label_values(&[procedure, "deadline_exceeded"])
        .inc();
    policy
        .metrics
        .duration
        .with_label_values(&[procedure])
        .observe(started.elapsed().as_secs_f64());
}

/// Detects the bare `deadline_exceeded` response that the Connect transport synthesizes when a
/// client's `Connect-Timeout-Ms` expires first. The transport cancels the handler at that point,
/// so the DevBase error conversion never runs and the response lacks the typed detail and trace
/// header. Handler-produced errors always carry the trace header and the handler never reports
/// `deadline_exceeded`, so a 504 without the header can only come from that transport path.
///
/// Adapting the response here is deterministic whichever timer fires first. A second, slightly
/// earlier timer would race the transport's own timer inside one runtime clock tick. Git Tracker
/// is unaffected: it installs no client-deadline handling of this kind.
fn is_untyped_client_deadline(service: Service, response: &Response) -> bool {
    matches!(service, Service::DevBase)
        && response.status() == StatusCode::GATEWAY_TIMEOUT
        && !response.headers().contains_key("x-devbase-trace-id")
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

    #[tokio::test]
    async fn timeouts_include_correlation_for_handlers_and_slow_bodies() {
        for service in [Service::DevBase, Service::GitTracker] {
            let metrics = Arc::new(Metrics::new(service));
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
                        metrics: metrics.clone(),
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
            if matches!(service, Service::GitTracker) {
                // Each of the two timed-out requests adds exactly one outcome increment.
                assert_eq!(
                    metrics.requests.with_label_values(&["server_error"]).get(),
                    2
                );
            } else {
                // Without a procedure resolver nothing is attributed to a procedure.
                assert!(!metrics.encode().contains("devbase_requests_total{"));
            }
            task.abort();
        }
    }

    #[test]
    fn accounting_is_claimed_once_across_clones() {
        let accounting = Accounting::default();
        let other = accounting.clone();
        assert!(accounting.claim());
        assert!(!other.claim());
        assert!(!accounting.claim());
    }

    fn policy(service: Service, procedure: Option<ProcedureResolver>) -> Policy {
        Policy {
            origin: String::new(),
            timeout: Duration::from_secs(1),
            service,
            metrics: Arc::new(Metrics::new(service)),
            procedure,
        }
    }

    #[test]
    fn record_timeout_counts_only_known_devbase_procedures() {
        let known = policy(Service::DevBase, Some(|_| Some("GetProject")));
        let started = Instant::now() - Duration::from_millis(50);
        record_timeout(&known, Some("GetProject"), started);
        let requests = known
            .metrics
            .requests
            .with_label_values(&["GetProject", "deadline_exceeded"]);
        assert_eq!(requests.get(), 1);
        let duration = known.metrics.duration.with_label_values(&["GetProject"]);
        assert_eq!(duration.get_sample_count(), 1);
        assert!(duration.get_sample_sum() >= 0.05);

        let unresolved = policy(Service::DevBase, Some(|_| None));
        record_timeout(&unresolved, None, started);
        assert!(
            !unresolved
                .metrics
                .encode()
                .contains("devbase_requests_total{")
        );

        // Git Tracker keeps its single per-response outcome increment.
        let tracker = policy(Service::GitTracker, Some(|_| Some("GetProject")));
        record_timeout(&tracker, Some("GetProject"), started);
        assert!(
            !tracker
                .metrics
                .encode()
                .contains("devbase_request_duration_seconds")
        );
        assert_eq!(
            tracker
                .metrics
                .requests
                .with_label_values(&["server_error"])
                .get(),
            0
        );
    }

    #[tokio::test]
    async fn untyped_client_deadline_responses_are_adapted_for_devbase_only() {
        // `/bare` mimics the response the Connect transport produces when a client deadline
        // expires first; `/typed` is a 504 that already carries the DevBase trace header.
        let app = |service: Service| {
            Router::new()
                .route(
                    "/bare",
                    post(|| async {
                        (
                            StatusCode::GATEWAY_TIMEOUT,
                            [("Content-Type", "application/json")],
                            r#"{"code":"deadline_exceeded","message":"request timeout"}"#,
                        )
                    }),
                )
                .route(
                    "/typed",
                    post(|| async {
                        (
                            StatusCode::GATEWAY_TIMEOUT,
                            [("X-Devbase-Trace-Id", "existing-trace")],
                            "typed",
                        )
                    }),
                )
                .layer(axum::middleware::from_fn_with_state(
                    policy(service, Some(|_| Some("GetProject"))),
                    middleware,
                ))
        };
        let send = |app: Router, path: &'static str| async move {
            let response = tower::ServiceExt::oneshot(
                app,
                Request::post(path).body(axum::body::Body::empty()).unwrap(),
            )
            .await
            .unwrap();
            let (parts, body) = response.into_parts();
            let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
            (parts, body)
        };

        // DevBase: the bare response becomes the typed timeout and is counted once.
        let app_devbase = app(Service::DevBase);
        let (parts, body) = send(app_devbase.clone(), "/bare").await;
        assert_eq!(parts.status, StatusCode::GATEWAY_TIMEOUT);
        let trace = parts.headers["x-devbase-trace-id"].to_str().unwrap();
        assert!(uuid::Uuid::parse_str(trace).is_ok());
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "deadline_exceeded");
        let raw = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(body["details"][0]["value"].as_str().unwrap())
            .unwrap();
        let detail = delino_protocol::devbase::v1::ErrorDetail::decode_from_slice(&raw).unwrap();
        assert_eq!(detail.trace_id, trace);

        // DevBase: a 504 that already carries the trace header is passed through unchanged.
        let (parts, body) = send(app_devbase, "/typed").await;
        assert_eq!(parts.headers["x-devbase-trace-id"], "existing-trace");
        assert_eq!(&body[..], b"typed");

        // Git Tracker: the same bare response is never rewritten.
        let (parts, body) = send(app(Service::GitTracker), "/bare").await;
        assert_eq!(parts.status, StatusCode::GATEWAY_TIMEOUT);
        assert!(!parts.headers.contains_key("x-devbase-trace-id"));
        assert!(!parts.headers.contains_key("x-correlation-id"));
        assert!(String::from_utf8_lossy(&body).contains("request timeout"));
    }
}
