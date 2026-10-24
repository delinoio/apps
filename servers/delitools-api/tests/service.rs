// SPDX-License-Identifier: FSL-1.1-ALv2
//! Mocked-service tests: DeliTools over real HTTP against loopback mocks of Delcore (with the
//! Logto token endpoint) and the Vercel AI Gateway. None of them needs PostgreSQL: the cases here
//! reject before execution or run with unknown provider usage, so no outbox row is written.

mod common;

use buffa::Message;
use common::*;
use delitools_api::delcore::IngestFailure;
use serde_json::json;
use std::io::Write;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;

#[tokio::test]
async fn health_ready_and_metrics_endpoints_respond() {
    let h = Harness::start(None).await;
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{}/healthz", h.base))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    // The lazy pool cannot reach PostgreSQL, so the schema check fails closed.
    assert_eq!(
        client
            .get(format!("{}/readyz", h.base))
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    let metrics = client
        .get(format!("{}/metrics", h.base))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(metrics.contains("delitools_outbox_pending"));
}

#[tokio::test]
async fn browser_origins_are_rejected() {
    let h = Harness::start(None).await;
    let response = reqwest::Client::new()
        .post(format!(
            "{}/delitools.v1.DelitoolsService/Translate",
            h.base
        ))
        .header("Origin", "https://evil.example")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
}

#[tokio::test]
async fn translation_succeeds_with_a_user_token_and_unknown_usage_is_not_charged() {
    let h = Harness::start(None).await;
    h.gateway.push(Reply::without_usage(
        r#"{"translations":["Bonjour","Monde"]}"#,
    ));
    let wire = h
        .translate(USER_TOKEN, translate_body(&["Hello", "World"]))
        .await;
    assert!(wire.ok(), "{}", wire.body);
    assert_eq!(wire.body["translations"], json!(["Bonjour", "Monde"]));
    let id = wire.body["requestId"].as_str().unwrap();
    assert_eq!(wire.request_id_header.as_deref(), Some(id));
    assert!(server_common::canonical_id(id));
    // The paid Authorize call names the user, tenant, capability and paid=true.
    let state = h.delcore.lock().unwrap();
    let call = &state.authorize_calls[0];
    assert_eq!(call["tenantId"], TENANT);
    assert_eq!(call["userId"], USER);
    assert_eq!(call["capability"], "translate");
    assert_eq!(call["paid"], true);
    assert!(state.scopes.iter().all(|s| s == "authorize usage:write"));
    drop(state);
    let metrics = h.server.metrics.encode();
    assert!(metrics.contains("delitools_unknown_usage_total{reason=\"missing\"} 1"));
    // The request body sent to the Gateway uses the requested model and carries no tenant identity.
    let sent = h.gateway.last_request();
    assert_eq!(sent["model"], MODEL);
    assert!(!sent.to_string().contains(TENANT));
}

#[tokio::test]
async fn translation_with_a_tenant_api_key_uses_authorize_api_key() {
    let h = Harness::start(None).await;
    h.gateway.push(Reply::without_usage(r#"["Hola"]"#));
    let mut body = translate_body(&["Hello"]);
    body["tenantId"] = json!("");
    let wire = h.translate(KEY_SECRET, body).await;
    assert!(wire.ok(), "{}", wire.body);
    let state = h.delcore.lock().unwrap();
    assert_eq!(state.key_calls, 1);
    assert!(
        state.authorize_calls.is_empty(),
        "keys never use the user Authorize path"
    );
}

#[tokio::test]
async fn api_key_tenant_must_match_the_request() {
    let h = Harness::start(None).await;
    let mut body = translate_body(&["Hello"]);
    body["tenantId"] = json!(OTHER_TENANT);
    let wire = h.translate(KEY_SECRET, body).await;
    assert_eq!(wire.code(), "permission_denied");
    assert_eq!(wire.reason(), "ERROR_REASON_PERMISSION_DENIED");
    assert_eq!(h.gateway.calls(), 0);
}

#[tokio::test]
async fn invalid_credentials_never_reach_the_gateway() {
    let h = Harness::start(None).await;
    for token in [None, Some("garbage"), Some("dlk_unknown_secret"), Some("")] {
        let wire = h.call("Translate", token, translate_body(&["Hello"])).await;
        assert_eq!(wire.code(), "unauthenticated", "{token:?}");
        assert_eq!(wire.reason(), "ERROR_REASON_UNAUTHENTICATED");
        assert_eq!(wire.status, 401);
    }
    assert_eq!(h.gateway.calls(), 0);
}

#[tokio::test]
async fn permission_and_billing_denials_are_distinguished() {
    let h = Harness::start(None).await;
    h.delcore.lock().unwrap().authorize = deny_permission();
    let wire = h.translate(USER_TOKEN, translate_body(&["Hello"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_PERMISSION_DENIED");
    h.delcore.lock().unwrap().authorize = deny_billing();
    let wire = h.translate(USER_TOKEN, translate_body(&["Hello"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_BILLING_BLOCKED");
    assert_eq!(wire.code(), "failed_precondition");
    // A key that is valid but not allowed (grant missing).
    h.delcore
        .lock()
        .unwrap()
        .keys
        .insert(KEY_SECRET.into(), key_reply(TENANT, false));
    let wire = h.translate(KEY_SECRET, translate_body(&["Hello"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_PERMISSION_DENIED");
    assert_eq!(h.gateway.calls(), 0);
    let metrics = h.server.metrics.encode();
    assert!(metrics.contains("delitools_denied_total{reason=\"permission_denied\"} 2"));
    assert!(metrics.contains("delitools_denied_total{reason=\"billing_blocked\"} 1"));
}

#[tokio::test]
async fn user_tokens_require_a_canonical_tenant() {
    let h = Harness::start(None).await;
    for tenant in ["", "not-a-uuid", "019B0000-0000-7000-8000-0000000000B1"] {
        let mut body = translate_body(&["Hello"]);
        body["tenantId"] = json!(tenant);
        let wire = h.translate(USER_TOKEN, body).await;
        assert_eq!(wire.reason(), "ERROR_REASON_INVALID_INPUT", "{tenant}");
    }
}

#[tokio::test]
async fn paid_execution_fails_closed_when_delcore_is_unavailable_or_malformed() {
    let h = Harness::start(None).await;
    for (status, body) in [
        (503, json!({})),
        (200, json!([])),
        (200, json!({"allowed": "yes"})),
        (200, json!({})),
    ] {
        h.delcore
            .lock()
            .unwrap()
            .authorize_script
            .push_back((status, body));
        let wire = h.translate(USER_TOKEN, translate_body(&["Hello"])).await;
        assert_eq!(
            wire.reason(),
            "ERROR_REASON_DEPENDENCY_UNAVAILABLE",
            "{status}"
        );
        assert_eq!(wire.code(), "unavailable");
    }
    h.delcore.lock().unwrap().down = true;
    let wire = h.translate(USER_TOKEN, translate_body(&["Hello"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_DEPENDENCY_UNAVAILABLE");
    let wire = h.translate(KEY_SECRET, translate_body(&["Hello"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_DEPENDENCY_UNAVAILABLE");
    assert_eq!(h.gateway.calls(), 0);
}

#[tokio::test]
async fn validation_errors_use_invalid_input_and_limit_reasons() {
    let h = Harness::start(None).await;
    let cases = [
        (
            json!({"texts": [], "targetLanguage": "fr", "model": MODEL}),
            "ERROR_REASON_INVALID_INPUT",
        ),
        (
            json!({"texts": ["a", ""], "targetLanguage": "fr", "model": MODEL}),
            "ERROR_REASON_INVALID_INPUT",
        ),
        (
            json!({"texts": ["a"], "model": MODEL}),
            "ERROR_REASON_INVALID_INPUT",
        ),
        (
            json!({"texts": ["a"], "targetLanguage": "not a tag", "model": MODEL}),
            "ERROR_REASON_INVALID_INPUT",
        ),
        (
            json!({"texts": ["a"], "targetLanguage": "fr", "sourceLanguage": "xx_YY", "model": MODEL}),
            "ERROR_REASON_INVALID_INPUT",
        ),
        (
            json!({"texts": ["a"], "targetLanguage": "fr"}),
            "ERROR_REASON_INVALID_INPUT",
        ),
        (
            json!({"texts": ["a"], "targetLanguage": "fr", "model": "vendor/not-listed"}),
            "ERROR_REASON_MODEL_UNAVAILABLE",
        ),
        (
            json!({"texts": ["a","b","c","d","e","f"], "targetLanguage": "fr", "model": MODEL}),
            "ERROR_REASON_LIMIT_EXCEEDED",
        ),
        (
            json!({"texts": ["x".repeat(4097)], "targetLanguage": "fr", "model": MODEL}),
            "ERROR_REASON_LIMIT_EXCEEDED",
        ),
    ];
    for (mut body, reason) in cases {
        body["tenantId"] = json!(TENANT);
        let wire = h.translate(USER_TOKEN, body.clone()).await;
        assert_eq!(wire.reason(), reason, "{body}");
        assert!(!wire.ok());
        // The error carries a request ID that matches the header.
        assert_eq!(wire.detail().1, wire.request_id_header.clone().unwrap());
    }
    assert_eq!(h.gateway.calls(), 0);
}

#[tokio::test]
async fn models_without_registered_prices_are_rejected_without_substitution() {
    let h = Harness::start(None).await;
    // Allowed but unpriced.
    let mut body = translate_body(&["a"]);
    body["model"] = json!("vendor/unpriced");
    let wire = h.translate(USER_TOKEN, body).await;
    assert_eq!(wire.reason(), "ERROR_REASON_MODEL_UNAVAILABLE");
    assert_eq!(wire.code(), "failed_precondition");
    // Only the input price is registered.
    h.delcore.lock().unwrap().priced = vec!["gem_in".into()];
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_MODEL_UNAVAILABLE");
    // Registering both prices makes the model usable on the next request.
    h.delcore.lock().unwrap().priced = vec!["gem_in".into(), "gem_out".into()];
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    h.gateway.push(Reply::without_usage(r#"["x"]"#));
    assert!(h.translate(USER_TOKEN, translate_body(&["a"])).await.ok());
    assert_eq!(h.gateway.calls(), 1);
}

#[tokio::test]
async fn catalog_pagination_and_failures_are_handled() {
    let h = Harness::start(None).await;
    h.delcore.lock().unwrap().paginate_catalog = true;
    h.gateway.push(Reply::without_usage(r#"["x"]"#));
    assert!(h.translate(USER_TOKEN, translate_body(&["a"])).await.ok());
    assert!(h.delcore.lock().unwrap().catalog_calls >= 2);
    h.delcore.lock().unwrap().catalog_fails = true;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_DEPENDENCY_UNAVAILABLE");
}

#[tokio::test]
async fn model_output_must_match_the_request_shape() {
    for content in [
        "not json",
        r#"["only one"]"#,
        r#"["a","b","c"]"#,
        r#"["a", 2]"#,
        r#"{"translations":"x"}"#,
    ] {
        let h = Harness::start(None).await;
        h.gateway.push(Reply::without_usage(content));
        let wire = h
            .translate(USER_TOKEN, translate_body(&["Hello", "World"]))
            .await;
        assert_eq!(wire.reason(), "ERROR_REASON_INVALID_OUTPUT", "{content}");
        // Invalid output is never retried to repair it.
        assert_eq!(h.gateway.calls(), 1, "{content}");
    }
}

#[tokio::test]
async fn transient_failures_retry_with_a_bounded_budget() {
    let h = Harness::start(None).await;
    h.gateway.push(Reply::Status(503));
    h.gateway.push(Reply::RetryAfter(429, 0));
    h.gateway.push(Reply::without_usage(r#"["ok"]"#));
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert!(wire.ok(), "{}", wire.body);
    assert_eq!(h.gateway.calls(), 3);

    let h = Harness::start(None).await;
    for _ in 0..5 {
        h.gateway.push(Reply::Status(502));
    }
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_PROVIDER_FAILED");
    assert_eq!(wire.code(), "unavailable");
    // One attempt plus two retries.
    assert_eq!(h.gateway.calls(), 3);
}

#[tokio::test]
async fn http_date_retry_after_raises_the_wait_to_the_cap() {
    // Harness policy: base 5ms, cap 20ms, so jitter without a hint stays at or below 5ms before
    // the first retry. A distant-future HTTP-date hint raises the ceiling to the cap, so the wait
    // is at least half of it. Sleeps never finish early, which keeps the lower bound deterministic.
    for status in [429, 503] {
        let h = Harness::start(None).await;
        h.gateway.push(Reply::RetryAfterRaw(
            status,
            "Fri, 31 Dec 9999 23:59:59 GMT".to_owned(),
        ));
        h.gateway.push(Reply::without_usage(r#"["ok"]"#));
        let started = std::time::Instant::now();
        let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
        assert!(wire.ok(), "{status}: {}", wire.body);
        assert_eq!(h.gateway.calls(), 2, "{status}");
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(10),
            "{status}: {:?}",
            started.elapsed()
        );
    }
}

#[tokio::test]
async fn malformed_retry_after_values_still_retry_with_ordinary_backoff() {
    for value in ["-1", "1.5", "tomorrow", "Sun, 32 Nov 1994 08:49:37 GMT"] {
        let h = Harness::start(None).await;
        h.gateway.push(Reply::RetryAfterRaw(429, value.to_owned()));
        h.gateway.push(Reply::without_usage(r#"["ok"]"#));
        let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
        assert!(wire.ok(), "{value}: {}", wire.body);
        assert_eq!(h.gateway.calls(), 2, "{value}");
    }
}

#[tokio::test]
async fn permanent_provider_rejection_is_not_retried() {
    for status in [400, 401, 403, 404, 422] {
        let h = Harness::start(None).await;
        h.gateway.push(Reply::Status(status));
        let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
        assert_eq!(wire.reason(), "ERROR_REASON_PROVIDER_FAILED", "{status}");
        assert_eq!(wire.code(), "internal");
        assert_eq!(h.gateway.calls(), 1, "{status}");
    }
}

#[tokio::test]
async fn an_unreadable_gateway_body_is_invalid_output_without_retry() {
    let h = Harness::start(None).await;
    h.gateway.push(Reply::Garbage);
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_INVALID_OUTPUT");
    assert_eq!(h.gateway.calls(), 1);
}

#[tokio::test]
async fn retries_are_disabled_by_zero_max_retries() {
    let h = Harness::start_with(None, |c| c.retry.max_retries = 0).await;
    h.gateway.push(Reply::Status(503));
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_PROVIDER_FAILED");
    assert_eq!(h.gateway.calls(), 1);
}

#[tokio::test]
async fn authorization_is_rechecked_before_each_retry_and_stops_when_revoked() {
    let h = Harness::start(None).await;
    let delcore = h.delcore.clone();
    // The first attempt fails transiently; the key is revoked while the request waits to retry.
    h.gateway.push(Reply::Hook(
        std::sync::Arc::new(move || {
            delcore.lock().unwrap().keys.remove(KEY_SECRET);
        }),
        Box::new(Reply::Status(503)),
    ));
    h.gateway.push(Reply::without_usage(r#"["never reached"]"#));
    let mut body = translate_body(&["a"]);
    body["tenantId"] = json!("");
    let wire = h.translate(KEY_SECRET, body).await;
    assert_eq!(wire.reason(), "ERROR_REASON_UNAUTHENTICATED");
    assert_eq!(
        h.gateway.calls(),
        1,
        "no further provider attempt after revocation"
    );
    assert_eq!(h.delcore.lock().unwrap().key_calls, 2);
}

#[tokio::test]
async fn permission_loss_between_attempts_stops_user_retries() {
    let h = Harness::start(None).await;
    let delcore = h.delcore.clone();
    h.gateway.push(Reply::Hook(
        std::sync::Arc::new(move || delcore.lock().unwrap().authorize = deny_permission()),
        Box::new(Reply::Status(500)),
    ));
    h.gateway.push(Reply::without_usage(r#"["never"]"#));
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_PERMISSION_DENIED");
    assert_eq!(h.gateway.calls(), 1);
    assert_eq!(h.delcore.lock().unwrap().authorize_calls.len(), 2);
}

#[tokio::test]
async fn authorization_outage_between_attempts_fails_closed() {
    let h = Harness::start(None).await;
    let delcore = h.delcore.clone();
    h.gateway.push(Reply::Hook(
        std::sync::Arc::new(move || {
            delcore
                .lock()
                .unwrap()
                .authorize_script
                .push_back((503, json!({})))
        }),
        Box::new(Reply::Status(500)),
    ));
    h.gateway.push(Reply::without_usage(r#"["never"]"#));
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_DEPENDENCY_UNAVAILABLE");
    assert_eq!(h.gateway.calls(), 1);
}

#[tokio::test]
async fn the_execution_timeout_cancels_the_provider_call() {
    let h = Harness::start_with(None, |c| {
        c.limits.execution_timeout = std::time::Duration::from_millis(300)
    })
    .await;
    h.gateway.push(Reply::Hang);
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_TIMEOUT");
    assert_eq!(wire.code(), "deadline_exceeded");
    // Dropping the provider request is observable on the mock.
    for _ in 0..50 {
        if h.gateway.dropped() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(h.gateway.dropped(), 1);
    assert_eq!(h.gateway.calls(), 1);
}

#[tokio::test]
async fn caller_cancellation_aborts_the_provider_call_and_stops_retries() {
    let h = Harness::start(None).await;
    h.gateway.push(Reply::Hang);
    h.gateway.push(Reply::without_usage(r#"["never"]"#));
    let pending = tokio::spawn(h.call_future("Translate", USER_TOKEN, translate_body(&["a"])));
    for _ in 0..100 {
        if h.gateway.calls() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(h.gateway.calls(), 1);
    pending.abort();
    let _ = pending.await;
    for _ in 0..100 {
        if h.gateway.dropped() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(h.gateway.dropped(), 1, "the provider call is canceled");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(h.gateway.calls(), 1, "no retry after cancellation");
    // The tenant permit is released after the execution task ends.
    for _ in 0..50 {
        if h.server.in_flight(TENANT) == 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the tenant permit was not released");
}

/// Waits until the mock Gateway has received `calls` requests.
async fn wait_for_gateway_calls(h: &Harness, calls: usize) {
    for _ in 0..200 {
        if h.gateway.calls() >= calls {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the Gateway did not receive {calls} calls");
}

#[tokio::test]
async fn the_instance_fence_cancels_a_held_provider_call_promptly() {
    // The execution timeout is far longer than the assertion bound: only the fence can end the call.
    let h = Harness::start_with(None, |c| {
        c.limits.execution_timeout = std::time::Duration::from_secs(60);
        c.dependency_timeout = std::time::Duration::from_secs(2);
    })
    .await;
    h.gateway.push(Reply::Hang);
    h.gateway.push(Reply::without_usage(r#"["never"]"#));
    let pending = tokio::spawn(h.call_future("Translate", USER_TOKEN, translate_body(&["a"])));
    wait_for_gateway_calls(&h, 1).await;
    let fenced = std::time::Instant::now();
    h.server.fence();
    for _ in 0..100 {
        if h.gateway.dropped() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(h.gateway.dropped(), 1, "the provider connection is dropped");
    assert!(fenced.elapsed() < std::time::Duration::from_secs(1));
    let response = pending.await.unwrap().unwrap();
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "canceled");
    // No retry follows, no usage is estimated, and the tenant permit is released.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(h.gateway.calls(), 1);
    let metrics = h.server.metrics.encode();
    assert!(
        !metrics.contains("delitools_usage_persisted_total{"),
        "{metrics}"
    );
    assert_eq!(h.server.in_flight(TENANT), 0);
}

#[tokio::test]
async fn the_instance_fence_stops_retries_during_backoff_and_after_a_transient_failure() {
    // A long backoff keeps the retry pending until the fence closes.
    let h = Harness::start_with(None, |c| {
        c.limits.execution_timeout = std::time::Duration::from_secs(60);
        c.retry.base_delay = std::time::Duration::from_secs(20);
        c.retry.max_delay = std::time::Duration::from_secs(20);
    })
    .await;
    h.gateway.push(Reply::Status(503));
    h.gateway.push(Reply::without_usage(r#"["never"]"#));
    let pending = tokio::spawn(h.call_future("Translate", USER_TOKEN, translate_body(&["a"])));
    wait_for_gateway_calls(&h, 1).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let authorizations = h.delcore.lock().unwrap().authorize_calls.len();
    h.server.fence();
    let body: serde_json::Value =
        tokio::time::timeout(std::time::Duration::from_secs(2), async move {
            pending.await.unwrap().unwrap().json().await.unwrap()
        })
        .await
        .expect("the retry wait ends at the fence");
    assert_eq!(body["code"], "canceled");
    assert_eq!(h.gateway.calls(), 1, "no attempt starts after the fence");
    assert_eq!(
        h.delcore.lock().unwrap().authorize_calls.len(),
        authorizations,
        "no retry authorization runs after the fence"
    );

    // A transient failure that returns exactly as the fence closes cannot start another attempt.
    let h = Harness::start(None).await;
    let server = h.server.clone();
    h.gateway.push(Reply::Hook(
        std::sync::Arc::new(move || server.fence()),
        Box::new(Reply::Status(503)),
    ));
    h.gateway.push(Reply::without_usage(r#"["never"]"#));
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.code(), "canceled");
    assert_eq!(h.gateway.calls(), 1);
}

#[tokio::test]
async fn requests_admitted_after_the_fence_start_no_provider_work() {
    let h = Harness::start(None).await;
    h.gateway.push(Reply::without_usage(r#"["never"]"#));
    h.server.fence();
    h.server.fence();
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.code(), "canceled");
    assert_eq!(wire.reason(), "ERROR_REASON_CANCELED");
    assert_eq!(h.gateway.calls(), 0);
    assert_eq!(h.server.in_flight(TENANT), 0);
}

#[tokio::test]
async fn a_request_whose_admission_races_the_fence_is_canceled_without_a_provider_call() {
    // The fence closes while delcore is answering the authorization; the job must not slip through
    // although authorization still succeeds and admission completes.
    let h = Harness::start(None).await;
    h.gateway.push(Reply::without_usage(r#"["never"]"#));
    let server = h.server.clone();
    h.delcore.lock().unwrap().authorize_hook = Some(std::sync::Arc::new(move || server.fence()));
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.code(), "canceled");
    assert_eq!(h.gateway.calls(), 0);
    assert_eq!(h.server.in_flight(TENANT), 0);
}

#[tokio::test]
async fn tenant_rate_and_concurrency_limits_apply_per_tenant() {
    let h = Harness::start_with(None, |c| {
        c.limits.tenant_concurrency = 1;
        c.limits.tenant_rate = 1000.0;
    })
    .await;
    h.gateway.push(Reply::Hang);
    let first = tokio::spawn(h.call_future("Translate", USER_TOKEN, translate_body(&["a"])));
    for _ in 0..100 {
        if h.gateway.calls() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let wire = h.translate(USER_TOKEN, translate_body(&["b"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_RATE_LIMITED");
    assert_eq!(wire.code(), "resource_exhausted");
    // Another tenant is unaffected.
    h.gateway.push(Reply::without_usage(r#"["x"]"#));
    let mut other = translate_body(&["b"]);
    other["tenantId"] = json!(OTHER_TENANT);
    assert!(h.translate(USER_TOKEN, other).await.ok());
    first.abort();

    let h = Harness::start_with(None, |c| {
        c.limits.tenant_rate = 0.001;
        c.limits.tenant_burst = 1;
    })
    .await;
    h.gateway.push(Reply::without_usage(r#"["x"]"#));
    assert!(h.translate(USER_TOKEN, translate_body(&["a"])).await.ok());
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_RATE_LIMITED");
    assert!(wire.retry_after.is_some());
}

#[tokio::test]
async fn transport_bound_admits_input_that_only_escaping_makes_large() {
    let h = Harness::start_with(None, |c| {
        c.limits.max_input_bytes = 200_000;
        c.limits.max_schema_bytes = 1_000;
    })
    .await;
    // 190,000 control characters stay within the input limit but encode as
    // `\u0001` (six bytes each) on the wire, far above the decoded limit plus 64 KiB.
    let text = "\u{1}".repeat(190_000);
    h.gateway.push(Reply::without_usage(r#"["x"]"#));
    let wire = h.translate(USER_TOKEN, translate_body(&[&text])).await;
    assert!(wire.ok(), "{}", wire.body);
    // A decoded input above the limit is still refused by the handler.
    let too_large = "a".repeat(200_001);
    let wire = h.translate(USER_TOKEN, translate_body(&[&too_large])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_LIMIT_EXCEEDED");
}

/// Transport allowance for the given limits: six bytes per decoded byte plus 64 KiB.
fn allowance(input: usize, schema: usize) -> usize {
    (input + schema) * 6 + (64 << 10)
}

async fn harness_with_input_limit(input: usize) -> Harness {
    Harness::start_with(None, |c| {
        c.limits.max_input_bytes = input;
        c.limits.max_schema_bytes = 32_768;
    })
    .await
}

fn json_translate(text: &str) -> Vec<u8> {
    serde_json::to_vec(&translate_body(&[text])).unwrap()
}

fn proto_translate(text: &str) -> Vec<u8> {
    delino_protocol::delitools::v1::TranslateRequest {
        tenant_id: TENANT.into(),
        texts: vec![text.into()],
        target_language: "fr".into(),
        model: MODEL.into(),
        ..Default::default()
    }
    .encode_to_vec()
}

/// A valid JSON request followed by trailing whitespace up to exactly `size` bytes.
fn json_padded_to(size: usize) -> Vec<u8> {
    let mut body = json_translate("a");
    assert!(body.len() <= size);
    body.resize(size, b' ');
    body
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

/// The documented transport rejection: plain `resource_exhausted`, no `ErrorDetail` and no
/// request ID, produced before any handler work.
fn assert_transport_rejection(reply: &RawReply, what: &str) {
    assert_eq!(reply.status, 429, "{what}: {}", reply.head);
    let body = reply.json();
    assert_eq!(body["code"], "resource_exhausted", "{what}: {body}");
    assert!(
        body.get("details")
            .and_then(|d| d.as_array())
            .is_none_or(|d| d.is_empty()),
        "{what}: {body}"
    );
    assert_eq!(reply.header(REQUEST_ID), None, "{what}");
}

/// A body that reached the handler: typed `UNAUTHENTICATED` (no credentials were sent) with a
/// request ID that matches the response header.
fn assert_reached_handler(reply: &RawReply, what: &str) {
    let wire = reply.wire();
    assert_eq!(wire.status, 401, "{what}: {}", reply.head);
    let (reason, request_id) = wire.detail();
    assert_eq!(reason, "ERROR_REASON_UNAUTHENTICATED", "{what}");
    assert!(!request_id.is_empty(), "{what}");
    assert_eq!(wire.request_id_header.as_deref(), Some(request_id.as_str()));
}

#[tokio::test]
async fn escaped_bodies_above_four_mib_reach_the_handler_within_the_allowance() {
    let h = harness_with_input_limit(1_000_000).await;
    // 1,000,000 U+0001 characters encode as six bytes each: about 6 MB on the wire, above the
    // 4 MiB default body limit and below the computed allowance.
    let text = "\u{1}".repeat(1_000_000);
    let body = json_translate(&text);
    assert!(body.len() > 4 << 20);
    assert!(body.len() <= allowance(1_000_000, 32_768));

    let reply = h
        .raw(
            "Translate",
            "application/json",
            None,
            None,
            Framing::ContentLength,
            body,
        )
        .await;
    assert_reached_handler(&reply, "without credentials");
    assert_eq!(h.gateway.calls(), 0);

    h.gateway.push(Reply::without_usage(r#"["x"]"#));
    let wire = h.translate(USER_TOKEN, translate_body(&[&text])).await;
    assert!(wire.ok(), "{}", wire.body);
    assert_eq!(h.gateway.calls(), 1);

    // A decoded input one byte above the limit still gets the typed handler error, even though
    // its escaped body is also above 4 MiB.
    let too_large = "\u{1}".repeat(1_000_001);
    let wire = h.translate(USER_TOKEN, translate_body(&[&too_large])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_LIMIT_EXCEEDED");
    assert_eq!(wire.code(), "resource_exhausted");
    assert!(!wire.detail().1.is_empty());
    assert_eq!(
        wire.request_id_header.as_deref(),
        Some(wire.detail().1.as_str())
    );
    let too_large = "a".repeat(1_000_001);
    let wire = h.translate(USER_TOKEN, translate_body(&[&too_large])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_LIMIT_EXCEEDED");
    assert_eq!(h.gateway.calls(), 1);
}

#[tokio::test]
async fn four_mib_of_decoded_input_is_transportable_for_json_and_protobuf() {
    let max = 4_194_304;
    let h = harness_with_input_limit(max).await;
    let text = "a".repeat(max);

    let json = json_translate(&text);
    assert!(json.len() > 4 << 20);
    h.gateway.push(Reply::without_usage(r#"["x"]"#));
    let wire = h.translate(USER_TOKEN, translate_body(&[&text])).await;
    assert!(wire.ok(), "{}", wire.body);
    assert_eq!(h.gateway.calls(), 1);

    let proto = proto_translate(&text);
    assert!(proto.len() > 4 << 20);
    h.gateway.push(Reply::without_usage(r#"["x"]"#));
    let reply = h
        .raw(
            "Translate",
            "application/proto",
            None,
            Some(USER_TOKEN),
            Framing::ContentLength,
            proto,
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.head);
    assert!(reply.header(REQUEST_ID).is_some());
    assert_eq!(h.gateway.calls(), 2);

    // The same request without credentials is classified by the handler, not the transport.
    let reply = h
        .raw(
            "Translate",
            "application/json",
            None,
            None,
            Framing::Chunked,
            json,
        )
        .await;
    assert_reached_handler(&reply, "chunked 4 MiB input");

    // One byte above the computed allowance is still rejected before any handler work.
    let limit = allowance(max, 32_768);
    let reply = h
        .raw(
            "Translate",
            "application/json",
            None,
            Some(USER_TOKEN),
            Framing::ContentLength,
            json_padded_to(limit + 1),
        )
        .await;
    assert_transport_rejection(&reply, "maximum configuration");
    assert_eq!(h.gateway.calls(), 2);
}

#[tokio::test]
async fn bodies_above_the_allowance_are_rejected_before_handler_work() {
    let h = Harness::start(None).await;
    let limit = allowance(
        h.config.limits.max_input_bytes,
        h.config.limits.max_schema_bytes,
    );
    for (framing, name) in [
        (Framing::ContentLength, "content-length"),
        (Framing::Chunked, "chunked"),
    ] {
        h.gateway.push(Reply::without_usage(r#"["x"]"#));
        let reply = h
            .raw(
                "Translate",
                "application/json",
                None,
                Some(USER_TOKEN),
                framing,
                json_padded_to(limit),
            )
            .await;
        // Exactly the allowance is admitted: the padded body is a valid request, so the
        // handler runs and the scripted Gateway answers it.
        assert_eq!(reply.status, 200, "{name}: {}", reply.head);
        assert!(reply.header(REQUEST_ID).is_some(), "{name}");

        let reply = h
            .raw(
                "Translate",
                "application/json",
                None,
                Some(USER_TOKEN),
                framing,
                json_padded_to(limit + 1),
            )
            .await;
        assert_transport_rejection(&reply, name);
    }
    let reply = h
        .raw(
            "Extract",
            "application/proto",
            None,
            None,
            Framing::Chunked,
            vec![0; limit + 1],
        )
        .await;
    assert_transport_rejection(&reply, "protobuf");
    // Only the two admitted boundary requests reached the Gateway.
    assert_eq!(h.gateway.calls(), 2);
}

#[tokio::test]
async fn decompressed_messages_above_the_allowance_are_rejected() {
    let h = Harness::start(None).await;
    let limit = allowance(
        h.config.limits.max_input_bytes,
        h.config.limits.max_schema_bytes,
    );

    // The compressed body is tiny, so only the decompression limit can reject it.
    let bomb = gzip(&json_padded_to(limit + 1));
    assert!(bomb.len() < limit);
    let reply = h
        .raw(
            "Translate",
            "application/json",
            Some("gzip"),
            Some(USER_TOKEN),
            Framing::ContentLength,
            bomb,
        )
        .await;
    assert_transport_rejection(&reply, "decompression above the allowance");

    // A body that expands to exactly the allowance is handed to the handler.
    let reply = h
        .raw(
            "Translate",
            "application/json",
            Some("gzip"),
            None,
            Framing::ContentLength,
            gzip(&json_padded_to(limit)),
        )
        .await;
    assert_reached_handler(&reply, "decompression at the allowance");
    assert_eq!(h.gateway.calls(), 0);
}

#[tokio::test]
async fn extraction_returns_schema_conformant_json_with_the_fixed_model() {
    let h = Harness::start(None).await;
    h.gateway.push(Reply::without_usage(&extracted(
        json!({"name":"Ada","age":36}),
    )));
    let wire = h.extract(USER_TOKEN, extract_body()).await;
    assert!(wire.ok(), "{}", wire.body);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(wire.body["json"].as_str().unwrap()).unwrap(),
        json!({"name":"Ada","age":36})
    );
    assert_eq!(h.gateway.last_request()["model"], LUNA);
    let state = h.delcore.lock().unwrap();
    assert_eq!(state.authorize_calls[0]["capability"], "extract");
}

#[tokio::test]
async fn extraction_rejects_unsupported_schemas_before_execution() {
    let h = Harness::start(None).await;
    for schema in [
        r##"{"$ref":"#/$defs/x"}"##,
        r#"{"oneOf":[{"type":"string"}]}"#,
        r#"{"type":"object","patternProperties":{"a":{"type":"string"}}}"#,
    ] {
        let mut body = extract_body();
        body["jsonSchema"] = json!(schema);
        let wire = h.extract(USER_TOKEN, body).await;
        assert_eq!(wire.reason(), "ERROR_REASON_UNSUPPORTED_SCHEMA", "{schema}");
        assert_eq!(wire.code(), "invalid_argument");
    }
    let mut body = extract_body();
    body["jsonSchema"] = json!("{broken");
    assert_eq!(
        h.extract(USER_TOKEN, body).await.reason(),
        "ERROR_REASON_INVALID_INPUT"
    );
    let mut body = extract_body();
    body["text"] = json!("");
    assert_eq!(
        h.extract(USER_TOKEN, body).await.reason(),
        "ERROR_REASON_INVALID_INPUT"
    );
    assert_eq!(h.gateway.calls(), 0);
}

#[tokio::test]
async fn extraction_failures_and_invalid_output_are_distinct_and_never_retried() {
    let h = Harness::start(None).await;
    h.gateway.push(Reply::without_usage(
        r#"{"status":"missing_information","missing":["name"]}"#,
    ));
    let wire = h.extract(USER_TOKEN, extract_body()).await;
    assert_eq!(wire.reason(), "ERROR_REASON_EXTRACTION_FAILED");
    assert_eq!(h.gateway.calls(), 1);

    for content in [
        "{not json".to_owned(),
        extracted(json!({"age": 36})),
        extracted(json!({"name": 7})),
        extracted(json!({"name": "Ada", "unexpected": true})),
        r#"{"status":"unknown"}"#.to_owned(),
    ] {
        let h = Harness::start(None).await;
        h.gateway.push(Reply::without_usage(&content));
        let wire = h.extract(USER_TOKEN, extract_body()).await;
        assert_eq!(wire.reason(), "ERROR_REASON_INVALID_OUTPUT", "{content}");
        assert_eq!(h.gateway.calls(), 1, "{content}");
    }
}

#[tokio::test]
async fn extraction_requires_prices_for_the_fixed_model() {
    let h = Harness::start(None).await;
    h.delcore.lock().unwrap().priced = vec!["gem_in".into(), "gem_out".into()];
    let wire = h.extract(USER_TOKEN, extract_body()).await;
    assert_eq!(wire.reason(), "ERROR_REASON_MODEL_UNAVAILABLE");
    assert_eq!(h.gateway.calls(), 0);
}

#[tokio::test]
async fn each_request_executes_anew_without_replay() {
    let h = Harness::start(None).await;
    h.gateway.push(Reply::without_usage(r#"["un"]"#));
    h.gateway.push(Reply::without_usage(r#"["deux"]"#));
    let first = h.translate(USER_TOKEN, translate_body(&["one"])).await;
    let second = h.translate(USER_TOKEN, translate_body(&["one"])).await;
    assert_eq!(first.body["translations"], json!(["un"]));
    assert_eq!(second.body["translations"], json!(["deux"]));
    assert_ne!(first.body["requestId"], second.body["requestId"]);
    assert_eq!(h.gateway.calls(), 2);
}

#[tokio::test]
async fn an_unrepresentable_execution_timeout_fails_closed_without_panicking() {
    // Config::validate rejects this at startup; the service must still not panic if a Config
    // bypasses validation.
    for timeout in [
        std::time::Duration::MAX,
        std::time::Duration::MAX - std::time::Duration::from_secs(5),
    ] {
        let h = Harness::start_with(None, |c| c.limits.execution_timeout = timeout).await;
        for method in ["Translate", "Extract"] {
            let body = if method == "Translate" {
                translate_body(&["a"])
            } else {
                extract_body()
            };
            let wire = h.call(method, Some(USER_TOKEN), body).await;
            assert_eq!(wire.code(), "internal", "{method}");
            assert_eq!(wire.status, 500, "{method}");
            assert!(wire.request_id_header.is_some(), "{method}");
        }
        assert_eq!(h.gateway.calls(), 0);
    }
}

#[tokio::test]
async fn long_supported_timeouts_keep_authentication_and_timeout_behavior() {
    let h = Harness::start_with(None, |c| {
        c.limits.execution_timeout = std::time::Duration::from_secs(30 * 24 * 3600)
    })
    .await;
    for method in ["Translate", "Extract"] {
        let wire = h.call(method, None, translate_body(&["a"])).await;
        assert_eq!(wire.reason(), "ERROR_REASON_UNAUTHENTICATED", "{method}");
    }
    let h = Harness::start_with(None, |c| {
        c.limits.execution_timeout = std::time::Duration::from_millis(200)
    })
    .await;
    h.gateway.push(Reply::Hang);
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_TIMEOUT");
}

// The cases below run through the production shared transport (`transport::serve`) rather than
// `axum::serve`, with a socket idle-read interval far below the execution budget. reqwest keeps
// its connection open after sending the whole request, as an ordinary HTTP/1 client does.

const TRANSPORT_IDLE: std::time::Duration = std::time::Duration::from_millis(150);

#[tokio::test]
async fn translation_outlives_the_socket_idle_interval_through_the_shared_transport() {
    let h = Harness::start_transport(None, TRANSPORT_IDLE, |_| {}).await;
    h.gateway.push(Reply::Delay(
        600,
        Box::new(Reply::without_usage(r#"["Bonjour"]"#)),
    ));
    let wire = h.translate(USER_TOKEN, translate_body(&["Hello"])).await;
    assert!(wire.ok(), "{}", wire.body);
    assert_eq!(wire.body["translations"], json!(["Bonjour"]));
    assert_eq!(h.gateway.calls(), 1, "one attempt and no retry");
    assert_eq!(h.gateway.dropped(), 0);
}

#[tokio::test]
async fn extraction_outlives_the_socket_idle_interval_through_the_shared_transport() {
    let h = Harness::start_transport(None, TRANSPORT_IDLE, |_| {}).await;
    h.gateway.push(Reply::Delay(
        600,
        Box::new(Reply::without_usage(&extracted(
            json!({"name":"Ada","age":36}),
        ))),
    ));
    let wire = h.extract(USER_TOKEN, extract_body()).await;
    assert!(wire.ok(), "{}", wire.body);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(wire.body["json"].as_str().unwrap()).unwrap(),
        json!({"name":"Ada","age":36})
    );
    assert_eq!(h.gateway.calls(), 1);
    assert_eq!(h.gateway.dropped(), 0);
}

#[tokio::test]
async fn the_execution_timeout_wins_over_the_socket_idle_interval() {
    let h = Harness::start_transport(None, TRANSPORT_IDLE, |c| {
        c.limits.execution_timeout = std::time::Duration::from_millis(700)
    })
    .await;
    h.gateway.push(Reply::Hang);
    h.gateway.push(Reply::without_usage(r#"["never"]"#));
    let started = std::time::Instant::now();
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert!(started.elapsed() >= std::time::Duration::from_millis(600));
    assert_eq!(wire.reason(), "ERROR_REASON_TIMEOUT");
    assert_eq!(wire.code(), "deadline_exceeded");
    assert!(
        !wire.detail().1.is_empty(),
        "the typed timeout names its request"
    );
    for _ in 0..50 {
        if h.gateway.dropped() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(h.gateway.dropped(), 1, "the provider call is canceled");
    assert_eq!(h.gateway.calls(), 1, "no retry");
}

#[tokio::test]
async fn a_closed_client_still_cancels_the_provider_call_through_the_shared_transport() {
    // The execution budget and idle interval both outlast the scenario, so only the socket
    // closure can cancel the provider call.
    let h = Harness::start_transport(None, std::time::Duration::from_secs(30), |_| {}).await;
    h.gateway.push(Reply::Hang);
    h.gateway.push(Reply::without_usage(r#"["never"]"#));
    let pending = tokio::spawn(h.call_future("Translate", USER_TOKEN, translate_body(&["a"])));
    for _ in 0..100 {
        if h.gateway.calls() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(h.gateway.calls(), 1);
    pending.abort();
    let _ = pending.await;
    for _ in 0..100 {
        if h.gateway.dropped() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        h.gateway.dropped(),
        1,
        "the provider call is canceled promptly"
    );
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(h.gateway.calls(), 1, "no retry after cancellation");
}

// ---------------------------------------------------------------------------------------------
// IngestEvent acknowledgment validation
// ---------------------------------------------------------------------------------------------

fn usage_event() -> delitools_api::delcore::UsageEvent {
    delitools_api::delcore::UsageEvent {
        request_key: Uuid::now_v7().to_string(),
        tenant_id: TENANT.into(),
        event_key: format!("{}:1:in", Uuid::now_v7()),
        meter_key: "gem_in".into(),
        quantity: 7,
        occurred_at: chrono::Utc::now(),
    }
}

/// HTTP-success bodies that do not acknowledge the ingestion.
fn invalid_acknowledgments() -> Vec<serde_json::Value> {
    let id = VALID_EVENT_ID;
    vec![
        json!({}),
        json!({"eventId": 42, "revision": "broken"}),
        json!({"eventId": 42, "revision": 1}),
        json!({"eventId": id, "revision": "broken"}),
        json!({"eventId": "", "revision": 1}),
        json!({"revision": 1}),
        json!({"eventId": null, "revision": 1}),
        json!({"eventId": "e", "revision": 1}),
        // UUID v4, uppercase and braced forms are not canonical UUID v7 identities.
        json!({"eventId": "3b241101-e2bb-4255-8caf-4136c566a962", "revision": 1}),
        json!({"eventId": "0190ABCD-0000-7000-8000-00000000ABCD", "revision": 1}),
        json!({"eventId": "{0190abcd-0000-7000-8000-00000000abcd}", "revision": 1}),
        json!({"eventId": id}),
        json!({"eventId": id, "revision": null}),
        json!({"eventId": id, "revision": 0}),
        json!({"eventId": id, "revision": -1}),
        json!({"eventId": id, "revision": 2}),
        json!({"eventId": id, "revision": "0"}),
        json!({"eventId": id, "revision": "2"}),
        json!({"eventId": id, "revision": "one"}),
        json!({"eventId": id, "revision": true}),
        json!({"eventId": id, "revision": {}}),
    ]
}

#[tokio::test]
async fn invalid_ingest_acknowledgments_are_retryable_malformed_responses() {
    let h = Harness::start(None).await;
    for body in invalid_acknowledgments() {
        h.delcore.lock().unwrap().ingest_default = Some(Ingest::Raw {
            status: 200,
            body: body.clone(),
        });
        assert_eq!(
            h.server
                .delcore
                .ingest_event(&usage_event(), &|| true)
                .await,
            Err(delitools_api::delcore::IngestFailure::Retryable(
                "malformed_response"
            )),
            "{body}"
        );
    }
}

#[tokio::test]
async fn valid_ingest_acknowledgments_deliver_including_supported_encodings() {
    let h = Harness::start(None).await;
    for body in [
        json!({"eventId": VALID_EVENT_ID, "revision": 1}),
        // Generated ProtoJSON accepts integer strings and snake_case field names.
        json!({"eventId": VALID_EVENT_ID, "revision": "1"}),
        json!({"event_id": VALID_EVENT_ID, "revision": 1}),
        // Identical replay returns the original identity and revision 1 again.
        json!({"eventId": VALID_EVENT_ID, "revision": 1}),
        // Correlation metadata is not a delivery condition.
        json!({"metadata": {"correlationId": "c"}, "eventId": VALID_EVENT_ID, "revision": 1}),
        json!({"eventId": VALID_EVENT_ID, "revision": 1, "unknownField": true}),
    ] {
        h.delcore.lock().unwrap().ingest_default = Some(Ingest::Raw {
            status: 200,
            body: body.clone(),
        });
        assert_eq!(
            h.server
                .delcore
                .ingest_event(&usage_event(), &|| true)
                .await,
            Ok(()),
            "{body}"
        );
    }
}

#[tokio::test]
async fn non_object_ingest_bodies_and_poison_classification_are_unchanged() {
    use delitools_api::delcore::IngestFailure;
    let h = Harness::start(None).await;
    for body in [json!([]), json!("ok"), json!(null), json!(7)] {
        h.delcore.lock().unwrap().ingest_default = Some(Ingest::Raw {
            status: 200,
            body: body.clone(),
        });
        assert_eq!(
            h.server
                .delcore
                .ingest_event(&usage_event(), &|| true)
                .await,
            Err(IngestFailure::Retryable("malformed_response")),
            "{body}"
        );
    }
    h.delcore.lock().unwrap().ingest_default = Some(Ingest::Error {
        status: 409,
        code: "already_exists",
        reason: Some(11),
    });
    assert_eq!(
        h.server
            .delcore
            .ingest_event(&usage_event(), &|| true)
            .await,
        Err(IngestFailure::Poison("identity_conflict"))
    );
}

#[tokio::test]
async fn ingest_never_starts_a_request_after_the_deadline_passes_during_token_acquisition() {
    let h = Harness::start(None).await;
    let live = Arc::new(AtomicBool::new(true));
    // The token request is still being answered when the row expires.
    let flag = live.clone();
    h.delcore.lock().unwrap().on_token =
        Some(Arc::new(move || flag.store(false, Ordering::SeqCst)));
    let flag = live.clone();
    let still_live = move || flag.load(Ordering::SeqCst);
    let result = h
        .server
        .delcore
        .ingest_event(&usage_event(), &still_live)
        .await;
    assert_eq!(result, Err(IngestFailure::Expired));
    let state = h.delcore.lock().unwrap();
    assert_eq!(state.token_calls, 1, "the token request itself had started");
    assert!(state.ingested.is_empty(), "no IngestEvent request was sent");
}

#[tokio::test]
async fn ingest_sends_when_the_row_is_still_live_after_the_token_is_available() {
    let h = Harness::start(None).await;
    let result = h
        .server
        .delcore
        .ingest_event(&usage_event(), &|| true)
        .await;
    assert_eq!(result, Ok(()));
    assert_eq!(h.delcore.lock().unwrap().ingested.len(), 1);
    // A cached token is re-checked as well: expiry before the send still sends nothing.
    let result = h
        .server
        .delcore
        .ingest_event(&usage_event(), &|| false)
        .await;
    assert_eq!(result, Err(IngestFailure::Expired));
    assert_eq!(h.delcore.lock().unwrap().ingested.len(), 1);
    assert_eq!(h.delcore.lock().unwrap().token_calls, 1);
}

const SLOW: std::time::Duration = std::time::Duration::from_millis(300);
const WATCHDOG: std::time::Duration = std::time::Duration::from_secs(7);

fn body_for(method: &str) -> serde_json::Value {
    if method == "Translate" {
        translate_body(&["Hello"])
    } else {
        extract_body()
    }
}

#[tokio::test]
async fn a_body_that_completes_after_the_deadline_never_reaches_the_provider() {
    for method in ["Translate", "Extract"] {
        let h = Harness::start_with(None, |c| {
            c.limits.execution_timeout = std::time::Duration::from_millis(100);
            c.dependency_timeout = std::time::Duration::from_millis(80);
        })
        .await;
        h.gateway.push(Reply::without_usage(r#"["Bonjour"]"#));
        let (wire, elapsed) = h
            .call_slow(method, USER_TOKEN, body_for(method), SLOW, WATCHDOG)
            .await;
        assert_eq!(wire.status, 504, "{method}");
        assert_eq!(wire.code(), "deadline_exceeded", "{method}");
        assert_eq!(wire.reason(), "ERROR_REASON_TIMEOUT", "{method}");
        let (_, detail_request_id) = wire.detail();
        assert_eq!(
            wire.request_id_header.as_deref(),
            Some(detail_request_id.as_str()),
            "{method}"
        );
        assert!(elapsed < SLOW, "{method} answered at {elapsed:?}");
        // The body completes after the reply; give a late handler time to misbehave.
        tokio::time::sleep(SLOW).await;
        assert_eq!(h.gateway.calls(), 0, "{method}");
        assert_eq!(h.server.in_flight(TENANT), 0, "{method}");
    }
}

#[tokio::test]
async fn an_incomplete_body_is_cut_at_the_execution_timeout_not_the_transport_margin() {
    let h = Harness::start_with(None, |c| {
        c.limits.execution_timeout = std::time::Duration::from_millis(150);
        c.dependency_timeout = std::time::Duration::from_millis(80);
    })
    .await;
    // The body never completes within the test.
    let (wire, elapsed) = h
        .call_slow(
            "Translate",
            USER_TOKEN,
            translate_body(&["Hello"]),
            std::time::Duration::from_secs(30),
            WATCHDOG,
        )
        .await;
    assert_eq!(wire.reason(), "ERROR_REASON_TIMEOUT");
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "the reply must not wait for the five-second margin: {elapsed:?}"
    );
    assert_eq!(h.gateway.calls(), 0);
}

#[tokio::test]
async fn body_time_reduces_the_provider_budget() {
    let h = Harness::start_with(None, |c| {
        c.limits.execution_timeout = std::time::Duration::from_millis(500);
        c.dependency_timeout = std::time::Duration::from_millis(80);
    })
    .await;
    // 300ms of body plus 300ms of provider time exceeds the 500ms budget measured from receipt.
    h.gateway.push(Reply::Delay(
        300,
        Box::new(Reply::without_usage(r#"["Bonjour"]"#)),
    ));
    let (wire, elapsed) = h
        .call_slow(
            "Translate",
            USER_TOKEN,
            translate_body(&["Hello"]),
            SLOW,
            WATCHDOG,
        )
        .await;
    assert_eq!(wire.reason(), "ERROR_REASON_TIMEOUT");
    assert!(
        elapsed >= std::time::Duration::from_millis(480)
            && elapsed < std::time::Duration::from_millis(900),
        "the provider call must be canceled near the original deadline: {elapsed:?}"
    );
    assert_eq!(h.gateway.calls(), 1);
    for _ in 0..50 {
        if h.gateway.dropped() == 0 && h.server.in_flight(TENANT) == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(h.server.in_flight(TENANT), 0);
    assert_eq!(h.gateway.calls(), 1, "no attempt starts after expiry");
}

#[tokio::test]
async fn a_slow_body_within_the_budget_still_succeeds() {
    let h = Harness::start_with(None, |c| {
        c.limits.execution_timeout = std::time::Duration::from_secs(5)
    })
    .await;
    for method in ["Translate", "Extract"] {
        h.gateway.push(if method == "Translate" {
            Reply::without_usage(r#"["Bonjour"]"#)
        } else {
            Reply::without_usage(&extracted(json!({"name": "Ada", "age": 36})))
        });
        let (wire, _) = h
            .call_slow(method, USER_TOKEN, body_for(method), SLOW, WATCHDOG)
            .await;
        assert!(wire.ok(), "{method}: {:?}", wire.body);
    }
    assert_eq!(h.gateway.calls(), 2);
}

/// Asserted caller deadlines: absent, short, long and malformed. None of them may select a
/// server deadline.
const CALLER_DEADLINES: [Option<&str>; 5] =
    [None, Some("1"), Some("40"), Some("garbage"), Some("10000")];
const ENCODINGS: [Encoding; 2] = [Encoding::Json, Encoding::Proto];

fn delayed_success(method: &str) -> Reply {
    let content = if method == "Translate" {
        r#"["Bonjour"]"#.to_owned()
    } else {
        extracted(json!({"name": "Ada", "age": 36}))
    };
    Reply::Delay(120, Box::new(Reply::without_usage(&content)))
}

async fn wait_for_dropped(h: &Harness, expected: usize) {
    for _ in 0..100 {
        if h.gateway.dropped() == expected {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

async fn wait_for_permit_release(h: &Harness, context: &str) {
    for _ in 0..100 {
        if h.server.in_flight(TENANT) == 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the tenant permit was not released: {context}");
}

#[tokio::test]
async fn a_caller_deadline_header_does_not_end_a_connected_call() {
    for method in ["Translate", "Extract"] {
        for encoding in ENCODINGS {
            for header in CALLER_DEADLINES {
                let context = format!("{method} {encoding:?} {header:?}");
                let h = Harness::start_with(None, |c| {
                    c.limits.execution_timeout = std::time::Duration::from_secs(1)
                })
                .await;
                h.gateway.push(delayed_success(method));
                let wire = h
                    .call_with(method, USER_TOKEN, &body_for(method), encoding, header)
                    .await;
                assert!(wire.ok(), "{context}: {} {}", wire.status, wire.body);
                assert_eq!(wire.status, 200, "{context}");
                assert!(
                    wire.body["requestId"]
                        .as_str()
                        .is_some_and(|id| !id.is_empty()),
                    "{context}"
                );
                if method == "Translate" {
                    assert_eq!(wire.body["translations"], json!(["Bonjour"]), "{context}");
                } else {
                    assert!(
                        wire.body["json"]
                            .as_str()
                            .is_some_and(|d| d.contains("Ada")),
                        "{context}: {}",
                        wire.body
                    );
                }
                assert_eq!(h.gateway.calls(), 1, "{context}");
                assert_eq!(h.gateway.dropped(), 0, "{context}");
                wait_for_permit_release(&h, &context).await;
            }
        }
    }
}

#[tokio::test]
async fn the_server_budget_wins_over_every_caller_deadline_header() {
    for method in ["Translate", "Extract"] {
        for encoding in ENCODINGS {
            for header in CALLER_DEADLINES {
                let context = format!("{method} {encoding:?} {header:?}");
                let h = Harness::start_with(None, |c| {
                    c.limits.execution_timeout = std::time::Duration::from_millis(300)
                })
                .await;
                h.gateway.push(Reply::Hang);
                h.gateway.push(Reply::without_usage(r#"["never"]"#));
                let started = tokio::time::Instant::now();
                let wire = h
                    .call_with(method, USER_TOKEN, &body_for(method), encoding, header)
                    .await;
                let elapsed = started.elapsed();
                assert_eq!(wire.status, 504, "{context}");
                assert_eq!(wire.code(), "deadline_exceeded", "{context}");
                assert_eq!(wire.reason(), "ERROR_REASON_TIMEOUT", "{context}");
                let (_, detail_request_id) = wire.detail();
                assert_eq!(
                    wire.request_id_header.as_deref(),
                    Some(detail_request_id.as_str()),
                    "{context}"
                );
                assert!(
                    elapsed >= std::time::Duration::from_millis(250),
                    "{context}: answered early at {elapsed:?}"
                );
                wait_for_dropped(&h, 1).await;
                assert_eq!(h.gateway.dropped(), 1, "{context}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                assert_eq!(h.gateway.calls(), 1, "{context}: no retry after expiry");
                wait_for_permit_release(&h, &context).await;
            }
        }
    }
}

#[tokio::test]
async fn an_actual_disconnect_still_cancels_with_any_caller_deadline_header() {
    for method in ["Translate", "Extract"] {
        for encoding in ENCODINGS {
            for header in [None, Some("10000"), Some("garbage")] {
                let context = format!("{method} {encoding:?} {header:?}");
                let h = Harness::start(None).await;
                h.gateway.push(Reply::Hang);
                h.gateway.push(Reply::without_usage(r#"["never"]"#));
                let pending = tokio::spawn(
                    h.request_with(method, USER_TOKEN, &body_for(method), encoding, header)
                        .send(),
                );
                for _ in 0..100 {
                    if h.gateway.calls() == 1 {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                assert_eq!(h.gateway.calls(), 1, "{context}");
                pending.abort();
                let _ = pending.await;
                wait_for_dropped(&h, 1).await;
                assert_eq!(h.gateway.dropped(), 1, "{context}: provider call canceled");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                assert_eq!(
                    h.gateway.calls(),
                    1,
                    "{context}: no retry after cancellation"
                );
                wait_for_permit_release(&h, &context).await;
            }
        }
    }
}

#[tokio::test]
async fn origin_and_message_size_rejection_ignore_the_caller_deadline_header() {
    let h = Harness::start(None).await;
    let url = format!("{}/delitools.v1.DelitoolsService/Translate", h.base);
    let response = reqwest::Client::new()
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Connect-Protocol-Version", "1")
        .header("Connect-Timeout-Ms", "40")
        .header("Origin", "https://example.test")
        .bearer_auth(USER_TOKEN)
        .json(&translate_body(&["a"]))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let oversized = "a".repeat(400_000);
    for encoding in ENCODINGS {
        let wire = h
            .call_with(
                "Translate",
                USER_TOKEN,
                &translate_body(&[&oversized]),
                encoding,
                Some("40"),
            )
            .await;
        assert_eq!(wire.code(), "resource_exhausted", "{encoding:?}");
        assert!(
            wire.body["details"].is_null(),
            "{encoding:?}: no ErrorDetail"
        );
    }
    assert_eq!(h.gateway.calls(), 0);
}

// ---------------------------------------------------------------------------------------------
// Request decoding failures: the dependency decodes the body before any handler runs, and its
// diagnostics can echo submitted values. The router replaces them with a fixed typed error.
// ---------------------------------------------------------------------------------------------

const DECODE_MESSAGE: &str = "request body is invalid";

/// A synthetic body that fails to decode, with the marker it must never reveal.
struct BadBody {
    name: &'static str,
    method: &'static str,
    marker: &'static str,
    body: Vec<u8>,
}

fn bad_bodies() -> Vec<BadBody> {
    let json = |value: serde_json::Value| serde_json::to_vec(&value).unwrap();
    vec![
        BadBody {
            name: "scalar texts",
            method: "Translate",
            marker: "PRIVATE-CONTENT-MARKER",
            body: json(
                json!({"texts": "PRIVATE-CONTENT-MARKER", "targetLanguage": "en", "model": "m"}),
            ),
        },
        BadBody {
            name: "scalar texts with tenant",
            method: "Translate",
            marker: "PRIVATE-SECOND-MARKER",
            body: json(translate_body_with_texts(json!("PRIVATE-SECOND-MARKER"))),
        },
        BadBody {
            name: "object item",
            method: "Translate",
            marker: "PRIVATE-ITEM-MARKER",
            body: json(translate_body_with_texts(
                json!(["safe", {"privateField": "PRIVATE-ITEM-MARKER"}]),
            )),
        },
        BadBody {
            name: "numeric target language",
            method: "Translate",
            marker: "12345",
            body: json(json!({
                "tenantId": TENANT, "texts": ["safe"], "targetLanguage": 12345, "model": MODEL
            })),
        },
        BadBody {
            name: "object model",
            method: "Translate",
            marker: "PRIVATE-MODEL-MARKER",
            body: json(json!({
                "tenantId": TENANT, "texts": ["safe"], "targetLanguage": "fr",
                "model": {"privateField": "PRIVATE-MODEL-MARKER"}
            })),
        },
        BadBody {
            name: "extract text array",
            method: "Extract",
            marker: "PRIVATE-EXTRACT-MARKER",
            body: json(json!({
                "tenantId": TENANT, "text": ["PRIVATE-EXTRACT-MARKER"], "jsonSchema": PERSON_SCHEMA
            })),
        },
        BadBody {
            name: "truncated json",
            method: "Translate",
            marker: "PRIVATE-TRUNCATED-MARKER",
            body: br#"{"tenantId":"x","texts":["PRIVATE-TRUNCATED-MARKER"#.to_vec(),
        },
        BadBody {
            name: "syntax error",
            method: "Extract",
            marker: "PRIVATE-SYNTAX-MARKER",
            body: br#"{"text": PRIVATE-SYNTAX-MARKER, "jsonSchema": }"#.to_vec(),
        },
    ]
}

fn translate_body_with_texts(texts: serde_json::Value) -> serde_json::Value {
    json!({"tenantId": TENANT, "texts": texts, "targetLanguage": "fr", "model": MODEL})
}

fn assert_no_business_calls(h: &Harness) {
    assert_eq!(h.gateway.calls(), 0);
    let delcore = h.delcore.lock().unwrap();
    assert_eq!(delcore.token_calls, 0, "authentication ran");
    assert_eq!(delcore.key_calls, 0, "authentication ran");
    assert!(delcore.authorize_calls.is_empty(), "authorization ran");
    assert!(delcore.ingested.is_empty());
}

#[tokio::test]
async fn undecodable_json_requests_get_a_fixed_typed_error_before_authentication() {
    let logs = LogCapture::start();
    let h = Harness::start(None).await;
    let mut count = 0;
    for case in bad_bodies() {
        for token in [None, Some(USER_TOKEN), Some(KEY_SECRET), Some("garbage")] {
            let wire = h
                .post(
                    case.method,
                    token,
                    "application/json",
                    case.body.clone(),
                    None,
                )
                .await;
            let context = format!("{} / {token:?}: {}", case.name, wire.raw);
            assert_eq!(wire.status, 400, "{context}");
            assert_eq!(wire.code(), "invalid_argument", "{context}");
            assert_eq!(wire.body["message"], DECODE_MESSAGE, "{context}");
            assert_eq!(wire.reason(), "ERROR_REASON_INVALID_INPUT", "{context}");
            let header = wire.request_id_header.clone().expect(&context);
            assert_eq!(header, wire.detail().1, "{context}");
            assert_eq!(
                uuid::Uuid::parse_str(&header).unwrap().get_version_num(),
                7,
                "{context}"
            );
            assert!(!wire.raw.contains(case.marker), "{context}");
            assert!(
                wire.body
                    .as_object()
                    .unwrap()
                    .keys()
                    .all(|key| matches!(key.as_str(), "code" | "message" | "details")),
                "{context}"
            );
            count += 1;
        }
    }
    assert_no_business_calls(&h);
    // Each rejection is counted as a denied call with the invalid-input reason.
    let metrics = h.server.metrics.encode();
    assert!(
        metrics.contains(&format!(
            "delitools_denied_total{{reason=\"invalid_input\"}} {count}"
        )),
        "{metrics}"
    );
    // The rejection is logged with a bounded category and no submitted value or decoder text.
    let logged = logs.text();
    assert!(
        logged.contains("request body could not be decoded"),
        "{logged}"
    );
    for case in bad_bodies() {
        assert!(!logged.contains(case.marker), "{}: {logged}", case.name);
    }
    for diagnostic in [
        "invalid type",
        "expected a sequence",
        "EOF while",
        "at line",
    ] {
        assert!(!logged.contains(diagnostic), "{diagnostic}: {logged}");
    }
}

#[tokio::test]
async fn correctly_decoded_requests_keep_authentication_validation_and_success() {
    let h = Harness::start(None).await;
    // A string array in place of the scalar is decoded and then authenticated as usual.
    let wire = h
        .call(
            "Translate",
            None,
            translate_body_with_texts(json!(["PRIVATE-CONTENT-MARKER"])),
        )
        .await;
    assert_eq!(wire.status, 401);
    assert_eq!(wire.reason(), "ERROR_REASON_UNAUTHENTICATED");
    assert_eq!(wire.body["message"], "authentication failed");
    assert_eq!(h.gateway.calls(), 0);
    // Handler validation keeps its own typed errors and messages.
    let wire = h
        .translate(USER_TOKEN, translate_body_with_texts(json!([])))
        .await;
    assert_eq!(wire.status, 400);
    assert_eq!(wire.reason(), "ERROR_REASON_INVALID_INPUT");
    assert_ne!(wire.body["message"], DECODE_MESSAGE);
    let mut body = translate_body(&["a"]);
    body["model"] = json!("vendor/not-listed");
    let wire = h.translate(USER_TOKEN, body).await;
    assert_eq!(wire.status, 400);
    assert_eq!(wire.reason(), "ERROR_REASON_MODEL_UNAVAILABLE");
    // Valid requests still succeed.
    h.gateway
        .push(Reply::without_usage(r#"{"translations":["Bonjour"]}"#));
    let wire = h.translate(USER_TOKEN, translate_body(&["Hello"])).await;
    assert!(wire.ok(), "{}", wire.raw);
    h.gateway
        .push(Reply::without_usage(&extracted(json!({"name": "Ada"}))));
    let wire = h.extract(USER_TOKEN, extract_body()).await;
    assert!(wire.ok(), "{}", wire.raw);
    assert_eq!(h.gateway.calls(), 2);
    // Handler failures are not counted as decode failures.
    let metrics = h.server.metrics.encode();
    assert!(metrics.contains("delitools_denied_total{reason=\"invalid_input\"} 1"));
}

#[tokio::test]
async fn binary_requests_keep_their_behavior() {
    use buffa::Message;
    use delino_protocol::delitools::v1::{TranslateRequest, TranslateResponse};
    let h = Harness::start(None).await;
    let request = TranslateRequest {
        tenant_id: TENANT.into(),
        texts: vec!["Hello".into()],
        target_language: "fr".into(),
        model: MODEL.into(),
        ..Default::default()
    };
    h.gateway
        .push(Reply::without_usage(r#"{"translations":["Bonjour"]}"#));
    let wire = h
        .post(
            "Translate",
            Some(USER_TOKEN),
            "application/proto",
            request.encode_to_vec(),
            None,
        )
        .await;
    // The success body is binary protobuf, not JSON.
    assert_eq!(wire.status, 200);
    assert!(wire.request_id_header.is_some());
    assert_eq!(wire.body, serde_json::Value::Null);
    let reply = TranslateResponse::decode_from_slice(&wire.bytes).unwrap();
    assert_eq!(reply.translations, vec!["Bonjour".to_owned()]);
    assert_eq!(
        wire.request_id_header.as_deref(),
        Some(reply.request_id.as_str())
    );
    // Typed handler errors of binary calls keep their detail.
    let invalid = TranslateRequest {
        tenant_id: TENANT.into(),
        ..Default::default()
    };
    let wire = h
        .post(
            "Translate",
            Some(USER_TOKEN),
            "application/proto",
            invalid.encode_to_vec(),
            None,
        )
        .await;
    assert_eq!(wire.status, 400);
    assert_eq!(wire.reason(), "ERROR_REASON_INVALID_INPUT");
    assert_ne!(wire.body["message"], DECODE_MESSAGE);
    // A malformed binary body is not rewritten: it carries no DeliTools detail.
    let wire = h
        .post(
            "Translate",
            Some(USER_TOKEN),
            "application/proto",
            vec![0xff, 0xff, 0xff],
            None,
        )
        .await;
    assert_eq!(wire.status, 400);
    assert_eq!(wire.code(), "invalid_argument");
    assert!(wire.body.get("details").is_none(), "{}", wire.raw);
    assert!(wire.request_id_header.is_none());
    assert_eq!(h.gateway.calls(), 1);
}

#[tokio::test]
async fn oversize_bodies_keep_the_plain_untyped_rejection() {
    let logs = LogCapture::start();
    let h = Harness::start(None).await;
    // The default configuration allows (4096 + 2048) * 6 + 64 KiB = 102,400 bytes.
    let text = format!("PRIVATE-OVERSIZE-MARKER{}", "a".repeat(102_468));
    let body = serde_json::to_vec(&translate_body_with_texts(json!([text]))).unwrap();
    assert!(body.len() > 102_400);
    let wire = h
        .post(
            "Translate",
            Some(USER_TOKEN),
            "application/json",
            body,
            None,
        )
        .await;
    assert_eq!(wire.code(), "resource_exhausted", "{}", wire.raw);
    assert_ne!(wire.status, 200);
    assert!(wire.body.get("details").is_none(), "{}", wire.raw);
    assert!(wire.request_id_header.is_none());
    assert!(!wire.raw.contains("PRIVATE-OVERSIZE-MARKER"));
    assert!(!logs.text().contains("PRIVATE-OVERSIZE-MARKER"));
    assert_no_business_calls(&h);
}

#[tokio::test]
async fn origin_rejection_precedes_decoding() {
    let h = Harness::start(None).await;
    for case in bad_bodies() {
        let response = reqwest::Client::new()
            .post(format!(
                "{}/delitools.v1.DelitoolsService/{}",
                h.base, case.method
            ))
            .header("Content-Type", "application/json")
            .header("Origin", "https://evil.example")
            .body(case.body.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403, "{}", case.name);
        assert!(!response.text().await.unwrap().contains(case.marker));
    }
    assert_no_business_calls(&h);
}
