// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::telemetry::{Metrics, Service};
use axum::{
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{sync::Arc, time::Duration};
use tracing::Instrument;
#[derive(Clone)]
pub struct Policy {
    pub origin: String,
    pub timeout: Duration,
    pub service: Service,
    pub metrics: Arc<Metrics>,
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
        match tokio::time::timeout(policy.timeout,next.run(request).instrument(span)).await {Ok(r)=>r,Err(_)=>(StatusCode::GATEWAY_TIMEOUT,axum::Json(serde_json::json!({"code":"deadline_exceeded","message":"request deadline exceeded"}))).into_response()}
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
