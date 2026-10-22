// SPDX-License-Identifier: FSL-1.1-ALv2
//! Optional idempotency: scope, conflicts, in-progress, completed, unknown and expired keys.
//! These tests need a disposable PostgreSQL database in `TEST_DATABASE_URL`.
mod support;
use delitools_api::store::{self, recovery, retention};
use serde_json::{Value, json};
use std::time::Duration;
use support::{
    Env, INVOICE_SCHEMA, echo_translations, extraction_content,
    mocks::{MemberRole, Script, UsageSpec, reply},
    reason,
};

const TRANSLATE: &str = "TranslateTextArray";
const EXTRACT: &str = "ExtractDataFromText";
const SERVICE: &str = "KEY_KIND_SERVICE";
const PERSONAL: &str = "KEY_KIND_PERSONAL";
const ALL: [&str; 2] = ["CAPABILITY_TRANSLATOR", "CAPABILITY_EXTRACTOR"];

fn usage() -> Option<UsageSpec> {
    Some(UsageSpec::full(100, 20, 30, 10))
}
fn echo(env: &Env) {
    env.gateway.respond(|n, body| {
        if body["response_format"]["json_schema"]["name"] == "delitools_extraction" {
            reply(
                &extraction_content(
                    "extracted",
                    json!({"vendor":"A","total":1,"currency":"USD"}),
                ),
                &format!("gen_i{n}"),
                usage(),
            )
        } else {
            reply(&echo_translations(body), &format!("gen_i{n}"), usage())
        }
    });
}

fn spawn_call(
    env: &Env,
    bearer: &str,
    method: &'static str,
    body: Value,
) -> tokio::task::JoinHandle<(u16, Value)> {
    let (http, base, bearer) = (env.fx.http.clone(), env.fx.base.clone(), bearer.to_owned());
    tokio::spawn(async move {
        let response = http
            .post(format!("{base}/delitools.v1.DeliToolsService/{method}"))
            .header("connect-protocol-version", "1")
            .bearer_auth(bearer)
            .json(&body)
            .send()
            .await
            .unwrap();
        (response.status().as_u16(), response.json().await.unwrap())
    })
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn a_transient_storage_error_does_not_leave_the_request_processing() {
    let env = Env::start(&[], false).await;
    echo(&env);
    let key = env.service_key().await;
    // Fail the first two writes of a completed state, as a transient database error would.
    for statement in [
        "CREATE SEQUENCE finalize_failures",
        "CREATE FUNCTION fail_first_finalizations() RETURNS trigger AS $$ BEGIN \
         IF NEW.state = 2 AND nextval('finalize_failures') <= 2 THEN \
         RAISE EXCEPTION 'injected transient failure'; END IF; RETURN NEW; END; $$ \
         LANGUAGE plpgsql",
        "CREATE TRIGGER fail_finalizations BEFORE UPDATE ON requests FOR EACH ROW \
         EXECUTE FUNCTION fail_first_finalizations()",
    ] {
        sqlx::query(statement).execute(&env.fx.pool).await.unwrap();
    }
    env.fx
        .ok(
            &key,
            TRANSLATE,
            env.translate_body(&["a"], Some("flaky-store")),
        )
        .await;
    // The terminal write was retried, so a duplicate sees a completed request, not PROCESSING.
    env.fx
        .fails(
            Some(&key),
            TRANSLATE,
            env.translate_body(&["a"], Some("flaky-store")),
            "already_exists",
            reason::ALREADY_PROCESSED,
        )
        .await;
    let states: Vec<i16> = sqlx::query_scalar("SELECT state FROM requests")
        .fetch_all(&env.fx.pool)
        .await
        .unwrap();
    assert_eq!(states, vec![2]);
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn scope_conflict_completed_duplicate_and_independent_scopes() {
    let env = Env::start(&[], false).await;
    echo(&env);
    let (owner, _) = env.user("owner", Some(MemberRole::Owner));
    let (member, member_id) = env.user("member", Some(MemberRole::Member));
    let (service, _) = env.create_key(&owner, SERVICE, &ALL).await;
    let (personal, _) = env.create_key(&member, PERSONAL, &ALL).await;

    let first = env
        .fx
        .ok(
            &service,
            TRANSLATE,
            env.translate_body(&["a", "b"], Some("order-1")),
        )
        .await;
    let first_id = first.body["requestId"].as_str().unwrap().to_owned();
    assert_eq!(env.gateway.chat_count(), 1);

    // The same tenant, method and key through a different credential: already processed, with the
    // existing request ID, without replaying a result and without a new AI call.
    for bearer in [&personal, &service, &member] {
        let duplicate = env
            .fx
            .fails(
                Some(bearer),
                TRANSLATE,
                env.translate_body(&["a", "b"], Some("order-1")),
                "already_exists",
                reason::ALREADY_PROCESSED,
            )
            .await;
        assert_eq!(duplicate.detail().request_id, first_id);
        assert_eq!(duplicate.request_id_header, first_id);
        assert!(duplicate.body.get("texts").is_none(), "no result replay");
    }
    assert_eq!(env.gateway.chat_count(), 1);

    // A different input under the same key is a conflict, whatever the stored state.
    env.fx
        .fails(
            Some(&service),
            TRANSLATE,
            env.translate_body(&["a", "different"], Some("order-1")),
            "failed_precondition",
            reason::INPUT_CONFLICT,
        )
        .await;
    let mut other_language = env.translate_body(&["a", "b"], Some("order-1"));
    other_language["outputLanguage"] = json!("ja");
    env.fx
        .fails(
            Some(&service),
            TRANSLATE,
            other_language,
            "failed_precondition",
            reason::INPUT_CONFLICT,
        )
        .await;
    assert_eq!(env.gateway.chat_count(), 1);

    // Another method with the same key is a different scope and executes.
    env.fx
        .ok(
            &service,
            EXTRACT,
            env.extract_body("text", INVOICE_SCHEMA, Some("order-1")),
        )
        .await;
    assert_eq!(env.gateway.chat_count(), 2);
    // Another tenant with the same key and method is a different scope too.
    let tenant2 = support::id();
    env.delcore.set_role(tenant2, member_id, MemberRole::Member);
    let mut other_tenant = env.translate_body(&["a", "b"], Some("order-1"));
    other_tenant["tenantId"] = json!(tenant2.to_string());
    env.fx.ok(&member, TRANSLATE, other_tenant).await;
    assert_eq!(env.gateway.chat_count(), 3);
    // Without a key every call is a new execution.
    env.fx
        .ok(&service, TRANSLATE, env.translate_body(&["a", "b"], None))
        .await;
    env.fx
        .ok(&service, TRANSLATE, env.translate_body(&["a", "b"], None))
        .await;
    assert_eq!(env.gateway.chat_count(), 5);

    // Only the key, a hash and the state are stored: no source, schema or result.
    let (key, hash_len, state): (String, i32, i16) = sqlx::query_as(
        "SELECT idempotency_key, octet_length(input_hash), state FROM idempotency_records WHERE method=1 AND tenant_id=$1",
    )
    .bind(env.tenant)
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert_eq!((key.as_str(), hash_len, state), ("order-1", 32, 2));
    let dump: String =
        sqlx::query_scalar("SELECT string_agg(r::text, ' ') FROM idempotency_records r")
            .fetch_one(&env.fx.pool)
            .await
            .unwrap();
    assert!(!dump.contains("different") && !dump.contains("[ko]"));
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn concurrent_same_key_calls_execute_once_and_the_rest_see_processing() {
    // Enough concurrency that simultaneous first calls are decided by the key claim, not by
    // tenant admission.
    let env = Env::start(&[("TENANT_CONCURRENCY", "10")], false).await;
    env.gateway.respond(|n, body| {
        Script::Slow(
            Duration::from_millis(600),
            Box::new(reply(
                &echo_translations(body),
                &format!("gen_c{n}"),
                usage(),
            )),
        )
    });
    let key = env.service_key().await;
    let calls: Vec<_> = (0..5)
        .map(|_| {
            spawn_call(
                &env,
                &key,
                TRANSLATE,
                env.translate_body(&["a"], Some("race")),
            )
        })
        .collect();
    let mut results = Vec::new();
    for call in calls {
        results.push(call.await.unwrap());
    }
    let winners: Vec<&(u16, Value)> = results
        .iter()
        .filter(|(status, _)| *status == 200)
        .collect();
    assert_eq!(winners.len(), 1, "{results:?}");
    let winner_id = winners[0].1["requestId"].as_str().unwrap();
    for (status, body) in results.iter().filter(|(status, _)| *status != 200) {
        assert_eq!(body["code"], "aborted", "{status} {body}");
        // An in-progress duplicate returns immediately with the existing request ID.
        let detail = support::detail_of(body);
        assert_eq!(detail.reason.to_i32(), reason::PROCESSING);
        assert_eq!(detail.request_id, winner_id);
    }
    assert_eq!(env.gateway.chat_count(), 1);
    // Once complete, the key reports already-processed.
    env.fx
        .fails(
            Some(&key),
            TRANSLATE,
            env.translate_body(&["a"], Some("race")),
            "already_exists",
            reason::ALREADY_PROCESSED,
        )
        .await;
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn ambiguous_executions_report_an_unknown_outcome_and_are_never_repeated() {
    let env = Env::start(&[], false).await;
    let key = env.service_key().await;
    env.gateway.sequence(vec![Script::Hang {
        generation: "gen_amb".into(),
    }]);

    // 1. The client leaves while the call is in flight: the outcome is unknown.
    let call = spawn_call(
        &env,
        &key,
        TRANSLATE,
        env.translate_body(&["a"], Some("amb-1")),
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    call.abort();
    for _ in 0..60 {
        let state: Option<i16> = sqlx::query_scalar(
            "SELECT state FROM idempotency_records WHERE idempotency_key='amb-1'",
        )
        .fetch_optional(&env.fx.pool)
        .await
        .unwrap();
        if state == Some(3) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let duplicate = env
        .fx
        .fails(
            Some(&key),
            TRANSLATE,
            env.translate_body(&["a"], Some("amb-1")),
            "unknown",
            reason::UNKNOWN_OUTCOME,
        )
        .await;
    let existing: String = sqlx::query_scalar(
        "SELECT request_id::text FROM idempotency_records WHERE idempotency_key='amb-1'",
    )
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert_eq!(duplicate.detail().request_id, existing);
    assert_eq!(
        env.gateway.chat_count(),
        1,
        "the prompt is never repeated automatically"
    );

    // 2. A crash: a request still processing at startup becomes unknown, never resumed.
    let in_flight = spawn_call(
        &env,
        &key,
        TRANSLATE,
        env.translate_body(&["b"], Some("amb-2")),
    );
    for _ in 0..50 {
        if env.gateway.chat_count() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let interrupted = recovery::mark_interrupted_unknown(&env.fx.pool, store::now())
        .await
        .unwrap();
    assert_eq!(interrupted, 1);
    env.fx
        .fails(
            Some(&key),
            TRANSLATE,
            env.translate_body(&["b"], Some("amb-2")),
            "unknown",
            reason::UNKNOWN_OUTCOME,
        )
        .await;
    assert_eq!(env.gateway.chat_count(), 2);
    in_flight.abort();
    // Inspection shows the unknown state to an operator without any original content.
    let operator = env.fx.operator();
    let inspected = env
        .fx
        .ok(&operator, "InspectRequest", json!({"requestId": existing}))
        .await;
    assert_eq!(inspected.body["state"], "REQUEST_STATE_UNKNOWN");
    assert_eq!(inspected.body["attempts"][0]["generationId"], "gen_amb");
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn authorization_is_checked_before_any_duplicate_state_is_disclosed() {
    let env = Env::start(&[], false).await;
    echo(&env);
    let (owner, _) = env.user("owner", Some(MemberRole::Owner));
    let (member, member_id) = env.user("member", Some(MemberRole::Member));
    let (service, _) = env.create_key(&owner, SERVICE, &ALL).await;
    let (personal, personal_id) = env.create_key(&member, PERSONAL, &ALL).await;
    let (revoked, revoked_id) = env.create_key(&member, PERSONAL, &ALL).await;
    env.fx
        .ok(
            &service,
            TRANSLATE,
            env.translate_body(&["a"], Some("secret-order")),
        )
        .await;

    let body = || env.translate_body(&["a"], Some("secret-order"));
    // No credential, a bad credential and a key of another capability set learn nothing.
    env.fx
        .fails(
            None,
            TRANSLATE,
            body(),
            "unauthenticated",
            reason::UNAUTHENTICATED,
        )
        .await;
    env.fx
        .fails(
            Some("dtk_bad"),
            TRANSLATE,
            body(),
            "unauthenticated",
            reason::UNAUTHENTICATED,
        )
        .await;
    // A revoked key is rejected as unauthenticated, not told the duplicate state.
    env.fx
        .ok(
            &member,
            "RevokeKey",
            json!({"tenantId": env.tenant.to_string(), "keyId": revoked_id}),
        )
        .await;
    env.fx
        .fails(
            Some(&revoked),
            TRANSLATE,
            body(),
            "unauthenticated",
            reason::UNAUTHENTICATED,
        )
        .await;
    // A user whose access was removed learns nothing either.
    env.delcore.remove_member(env.tenant, member_id);
    env.fx
        .fails(
            Some(&personal),
            TRANSLATE,
            body(),
            "permission_denied",
            reason::FORBIDDEN,
        )
        .await;
    env.fx
        .fails(
            Some(&member),
            TRANSLATE,
            body(),
            "permission_denied",
            reason::FORBIDDEN,
        )
        .await;
    // Billing blocks apply before disclosure as well.
    env.delcore.set_billing_blocked(env.tenant, true);
    env.fx
        .fails(
            Some(&service),
            TRANSLATE,
            body(),
            "failed_precondition",
            reason::BILLING_BLOCKED,
        )
        .await;
    env.delcore.set_billing_blocked(env.tenant, false);
    // An authorized caller still sees the state.
    env.fx
        .fails(
            Some(&service),
            TRANSLATE,
            body(),
            "already_exists",
            reason::ALREADY_PROCESSED,
        )
        .await;
    let _ = personal_id;
    assert_eq!(env.gateway.chat_count(), 1);
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn expiry_thirty_days_after_first_creation_permits_a_fresh_charged_execution() {
    let env = Env::start(&[], false).await;
    echo(&env);
    let key = env.service_key().await;
    let first = env
        .fx
        .ok(&key, TRANSLATE, env.translate_body(&["a"], Some("old")))
        .await;
    // created_at is immutable in production; the test schema lifts the trigger only to age rows.
    sqlx::query("ALTER TABLE idempotency_records DISABLE TRIGGER idempotency_records_created_at")
        .execute(&env.fx.pool)
        .await
        .unwrap();

    // One minute inside the window: still deduplicated.
    sqlx::query(
        "UPDATE idempotency_records SET created_at=now()-interval '30 days'+interval '1 minute'",
    )
    .execute(&env.fx.pool)
    .await
    .unwrap();
    env.fx
        .fails(
            Some(&key),
            TRANSLATE,
            env.translate_body(&["a"], Some("old")),
            "already_exists",
            reason::ALREADY_PROCESSED,
        )
        .await;
    assert_eq!(env.gateway.chat_count(), 1);

    // Past 30 days from first creation: the key starts a new, normally charged execution even
    // before the retention sweep removed the record.
    sqlx::query(
        "UPDATE idempotency_records SET created_at=now()-interval '30 days'-interval '1 minute'",
    )
    .execute(&env.fx.pool)
    .await
    .unwrap();
    let fresh = env
        .fx
        .ok(&key, TRANSLATE, env.translate_body(&["a"], Some("old")))
        .await;
    assert_ne!(fresh.body["requestId"], first.body["requestId"]);
    assert_eq!(env.gateway.chat_count(), 2);
    let (created_recent, request): (bool, String) = sqlx::query_as(
        "SELECT created_at > now()-interval '1 minute', request_id::text FROM idempotency_records WHERE idempotency_key='old'",
    )
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert!(created_recent, "the new record starts a new 30-day window");
    assert_eq!(request, fresh.body["requestId"].as_str().unwrap());

    // And the retention sweep deletes an expired record in any state.
    sqlx::query("UPDATE idempotency_records SET created_at=now()-interval '31 days', state=1")
        .execute(&env.fx.pool)
        .await
        .unwrap();
    let report = retention::sweep(&env.fx.pool, store::now()).await.unwrap();
    assert!(report.total() >= 1);
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM idempotency_records")
        .fetch_one(&env.fx.pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
    env.fx
        .ok(&key, TRANSLATE, env.translate_body(&["a"], Some("old")))
        .await;
    assert_eq!(env.gateway.chat_count(), 3);
    env.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn a_racing_first_call_is_answered_by_the_claim_even_when_the_price_was_withdrawn() {
    use delitools_api::{
        platform::{CredentialKind, ExecutionPrincipal},
        store::exec::{IdempotencyClaim, NewRequest, Start, start_request},
    };
    let env = Env::start(&[], false).await;
    let principal = ExecutionPrincipal {
        tenant_id: env.tenant,
        capability: delino_protocol::delitools::v1::Capability::CAPABILITY_TRANSLATOR,
        credential: CredentialKind::ServiceKey,
        api_key_id: None,
        user_id: None,
    };
    // A model without any effective price version, as after a catalog synchronization withdrew it
    // between a loser's preliminary duplicate check and its creation transaction.
    let model = "withdrawn/model";
    let winner = uuid::Uuid::now_v7();

    // The winner's claim is uncommitted when the loser starts, so the preliminary lookup missed it.
    let mut winning_tx = env.fx.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO idempotency_records(tenant_id,method,idempotency_key,request_id,input_hash,state,created_at,updated_at) VALUES($1,1,'race-key',$2,$3,1,now(),now())")
        .bind(env.tenant)
        .bind(winner)
        .bind([7_u8; 32].as_slice())
        .execute(&mut *winning_tx)
        .await
        .unwrap();
    let loser = {
        let pool = env.fx.pool.clone();
        let principal = principal.clone();
        tokio::spawn(async move {
            let claim = IdempotencyClaim {
                key: "race-key",
                input_hash: [7; 32],
            };
            let request = NewRequest {
                id: uuid::Uuid::now_v7(),
                principal: &principal,
                model: "withdrawn/model",
            };
            start_request(&pool, &request, Some(&claim), store::now()).await
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !loser.is_finished(),
        "the loser waits for the claim instead of concluding that no price exists"
    );
    winning_tx.commit().await.unwrap();
    match loser.await.unwrap().unwrap() {
        Start::Duplicate(existing) => assert_eq!(existing.request_id, winner),
        other => panic!("the existing claim decides the call, got {other:?}"),
    }

    // A genuinely new call without a price still ends as NoPrice and releases its claim.
    let new_call = NewRequest {
        id: uuid::Uuid::now_v7(),
        principal: &principal,
        model,
    };
    let fresh = IdempotencyClaim {
        key: "fresh-key",
        input_hash: [9; 32],
    };
    assert_eq!(
        start_request(&env.fx.pool, &new_call, Some(&fresh), store::now())
            .await
            .unwrap(),
        Start::NoPrice
    );
    let (records, requests): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM idempotency_records WHERE idempotency_key='fresh-key'), (SELECT count(*) FROM requests)",
    )
    .fetch_one(&env.fx.pool)
    .await
    .unwrap();
    assert_eq!(
        (records, requests),
        (0, 0),
        "no claim and no request remain"
    );
    env.finish().await;
}
