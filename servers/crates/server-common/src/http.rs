// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::telemetry::{Metrics, Service};
use axum::{
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    sync::{Arc, Mutex, PoisonError},
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

/// Pending observation of a known DevBase procedure, shared between the service handler and
/// this middleware. The middleware stores one in the request extensions. A handler that finishes
/// does not record metrics itself; it publishes its outcome with [`Accounting::report`] and the
/// middleware records exactly one observation for the response that is actually returned:
///
/// - a whole-request timeout, or any 504 response (the Connect transport's expired client
///   `Connect-Timeout-Ms` deadline), is recorded as `deadline_exceeded` with the latency since
///   the request headers arrived, even when the handler had already reported an earlier outcome;
/// - any other response records the handler's reported outcome and latency, and a response the
///   handler never reported (for example a transport rejection before the handler) records
///   nothing, as before.
///
/// A request abandoned by the client after the handler finished still records the handler's
/// outcome. Only the middleware records, so no combination of timers can count a request twice.
#[derive(Clone, Default)]
pub struct Accounting(Arc<Mutex<Option<Reported>>>);
#[derive(Clone, Copy)]
struct Reported {
    code: &'static str,
    elapsed: Duration,
}
impl Accounting {
    /// Publishes the handler's outcome (the Connect code or `ok`) and its own latency. A later
    /// call replaces an earlier one; handlers report once.
    pub fn report(&self, code: &'static str, elapsed: Duration) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(Reported { code, elapsed });
    }
    fn take(&self) -> Option<Reported> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// Records the single observation of one request. Settling happens at most once, either
/// explicitly with the response outcome or, when the request future is dropped first, from
/// `Drop` with the handler's reported outcome.
struct Settlement<'a> {
    policy: &'a Policy,
    procedure: Option<&'static str>,
    accounting: Accounting,
    started: Instant,
    settled: bool,
}
impl Settlement<'_> {
    fn settle(&mut self, deadline_exceeded: bool) {
        if std::mem::replace(&mut self.settled, true) {
            return;
        }
        let reported = self.accounting.take();
        if deadline_exceeded {
            record_timeout(self.policy, self.procedure, self.started);
        } else if let Some(reported) = reported {
            record_outcome(self.policy, self.procedure, reported.code, reported.elapsed);
        }
    }
}
impl Drop for Settlement<'_> {
    fn drop(&mut self) {
        // The request future was cancelled (for example by a client disconnect) before any
        // response was produced: keep an outcome the handler already finished.
        self.settle(false);
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
        let mut settlement = Settlement {
            policy: &policy,
            procedure,
            accounting,
            started: Instant::now(),
            settled: false,
        };
        match tokio::time::timeout(policy.timeout, next.run(request).instrument(span)).await {
            Ok(response) => {
                // A 504 reaching the client is a whole-request timeout whatever the handler
                // reported before the deadline passed. The same settlement records the adapted
                // client deadline below, so it can never count a request a second time.
                settlement.settle(response.status() == StatusCode::GATEWAY_TIMEOUT);
                if is_untyped_client_deadline(policy.service, &response) {
                    timeout_response(policy.service)
                } else {
                    response
                }
            }
            Err(_) => {
                settlement.settle(true);
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

/// Records one request and one latency sample for a whole-request timeout of a known DevBase
/// procedure. Only [`Settlement`] calls it, once per request, after any handler outcome has been
/// superseded by the timeout response.
pub fn record_timeout(policy: &Policy, procedure: Option<&'static str>, started: Instant) {
    record_outcome(policy, procedure, "deadline_exceeded", started.elapsed());
}

/// Records one request counter increment and one latency sample for a known DevBase procedure.
/// Anything else (unknown paths, Git Tracker) creates no procedure series.
fn record_outcome(policy: &Policy, procedure: Option<&'static str>, code: &str, elapsed: Duration) {
    let (Service::DevBase, Some(procedure)) = (policy.service, procedure) else {
        return;
    };
    policy
        .metrics
        .requests
        .with_label_values(&[procedure, code])
        .inc();
    policy
        .metrics
        .duration
        .with_label_values(&[procedure])
        .observe(elapsed.as_secs_f64());
}

/// Detects the bare `deadline_exceeded` response that the Connect transport synthesizes when a
/// client's `Connect-Timeout-Ms` expires first. The transport cancels the handler at that point,
/// so the service error conversion never runs and the response lacks the service's correlation
/// header (`X-Devbase-Trace-Id` for DevBase, `X-Correlation-Id` for Git Tracker) and, for DevBase,
/// the typed detail. Handler-produced errors always carry that header and the handler never
/// reports `deadline_exceeded`, so a 504 without the header can only come from that transport
/// path. A 504 that already carries the header is passed through unchanged.
///
/// Adapting the response here is deterministic whichever timer fires first. A second, slightly
/// earlier timer would race the transport's own timer inside one runtime clock tick.
fn is_untyped_client_deadline(service: Service, response: &Response) -> bool {
    let header = match service {
        Service::DevBase => "x-devbase-trace-id",
        Service::GitTracker => "x-correlation-id",
    };
    response.status() == StatusCode::GATEWAY_TIMEOUT && !response.headers().contains_key(header)
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
    fn accounting_reports_are_shared_across_clones_and_taken_once() {
        let accounting = Accounting::default();
        let other = accounting.clone();
        assert!(accounting.take().is_none());
        other.report("ok", Duration::from_millis(5));
        accounting.report("unavailable", Duration::from_millis(7));
        let reported = other.take().unwrap();
        assert_eq!(reported.code, "unavailable");
        assert_eq!(reported.elapsed, Duration::from_millis(7));
        assert!(accounting.take().is_none());
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

    /// Routes `/rpc/<scenario>` through the middleware. Each handler publishes an outcome as the
    /// DevBase handler does, then behaves as the scenario describes.
    fn accounting_app(timeout: Duration, metrics: Arc<Metrics>) -> Router {
        use axum::extract::Extension;
        let report = |accounting: Accounting| accounting.report("ok", Duration::from_millis(3));
        Router::new()
            .route(
                "/rpc/ok",
                post(
                    move |Extension(accounting): Extension<Accounting>| async move {
                        report(accounting);
                        StatusCode::OK
                    },
                ),
            )
            .route(
                "/rpc/slow-after-report",
                post(
                    move |Extension(accounting): Extension<Accounting>| async move {
                        report(accounting);
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        StatusCode::OK
                    },
                ),
            )
            .route(
                "/rpc/bare-504-after-report",
                post(
                    move |Extension(accounting): Extension<Accounting>| async move {
                        report(accounting);
                        StatusCode::GATEWAY_TIMEOUT
                    },
                ),
            )
            .route(
                "/rpc/bare-504",
                post(|| async { StatusCode::GATEWAY_TIMEOUT }),
            )
            .route(
                "/rpc/unreported",
                post(|| async { StatusCode::BAD_REQUEST }),
            )
            .route(
                "/other/bare-504",
                post(|| async { StatusCode::GATEWAY_TIMEOUT }),
            )
            .layer(axum::middleware::from_fn_with_state(
                Policy {
                    origin: String::new(),
                    timeout,
                    service: Service::DevBase,
                    metrics,
                    procedure: Some(|path| path.starts_with("/rpc/").then_some("GetProject")),
                },
                middleware,
            ))
    }

    async fn status_of(app: &Router, path: &str) -> StatusCode {
        tower::ServiceExt::oneshot(
            app.clone(),
            Request::post(path).body(axum::body::Body::empty()).unwrap(),
        )
        .await
        .unwrap()
        .status()
    }

    fn observed(metrics: &Metrics, code: &str) -> u64 {
        metrics
            .requests
            .with_label_values(&["GetProject", code])
            .get()
    }

    #[tokio::test]
    async fn the_returned_outcome_is_recorded_exactly_once() {
        let metrics = Arc::new(Metrics::new(Service::DevBase));
        let app = accounting_app(Duration::from_millis(100), metrics.clone());
        let samples = || metrics.duration.with_label_values(&["GetProject"]);

        // A reported handler outcome is recorded with the handler's own latency.
        assert_eq!(status_of(&app, "/rpc/ok").await, StatusCode::OK);
        assert_eq!(observed(&metrics, "ok"), 1);
        assert_eq!(samples().get_sample_count(), 1);
        assert!((samples().get_sample_sum() - 0.003).abs() < 1e-9);

        // The handler reported, but the outer deadline wins while the response is still pending:
        // the 504 the client receives is the one recorded, with the whole-request latency.
        let before = samples().get_sample_sum();
        assert_eq!(
            status_of(&app, "/rpc/slow-after-report").await,
            StatusCode::GATEWAY_TIMEOUT
        );
        assert_eq!(observed(&metrics, "deadline_exceeded"), 1);
        assert_eq!(observed(&metrics, "ok"), 1);
        assert_eq!(samples().get_sample_count(), 2);
        assert!(samples().get_sample_sum() - before >= 0.09);

        // A 504 produced inside the deadline (the transport's expired client deadline) is a
        // timeout whether or not the handler reported an earlier outcome, and counts once.
        for path in ["/rpc/bare-504-after-report", "/rpc/bare-504"] {
            assert_eq!(status_of(&app, path).await, StatusCode::GATEWAY_TIMEOUT);
        }
        assert_eq!(observed(&metrics, "deadline_exceeded"), 3);
        assert_eq!(observed(&metrics, "ok"), 1);
        assert_eq!(samples().get_sample_count(), 4);

        // A response the handler never reported and that is no timeout records nothing, and a
        // 504 on a path outside the procedure allowlist creates no series.
        assert_eq!(
            status_of(&app, "/rpc/unreported").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(&app, "/other/bare-504").await,
            StatusCode::GATEWAY_TIMEOUT
        );
        assert_eq!(samples().get_sample_count(), 4);
        assert!(!metrics.encode().contains("\"/other"));
    }

    #[tokio::test]
    async fn an_adapted_client_deadline_reuses_the_single_settlement() {
        // The handler already reported `ok`, then the transport's client deadline produced a bare
        // 504 without the DevBase trace header. The HTTP boundary replaces it with the typed
        // timeout, and the one recorded observation is that timeout, not a second sample.
        let metrics = Arc::new(Metrics::new(Service::DevBase));
        let app = accounting_app(Duration::from_secs(5), metrics.clone());
        let response = tower::ServiceExt::oneshot(
            app,
            Request::post("/rpc/bare-504-after-report")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        let trace = response.headers()["x-devbase-trace-id"].to_str().unwrap();
        assert!(uuid::Uuid::parse_str(trace).is_ok());
        assert_eq!(observed(&metrics, "deadline_exceeded"), 1);
        assert_eq!(observed(&metrics, "ok"), 0);
        assert_eq!(
            metrics
                .duration
                .with_label_values(&["GetProject"])
                .get_sample_count(),
            1
        );
    }

    #[test]
    fn an_abandoned_request_keeps_the_finished_handler_outcome() {
        let policy = policy(Service::DevBase, Some(|_| Some("GetProject")));
        let accounting = Accounting::default();
        {
            let _abandoned = Settlement {
                policy: &policy,
                procedure: Some("GetProject"),
                accounting: accounting.clone(),
                started: Instant::now(),
                settled: false,
            };
            accounting.report("not_found", Duration::from_millis(4));
        }
        let requests = |code| {
            policy
                .metrics
                .requests
                .with_label_values(&["GetProject", code])
        };
        assert_eq!(requests("not_found").get(), 1);
        assert_eq!(requests("deadline_exceeded").get(), 0);

        // Cancelled before the handler reported anything: nothing to record.
        drop(Settlement {
            policy: &policy,
            procedure: Some("GetProject"),
            accounting: Accounting::default(),
            started: Instant::now(),
            settled: false,
        });
        assert_eq!(requests("not_found").get(), 1);
        assert_eq!(
            policy
                .metrics
                .duration
                .with_label_values(&["GetProject"])
                .get_sample_count(),
            1
        );
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
    async fn untyped_client_deadline_responses_are_adapted_for_both_services() {
        // `/bare` mimics the response the Connect transport produces when a client deadline
        // expires first; `/typed` is a 504 that already carries the service's correlation header.
        let app = |policy: Policy| {
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
                            [
                                ("X-Devbase-Trace-Id", "existing-trace"),
                                ("X-Correlation-Id", "existing-trace"),
                            ],
                            "typed",
                        )
                    }),
                )
                .layer(axum::middleware::from_fn_with_state(policy, middleware))
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
        let app_devbase = app(policy(Service::DevBase, Some(|_| Some("GetProject"))));
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

        // Git Tracker: the same bare response gets a server-generated correlation ID, one
        // matching safe warning, no DevBase detail or header, `no-store` and one metric increment.
        let logs = capture_logs();
        let metrics = Arc::new(Metrics::new(Service::GitTracker));
        let git_tracker = app(Policy {
            origin: "https://console.example".into(),
            metrics: metrics.clone(),
            ..policy(Service::GitTracker, None)
        });
        let response = tower::ServiceExt::oneshot(
            git_tracker.clone(),
            Request::post("/bare")
                .header("Origin", "https://console.example")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        let (parts, body) = response.into_parts();
        let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        assert_eq!(parts.status, StatusCode::GATEWAY_TIMEOUT);
        let trace = parts.headers["x-correlation-id"].to_str().unwrap();
        assert!(uuid::Uuid::parse_str(trace).is_ok());
        assert!(!parts.headers.contains_key("x-devbase-trace-id"));
        assert_eq!(parts.headers["cache-control"], "no-store");
        assert_eq!(
            parts.headers["access-control-expose-headers"],
            "X-Correlation-Id"
        );
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "deadline_exceeded");
        assert_eq!(body["message"], "request deadline exceeded");
        assert!(body.get("details").is_none());
        assert_eq!(
            metrics.requests.with_label_values(&["server_error"]).get(),
            1
        );
        let logs = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        assert!(logs.contains("request deadline exceeded"), "{logs}");
        assert!(logs.contains(trace), "{logs}");
        assert_eq!(
            logs.matches("request deadline exceeded").count(),
            1,
            "{logs}"
        );

        // Git Tracker: a 504 that already carries its correlation ID is passed through unchanged
        // and still counts once.
        let (parts, body) = send(git_tracker, "/typed").await;
        assert_eq!(parts.headers["x-correlation-id"], "existing-trace");
        assert_eq!(&body[..], b"typed");
        assert_eq!(
            metrics.requests.with_label_values(&["server_error"]).get(),
            2
        );
    }

    /// Installs a thread-local subscriber that records formatted log lines. The test runtime is
    /// single-threaded, so the guard also covers the in-process request futures.
    fn capture_logs() -> Arc<Mutex<Vec<u8>>> {
        #[derive(Clone)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let logs = Arc::new(Mutex::new(Vec::new()));
        let sink = Sink(logs.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .finish();
        // Leak the guard for the rest of this test thread; each test owns its thread.
        std::mem::forget(tracing::subscriber::set_default(subscriber));
        logs
    }
}
