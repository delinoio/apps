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
async fn policy(State(s): State<Arc<Server>>, req: Request, next: Next) -> Response {
    let trace = crate::id();
    let start = Instant::now();
    let path = req.uri().path().to_owned();
    let subscription = path.ends_with("/WatchChanges");
    let api = path.starts_with("/lane.v1.") || path == "/mcp";
    let allowed = req
        .headers()
        .get("origin")
        .is_none_or(|v| v == s.config.web_origin.as_str());
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
        let outcome = if status == StatusCode::GATEWAY_TIMEOUT {
            crate::metrics::Outcome::Timeout
        } else if status.is_server_error() {
            crate::metrics::Outcome::ServerError
        } else if status.is_client_error() {
            crate::metrics::Outcome::ClientError
        } else {
            crate::metrics::Outcome::Success
        };
        s.metrics
            .observe(outcome, start.elapsed().as_secs_f64(), subscription);
        tracing::info!(request_id=%trace,status=status.as_u16(),duration_ms=start.elapsed().as_millis() as u64,"Lane request completed");
    }
    let h = response.headers_mut();
    h.entry("x-request-id").or_insert(trace.parse().unwrap());
    h.insert("cache-control", HeaderValue::from_static("no-store"));
    if allowed {
        h.insert(
            "access-control-allow-origin",
            s.config.web_origin.parse().unwrap(),
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
