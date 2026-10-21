// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{Server, postgres, worker::Worker};
use axum::{
    Router,
    body::to_bytes,
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tracing::Instrument;

#[derive(Clone)]
struct Policy {
    timeout: Duration,
    limit: i64,
    window: Arc<Mutex<(Instant, i64)>>,
}
async fn bounded(State(policy): State<Policy>, request: Request, next: Next) -> Response {
    let allowed = {
        let mut state = policy.window.lock().unwrap_or_else(|e| e.into_inner());
        if state.0.elapsed() >= Duration::from_secs(60) {
            *state = (Instant::now(), 0)
        }
        state.1 = state.1.saturating_add(1);
        state.1 <= policy.limit
    };
    if !allowed {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    match tokio::time::timeout(
        policy.timeout,
        next.run(request)
            .instrument(tracing::info_span!("delcore.connect")),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            [("content-type", "application/json")],
            r#"{"code":"deadline_exceeded","message":"request deadline exceeded"}"#,
        )
            .into_response(),
    }
}
pub fn router(server: Arc<Server>, worker: Arc<Worker>) -> Router {
    let rpc = connectrpc::Router::new()
        .add_service(server.clone())
        .into_axum_service()
        .with_limits(
            connectrpc::Limits::default().with_max_message_size(server.config.max_payload_bytes),
        );
    let ready = server.db.clone();
    let metrics = server.metrics.clone();
    Router::new()
        .route("/healthz", get(|| async { StatusCode::OK }))
        .route(
            "/readyz",
            get(move || {
                let db = ready.clone();
                async move {
                    match tokio::time::timeout(Duration::from_secs(1), postgres::check_schema(&db))
                        .await
                    {
                        Ok(Ok(())) => StatusCode::OK,
                        _ => StatusCode::SERVICE_UNAVAILABLE,
                    }
                }
            }),
        )
        .route(
            "/metrics",
            get(move || {
                let metrics = metrics.clone();
                async move {
                    (
                        [("content-type", "text/plain; version=0.0.4")],
                        metrics.encode(),
                    )
                }
            }),
        )
        .route("/webhooks/paddle", post(webhook).with_state(worker))
        .fallback_service(rpc)
        .layer(middleware::from_fn_with_state(
            Policy {
                timeout: server.config.timeout(),
                limit: server.config.requests_per_minute,
                window: Arc::new(Mutex::new((Instant::now(), 0))),
            },
            bounded,
        ))
}
async fn webhook(State(worker): State<Arc<Worker>>, request: Request) -> StatusCode {
    let signature = request
        .headers()
        .get("Paddle-Signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let Ok(raw) = to_bytes(request.into_body(), worker.config.max_payload_bytes).await else {
        return StatusCode::PAYLOAD_TOO_LARGE;
    };
    worker.ingest_webhook(&signature, &raw).await
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn rate_bounds_and_slow_bodies_are_enforced() {
        let app = Router::new()
            .route("/", post(|_: Bytes| async { StatusCode::OK }))
            .layer(middleware::from_fn_with_state(
                Policy {
                    timeout: Duration::from_millis(30),
                    limit: 1,
                    window: Arc::new(Mutex::new((Instant::now(), 0))),
                },
                bounded,
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        for status in [504, 429] {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            stream.write_all(b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1\r\nConnection: close\r\n\r\n").await.unwrap();
            let mut raw = String::new();
            tokio::time::timeout(Duration::from_secs(2), stream.read_to_string(&mut raw))
                .await
                .unwrap()
                .unwrap();
            assert!(raw.starts_with(&format!("HTTP/1.1 {status}")), "{raw}");
        }
        task.abort();
    }
}
