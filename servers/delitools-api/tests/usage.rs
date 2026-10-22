// SPDX-License-Identifier: FSL-1.1-ALv2
//! Pricing snapshots, usage delivery to delcore, Gateway reconciliation, operator association,
//! catalog synchronization and 30-day expiry. These tests need a disposable PostgreSQL database
//! in `TEST_DATABASE_URL`.
mod support;
use delitools_api::store::{self, retention};
use serde_json::{Value, json};
use std::{sync::atomic::Ordering, time::Duration};
use support::{
    EXTRACTION_MODEL, Env, INVOICE_SCHEMA, TRANSLATION_MODEL, echo_translations,
    extraction_content, mocks,
    mocks::{MemberRole, Script, StreamReply, UsageSpec, generation_data, reply},
    reason,
};

const TRANSLATE: &str = "TranslateTextArray";
const EXTRACT: &str = "ExtractDataFromText";
/// `SERVICE_ID` of `.env.example`.
const SERVICE_ID: &str = "01900000-0000-7000-8000-000000000001";

fn translating(env: &Env, usage: Option<UsageSpec>) {
    env.gateway.respond(move |n, body| {
        reply(
            &echo_translations(body),
            &format!("gen_t{n}"),
            usage.clone(),
        )
    });
}

async fn translate_ok(env: &Env, key: &str) -> String {
    env.fx
        .ok(key, TRANSLATE, env.translate_body(&["x"], None))
        .await
        .body["requestId"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn usage_service(env: &Env) -> std::sync::Arc<delitools_api::usage::UsageService> {
    env.fx.usage.clone().expect("execution mode")
}

/// Exact integer charge basis at 1e-15 USD from the event's raw tokens and prices.
fn basis(attributes: &Value) -> u128 {
    let scaled = |price: &str| -> u128 {
        let (whole, fraction) = price.split_once('.').unwrap_or((price, ""));
        format!("{whole}{fraction:0<15}").parse().unwrap()
    };
    ["input", "output", "cache_read", "cache_write"]
        .iter()
        .map(|category| {
            let tokens = &attributes[format!("{category}_tokens")];
            let price = &attributes[format!("{category}_usd_per_token")];
            match (tokens["number"].as_str(), price["number"].as_str()) {
                (Some(t), Some(p)) => t.parse::<u128>().unwrap() * scaled(p),
                _ => 0,
            }
        })
        .sum()
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn usage_events_carry_raw_tokens_and_frozen_prices_never_a_charge() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    // Translator: input 1000 + cache read 50 + cache write 10 = 1060 prompt tokens.
    translating(&env, Some(UsageSpec::full(1060, 200, 50, 10)));
    let request_id = translate_ok(&env, &key).await;
    let events = env.wait_for_events(1).await;
    let (attempt, row): (String, (String, String)) = sqlx::query_as(
        "SELECT id::text, ROW(to_char(occurred_at AT TIME ZONE 'utc','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'), to_char(price_effective_at AT TIME ZONE 'utc','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'))::text FROM attempts",
    )
    .fetch_one(&env.fx.pool)
    .await
    .map(|(id, row): (String, String)| {
        let trimmed = row.trim_matches(['(', ')']).replace('"', "");
        let (a, b) = trimmed.split_once(',').unwrap();
        (id, (a.to_owned(), b.to_owned()))
    })
    .unwrap();
    let body = &events[&attempt];
    // The durable attempt UUID is the request key and the event key.
    assert_eq!(body["requestKey"], attempt);
    assert_eq!(body["event"]["eventKey"], attempt);
    assert_ne!(attempt, request_id, "attempts have their own identity");
    assert_eq!(body["event"]["serviceId"], SERVICE_ID);
    assert_eq!(body["event"]["tenantId"], env.tenant.to_string());
    assert_eq!(body["event"]["eventType"], "translator.usage");
    assert_eq!(body["event"]["occurredAt"], row.0);
    let attributes = &body["event"]["attributes"];
    assert_eq!(attributes["model"], json!({"text": TRANSLATION_MODEL}));
    assert_eq!(attributes["price_effective_at"], json!({"text": row.1}));
    assert_eq!(
        *attributes,
        json!({
            "model": {"text": TRANSLATION_MODEL},
            "price_effective_at": {"text": row.1},
            "input_tokens": {"number": "1000"}, "input_usd_per_token": {"number": "0.000003"},
            "output_tokens": {"number": "200"}, "output_usd_per_token": {"number": "0.000015"},
            "cache_read_tokens": {"number": "50"}, "cache_read_usd_per_token": {"number": "0.0000003"},
            "cache_write_tokens": {"number": "10"}, "cache_write_usd_per_token": {"number": "0.00000375"},
        })
    );
    // Delcore applies the markup; the report is sufficient input for exactly 1.15 and 1.30.
    // 1000*3e-6 + 200*1.5e-5 + 50*3e-7 + 10*3.75e-6 = 0.003 + 0.003 + 0.000015 + 0.0000375 USD.
    let translator_basis = basis(attributes);
    assert_eq!(translator_basis, 6_052_500_000_000);
    assert_eq!(translator_basis * 115 / 100, 6_960_375_000_000);
    assert_eq!(translator_basis * 130 / 100, 7_868_250_000_000);
    assert!(
        env.delcore
            .token_scopes()
            .contains(&"usage:write".to_owned())
    );
    assert!(
        env.delcore
            .ingest_authorizations()
            .iter()
            .all(|a| a == "m2m-usage:write")
    );

    // Extractor: its own event type and fixed model.
    env.gateway.respond(|n, _| {
        reply(
            &extraction_content(
                "extracted",
                json!({"vendor":"A","total":1,"currency":"USD"}),
            ),
            &format!("gen_e{n}"),
            Some(UsageSpec::full(500, 100, 0, 0)),
        )
    });
    env.fx
        .ok(
            &key,
            EXTRACT,
            env.extract_body("text", INVOICE_SCHEMA, None),
        )
        .await;
    let events = env.wait_for_events(2).await;
    let extractor = events
        .values()
        .find(|e| e["event"]["eventType"] == "extractor.usage")
        .expect("extractor event");
    let attributes = &extractor["event"]["attributes"];
    assert_eq!(attributes["model"], json!({"text": EXTRACTION_MODEL}));
    assert_eq!(attributes["input_tokens"]["number"], "500");
    assert_eq!(
        attributes["cache_read_tokens"]["number"], "0",
        "published categories are reported even at zero"
    );
    // 500*1e-7 + 100*5e-7 = 1e-4 USD.
    assert_eq!(basis(attributes), 100_000_000_000);
    assert_eq!(basis(attributes) * 130 / 100, 130_000_000_000);
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn categories_without_a_published_price_are_omitted_or_keep_usage_pending() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    // A newer catalog version publishes no cache prices for the model.
    env.set_price(
        TRANSLATION_MODEL,
        "anthropic",
        ("0.000003", "0.000015", None, None),
    )
    .await;

    translating(&env, Some(UsageSpec::full(100, 20, 0, 0)));
    translate_ok(&env, &key).await;
    let events = env.wait_for_events(1).await;
    let attributes = &events.values().next().unwrap()["event"]["attributes"];
    for omitted in [
        "cache_read_tokens",
        "cache_read_usd_per_token",
        "cache_write_tokens",
        "cache_write_usd_per_token",
    ] {
        assert!(attributes.get(omitted).is_none(), "{omitted}");
    }

    // Cache reads that need an unpublished price are never reported or estimated.
    translating(&env, Some(UsageSpec::full(100, 20, 40, 0)));
    translate_ok(&env, &key).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(env.delcore.accepted().len(), 1);
    env.make_due().await;
    usage_service(&env).run_deliveries().await.unwrap();
    assert_eq!(env.delcore.accepted().len(), 1);
    let (known, delivered, failures): (bool, bool, i32) = sqlx::query_as(
        "SELECT measured_usage IS NOT NULL, delivered_at IS NOT NULL, delivery_failures FROM attempts ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert!(known && !delivered && failures >= 1);
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn delivery_retries_with_capped_backoff_and_a_duplicate_never_creates_a_second_event() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    translating(&env, Some(UsageSpec::full(100, 20, 30, 10)));
    env.delcore.fail_ingests(3);
    translate_ok(&env, &key).await;
    // The immediate delivery failed once.
    for _ in 0..50 {
        if env.delcore.ingest_calls().len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let state = |env: &Env| {
        let pool = env.fx.pool.clone();
        async move {
            let (failures, delay, delivered) = sqlx::query_as::<_, (i32, Option<f64>, bool)>(
                "SELECT delivery_failures, EXTRACT(EPOCH FROM (retry_at - updated_at))::float8, delivered_at IS NOT NULL FROM attempts",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            (failures, delay.unwrap_or(0.0), delivered)
        }
    };
    let (failures, delay, delivered) = state(&env).await;
    assert_eq!((failures, delivered), (1, false));
    assert!((0.9..=1.1).contains(&delay), "initial retry delay {delay}");
    // Not yet due: the background run does nothing.
    usage_service(&env).run_deliveries().await.unwrap();
    assert_eq!(env.delcore.ingest_calls().len(), 1);

    env.make_due().await;
    usage_service(&env).run_deliveries().await.unwrap();
    let (failures, delay, _) = state(&env).await;
    assert_eq!(failures, 2);
    assert!((1.9..=2.1).contains(&delay), "exponential delay {delay}");
    env.make_due().await;
    usage_service(&env).run_deliveries().await.unwrap();
    let (failures, delay, _) = state(&env).await;
    assert_eq!(failures, 3);
    assert!(
        (3.9..=4.1).contains(&delay),
        "delay capped at MAX_BACKOFF_SECONDS: {delay}"
    );

    env.make_due().await;
    usage_service(&env).run_deliveries().await.unwrap();
    let (_, _, delivered) = state(&env).await;
    assert!(delivered);
    assert_eq!(env.delcore.accepted().len(), 1);
    let calls = env.delcore.ingest_calls();
    assert_eq!(calls.len(), 4);
    assert!(
        calls.iter().all(|c| c == &calls[0]),
        "every retry resends identical content"
    );

    // A duplicate delivery (for example after a crash between the report and its bookkeeping) is
    // answered from delcore's retained identity: still exactly one accepted event.
    sqlx::query("UPDATE attempts SET delivered_at=NULL, retry_at=now()-interval '1 second'")
        .execute(&env.fx.pool)
        .await
        .unwrap();
    usage_service(&env).run_deliveries().await.unwrap();
    assert_eq!(env.delcore.ingest_calls().len(), 5);
    assert_eq!(env.delcore.accepted().len(), 1);
    let metrics = env.fx.platform.metrics.encode();
    assert!(metrics.contains("delitools_usage_deliveries_total{outcome=\"failed\"} 3"));
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn a_slow_failing_delivery_backs_off_from_the_end_of_the_report() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    translating(&env, Some(UsageSpec::full(100, 20, 30, 10)));
    // The report takes longer than the initial retry delay (one second) and then fails.
    env.delcore.delay_ingests(Duration::from_millis(1500));
    env.delcore.fail_ingests(1);
    translate_ok(&env, &key).await;
    let mut remaining = None;
    for _ in 0..100 {
        let row = sqlx::query_as::<_, (i32, Option<f64>)>(
            "SELECT delivery_failures, EXTRACT(EPOCH FROM (retry_at - clock_timestamp()))::float8 FROM attempts",
        )
        .fetch_one(&env.fx.pool)
        .await
        .unwrap();
        if row.0 == 1 {
            remaining = row.1;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let remaining = remaining.expect("the failed delivery was recorded");
    assert!(
        remaining > 0.5,
        "the retry is scheduled a full delay after the failed report, not from its start: {remaining}s left"
    );
    // Not yet due, so a background run does not resend the attempt.
    env.delcore.delay_ingests(Duration::ZERO);
    usage_service(&env).run_deliveries().await.unwrap();
    assert_eq!(env.delcore.ingest_calls().len(), 1);
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn a_rejected_reporting_token_is_replaced_before_the_next_retry() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    translating(&env, Some(UsageSpec::full(100, 20, 30, 10)));
    // Delcore refuses the credential once (for example after rotation) although the cached token
    // has not reached its advertised expiry.
    env.delcore.fail_ingest_auth(1);
    translate_ok(&env, &key).await;
    for _ in 0..50 {
        if env.delcore.ingest_authorizations().len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let reporting_tokens = |env: &Env| {
        env.delcore
            .token_scopes()
            .iter()
            .filter(|scope| scope.as_str() == "usage:write")
            .count()
    };
    assert_eq!(reporting_tokens(&env), 1);
    assert!(env.delcore.accepted().is_empty());

    env.make_due().await;
    usage_service(&env).run_deliveries().await.unwrap();
    assert_eq!(
        reporting_tokens(&env),
        2,
        "the retry obtained fresh credentials instead of reusing the rejected token"
    );
    assert_eq!(env.delcore.accepted().len(), 1);
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn a_success_status_without_acceptance_fields_does_not_settle_the_report() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    translating(&env, Some(UsageSpec::full(100, 20, 30, 10)));
    env.delcore.malformed_ingests(1);
    translate_ok(&env, &key).await;
    for _ in 0..50 {
        if env.delcore.ingest_calls().len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (failures, delivered): (i32, bool) =
        sqlx::query_as("SELECT delivery_failures, delivered_at IS NOT NULL FROM attempts")
            .fetch_one(&env.fx.pool)
            .await
            .unwrap();
    assert_eq!(
        (failures, delivered),
        (1, false),
        "a body without eventId and revision is a failed delivery that is retried"
    );
    assert!(env.delcore.accepted().is_empty());

    env.make_due().await;
    usage_service(&env).run_deliveries().await.unwrap();
    let delivered: bool = sqlx::query_scalar("SELECT delivered_at IS NOT NULL FROM attempts")
        .fetch_one(&env.fx.pool)
        .await
        .unwrap();
    assert!(delivered);
    assert_eq!(env.delcore.accepted().len(), 1);
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn background_delivery_survives_restart_from_persisted_state_alone() {
    let env = Env::start(&[], false).await;
    env.seed_crash_state().await;
    // A restart finds the request still processing and marks it unknown, then resumes usage work.
    let interrupted = store::recovery::mark_interrupted_unknown(&env.fx.pool, store::now())
        .await
        .unwrap();
    assert_eq!(interrupted, 1);
    env.gateway.set_generation(
        "gen_crash",
        Some(generation_data(
            "gen_crash",
            TRANSLATION_MODEL,
            50,
            5,
            10,
            0,
        )),
    );
    let usage = usage_service(&env);
    usage.run_deliveries().await.unwrap();
    usage.run_reconciliation().await.unwrap();
    let events = env.wait_for_events(2).await;
    assert_eq!(
        events.len(),
        2,
        "both the known and the reconciled attempt were delivered"
    );
    assert_eq!(
        env.gateway.chat_count(),
        0,
        "an interrupted prompt is never resumed"
    );
    let state: i16 = sqlx::query_scalar("SELECT state FROM requests")
        .fetch_one(&env.fx.pool)
        .await
        .unwrap();
    assert_eq!(state, 3);
    env.finish().await;
}

impl Env {
    /// Persisted leftovers of a crash: a processing request with an in-flight attempt whose
    /// generation is known and a completed-usage attempt that was never delivered.
    async fn seed_crash_state(&self) {
        let (request, known, unknown) = (support::id(), support::id(), support::id());
        sqlx::query("INSERT INTO requests(id,tenant_id,capability,state,model,credential_kind,created_at,updated_at) VALUES($1,$2,1,1,$3,3,now(),now())")
            .bind(request)
            .bind(self.tenant)
            .bind(TRANSLATION_MODEL)
            .execute(&self.fx.pool)
            .await
            .unwrap();
        let snapshot = json!({"input_usd_per_token":"0.000003","output_usd_per_token":"0.000015",
            "cache_read_usd_per_token":"0.0000003","cache_write_usd_per_token":"0.00000375"});
        for (id, generation, usage) in [
            (
                known,
                "gen_known",
                Some(
                    json!({"input_tokens":10,"output_tokens":2,"cache_read_tokens":0,"cache_write_tokens":0}),
                ),
            ),
            (unknown, "gen_crash", None),
        ] {
            sqlx::query("INSERT INTO attempts(id,request_id,tenant_id,capability,model,provider_route,generation_id,price_effective_at,catalog_price,occurred_at,measured_usage,created_at,updated_at) VALUES($1,$2,$3,1,$4,'anthropic',$5,now()-interval '1 minute',$6,now(),$7,now(),now())")
                .bind(id)
                .bind(request)
                .bind(self.tenant)
                .bind(TRANSLATION_MODEL)
                .bind(generation)
                .bind(&snapshot)
                .bind(usage)
                .execute(&self.fx.pool)
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn unknown_usage_is_reconciled_with_the_gateway_and_never_estimated() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    // No usage chunk at all.
    translating(&env, None);
    translate_ok(&env, &key).await;
    // A usage chunk without cache details is incomplete, not zero cache.
    env.gateway.respond(|n, body| {
        reply(
            &echo_translations(body),
            &format!("gen_inc{n}"),
            Some(UsageSpec {
                prompt: 80,
                completion: 8,
                cached: None,
                created: None,
            }),
        )
    });
    translate_ok(&env, &key).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        env.delcore.ingest_calls().is_empty(),
        "no estimate and no partial report"
    );
    let rows: Vec<(String, bool)> = sqlx::query_as(
        "SELECT generation_id, measured_usage IS NOT NULL FROM attempts ORDER BY created_at",
    )
    .fetch_all(&env.fx.pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![("gen_t0".into(), false), ("gen_inc1".into(), false)]
    );

    let usage = usage_service(&env);
    // The Gateway has not ingested the usage yet (404): try again later with backoff.
    env.make_due().await;
    usage.run_reconciliation().await.unwrap();
    assert_eq!(env.gateway.generation_lookups().len(), 2);
    let failures: Vec<i32> =
        sqlx::query_scalar("SELECT delivery_failures FROM attempts ORDER BY created_at")
            .fetch_all(&env.fx.pool)
            .await
            .unwrap();
    assert_eq!(failures, vec![1, 1]);
    assert!(env.delcore.ingest_calls().is_empty());

    // Unreadable or inconsistent lookups are not accepted: wrong model, a missing count and cached
    // tokens above the prompt total all leave the attempt pending.
    env.gateway.set_generation(
        "gen_t0",
        Some(generation_data("gen_t0", "other/model", 10, 2, 0, 0)),
    );
    let mut incomplete = generation_data("gen_inc1", TRANSLATION_MODEL, 10, 2, 0, 0);
    incomplete
        .as_object_mut()
        .unwrap()
        .remove("native_tokens_cache_creation");
    env.gateway.set_generation("gen_inc1", Some(incomplete));
    env.make_due().await;
    usage.run_reconciliation().await.unwrap();
    assert!(env.delcore.ingest_calls().is_empty());
    env.gateway.set_generation(
        "gen_inc1",
        Some(generation_data("gen_inc1", TRANSLATION_MODEL, 5, 2, 9, 0)),
    );
    env.make_due().await;
    usage.run_reconciliation().await.unwrap();
    assert!(env.delcore.ingest_calls().is_empty());

    // Valid measured counts are accepted and delivered, split into the four categories.
    env.gateway.set_generation(
        "gen_t0",
        Some(generation_data("gen_t0", TRANSLATION_MODEL, 90, 11, 20, 5)),
    );
    env.gateway.set_generation(
        "gen_inc1",
        Some(generation_data("gen_inc1", TRANSLATION_MODEL, 80, 8, 0, 0)),
    );
    env.make_due().await;
    usage.run_reconciliation().await.unwrap();
    let events = env.wait_for_events(2).await;
    let mut inputs: Vec<(String, String, String, String)> = events
        .values()
        .map(|e| {
            let a = &e["event"]["attributes"];
            (
                a["input_tokens"]["number"].as_str().unwrap().to_owned(),
                a["output_tokens"]["number"].as_str().unwrap().to_owned(),
                a["cache_read_tokens"]["number"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                a["cache_write_tokens"]["number"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
        })
        .collect();
    inputs.sort();
    assert_eq!(
        inputs,
        vec![
            ("65".into(), "11".into(), "20".into(), "5".into()),
            ("80".into(), "8".into(), "0".into(), "0".into()),
        ]
    );
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn usage_confirmed_after_the_expiry_boundary_is_not_stored_or_delivered() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    env.gateway.sequence(vec![Script::StreamError {
        generation: "gen_edge".into(),
    }]);
    env.fx
        .fails(
            Some(&key),
            TRANSLATE,
            env.translate_body(&["x"], None),
            "unavailable",
            reason::PROVIDER,
        )
        .await;
    let id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM attempts ORDER BY created_at LIMIT 1")
        .fetch_one(&env.fx.pool)
        .await
        .unwrap();
    // created_at is immutable in production; the test schema lifts the trigger only to age rows.
    sqlx::query("ALTER TABLE attempts DISABLE TRIGGER attempts_created_at")
        .execute(&env.fx.pool)
        .await
        .unwrap();
    // The attempt expires two minutes from now, as if the Gateway lookup took that long.
    sqlx::query("UPDATE attempts SET created_at=now()-interval '30 days'+interval '1 minute'")
        .execute(&env.fx.pool)
        .await
        .unwrap();
    let tokens = delitools_api::gateway::Tokens {
        input: 10,
        output: 2,
        cache_read: 0,
        cache_write: 0,
    };
    let after_expiry = store::now() + chrono::Duration::minutes(2);
    assert!(
        !store::exec::set_measured(&env.fx.pool, id, &tokens, after_expiry)
            .await
            .unwrap(),
        "usage established after the boundary is abandoned"
    );
    let stored: bool =
        sqlx::query_scalar("SELECT measured_usage IS NOT NULL FROM attempts WHERE id=$1")
            .bind(id)
            .fetch_one(&env.fx.pool)
            .await
            .unwrap();
    assert!(!stored);
    // Inside the window the same confirmation is stored, but delivery is no longer claimable
    // once the boundary has passed.
    assert!(
        store::exec::set_measured(&env.fx.pool, id, &tokens, store::now())
            .await
            .unwrap()
    );
    let lease = chrono::Duration::seconds(60);
    assert!(
        store::exec::claim_one(&env.fx.pool, id, after_expiry, lease)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store::exec::claim_one(&env.fx.pool, id, store::now(), lease)
            .await
            .unwrap()
            .is_some()
    );
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn reconciliation_failures_do_not_inflate_the_delivery_backoff() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    translating(&env, None);
    translate_ok(&env, &key).await;
    // Two lookups find nothing yet, so the attempt has two reconciliation failures.
    for _ in 0..2 {
        env.make_due().await;
        usage_service(&env).run_reconciliation().await.unwrap();
    }
    let failures: i32 = sqlx::query_scalar("SELECT delivery_failures FROM attempts")
        .fetch_one(&env.fx.pool)
        .await
        .unwrap();
    assert_eq!(failures, 2);

    // The Gateway now confirms the usage, and the first delivery to delcore fails.
    env.gateway.set_generation(
        "gen_t0",
        Some(generation_data(
            "gen_t0",
            TRANSLATION_MODEL,
            100,
            20,
            30,
            10,
        )),
    );
    env.delcore.fail_ingests(1);
    env.make_due().await;
    usage_service(&env).run_reconciliation().await.unwrap();
    let (failures, delay): (i32, f64) = sqlx::query_as(
        "SELECT delivery_failures, EXTRACT(EPOCH FROM (retry_at - updated_at))::float8 FROM attempts",
    )
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert_eq!(failures, 1, "only the real delivery failure is counted");
    assert!(
        (0.9..=1.1).contains(&delay),
        "the first delivery failure waits the initial delay, not an inflated one: {delay}"
    );
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM operational_events WHERE category='usage_delivery_failure'",
    )
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert_eq!(events, 1, "the first delivery failure is recorded");
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn failed_calls_and_unusable_output_still_bill_their_confirmed_usage() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    // Truncated output (finish_reason length) fails the request but consumed tokens.
    env.gateway.sequence(vec![Script::Reply(StreamReply {
        content: "{\"translations\":[\"a\"".into(),
        generation: "gen_len".into(),
        usage: Some(UsageSpec::full(100, 4096, 0, 0)),
        finish: "length",
    })]);
    env.fx
        .fails(
            Some(&key),
            TRANSLATE,
            env.translate_body(&["x"], None),
            "data_loss",
            reason::INVALID_OUTPUT,
        )
        .await;
    // Malformed content fails too.
    env.gateway.sequence(vec![reply(
        "garbage",
        "gen_bad",
        Some(UsageSpec::full(10, 3, 0, 0)),
    )]);
    env.fx
        .fails(
            Some(&key),
            TRANSLATE,
            env.translate_body(&["x"], None),
            "data_loss",
            reason::INVALID_OUTPUT,
        )
        .await;
    let events = env.wait_for_events(2).await;
    let outputs: Vec<String> = events
        .values()
        .map(|e| {
            e["event"]["attributes"]["output_tokens"]["number"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert!(outputs.contains(&"4096".to_owned()) && outputs.contains(&"3".to_owned()));
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn operators_associate_generations_with_validation_and_audit() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    let operator = env.fx.operator();
    // An attempt whose usage is unknown and which has no generation: a 5xx before any chunk.
    env.gateway.sequence(vec![Script::Status(503)]);
    env.fx
        .fails(
            Some(&key),
            TRANSLATE,
            env.translate_body(&["x"], None),
            "unavailable",
            reason::PROVIDER,
        )
        .await;
    let attempts: Vec<String> =
        sqlx::query_scalar("SELECT id::text FROM attempts ORDER BY created_at")
            .fetch_all(&env.fx.pool)
            .await
            .unwrap();
    assert_eq!(attempts.len(), 3);
    let call = |attempt: &str, generation: &str, reason_text: &str| json!({"attemptId": attempt, "generationId": generation, "reason": reason_text});

    // Authority: platform operators only, with a reason.
    let (tenant_owner, _) = env.user("owner", Some(MemberRole::Owner));
    env.fx
        .fails(
            Some(&tenant_owner),
            "AssociateGeneration",
            call(&attempts[0], "gen_a", "found it"),
            "permission_denied",
            reason::FORBIDDEN,
        )
        .await;
    env.fx
        .fails(
            Some(&key),
            "AssociateGeneration",
            call(&attempts[0], "gen_a", "found it"),
            "permission_denied",
            reason::FORBIDDEN,
        )
        .await;
    env.fx
        .fails(
            Some(&operator),
            "AssociateGeneration",
            call(&attempts[0], "gen_a", "x"),
            "invalid_argument",
            reason::INVALID_INPUT,
        )
        .await;
    env.fx
        .fails(
            Some(&operator),
            "AssociateGeneration",
            call("not-a-uuid", "gen_a", "found it"),
            "invalid_argument",
            reason::INVALID_INPUT,
        )
        .await;
    env.fx
        .fails(
            Some(&operator),
            "AssociateGeneration",
            call(&support::id().to_string(), "gen_a", "found it"),
            "not_found",
            reason::INVALID_INPUT,
        )
        .await;
    env.fx
        .fails(
            Some(&operator),
            "AssociateGeneration",
            call(&attempts[0], "bad id!", "found it"),
            "invalid_argument",
            reason::INVALID_INPUT,
        )
        .await;
    // The generation must be readable at the Gateway and belong to the stored model.
    env.fx
        .fails(
            Some(&operator),
            "AssociateGeneration",
            call(&attempts[0], "gen_missing", "found it"),
            "invalid_argument",
            reason::INVALID_INPUT,
        )
        .await;
    env.gateway.set_generation(
        "gen_wrong",
        Some(generation_data("gen_wrong", "other/model", 10, 2, 0, 0)),
    );
    env.fx
        .fails(
            Some(&operator),
            "AssociateGeneration",
            call(&attempts[0], "gen_wrong", "found it"),
            "invalid_argument",
            reason::INVALID_INPUT,
        )
        .await;
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_records WHERE action='generation.associate'",
    )
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert_eq!(audits, 0, "rejected calls leave no audit record");

    // A valid association is audited, never replays inference and schedules normal
    // reconciliation, which then reports the measured usage.
    env.gateway.set_generation(
        "gen_a",
        Some(generation_data("gen_a", TRANSLATION_MODEL, 70, 7, 10, 0)),
    );
    let chats = env.gateway.chat_count();
    env.fx
        .ok(
            &operator,
            "AssociateGeneration",
            call(&attempts[0], "gen_a", "matched by user reference"),
        )
        .await;
    assert_eq!(env.gateway.chat_count(), chats);
    let (reason_text, target, details): (String, String, Value) = sqlx::query_as(
        "SELECT reason, target_id::text, details FROM audit_records WHERE action='generation.associate'",
    )
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert_eq!(reason_text, "matched by user reference");
    assert_eq!(target, attempts[0]);
    assert_eq!(details["generationId"], "gen_a");
    // A generation can be associated with one attempt only; the attempt only once.
    env.fx
        .fails(
            Some(&operator),
            "AssociateGeneration",
            call(&attempts[1], "gen_a", "again"),
            "invalid_argument",
            reason::INVALID_INPUT,
        )
        .await;
    env.fx
        .fails(
            Some(&operator),
            "AssociateGeneration",
            call(&attempts[0], "gen_b", "again"),
            "invalid_argument",
            reason::INVALID_INPUT,
        )
        .await;
    usage_service(&env).run_reconciliation().await.unwrap();
    let events = env.wait_for_events(1).await;
    assert_eq!(events.len(), 1);
    // The associated attempt is now settled; a confirmed attempt cannot be re-associated.
    env.gateway.set_generation(
        "gen_c",
        Some(generation_data("gen_c", TRANSLATION_MODEL, 1, 1, 0, 0)),
    );
    env.fx
        .fails(
            Some(&operator),
            "AssociateGeneration",
            call(&attempts[0], "gen_c", "late"),
            "invalid_argument",
            reason::INVALID_INPUT,
        )
        .await;
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn an_attempt_at_its_expiry_boundary_cannot_be_associated_before_the_sweep() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    let operator = env.fx.operator();
    env.gateway.sequence(vec![Script::Status(503)]);
    env.fx
        .fails(
            Some(&key),
            TRANSLATE,
            env.translate_body(&["x"], None),
            "unavailable",
            reason::PROVIDER,
        )
        .await;
    let attempt: String = sqlx::query_scalar("SELECT id::text FROM attempts LIMIT 1")
        .fetch_one(&env.fx.pool)
        .await
        .unwrap();
    // created_at is immutable in production; the test schema lifts the trigger only to age rows.
    sqlx::query("ALTER TABLE attempts DISABLE TRIGGER attempts_created_at")
        .execute(&env.fx.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE attempts SET created_at=now()-interval '30 days'")
        .execute(&env.fx.pool)
        .await
        .unwrap();
    env.gateway.set_generation(
        "gen_late",
        Some(generation_data("gen_late", TRANSLATION_MODEL, 10, 2, 0, 0)),
    );
    // The retention sweep has not run, but the contract treats the attempt as already deleted.
    env.fx
        .fails(
            Some(&operator),
            "AssociateGeneration",
            json!({"attemptId": attempt, "generationId": "gen_late", "reason": "too late"}),
            "not_found",
            reason::INVALID_INPUT,
        )
        .await;
    let (generations, audits): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM attempts WHERE generation_id<>''), (SELECT count(*) FROM audit_records WHERE action='generation.associate')",
    )
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert_eq!(
        generations, 0,
        "no generation is stored for an expired attempt"
    );
    assert_eq!(audits, 0, "no audit record is written");

    // The transactional update enforces the same cutoff against a lookup that raced the expiry.
    let id: uuid::Uuid = attempt.parse().unwrap();
    assert!(
        store::exec::attempt_info(&env.fx.pool, id, store::now())
            .await
            .unwrap()
            .is_none()
    );
    let raced = store::exec::associate_generation(
        &env.fx.pool,
        id,
        "gen_late",
        store::audit::AuditEntry {
            action: store::audit::Action::GenerationAssociate,
            actor: "operator:test",
            reason: "raced the expiry",
            request_id: support::id(),
            tenant_id: None,
            target_id: Some(id),
            details: json!({}),
        },
        store::now(),
    )
    .await
    .unwrap();
    assert!(matches!(raced, store::exec::Association::NotEligible));
    sqlx::query("UPDATE attempts SET created_at=now()-interval '30 days'+interval '1 minute'")
        .execute(&env.fx.pool)
        .await
        .unwrap();
    assert!(
        store::exec::attempt_info(&env.fx.pool, id, store::now())
            .await
            .unwrap()
            .is_some()
    );
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn an_attempt_of_a_running_request_cannot_be_associated_until_the_request_ends() {
    let env = Env::start(&[], false).await;
    let operator = env.fx.operator();
    // A request still processing with an attempt that has no generation or usage yet: its own
    // call has not produced a first stream chunk.
    let (request, attempt) = (support::id(), support::id());
    sqlx::query("INSERT INTO requests(id,tenant_id,capability,state,model,credential_kind,created_at,updated_at) VALUES($1,$2,1,1,$3,3,now(),now())")
        .bind(request)
        .bind(env.tenant)
        .bind(TRANSLATION_MODEL)
        .execute(&env.fx.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO attempts(id,request_id,tenant_id,capability,model,provider_route,price_effective_at,catalog_price,occurred_at,created_at,updated_at) VALUES($1,$2,$3,1,$4,'anthropic',now()-interval '1 minute',$5,now(),now(),now())")
        .bind(attempt)
        .bind(request)
        .bind(env.tenant)
        .bind(TRANSLATION_MODEL)
        .bind(json!({"input_usd_per_token":"0.000003","output_usd_per_token":"0.000015"}))
        .execute(&env.fx.pool)
        .await
        .unwrap();
    env.gateway.set_generation(
        "gen_unrelated",
        Some(generation_data(
            "gen_unrelated",
            TRANSLATION_MODEL,
            10,
            2,
            0,
            0,
        )),
    );
    let call = json!({"attemptId": attempt.to_string(), "generationId": "gen_unrelated", "reason": "operator check"});
    let generations = |env: &Env| {
        let pool = env.fx.pool.clone();
        async move {
            sqlx::query_as::<_, (i64, i64)>(
                "SELECT (SELECT count(*) FROM attempts WHERE generation_id<>''), (SELECT count(*) FROM audit_records WHERE action='generation.associate')",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    // The lookup refuses the association and leaves neither a generation nor an audit record.
    let info = store::exec::attempt_info(&env.fx.pool, attempt, store::now())
        .await
        .unwrap()
        .expect("the attempt is retained");
    assert!(info.request_running);
    env.fx
        .fails(
            Some(&operator),
            "AssociateGeneration",
            call.clone(),
            "invalid_argument",
            reason::INVALID_INPUT,
        )
        .await;
    assert_eq!(generations(&env).await, (0, 0));

    // The transactional update enforces the same rule against a lookup that raced the request's
    // own state.
    let raced = store::exec::associate_generation(
        &env.fx.pool,
        attempt,
        "gen_unrelated",
        store::audit::AuditEntry {
            action: store::audit::Action::GenerationAssociate,
            actor: "operator:test",
            reason: "raced the running request",
            request_id: support::id(),
            tenant_id: None,
            target_id: Some(attempt),
            details: json!({}),
        },
        store::now(),
    )
    .await
    .unwrap();
    assert!(matches!(raced, store::exec::Association::NotEligible));
    assert_eq!(generations(&env).await, (0, 0));

    // Once the request has ended (here: interrupted, so its usage stays unknown) the same
    // association is accepted.
    sqlx::query("UPDATE requests SET state=3")
        .execute(&env.fx.pool)
        .await
        .unwrap();
    env.fx.ok(&operator, "AssociateGeneration", call).await;
    assert_eq!(generations(&env).await, (1, 1));
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn catalog_synchronization_versions_prices_and_never_enables_models() {
    let env = Env::start(&[], true).await;
    env.wait_for_price(TRANSLATION_MODEL).await;
    let prices = |env: &Env| {
        let pool = env.fx.pool.clone();
        async move {
            sqlx::query_as::<_, (String, String, String, String)>(
                "SELECT model, provider_route, input_usd_per_token::text, output_usd_per_token::text FROM catalog_prices ORDER BY model, effective_at",
            )
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };
    // Only the allowlisted models are priced, on the one provider route that charges the public
    // price (the azure endpoint prices differently). The unlisted catalog model stays unpriced.
    let rows = prices(&env).await;
    assert_eq!(
        rows,
        vec![
            (
                TRANSLATION_MODEL.into(),
                "anthropic".into(),
                "0.000003".into(),
                "0.000015".into()
            ),
            (
                EXTRACTION_MODEL.into(),
                "openai".into(),
                "0.0000001".into(),
                "0.0000005".into()
            ),
        ]
    );
    assert!(rows.iter().all(|r| r.0 != "google/gemini-unlisted"));
    // The public catalog is read without any credential.
    assert!(
        env.gateway
            .models_auth_headers()
            .iter()
            .all(String::is_empty)
    );

    // An unchanged price only refreshes its synchronization time.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(prices(&env).await.len(), 2);

    // A changed public price becomes a new effective version for later requests.
    let new_luna = (
        "0.0000002",
        "0.000001",
        Some("0.00000002"),
        Some("0.00000025"),
    );
    env.gateway.set_catalog(
        vec![
            mocks::catalog_entry(
                TRANSLATION_MODEL,
                "0.000003",
                "0.000015",
                Some("0.0000003"),
                Some("0.00000375"),
            ),
            mocks::catalog_entry(
                EXTRACTION_MODEL,
                new_luna.0,
                new_luna.1,
                new_luna.2,
                new_luna.3,
            ),
        ],
        vec![
            (
                TRANSLATION_MODEL,
                vec![mocks::endpoint_entry(
                    "anthropic",
                    "0.000003",
                    "0.000015",
                    Some("0.0000003"),
                    Some("0.00000375"),
                )],
            ),
            (
                EXTRACTION_MODEL,
                vec![mocks::endpoint_entry(
                    "openai", new_luna.0, new_luna.1, new_luna.2, new_luna.3,
                )],
            ),
        ],
    );
    for _ in 0..40 {
        if prices(&env).await.len() == 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let after = prices(&env).await;
    assert_eq!(after.len(), 3, "{after:?}");
    let key = env.service_key().await;
    env.gateway.respond(|n, _| {
        reply(
            &extraction_content(
                "extracted",
                json!({"vendor":"A","total":1,"currency":"USD"}),
            ),
            &format!("gen_s{n}"),
            Some(UsageSpec::full(10, 1, 0, 0)),
        )
    });
    env.fx
        .ok(&key, EXTRACT, env.extract_body("t", INVOICE_SCHEMA, None))
        .await;
    let latest: String = sqlx::query_scalar("SELECT catalog_price->>'input_usd_per_token' FROM attempts ORDER BY created_at DESC LIMIT 1")
        .fetch_one(&env.fx.pool)
        .await
        .unwrap();
    assert_eq!(
        latest, "0.0000002",
        "requests starting after the change use the new price"
    );

    // A tiered model has no single flat price: it keeps its last known price.
    let mut tiered = mocks::catalog_entry(TRANSLATION_MODEL, "0.5", "0.5", None, None);
    tiered["pricing"]["input_tiers"] = json!([{"cost": "0.4", "min": 0}]);
    env.gateway.set_catalog(
        vec![
            tiered,
            mocks::catalog_entry(
                EXTRACTION_MODEL,
                new_luna.0,
                new_luna.1,
                new_luna.2,
                new_luna.3,
            ),
        ],
        vec![(
            EXTRACTION_MODEL,
            vec![mocks::endpoint_entry(
                "openai", new_luna.0, new_luna.1, new_luna.2, new_luna.3,
            )],
        )],
    );
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(prices(&env).await.len(), 3);

    // A failing catalog keeps every known price in use indefinitely and keeps readiness.
    env.gateway.models_fail.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(prices(&env).await.len(), 3);
    let ready = env
        .fx
        .http
        .get(format!("{}/readyz", env.fx.base))
        .send()
        .await
        .unwrap();
    assert_eq!(ready.status(), 200);
    translating(&env, Some(UsageSpec::full(10, 1, 0, 0)));
    env.fx
        .ok(&key, TRANSLATE, env.translate_body(&["x"], None))
        .await;
    let metrics = env.fx.platform.metrics.encode();
    assert!(
        metrics.contains("delitools_failures_total{kind=\"catalog_sync\"}"),
        "{metrics}"
    );
    assert!(metrics.contains("delitools_price_syncs_total{outcome=\"failed\"}"));
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM operational_events WHERE category='price_sync_failure'",
    )
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert!(events >= 1);
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn a_model_without_a_matching_endpoint_loses_its_price_but_a_failed_read_keeps_it() {
    use support::{LUNA_PRICE, SONNET_PRICE};
    let env = Env::start(&[], true).await;
    let key = env.service_key().await;
    translating(&env, Some(UsageSpec::full(10, 1, 0, 0)));
    let publish = |translation_endpoints: Option<Vec<Value>>| {
        let (input, output, read, write) = SONNET_PRICE;
        let (luna_input, luna_output, luna_read, luna_write) = LUNA_PRICE;
        let mut endpoints = vec![(
            EXTRACTION_MODEL,
            vec![mocks::endpoint_entry(
                "openai",
                luna_input,
                luna_output,
                luna_read,
                luna_write,
            )],
        )];
        if let Some(list) = translation_endpoints {
            endpoints.push((TRANSLATION_MODEL, list));
        }
        env.gateway.set_catalog(
            vec![
                mocks::catalog_entry(TRANSLATION_MODEL, input, output, read, write),
                mocks::catalog_entry(
                    EXTRACTION_MODEL,
                    luna_input,
                    luna_output,
                    luna_read,
                    luna_write,
                ),
            ],
            endpoints,
        );
    };
    let withdrawals = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM catalog_prices WHERE model=$1 AND NOT priced",
        )
        .bind(TRANSLATION_MODEL)
        .fetch_one(&env.fx.pool)
        .await
        .unwrap()
    };
    translate_ok(&env, &key).await;

    // A failed endpoint read (here a 404) is not evidence that the price is gone: the last known
    // price stays in use.
    publish(None);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(withdrawals().await, 0);
    translate_ok(&env, &key).await;

    // A successful read that finds no active endpoint charging the public price withdraws it, so
    // new work is rejected instead of pinning a stale route and price.
    let (input, output, read, write) = SONNET_PRICE;
    publish(Some(vec![
        mocks::endpoint_entry("azure", "0.00001", output, read, write),
        mocks::endpoint_entry("anthropic", input, output, read, write),
    ]));
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(withdrawals().await, 0, "an exact match keeps the price");
    publish(Some(vec![mocks::endpoint_entry(
        "azure", "0.00001", output, read, write,
    )]));
    for _ in 0..60 {
        if withdrawals().await == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(withdrawals().await, 1);
    env.fx
        .fails(
            Some(&key),
            TRANSLATE,
            env.translate_body(&["x"], None),
            "failed_precondition",
            reason::PRICE_UNAVAILABLE,
        )
        .await;
    // Repeated synchronizations do not stack withdrawals, and other models are unaffected.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(withdrawals().await, 1);
    let ready = env
        .fx
        .http
        .get(format!("{}/readyz", env.fx.base))
        .send()
        .await
        .unwrap();
    assert_eq!(ready.status(), 200, "the extraction model is still priced");

    // When an endpoint charges the public price again the model is priced by a new version.
    publish(Some(vec![mocks::endpoint_entry(
        "anthropic",
        input,
        output,
        read,
        write,
    )]));
    let mut restored = false;
    for _ in 0..60 {
        let reply = env
            .fx
            .call(Some(&key), TRANSLATE, env.translate_body(&["x"], None))
            .await;
        if reply.code().is_empty() {
            restored = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(restored, "the price returned with the matching endpoint");
    let versions: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_prices WHERE model=$1")
        .bind(TRANSLATION_MODEL)
        .fetch_one(&env.fx.pool)
        .await
        .unwrap();
    assert_eq!(versions, 3, "original price, withdrawal and restored price");
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn a_model_the_catalog_stops_publishing_loses_its_price_but_a_failed_fetch_keeps_it() {
    use support::{LUNA_PRICE, SONNET_PRICE};
    let env = Env::start(&[], true).await;
    let key = env.service_key().await;
    translating(&env, Some(UsageSpec::full(10, 1, 0, 0)));
    let (input, output, read, write) = SONNET_PRICE;
    let (luna_input, luna_output, luna_read, luna_write) = LUNA_PRICE;
    // The extraction model is always published and has a matching endpoint; the translation
    // model is published as given, with a matching endpoint.
    let publish = |translation: Option<Value>| {
        let mut entries = vec![mocks::catalog_entry(
            EXTRACTION_MODEL,
            luna_input,
            luna_output,
            luna_read,
            luna_write,
        )];
        entries.extend(translation);
        env.gateway.set_catalog(
            entries,
            vec![
                (
                    EXTRACTION_MODEL,
                    vec![mocks::endpoint_entry(
                        "openai",
                        luna_input,
                        luna_output,
                        luna_read,
                        luna_write,
                    )],
                ),
                (
                    TRANSLATION_MODEL,
                    vec![mocks::endpoint_entry(
                        "anthropic",
                        input,
                        output,
                        read,
                        write,
                    )],
                ),
            ],
        );
    };
    let priced = || mocks::catalog_entry(TRANSLATION_MODEL, input, output, read, write);
    let withdrawals = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM catalog_prices WHERE model=$1 AND NOT priced",
        )
        .bind(TRANSLATION_MODEL)
        .fetch_one(&env.fx.pool)
        .await
        .unwrap()
    };
    let until_withdrawals = |expected: i64| {
        let withdrawals = &withdrawals;
        async move {
            for _ in 0..60 {
                if withdrawals().await == expected {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert_eq!(withdrawals().await, expected);
        }
    };
    let rejected = || async {
        env.fx
            .fails(
                Some(&key),
                TRANSLATE,
                env.translate_body(&["x"], None),
                "failed_precondition",
                reason::PRICE_UNAVAILABLE,
            )
            .await;
    };
    publish(Some(priced()));
    translate_ok(&env, &key).await;

    // A failed catalog fetch is no evidence that the price is gone: the last known price stays.
    env.gateway.models_fail.store(true, Ordering::SeqCst);
    publish(None);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(withdrawals().await, 0);
    translate_ok(&env, &key).await;
    env.gateway.models_fail.store(false, Ordering::SeqCst);

    // A successfully read catalog that omits the model withdraws its price.
    until_withdrawals(1).await;
    rejected().await;
    // Repeated synchronizations do not stack withdrawals.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(withdrawals().await, 1);

    // Publishing the model again prices it by a new version.
    publish(Some(priced()));
    let mut restored = false;
    for _ in 0..60 {
        if env
            .fx
            .call(Some(&key), TRANSLATE, env.translate_body(&["x"], None))
            .await
            .code()
            .is_empty()
        {
            restored = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(restored, "the price returned with the published model");

    // A listed model that lost its pricing, or is no longer a language model, loses its price.
    publish(Some(
        json!({"id": TRANSLATION_MODEL, "object": "model", "type": "language"}),
    ));
    until_withdrawals(2).await;
    rejected().await;
    publish(Some(priced()));
    let mut restored = false;
    for _ in 0..60 {
        if env
            .fx
            .call(Some(&key), TRANSLATE, env.translate_body(&["x"], None))
            .await
            .code()
            .is_empty()
        {
            restored = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(restored);
    let mut embedding = priced();
    embedding["type"] = json!("embedding");
    publish(Some(embedding));
    until_withdrawals(3).await;
    rejected().await;
    // The extraction model is still priced, so the service stays ready.
    let ready = env
        .fx
        .http
        .get(format!("{}/readyz", env.fx.base))
        .send()
        .await
        .unwrap();
    assert_eq!(ready.status(), 200);
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn a_model_removed_from_the_allowlist_loses_its_price_until_it_is_synchronized_again() {
    let env = Env::start(&[], true).await;
    let key = env.service_key().await;
    translating(&env, Some(UsageSpec::full(10, 1, 0, 0)));
    env.wait_for_price(TRANSLATION_MODEL).await;
    translate_ok(&env, &key).await;
    let withdrawals = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM catalog_prices WHERE model=$1 AND NOT priced",
        )
        .bind(TRANSLATION_MODEL)
        .fetch_one(&env.fx.pool)
        .await
        .unwrap()
    };
    assert_eq!(withdrawals().await, 0);

    // The operator removes the model: the next synchronization withdraws the stored price.
    let operator = env.fx.operator();
    let mut settings =
        env.fx.ok(&operator, "GetSettings", json!({})).await.body["settings"].clone();
    settings["translationModels"] = json!([EXTRACTION_MODEL]);
    env.fx
        .ok(
            &operator,
            "UpdateSettings",
            json!({"settings": settings.clone(), "reason": "retire the translation model"}),
        )
        .await;
    let mut withdrawn = false;
    for _ in 0..60 {
        if withdrawals().await == 1 {
            withdrawn = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        withdrawn,
        "the price of a model that left the allowlist is withdrawn"
    );
    // Further synchronizations do not stack withdrawals, and the extraction model keeps the
    // service ready.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(withdrawals().await, 1);
    let ready = env
        .fx
        .http
        .get(format!("{}/readyz", env.fx.base))
        .send()
        .await
        .unwrap();
    assert_eq!(ready.status(), 200);

    // Allowing the model again prices it by a new version at the next synchronization.
    settings["translationModels"] = json!([TRANSLATION_MODEL, EXTRACTION_MODEL]);
    env.fx
        .ok(
            &operator,
            "UpdateSettings",
            json!({"settings": settings, "reason": "allow the translation model again"}),
        )
        .await;
    let mut restored = false;
    for _ in 0..60 {
        if env
            .fx
            .call(Some(&key), TRANSLATE, env.translate_body(&["x"], None))
            .await
            .code()
            .is_empty()
        {
            restored = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        restored,
        "the model is priced again after it is allowed again"
    );
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn a_request_freezes_the_price_effective_at_its_persisted_start_time() {
    use delitools_api::{
        gateway::CatalogPrice,
        platform::{CredentialKind, ExecutionPrincipal},
        store::exec::{NewRequest, Start, record_price, start_request},
    };
    let env = Env::start(&[], false).await;
    let principal = ExecutionPrincipal {
        tenant_id: env.tenant,
        capability: delino_protocol::delitools::v1::Capability::CAPABILITY_TRANSLATOR,
        credential: CredentialKind::ServiceKey,
        api_key_id: None,
        user_id: None,
    };
    // Price changes race request starts. Each request must freeze the version that was effective
    // at the start time stored on the request, whatever the interleaving.
    let mut tasks = tokio::task::JoinSet::new();
    for round in 0..40_u32 {
        let pool = env.fx.pool.clone();
        tasks.spawn(async move {
            let price = CatalogPrice {
                input: format!("0.00001{round:02}"),
                output: "0.00002".into(),
                cache_read: None,
                cache_write: None,
                tiered: false,
            };
            record_price(&pool, TRANSLATION_MODEL, "anthropic", &price, "race")
                .await
                .unwrap();
        });
        for _ in 0..3 {
            let pool = env.fx.pool.clone();
            let principal = principal.clone();
            tasks.spawn(async move {
                let request = NewRequest {
                    id: uuid::Uuid::now_v7(),
                    principal: &principal,
                    model: TRANSLATION_MODEL,
                };
                let started = start_request(&pool, &request, None, store::now())
                    .await
                    .unwrap();
                assert!(matches!(started, Start::Started(_)));
            });
        }
    }
    while let Some(done) = tasks.join_next().await {
        done.unwrap();
    }
    let mismatched: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM requests r WHERE r.price_id IS DISTINCT FROM (SELECT p.id FROM catalog_prices p WHERE p.model=r.model AND p.effective_at<=r.created_at ORDER BY p.effective_at DESC, p.synced_at DESC LIMIT 1)",
    )
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert_eq!(
        mismatched, 0,
        "every request froze the price of its start time"
    );
    let (requests, versions): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM requests), (SELECT count(*) FROM catalog_prices WHERE model=$1)",
    )
    .bind(TRANSLATION_MODEL)
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert_eq!(requests, 120, "before the waiting start below");
    assert!(
        versions > 10,
        "the test really changed the price: {versions}"
    );

    // A request start waits for a price writer that holds the model lock and then keeps the
    // version that was effective at its own start time, not the one committed meanwhile.
    let mut writer = env.fx.pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(TRANSLATION_MODEL)
        .execute(&mut *writer)
        .await
        .unwrap();
    let waiting = {
        let pool = env.fx.pool.clone();
        let principal = principal.clone();
        tokio::spawn(async move {
            let request = NewRequest {
                id: uuid::Uuid::now_v7(),
                principal: &principal,
                model: TRANSLATION_MODEL,
            };
            start_request(&pool, &request, None, store::now()).await
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !waiting.is_finished(),
        "the start waits for the price writer instead of reading around it"
    );
    sqlx::query("INSERT INTO catalog_prices(id,model,provider_route,effective_at,input_usd_per_token,output_usd_per_token,synced_at,created_at) VALUES($1,$2,'anthropic',clock_timestamp(),0.5,0.5,now(),now())")
        .bind(uuid::Uuid::now_v7())
        .bind(TRANSLATION_MODEL)
        .execute(&mut *writer)
        .await
        .unwrap();
    writer.commit().await.unwrap();
    let Start::Started(frozen) = waiting.await.unwrap().unwrap() else {
        panic!("the request started");
    };
    assert_ne!(
        frozen.input, "0.5",
        "a version that became effective after the start time is not frozen"
    );

    // A model without any effective version creates nothing and claims nothing.
    let none = NewRequest {
        id: uuid::Uuid::now_v7(),
        principal: &principal,
        model: "unknown/model",
    };
    assert_eq!(
        start_request(&env.fx.pool, &none, None, store::now())
            .await
            .unwrap(),
        Start::NoPrice
    );
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn a_transient_storage_error_does_not_discard_confirmed_usage() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    // No generation lookup can recover the counts: the mocked Gateway knows no generation. Fail
    // the first two writes of measured usage, as a transient database error would.
    for statement in [
        "CREATE SEQUENCE usage_write_failures",
        "CREATE FUNCTION fail_first_usage_writes() RETURNS trigger AS $$ BEGIN \
         IF NEW.measured_usage IS NOT NULL AND nextval('usage_write_failures') <= 2 THEN \
         RAISE EXCEPTION 'injected transient failure'; END IF; RETURN NEW; END; $$ \
         LANGUAGE plpgsql",
        "CREATE TRIGGER fail_usage_writes BEFORE UPDATE ON attempts FOR EACH ROW \
         EXECUTE FUNCTION fail_first_usage_writes()",
    ] {
        sqlx::query(statement).execute(&env.fx.pool).await.unwrap();
    }
    translating(&env, Some(UsageSpec::full(100, 20, 30, 10)));
    translate_ok(&env, &key).await;
    // The confirmed counts survived the failed writes and were delivered.
    let events = env.wait_for_events(1).await;
    assert_eq!(events.len(), 1);
    let stored: bool = sqlx::query_scalar("SELECT measured_usage IS NOT NULL FROM attempts")
        .fetch_one(&env.fx.pool)
        .await
        .unwrap();
    assert!(stored);
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn an_attempt_that_expires_while_an_earlier_report_runs_is_not_reported() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    translating(&env, Some(UsageSpec::full(100, 20, 30, 10)));
    env.delcore.fail_ingests(100);
    translate_ok(&env, &key).await;
    translate_ok(&env, &key).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(env.delcore.accepted().is_empty());
    let before = env.delcore.ingest_calls().len();

    // created_at is immutable in production; the test schema lifts the trigger only to age rows.
    sqlx::query("ALTER TABLE attempts DISABLE TRIGGER attempts_created_at")
        .execute(&env.fx.pool)
        .await
        .unwrap();
    // The later attempt is inside its window when the batch is claimed and leaves it two seconds
    // later, while the first report of the batch is still running (it takes three seconds).
    let ids: Vec<uuid::Uuid> = sqlx::query_scalar("SELECT id FROM attempts ORDER BY id")
        .fetch_all(&env.fx.pool)
        .await
        .unwrap();
    assert_eq!(ids.len(), 2);
    sqlx::query("UPDATE attempts SET retry_at=now()-interval '1 second'")
        .execute(&env.fx.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE attempts SET created_at=now()-interval '30 days'+interval '2 seconds' WHERE id=$1",
    )
    .bind(ids[1])
    .execute(&env.fx.pool)
    .await
    .unwrap();
    env.delcore.fail_ingests(0);
    env.delcore.delay_ingests(Duration::from_secs(3));
    usage_service(&env).run_deliveries().await.unwrap();
    assert_eq!(
        env.delcore.ingest_calls().len(),
        before + 1,
        "only the first attempt was reported; the second reached its boundary before its turn"
    );
    assert!(
        env.delcore.accepted().contains_key(&ids[0].to_string()),
        "the attempt that was still inside its window was delivered"
    );
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn pending_usage_stops_at_thirty_days_and_is_deleted_in_every_state() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    translating(&env, Some(UsageSpec::full(100, 20, 30, 10)));
    env.delcore.fail_ingests(100);
    translate_ok(&env, &key).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(env.delcore.accepted().is_empty());
    let before = env.delcore.ingest_calls().len();

    // created_at is immutable in production; the test schema lifts the trigger only to age rows.
    sqlx::query("ALTER TABLE attempts DISABLE TRIGGER attempts_created_at")
        .execute(&env.fx.pool)
        .await
        .unwrap();
    // Just inside the window the attempt is still retried ...
    sqlx::query("UPDATE attempts SET created_at=now()-interval '30 days'+interval '1 minute', retry_at=now()-interval '1 second'")
        .execute(&env.fx.pool)
        .await
        .unwrap();
    usage_service(&env).run_deliveries().await.unwrap();
    assert_eq!(env.delcore.ingest_calls().len(), before + 1);
    // ... and at the expiry boundary it is neither retried nor kept: unconfirmed usage is lost.
    sqlx::query("UPDATE attempts SET created_at=now()-interval '30 days', retry_at=now()-interval '1 second'")
        .execute(&env.fx.pool)
        .await
        .unwrap();
    usage_service(&env).run_deliveries().await.unwrap();
    assert_eq!(
        env.delcore.ingest_calls().len(),
        before + 1,
        "no delivery attempt after expiry"
    );
    let report = retention::sweep(&env.fx.pool, store::now()).await.unwrap();
    assert!(report.total() >= 1);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM attempts")
        .fetch_one(&env.fx.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    env.finish().await;
}
