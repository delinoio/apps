// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{Fault, Server, account_lock};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Path, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use delino_protocol::lane::v1::*;
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub fn router(server: Arc<Server>) -> Router {
    let max = server.max_request_bytes();
    let rpc = connectrpc::Router::new()
        .add_service::<_, LaneServiceRegisterMarker>(server.clone())
        .add_service::<_, PlanningServiceRegisterMarker>(server.clone())
        .add_service::<_, CaptureServiceRegisterMarker>(server.clone())
        .add_service::<_, SyncServiceRegisterMarker>(server.clone())
        .add_service::<_, AccountServiceRegisterMarker>(server.clone())
        .add_service::<_, WebhookServiceRegisterMarker>(server.clone())
        .into_axum_service()
        .with_limits(
            connectrpc::Limits::default()
                .with_max_message_size(max)
                .with_max_request_body_size(max),
        );
    Router::new()
        .route("/health/live", get(|| async { StatusCode::OK }))
        .route("/health/ready", get(ready))
        .route("/metrics", get(metrics))
        .route("/auth/push-config", get(push_config))
        .route("/webhooks/{receiver}", post(intake))
        .with_state(server.clone())
        .merge(crate::auth::router(server.auth.clone()))
        .merge(crate::mcp::router(server.clone()))
        .fallback_service(rpc)
        .layer(middleware::from_fn_with_state(server, policy))
}
async fn ready(State(s): State<Arc<Server>>) -> StatusCode {
    match tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::query("SELECT 1").execute(&s.db),
    )
    .await
    {
        Ok(Ok(_)) => StatusCode::OK,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}
async fn metrics(State(s): State<Arc<Server>>, headers: HeaderMap) -> Response {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let supplied = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    let mut expected =
        Hmac::<Sha256>::new_from_slice(s.config.metrics_token.as_bytes()).expect("HMAC key");
    expected.update(format!("Bearer {}", s.config.metrics_token).as_bytes());
    let expected = expected.finalize().into_bytes();
    let mut actual =
        Hmac::<Sha256>::new_from_slice(s.config.metrics_token.as_bytes()).expect("HMAC key");
    actual.update(supplied.as_bytes());
    if actual.verify_slice(&expected).is_err() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match s.push.metrics().await {
        Ok(push) => {
            let pending: Result<i64, _> =
                sqlx::query_scalar("SELECT count(*) FROM lane_sync_rejections")
                    .fetch_one(&s.db)
                    .await;
            match pending {
                Ok(n) => (
                    [("content-type", "text/plain; version=0.0.4")],
                    format!(
                        "{}{push}lane_sync_pending_failures {n}\n",
                        s.metrics.encode()
                    ),
                )
                    .into_response(),
                Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
            }
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
async fn push_config(State(s): State<Arc<Server>>, headers: HeaderMap) -> Response {
    let out = async {
        let p = s.auth.authenticate(&headers).await?;
        let mut tx = s.db.begin().await?;
        account_lock(&mut tx, &p.account_id).await?;
        s.auth.authorize(&mut tx, &p).await?;
        tx.commit().await?;
        Ok::<_, Fault>(json!({"public_key":s.push.public_key()}))
    }
    .await;
    match out {
        Ok(v) => Json(v).into_response(),
        Err(e) => fault(e),
    }
}
async fn intake(
    State(s): State<Arc<Server>>,
    Path(receiver): Path<String>,
    req: Request,
) -> Response {
    let headers = req.headers().clone();
    let timestamp = headers
        .get("lane-timestamp")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let signature = headers
        .get("lane-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let raw = match to_bytes(req.into_body(), s.config.max_webhook_bytes).await {
        Ok(b) => b,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let out = async {
        if !crate::canonical_id(&receiver) {
            return Err(Fault::invalid("invalid receiver ID"));
        }
        let mut conn = s.db.acquire().await?;
        let account = crate::webhooks::receiver_account(&mut conn, &receiver).await?;
        drop(conn);
        crate::rate::check(
            &s.db,
            "webhook",
            &receiver,
            s.config.webhook_requests_per_minute,
        )
        .await?;
        let mut tx = s.db.begin().await?;
        account_lock(&mut tx, &account).await?;
        s.auth.authorize_account(&mut tx, &account).await?;
        let out =
            crate::webhooks::intake(&mut tx, &receiver, &raw, timestamp, signature, &s.webhooks)
                .await?;
        tx.commit().await?;
        Ok::<_, Fault>(out)
    }
    .await;
    match out {
        Ok(v) => Json(v).into_response(),
        Err(e) => fault(e),
    }
}
fn fault(f: Fault) -> Response {
    use connectrpc::ErrorCode::*;
    let status = match f.code {
        Unauthenticated => StatusCode::UNAUTHORIZED,
        PermissionDenied => StatusCode::FORBIDDEN,
        NotFound => StatusCode::NOT_FOUND,
        ResourceExhausted => StatusCode::TOO_MANY_REQUESTS,
        Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        Aborted | AlreadyExists => StatusCode::CONFLICT,
        _ => StatusCode::BAD_REQUEST,
    };
    (
        status,
        Json(
            json!({"error":format!("{:?}",crate::error::reason(&f)),"correlation_id":crate::id()}),
        ),
    )
        .into_response()
}
/// Origin admission decision for one request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Admission {
    /// No Origin header or the configured frontend origin: existing behavior including CORS.
    Frontend,
    /// The API's own origin on the same-origin OAuth consent form POST. Admitted without
    /// granting any credentialed cross-origin response access.
    ApiConsent,
    /// Any other present Origin, including the opaque `null` Origin.
    Rejected,
}
const CONSENT_PATH: &str = "/oauth/consent";
fn admission(
    config: &crate::config::Config,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
) -> Admission {
    let Some(origin) = headers.get("origin") else {
        return Admission::Frontend;
    };
    if origin == config.web_origin.as_str() {
        Admission::Frontend
    } else if origin == config.public_url.as_str() && method == Method::POST && path == CONSENT_PATH
    {
        // The API-hosted consent page posts to itself. This exception is exact in origin, method
        // and path; all other routes keep the frontend-only Origin admission rule.
        Admission::ApiConsent
    } else {
        Admission::Rejected
    }
}
async fn policy(State(s): State<Arc<Server>>, req: Request, next: Next) -> Response {
    policy_with(&s.config, &s.metrics, req, next).await
}

async fn policy_with(
    config: &crate::config::Config,
    metrics: &crate::metrics::Metrics,
    req: Request,
    next: Next,
) -> Response {
    let trace = crate::id();
    let start = Instant::now();
    let path = req.uri().path().to_owned();
    let subscription = path.ends_with("/WatchChanges");
    let api = path.starts_with("/lane.v1.") || path == "/mcp";
    let admission = admission(config, req.method(), &path, req.headers());
    let allowed = admission != Admission::Rejected;
    if !allowed {
        tracing::warn!(request_id=%trace,method=%req.method(),path=%path,"Lane request rejected by Origin policy");
    }
    let preflight = req.method() == Method::OPTIONS;
    let mut response = if !allowed {
        StatusCode::FORBIDDEN.into_response()
    } else if preflight {
        StatusCode::NO_CONTENT.into_response()
    } else {
        match tokio::time::timeout(Duration::from_secs(60), next.run(req)).await {
            Ok(response) => response,
            Err(_) => StatusCode::GATEWAY_TIMEOUT.into_response(),
        }
    };
    if api && !preflight {
        let status = response.status();
        let outcome = response
            .extensions()
            .get::<crate::metrics::BackendOutcome>()
            .map_or_else(
                || crate::metrics::outcome_from_status(status),
                |typed| typed.0,
            );
        metrics.observe(outcome, start.elapsed().as_secs_f64(), subscription);
        tracing::info!(request_id=%trace,status=status.as_u16(),duration_ms=start.elapsed().as_millis() as u64,"Lane request completed");
    }
    let h = response.headers_mut();
    h.entry("x-request-id").or_insert(trace.parse().unwrap());
    h.insert("cache-control", HeaderValue::from_static("no-store"));
    // Credentialed CORS response access stays limited to the configured frontend.
    if admission == Admission::Frontend {
        h.insert(
            "access-control-allow-origin",
            config.web_origin.parse().unwrap(),
        );
        h.insert(
            "access-control-allow-credentials",
            HeaderValue::from_static("true"),
        );
        h.insert("vary", HeaderValue::from_static("Origin"));
        h.insert(
            "access-control-allow-methods",
            HeaderValue::from_static("GET, POST, DELETE, OPTIONS"),
        );
        h.insert("access-control-allow-headers",HeaderValue::from_static("authorization, content-type, connect-protocol-version, x-lane-csrf, mcp-protocol-version, mcp-session-id"));
        h.insert(
            "access-control-expose-headers",
            HeaderValue::from_static("x-request-id, www-authenticate"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::mcp::{Backend, router_with_backend};
    use axum::body::{Body, to_bytes};
    use connectrpc::ErrorCode;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    struct MetricBackend {
        auth_fault: Option<ErrorCode>,
        tool_fault: Option<ErrorCode>,
        pending: bool,
    }

    impl Backend for MetricBackend {
        fn max_request_bytes(&self) -> usize {
            1 << 20
        }
        async fn authenticate(&self, _: &HeaderMap) -> crate::Result<()> {
            self.auth_fault
                .map_or(Ok(()), |code| Err(Fault::new(code, 0, "fixture")))
        }
        async fn execute(
            &self,
            _: &HeaderMap,
            _: &'static str,
            _: &'static str,
            _: Value,
        ) -> crate::Result<Value> {
            if self.pending {
                std::future::pending::<()>().await;
            }
            match self.tool_fault {
                Some(code) => Err(Fault::new(code, 0, "fixture")),
                None => Ok(json!({"items":[], "nextCursor":""})),
            }
        }
        fn resource_metadata_url(&self) -> String {
            "https://lane.example/.well-known/oauth-protected-resource/mcp".into()
        }
    }

    fn metered(app: Router) -> (Router, Arc<crate::metrics::Metrics>) {
        let config = Arc::new(config(
            "https://lane.api.example.test",
            "https://lane.example.test",
        ));
        let metrics = Arc::new(crate::metrics::Metrics::default());
        let policy_metrics = metrics.clone();
        let app = app.layer(middleware::from_fn(move |req: Request, next: Next| {
            let config = config.clone();
            let metrics = policy_metrics.clone();
            async move { policy_with(&config, &metrics, req, next).await }
        }));
        (app, metrics)
    }

    fn tool_request() -> Request {
        Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header("authorization", "Bearer test")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2025-06-18")
            .body(Body::from(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"lane_list_repositories","arguments":{}}}).to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn mcp_backend_failures_keep_wire_status_and_count_once() {
        for (auth_fault, tool_fault, expected_status, expected_bucket, is_error) in [
            (
                None,
                Some(ErrorCode::Unavailable),
                StatusCode::OK,
                "server_error",
                true,
            ),
            (
                None,
                Some(ErrorCode::Internal),
                StatusCode::OK,
                "server_error",
                true,
            ),
            (
                None,
                Some(ErrorCode::InvalidArgument),
                StatusCode::OK,
                "client_error",
                true,
            ),
            (
                None,
                Some(ErrorCode::Aborted),
                StatusCode::OK,
                "client_error",
                true,
            ),
            (None, None, StatusCode::OK, "success", false),
            (
                Some(ErrorCode::Unavailable),
                None,
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                false,
            ),
            (
                Some(ErrorCode::Unauthenticated),
                None,
                StatusCode::UNAUTHORIZED,
                "client_error",
                false,
            ),
            (
                None,
                Some(ErrorCode::Unauthenticated),
                StatusCode::UNAUTHORIZED,
                "client_error",
                false,
            ),
        ] {
            let backend = Arc::new(MetricBackend {
                auth_fault,
                tool_fault,
                pending: false,
            });
            let (app, metrics) = metered(router_with_backend(backend));
            let response = app.oneshot(tool_request()).await.unwrap();
            assert_eq!(response.status(), expected_status);
            assert_eq!(
                response.headers().contains_key("www-authenticate"),
                expected_status == StatusCode::UNAUTHORIZED
            );
            let body = to_bytes(response.into_body(), 1 << 20).await.unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            if expected_status == StatusCode::OK {
                assert_eq!(body["result"]["isError"], is_error);
                if let Some(code) = tool_fault {
                    assert_eq!(
                        body["result"]["structuredContent"]["error"]["code"],
                        code.as_str()
                    );
                }
            }
            let encoded = metrics.encode();
            assert!(
                encoded.contains(&format!("outcome=\"{expected_bucket}\"}} 1\n")),
                "{encoded}"
            );
            assert!(encoded.contains("_count 1\n"), "{encoded}");
            assert_eq!(
                encoded
                    .lines()
                    .filter(
                        |line| line.starts_with("lane_api_requests_total") && line.ends_with(" 1")
                    )
                    .count(),
                1
            );
        }
    }

    #[tokio::test]
    async fn policy_uses_http_status_without_backend_outcome() {
        let app = Router::new().route(
            "/lane.v1.Test/Fallback",
            get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        );
        let (app, metrics) = metered(app);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/lane.v1.Test/Fallback")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(metrics.encode().contains("outcome=\"server_error\"} 1\n"));
        assert!(metrics.encode().contains("_count 1\n"));
    }

    #[tokio::test(start_paused = true)]
    async fn outer_timeout_records_once_and_cancels_mcp_dispatch() {
        let backend = Arc::new(MetricBackend {
            auth_fault: None,
            tool_fault: None,
            pending: true,
        });
        let (app, metrics) = metered(router_with_backend(backend));
        let response = app.oneshot(tool_request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        let encoded = metrics.encode();
        assert!(encoded.contains("outcome=\"timeout\"} 1\n"), "{encoded}");
        assert!(encoded.contains("_count 1\n"), "{encoded}");
        assert_eq!(
            encoded
                .lines()
                .filter(|line| line.starts_with("lane_api_requests_total") && line.ends_with(" 1"))
                .count(),
            1
        );
    }

    fn config(public_url: &str, web_origin: &str) -> Config {
        Config {
            listen: "127.0.0.1:0".into(),
            public_url: public_url.into(),
            web_origin: web_origin.into(),
            metrics_token: "synthetic-metrics-token-at-least-32-characters".into(),
            max_item_bytes: 1,
            max_webhook_bytes: 1,
            requests_per_minute: 1,
            webhook_requests_per_minute: 1,
        }
    }
    fn decide(config: &Config, method: Method, path: &str, origin: Option<&str>) -> Admission {
        let mut headers = HeaderMap::new();
        if let Some(origin) = origin {
            headers.insert("origin", HeaderValue::from_str(origin).unwrap());
        }
        admission(config, &method, path, &headers)
    }

    #[test]
    fn api_origin_is_admitted_only_for_the_consent_post() {
        let c = config("https://lane.api.example.test", "https://lane.example.test");
        let api = Some("https://lane.api.example.test");
        assert_eq!(
            decide(&c, Method::POST, "/oauth/consent", api),
            Admission::ApiConsent
        );
        for (method, path) in [
            (Method::GET, "/oauth/consent"),
            (Method::OPTIONS, "/oauth/consent"),
            (Method::POST, "/oauth/consent/"),
            (Method::POST, "/oauth/token"),
            (Method::POST, "/auth/refresh"),
            (Method::POST, "/mcp"),
            (Method::POST, "/lane.v1.LaneService/ListTasks"),
        ] {
            assert_eq!(decide(&c, method, path, api), Admission::Rejected, "{path}");
        }
        for origin in [
            "null",
            "https://attacker.example",
            "https://lane.api.example.test/",
        ] {
            assert_eq!(
                decide(&c, Method::POST, "/oauth/consent", Some(origin)),
                Admission::Rejected,
                "{origin}"
            );
        }
        // Existing frontend and absent-Origin admission is unchanged.
        for path in ["/oauth/consent", "/mcp", "/auth/refresh"] {
            assert_eq!(
                decide(&c, Method::POST, path, Some("https://lane.example.test")),
                Admission::Frontend
            );
            assert_eq!(decide(&c, Method::POST, path, None), Admission::Frontend);
        }
    }

    #[test]
    fn equal_configured_origins_keep_frontend_cors() {
        let c = config("https://lane.example.test", "https://lane.example.test");
        assert_eq!(
            decide(
                &c,
                Method::POST,
                "/oauth/consent",
                Some("https://lane.example.test")
            ),
            Admission::Frontend
        );
    }
}
