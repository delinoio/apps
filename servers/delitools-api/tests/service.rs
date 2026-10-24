// SPDX-License-Identifier: FSL-1.1-ALv2
//! Mocked-service tests: DeliTools over real HTTP against loopback mocks of Delcore (with the
//! Logto token endpoint) and the Vercel AI Gateway. None of them needs PostgreSQL: the cases here
//! reject before execution or run with unknown provider usage, so no outbox row is written.

mod common;

use common::*;
use delitools_api::delcore::IngestFailure;
use serde_json::{Value, json};
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

fn catalog_price(meter: &str, kind: Value) -> Value {
    json!({
        "id": "p", "serviceId": SERVICE_ID, "meter": meter, "unitPrice": "0.000002",
        "kind": kind, "active": true
    })
}

fn both_prices() -> Value {
    json!([
        catalog_price("gem_in", json!("PRICE_KIND_USAGE")),
        catalog_price("gem_out", json!(1)),
    ])
}

fn script_catalog(h: &Harness, pages: impl IntoIterator<Item = Value>) {
    h.delcore.lock().unwrap().catalog_script.extend(pages);
}

#[tokio::test]
async fn malformed_catalog_fields_are_dependency_failures_before_any_provider_call() {
    let wrong_types = [json!(42), json!(true), json!([]), json!({})];
    let mut bodies = Vec::new();
    for wrong in &wrong_types {
        // Both meters are priced, so only the malformed cursor can reject the request.
        bodies.push(json!({"prices": both_prices(), "nextAfterId": wrong}));
    }
    for wrong in [json!("broken"), json!(42), json!(true), json!({})] {
        bodies.push(json!({"prices": wrong, "nextAfterId": ""}));
    }
    for body in bodies {
        let h = Harness::start(None).await;
        script_catalog(&h, [body.clone()]);
        let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
        assert_eq!(
            wire.reason(),
            "ERROR_REASON_DEPENDENCY_UNAVAILABLE",
            "{body}"
        );
        assert_eq!(wire.code(), "unavailable", "{body}");
        assert_eq!(h.gateway.calls(), 0, "{body}");
    }
}

#[tokio::test]
async fn a_malformed_later_catalog_page_invalidates_the_whole_read() {
    for second in [
        json!({"prices": {}}),
        json!({"prices": [], "nextAfterId": 7}),
    ] {
        let h = Harness::start(None).await;
        script_catalog(
            &h,
            [
                json!({"prices": both_prices(), "nextAfterId": "019b0000-0000-7000-8000-0000000000f1"}),
                second.clone(),
            ],
        );
        let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
        assert_eq!(
            wire.reason(),
            "ERROR_REASON_DEPENDENCY_UNAVAILABLE",
            "{second}"
        );
        assert_eq!(h.gateway.calls(), 0, "{second}");
    }
}

#[tokio::test]
async fn a_malformed_catalog_read_is_not_cached() {
    // A long TTL makes a cached result observable: the retry must read the catalog again.
    let h = Harness::start_with(None, |c| {
        c.price_cache_ttl = std::time::Duration::from_secs(60);
    })
    .await;
    script_catalog(&h, [json!({"prices": "broken"})]);
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_DEPENDENCY_UNAVAILABLE");
    assert_eq!(h.delcore.lock().unwrap().catalog_calls, 1);
    h.gateway.push(Reply::without_usage(r#"["x"]"#));
    assert!(h.translate(USER_TOKEN, translate_body(&["a"])).await.ok());
    assert_eq!(h.delcore.lock().unwrap().catalog_calls, 2);
    assert_eq!(h.gateway.calls(), 1);
}

#[tokio::test]
async fn omitted_null_and_empty_catalog_fields_are_protojson_defaults() {
    for body in [
        json!({}),
        json!({"prices": null}),
        json!({"prices": [], "nextAfterId": null}),
        json!({"prices": [], "nextAfterId": ""}),
    ] {
        let h = Harness::start(None).await;
        script_catalog(&h, [body.clone()]);
        let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
        assert_eq!(wire.reason(), "ERROR_REASON_MODEL_UNAVAILABLE", "{body}");
        assert_eq!(h.gateway.calls(), 0, "{body}");
    }
    for cursor in [None, Some(Value::Null), Some(json!(""))] {
        let h = Harness::start(None).await;
        let mut page = json!({"prices": both_prices()});
        if let Some(cursor) = cursor.clone() {
            page["nextAfterId"] = cursor;
        }
        script_catalog(&h, [page]);
        h.gateway.push(Reply::without_usage(r#"["x"]"#));
        let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
        assert!(wire.ok(), "{cursor:?}");
        assert_eq!(h.delcore.lock().unwrap().catalog_calls, 1);
    }
}

#[tokio::test]
async fn repeated_catalog_cursors_remain_dependency_failures() {
    let h = Harness::start(None).await;
    let cursor = "019b0000-0000-7000-8000-0000000000f1";
    script_catalog(
        &h,
        [
            json!({"prices": both_prices(), "nextAfterId": cursor}),
            json!({"prices": [], "nextAfterId": cursor}),
        ],
    );
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_DEPENDENCY_UNAVAILABLE");
    assert_eq!(h.gateway.calls(), 0);
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

// Gateway attempt accounting: `canceled` and `timeout` count only a provider call that had
// started, never a stop during retry backoff or the authorization recheck.

#[derive(Clone, Copy, Debug)]
enum StopKind {
    /// The caller disconnects, which cancels the execution.
    Cancel,
    /// The execution deadline expires.
    Timeout,
}

const STOP_KINDS: [StopKind; 2] = [StopKind::Cancel, StopKind::Timeout];

fn attempts(h: &Harness, outcome: &str) -> u64 {
    h.server
        .metrics
        .gateway_attempts
        .with_label_values(&[outcome])
        .get()
}

/// Polls `condition` for up to five seconds.
async fn eventually(what: &str, condition: impl Fn() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Sends a Translate request and ends it with `stop` once `ready` holds. A timeout ends by the
/// configured execution deadline, so the caller waits for the response; a cancellation aborts the
/// caller and waits until the execution task released its tenant permit.
async fn translate_then_stop(h: &Harness, stop: StopKind, ready: impl Fn() -> bool) {
    let request = h.call_future("Translate", USER_TOKEN, translate_body(&["a"]));
    match stop {
        StopKind::Timeout => {
            let response = request.await.unwrap();
            assert_eq!(response.status().as_u16(), 504);
            let body: serde_json::Value = response.json().await.unwrap();
            assert_eq!(body["code"], "deadline_exceeded");
        }
        StopKind::Cancel => {
            let pending = tokio::spawn(request);
            eventually("the stop condition", ready).await;
            pending.abort();
            let _ = pending.await;
        }
    }
    eventually("the tenant permit release", || {
        h.server.in_flight(TENANT) == 0
    })
    .await;
    // Allow a late metric increment to surface before the caller asserts on deltas.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
}

fn stop_config(stop: StopKind, c: &mut delitools_api::config::Config) {
    if let StopKind::Timeout = stop {
        c.limits.execution_timeout = std::time::Duration::from_millis(600);
    }
}

#[tokio::test]
async fn a_stop_during_retry_backoff_is_not_a_gateway_attempt() {
    for stop in STOP_KINDS {
        let h = Harness::start_with(None, |c| {
            stop_config(stop, c);
            // The minimum jittered delay (half of 10s) outlasts the stop window.
            c.retry.base_delay = std::time::Duration::from_secs(10);
            c.retry.max_delay = std::time::Duration::from_secs(10);
        })
        .await;
        h.gateway.push(Reply::Status(503));
        h.gateway.push(Reply::without_usage(r#"["never"]"#));
        translate_then_stop(&h, stop, || attempts(&h, "transient") == 1).await;
        assert_eq!(h.gateway.calls(), 1, "{stop:?}: no second provider call");
        assert_eq!(attempts(&h, "transient"), 1, "{stop:?}");
        assert_eq!(attempts(&h, "canceled"), 0, "{stop:?}");
        assert_eq!(attempts(&h, "timeout"), 0, "{stop:?}");
        assert_eq!(attempts(&h, "ok"), 0, "{stop:?}");
    }
}

#[tokio::test]
async fn a_stop_during_the_authorization_recheck_is_not_a_gateway_attempt() {
    for stop in STOP_KINDS {
        let h = Harness::start_with(None, |c| stop_config(stop, c)).await;
        // The admission check passes; the recheck before the second attempt is held open.
        let gate = AuthorizeGate::after(1);
        h.delcore.lock().unwrap().authorize_gate = Some(gate.clone());
        h.gateway.push(Reply::Status(503));
        h.gateway.push(Reply::without_usage(r#"["never"]"#));
        translate_then_stop(&h, stop, || {
            gate.entered.load(std::sync::atomic::Ordering::SeqCst) > 0
        })
        .await;
        gate.wait_entered().await;
        gate.open();
        assert_eq!(h.gateway.calls(), 1, "{stop:?}: no second provider call");
        assert_eq!(attempts(&h, "transient"), 1, "{stop:?}");
        assert_eq!(attempts(&h, "canceled"), 0, "{stop:?}");
        assert_eq!(attempts(&h, "timeout"), 0, "{stop:?}");
    }
}

#[tokio::test]
async fn a_stop_during_a_pending_provider_call_is_one_gateway_attempt() {
    for stop in STOP_KINDS {
        let h = Harness::start_with(None, |c| stop_config(stop, c)).await;
        h.gateway.push(Reply::Hang);
        h.gateway.push(Reply::without_usage(r#"["never"]"#));
        translate_then_stop(&h, stop, || h.gateway.calls() == 1).await;
        let (matching, other) = match stop {
            StopKind::Cancel => ("canceled", "timeout"),
            StopKind::Timeout => ("timeout", "canceled"),
        };
        assert_eq!(attempts(&h, matching), 1, "{stop:?}");
        assert_eq!(attempts(&h, other), 0, "{stop:?}");
        assert_eq!(attempts(&h, "transient"), 0, "{stop:?}");
        assert_eq!(h.gateway.calls(), 1, "{stop:?}: no retry");
        assert_eq!(
            h.gateway.dropped(),
            1,
            "{stop:?}: the provider call is dropped"
        );
    }
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
async fn completed_attempts_are_still_counted_once() {
    // A single 503 without retries.
    let h = Harness::start_with(None, |c| c.retry.max_retries = 0).await;
    h.gateway.push(Reply::Status(503));
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_PROVIDER_FAILED");
    assert_eq!(attempts(&h, "transient"), 1);
    assert_eq!(attempts(&h, "canceled") + attempts(&h, "timeout"), 0);

    // A 503 followed by a valid answer.
    let h = Harness::start(None).await;
    h.gateway.push(Reply::Status(503));
    h.gateway.push(Reply::without_usage(r#"["x"]"#));
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert!(wire.ok());
    assert_eq!(attempts(&h, "transient"), 1);
    assert_eq!(attempts(&h, "ok"), 1);
    assert_eq!(attempts(&h, "canceled") + attempts(&h, "timeout"), 0);

    // A 503 followed by a denied recheck.
    let h = Harness::start(None).await;
    let delcore = h.delcore.clone();
    h.gateway.push(Reply::Hook(
        std::sync::Arc::new(move || delcore.lock().unwrap().authorize = deny_permission()),
        Box::new(Reply::Status(503)),
    ));
    h.gateway.push(Reply::without_usage(r#"["never"]"#));
    let wire = h.translate(USER_TOKEN, translate_body(&["a"])).await;
    assert_eq!(wire.reason(), "ERROR_REASON_PERMISSION_DENIED");
    assert_eq!(h.gateway.calls(), 1);
    assert_eq!(attempts(&h, "transient"), 1);
    assert_eq!(
        attempts(&h, "ok") + attempts(&h, "canceled") + attempts(&h, "timeout"),
        0
    );
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
