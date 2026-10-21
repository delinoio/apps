// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{Server, store};
use axum::{Router, http::StatusCode, middleware, routing::get};
use server_common::{http::Policy, telemetry};
use std::{sync::Arc, time::Duration};

pub fn router(server: Arc<Server>) -> Router {
    let policy = Policy {
        origin: server.config.origin.clone(),
        timeout: server.config.request_timeout,
        service: telemetry::Service::DevBase,
        metrics: server.metrics.clone(),
    };
    let rpc = connectrpc::Router::new()
        .add_service(server.clone())
        .into_axum_service()
        .with_limits(connectrpc::Limits::default().with_max_message_size(256 << 10));
    let metrics = server.metrics.clone();
    let ready = server.db.clone();
    Router::new()
        .route("/healthz", get(|| async { StatusCode::OK }))
        .route(
            "/readyz",
            get(move || {
                let db = ready.clone();
                async move {
                    match tokio::time::timeout(Duration::from_secs(1), store::check(&db)).await {
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
        .fallback_service(Router::new().fallback_service(rpc).layer(
            middleware::from_fn_with_state(policy, server_common::http::middleware),
        ))
}
