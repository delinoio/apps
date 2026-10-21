// SPDX-License-Identifier: FSL-1.1-ALv2
//! Retries, cancellation, deadlines and mid-execution revocation against the mocked Gateway.
//! These tests need a disposable PostgreSQL database in `TEST_DATABASE_URL`.
mod support;
use delitools_api::store::{self, recovery};
use serde_json::{Value, json};
use std::time::Duration;
use support::{
    Env, TRANSLATION_MODEL, echo_translations,
    mocks::{MemberRole, Script, UsageSpec, generation_data, reply},
    reason,
};

const TRANSLATE: &str = "TranslateTextArray";
const SERVICE: &str = "KEY_KIND_SERVICE";
const PERSONAL: &str = "KEY_KIND_PERSONAL";
const ALL: [&str; 2] = ["CAPABILITY_TRANSLATOR", "CAPABILITY_EXTRACTOR"];

fn usage() -> Option<UsageSpec> {
    Some(UsageSpec::full(100, 20, 30, 10))
}
fn good(n: usize, body: &Value) -> Script {
    reply(&echo_translations(body), &format!("gen_ok{n}"), usage())
}

/// Send a translation call from a background task so the test can act while it runs.
fn spawn_call(env: &Env, bearer: &str, body: Value) -> tokio::task::JoinHandle<(u16, Value)> {
    let (http, base, bearer) = (env.fx.http.clone(), env.fx.base.clone(), bearer.to_owned());
    tokio::spawn(async move {
        let response = http
            .post(format!("{base}/delitools.v1.DeliToolsService/{TRANSLATE}"))
            .header("connect-protocol-version", "1")
            .bearer_auth(bearer)
            .json(&body)
            .send()
            .await
            .unwrap();
        (response.status().as_u16(), response.json().await.unwrap())
    })
}

async fn request_state(env: &Env) -> (i16, i16) {
    sqlx::query_as("SELECT state, error_reason FROM requests ORDER BY created_at DESC LIMIT 1")
        .fetch_one(&env.fx.pool)
        .await
        .unwrap()
}

async fn wait_for_request_state(env: &Env, state: i16) -> (i16, i16) {
    for _ in 0..80 {
        let current = request_state(env).await;
        if current.0 == state {
            return current;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "request never reached state {state}: {:?}",
        request_state(env).await
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn transient_failures_retry_with_a_frozen_price_and_no_model_fallback() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    env.gateway.respond(|n, body| match n {
        // The slow first failure lets the test publish a newer price mid-request.
        0 => Script::Slow(Duration::from_millis(400), Box::new(Script::Status(503))),
        1 => Script::Status(429),
        2 => Script::StreamError {
            generation: "gen_err".into(),
        },
        _ => good(n, body),
    });
    let done = {
        let call = spawn_call(&env, &key, env.translate_body(&["x", "y"], None));
        tokio::time::sleep(Duration::from_millis(150)).await;
        env.set_price(
            TRANSLATION_MODEL,
            "anthropic",
            ("0.00001", "0.00002", None, None),
        )
        .await;
        call.await.unwrap()
    };
    // MAX_AI_RETRIES is 2 in the default environment: three attempts, the third also fails, so
    // the request exhausts its retries with the provider error.
    assert_eq!(done.0, 503, "{:?}", done.1);
    assert_eq!(done.1["code"], "unavailable");
    assert_eq!(env.gateway.chat_count(), 3);

    // Every attempt used the same model and the same frozen route; no fallback was requested.
    for chat in env.gateway.chats() {
        assert_eq!(chat.body["model"], TRANSLATION_MODEL);
        assert_eq!(
            chat.body["providerOptions"]["gateway"]["only"],
            json!(["anthropic"])
        );
        assert!(chat.body.get("models").is_none());
    }
    // The price published during the request does not affect its attempts.
    let prices: Vec<(String, String)> = sqlx::query_as(
        "SELECT catalog_price::text, price_effective_at::text FROM attempts ORDER BY created_at,id",
    )
    .fetch_all(&env.fx.pool)
    .await
    .unwrap();
    assert_eq!(prices.len(), 3);
    assert!(prices.iter().all(|p| p == &prices[0]), "{prices:?}");
    assert!(
        prices[0].0.contains("0.000003"),
        "frozen at the request start: {}",
        prices[0].0
    );
    let frozen: Option<uuid::Uuid> = sqlx::query_scalar("SELECT price_id FROM requests")
        .fetch_one(&env.fx.pool)
        .await
        .unwrap();
    assert!(frozen.is_some());
    let metrics = env.fx.platform.metrics.encode();
    assert!(
        metrics.contains("delitools_ai_retries_total 2"),
        "{metrics}"
    );
    // Authorization was repeated before each retry: one initial check plus two retry checks.
    let paid = env
        .delcore
        .authorize_calls()
        .into_iter()
        .filter(|c| c["paid"] == true)
        .count();
    assert_eq!(paid, 3);

    // A later request uses the newly effective price.
    env.gateway.respond(good);
    env.fx
        .ok(&key, TRANSLATE, env.translate_body(&["x"], None))
        .await;
    let latest: String = sqlx::query_scalar(
        "SELECT catalog_price::text FROM attempts ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert!(
        latest.contains("0.00001") && !latest.contains("0.000003"),
        "{latest}"
    );
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn retry_succeeds_after_transient_failures_and_bills_every_confirmed_attempt() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    env.gateway.respond(|n, body| match n {
        0 => Script::Truncated {
            generation: "gen_cut".into(),
        },
        1 => Script::Status(502),
        _ => good(n, body),
    });
    let reply = env
        .fx
        .ok(&key, TRANSLATE, env.translate_body(&["x"], None))
        .await;
    assert_eq!(reply.body["texts"], json!(["[ko] x"]));
    assert_eq!(env.gateway.chat_count(), 3);
    // Three attempts, three distinct safe user references.
    let users: std::collections::HashSet<String> = env
        .gateway
        .chats()
        .iter()
        .map(|c| c.body["user"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(users.len(), 3);
    // The truncated stream consumed tokens that are not confirmed (its generation is known and
    // is reconciled); the 502 is unknown without a generation; the final attempt is confirmed.
    let rows: Vec<(String, bool)> = sqlx::query_as(
        "SELECT generation_id, measured_usage IS NOT NULL FROM attempts ORDER BY created_at,id",
    )
    .fetch_all(&env.fx.pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            ("gen_cut".into(), false),
            (String::new(), false),
            ("gen_ok2".into(), true)
        ]
    );
    let events = env.wait_for_events(1).await;
    assert_eq!(events.len(), 1, "only the confirmed attempt is reported");
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn exhaustion_non_transient_errors_and_zero_retries() {
    let env = Env::start(&[("MAX_AI_RETRIES", "1")], false).await;
    let key = env.service_key().await;

    env.gateway.sequence(vec![Script::Status(503)]);
    let failed = env
        .fx
        .fails(
            Some(&key),
            TRANSLATE,
            env.translate_body(&["x"], None),
            "unavailable",
            reason::PROVIDER,
        )
        .await;
    assert!(
        !failed.body.to_string().contains("upstream body"),
        "provider bodies are never returned"
    );
    assert_eq!(env.gateway.chat_count(), 2, "one retry, then exhausted");
    // Both 5xx attempts may have consumed tokens: unknown usage stays pending, never estimated.
    let backlog = recovery::backlog(&env.fx.pool, store::now(), 3600)
        .await
        .unwrap();
    assert_eq!(backlog.unknown_usage, 2);
    assert_eq!(env.delcore.ingest_calls().len(), 0);

    for status in [400, 401, 403, 404] {
        let before = env.gateway.chat_count();
        env.gateway.sequence(vec![Script::Status(status)]);
        env.fx
            .fails(
                Some(&key),
                TRANSLATE,
                env.translate_body(&["x"], None),
                "unavailable",
                reason::PROVIDER,
            )
            .await;
        assert_eq!(
            env.gateway.chat_count(),
            before + 1,
            "status {status} is not retried"
        );
    }
    // A rejected call provably consumed nothing, so there is nothing to report.
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM attempts WHERE delivered_at IS NULL AND measured_usage IS NULL",
    )
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert_eq!(pending, 2);
    let metrics = env.fx.platform.metrics.encode();
    assert!(metrics.contains("delitools_ai_attempts_total{outcome=\"permanent\"} 4"));
    env.finish().await;

    let zero = Env::start(&[("MAX_AI_RETRIES", "0")], false).await;
    let key = zero.service_key().await;
    zero.gateway.sequence(vec![Script::Status(503)]);
    zero.fx
        .fails(
            Some(&key),
            TRANSLATE,
            zero.translate_body(&["x"], None),
            "unavailable",
            reason::PROVIDER,
        )
        .await;
    assert_eq!(
        zero.gateway.chat_count(),
        1,
        "an explicit zero disables retries"
    );
    zero.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn client_disconnect_cancels_the_call_stops_retries_and_usage_is_reconciled_later() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    env.gateway.sequence(vec![Script::Hang {
        generation: "gen_cancel".into(),
    }]);
    let call = spawn_call(&env, &key, env.translate_body(&["x"], None));
    for _ in 0..50 {
        if env.gateway.chat_count() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await; // the first chunk (generation) arrived
    call.abort();
    for _ in 0..60 {
        if env.gateway.disconnected() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        env.gateway.disconnected() >= 1,
        "the in-flight Gateway call was cancelled"
    );
    let (state, _) = wait_for_request_state(&env, 3).await;
    assert_eq!(state, 3, "an interrupted call has an unknown outcome");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        env.gateway.chat_count(),
        1,
        "no retry after the client left"
    );
    assert_eq!(env.fx.platform.admission.running(env.tenant), 0);
    let row: (String, bool, bool) = sqlx::query_as(
        "SELECT generation_id, measured_usage IS NOT NULL, delivered_at IS NOT NULL FROM attempts",
    )
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert_eq!(row, ("gen_cancel".into(), false, false));
    assert!(
        env.delcore.accepted().is_empty(),
        "unknown usage is never estimated"
    );

    // Reconciliation continues in the background: the Gateway lookup later confirms the usage.
    env.gateway.set_generation(
        "gen_cancel",
        Some(generation_data(
            "gen_cancel",
            TRANSLATION_MODEL,
            40,
            10,
            5,
            0,
        )),
    );
    env.make_due().await;
    env.fx
        .usage
        .as_ref()
        .unwrap()
        .run_reconciliation()
        .await
        .unwrap();
    let events = env.wait_for_events(1).await;
    let attributes = &events.values().next().unwrap()["event"]["attributes"];
    assert_eq!(attributes["input_tokens"]["number"], "35");
    assert_eq!(attributes["output_tokens"]["number"], "10");
    assert_eq!(attributes["cache_read_tokens"]["number"], "5");
    assert_eq!(attributes["cache_write_tokens"]["number"], "0");
    assert_eq!(env.gateway.generation_lookups(), vec!["gen_cancel"]);
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn the_execution_deadline_cancels_the_call_and_returns_a_timeout() {
    let env = Env::start(&[("EXECUTION_TIMEOUT_SECONDS", "1")], false).await;
    let key = env.service_key().await;
    env.gateway.sequence(vec![Script::Hang {
        generation: "gen_slow".into(),
    }]);
    let started = std::time::Instant::now();
    env.fx
        .fails(
            Some(&key),
            TRANSLATE,
            env.translate_body(&["x"], None),
            "deadline_exceeded",
            reason::TIMEOUT,
        )
        .await;
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    wait_for_request_state(&env, 3).await;
    for _ in 0..40 {
        if env.gateway.disconnected() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(env.gateway.disconnected() >= 1);
    assert_eq!(env.gateway.chat_count(), 1, "no retry after the deadline");
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn no_provider_call_starts_after_the_deadline_passed_during_reauthorization() {
    let env = Env::start(&[("EXECUTION_TIMEOUT_SECONDS", "1")], false).await;
    let key = env.service_key().await;
    // The first attempt fails transiently; the re-authorization before the retry then outlasts
    // the one-second deadline (the first paid check, before execution, answers at once).
    env.gateway.respond(|n, body| match n {
        0 => Script::Status(503),
        _ => good(n, body),
    });
    env.delcore
        .delay_paid_authorizations_after(1, Duration::from_millis(1300));
    env.fx
        .fails(
            Some(&key),
            TRANSLATE,
            env.translate_body(&["x"], None),
            "deadline_exceeded",
            reason::TIMEOUT,
        )
        .await;
    assert_eq!(
        env.gateway.chat_count(),
        1,
        "no provider call after the deadline"
    );
    let attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM attempts")
        .fetch_one(&env.fx.pool)
        .await
        .unwrap();
    assert_eq!(
        attempts, 1,
        "no attempt record for a call that never started"
    );
    // No call was in flight, so the outcome is definitive rather than unknown.
    let (state, error) = wait_for_request_state(&env, 2).await;
    assert_eq!((state, error as i32), (2, reason::TIMEOUT));
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn revoking_a_key_during_a_call_lets_it_finish_and_bills_it_but_rejects_new_requests() {
    let env = Env::start(&[], false).await;
    let (member, _) = env.user("member", Some(MemberRole::Member));
    let (personal, key_id) = env.create_key(&member, PERSONAL, &ALL).await;
    env.gateway
        .respond(|n, body| Script::Slow(Duration::from_millis(500), Box::new(good(n, body))));
    let call = spawn_call(&env, &personal, env.translate_body(&["x"], None));
    tokio::time::sleep(Duration::from_millis(200)).await;
    env.fx
        .ok(
            &member,
            "RevokeKey",
            json!({"tenantId": env.tenant.to_string(), "keyId": key_id}),
        )
        .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["texts"], json!(["[ko] x"]));
    // The finished call is billed.
    let events = env.wait_for_events(1).await;
    assert_eq!(events.len(), 1);
    // New requests with the revoked key are rejected.
    env.fx
        .fails(
            Some(&personal),
            TRANSLATE,
            env.translate_body(&["x"], None),
            "unauthenticated",
            reason::UNAUTHENTICATED,
        )
        .await;
    assert_eq!(env.gateway.chat_count(), 1);
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn revocation_permission_removal_and_billing_blocks_stop_the_next_retry() {
    for scenario in ["revoke", "permission", "billing"] {
        let env = Env::start(&[], false).await;
        let (owner, _) = env.user("owner", Some(MemberRole::Owner));
        let (member, member_id) = env.user("member", Some(MemberRole::Member));
        let (service, service_id) = env.create_key(&owner, SERVICE, &ALL).await;
        let (personal, _) = env.create_key(&member, PERSONAL, &ALL).await;
        env.gateway.respond(|n, body| match n {
            0 => Script::Slow(Duration::from_millis(500), Box::new(Script::Status(503))),
            _ => good(n, body),
        });
        let bearer = if scenario == "permission" {
            &personal
        } else {
            &service
        };
        let call = spawn_call(&env, bearer, env.translate_body(&["x"], None));
        tokio::time::sleep(Duration::from_millis(200)).await;
        match scenario {
            "revoke" => {
                env.fx
                    .ok(
                        &owner,
                        "RevokeKey",
                        json!({"tenantId": env.tenant.to_string(), "keyId": service_id}),
                    )
                    .await;
            }
            "permission" => env.delcore.remove_member(env.tenant, member_id),
            _ => env.delcore.set_billing_blocked(env.tenant, true),
        }
        let (status, body) = call.await.unwrap();
        let (code, expected) = match scenario {
            "revoke" => ("unauthenticated", reason::UNAUTHENTICATED),
            "permission" => ("permission_denied", reason::FORBIDDEN),
            _ => ("failed_precondition", reason::BILLING_BLOCKED),
        };
        assert_eq!(body["code"], code, "{scenario}: {status} {body}");
        assert_eq!(
            env.gateway.chat_count(),
            1,
            "{scenario}: no further AI attempt"
        );
        let _ = expected;
        // The attempt that already ran is kept (unknown usage, pending) and the request is final.
        let (state, error) = request_state(&env).await;
        assert_eq!((state, error as i32), (2, expected), "{scenario}");
        env.finish().await;
    }
}
