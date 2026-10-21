// SPDX-License-Identifier: FSL-1.1-ALv2
//! Operator RPCs (settings and inspection), registered-but-unavailable execution RPCs, request
//! IDs, operational endpoints and the background worker.
mod support;
use delitools_api::{
    authority::TenantRole,
    execution::{CallContext, ExecutionService},
    readiness::ReadinessCheck,
    store,
    worker::{BackgroundTask, TaskContext, TaskName, Worker},
};
use serde_json::{Value, json};
use sqlx::Row;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};
use support::{Fixture, id, reason};
use uuid::Uuid;

async fn audit_rows(f: &Fixture) -> Vec<sqlx::postgres::PgRow> {
    sqlx::query("SELECT * FROM audit_records ORDER BY created_at,id")
        .fetch_all(&f.pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn only_the_platform_operator_role_reaches_operator_rpcs() {
    let f = Fixture::setup().await;
    let tenant = id();
    let (owner, _) = f.user("tenant-owner", Some((tenant, TenantRole::Owner)));
    let machine_operator = f.token("machine", &[support::OPERATOR_ROLE], true);
    let key = f
        .ok(
            &owner,
            "CreateKey",
            json!({"tenantId": tenant.to_string(), "kind": "KEY_KIND_SERVICE", "capabilities": ["CAPABILITY_TRANSLATOR"]}),
        )
        .await
        .body["secret"]
        .as_str()
        .unwrap()
        .to_owned();
    let settings = json!({"settings": {}, "reason": "attempt"});
    let inspect = json!({"requestId": id().to_string()});
    for (method, body) in [
        ("GetSettings", json!({})),
        ("UpdateSettings", settings),
        ("InspectRequest", inspect),
    ] {
        // Tenant Owner/Admin, machine tokens and API keys never imply platform authority.
        for denied in [&owner, &machine_operator, &key] {
            f.fails(
                Some(denied),
                method,
                body.clone(),
                "permission_denied",
                reason::FORBIDDEN,
            )
            .await;
        }
        f.fails(
            None,
            method,
            body.clone(),
            "unauthenticated",
            reason::UNAUTHENTICATED,
        )
        .await;
    }
    assert!(
        audit_rows(&f).await.is_empty(),
        "denied calls leave no audit row"
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn settings_updates_require_a_reason_validate_and_write_an_audit_record() {
    let f = Fixture::setup().await;
    let operator = f.operator();
    let current = f.ok(&operator, "GetSettings", json!({})).await;
    let settings = current.body["settings"].clone();
    assert_eq!(settings["maxInputBytes"], 262_144);
    assert_eq!(settings["maxAiRetries"], 2);
    assert_eq!(settings["translationModels"], json!(["openai/gpt-6-luna"]));

    let mut changed = settings.clone();
    changed["tenantConcurrency"] = json!(9);
    changed["maxAiRetries"] = json!(0);
    changed["translationModels"] = json!(["openai/gpt-6-luna", "anthropic/other"]);

    // Missing, blank and too-short reasons, absent settings and invalid values are all rejected
    // before anything changes.
    for (body, why) in [
        (json!({"settings": changed.clone()}), "missing reason"),
        (
            json!({"settings": changed.clone(), "reason": "  "}),
            "blank reason",
        ),
        (
            json!({"settings": changed.clone(), "reason": "ab"}),
            "short reason",
        ),
        (json!({"reason": "adjust limits"}), "missing settings"),
        (
            {
                let mut absent = changed.clone();
                absent.as_object_mut().unwrap().remove("maxAiRetries");
                json!({"settings": absent, "reason": "adjust limits"})
            },
            "absent retry count",
        ),
        (
            {
                let mut bad = changed.clone();
                bad["tenantConcurrency"] = json!(0);
                json!({"settings": bad, "reason": "adjust limits"})
            },
            "zero concurrency",
        ),
        (
            {
                let mut bad = changed.clone();
                bad["translationModels"] = json!([]);
                json!({"settings": bad, "reason": "adjust limits"})
            },
            "empty allowlist",
        ),
        (
            {
                let mut bad = changed.clone();
                bad["maxBackoffSeconds"] = json!(1);
                json!({"settings": bad, "reason": "adjust limits"})
            },
            "backoff below initial delay",
        ),
    ] {
        let reply = f
            .fails(
                Some(&operator),
                "UpdateSettings",
                body,
                "invalid_argument",
                reason::INVALID_INPUT,
            )
            .await;
        assert!(reply.body["message"].is_string(), "{why}");
    }
    assert!(audit_rows(&f).await.is_empty());
    assert_eq!(f.platform.settings().tenant_concurrency, 4);

    // Success: explicit zero retries is valid; the live snapshot and storage both change.
    let updated = f
        .ok(
            &operator,
            "UpdateSettings",
            json!({"settings": changed, "reason": "raise concurrency for load test"}),
        )
        .await;
    assert_eq!(f.platform.settings().tenant_concurrency, 9);
    assert_eq!(f.platform.settings().max_ai_retries, 0);
    let stored = store::settings::load(&f.pool).await.unwrap();
    assert_eq!(stored, *f.platform.settings());
    let reread = f.ok(&operator, "GetSettings", json!({})).await;
    assert_eq!(reread.body["settings"]["maxAiRetries"], 0);
    assert_eq!(reread.body["settings"]["tenantConcurrency"], 9);

    let rows = audit_rows(&f).await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.get::<String, _>("action"), "settings.update");
    assert_eq!(
        row.get::<String, _>("reason"),
        "raise concurrency for load test"
    );
    let actor: String = row.get("actor");
    assert!(actor.starts_with("operator:") && actor.len() == "operator:".len() + 64);
    assert_eq!(
        row.get::<Uuid, _>("request_id").to_string(),
        updated.body["requestId"].as_str().unwrap()
    );
    let details: Value = row.get("details");
    let names: Vec<&str> = details["changed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["tenantConcurrency", "maxAiRetries", "translationModels"]
    );
    assert_eq!(details["previous"]["tenantConcurrency"], 4);
    assert_eq!(details["current"]["tenantConcurrency"], 9);
    // The audit row stores no credential or raw subject.
    let dump = format!("{actor}{details}");
    assert!(!dump.contains("platform-operator") && !dump.contains("Bearer"));
    let event: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM operational_events WHERE category='settings_updated'",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(event, 1);
    f.finish().await;
}

/// A settings commit and the publication of its snapshot form one ordered step. While another
/// update holds the ordering guard, a second update must not commit, otherwise the two could
/// publish out of order and leave a stale live snapshot over newer stored settings.
#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn settings_commit_and_publication_are_ordered_across_updates() {
    let f = Arc::new(Fixture::setup().await);
    let operator = f.operator();
    let current = f.ok(&operator, "GetSettings", json!({})).await;
    let mut settings = current.body["settings"].clone();
    settings["tenantConcurrency"] = json!(11);
    let version = |f: Arc<Fixture>| async move {
        sqlx::query_scalar::<_, i64>("SELECT version FROM settings")
            .fetch_one(&f.pool)
            .await
            .unwrap()
    };

    let guard = f.platform.settings_update_guard().await;
    let pending = tokio::spawn({
        let (f, operator) = (f.clone(), operator.clone());
        async move {
            f.ok(
                &operator,
                "UpdateSettings",
                json!({"settings": settings, "reason": "ordered update"}),
            )
            .await;
        }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(version(f.clone()).await, 1, "the update committed early");
    assert_eq!(f.platform.settings().tenant_concurrency, 4);

    drop(guard);
    pending.await.unwrap();
    assert_eq!(version(f.clone()).await, 2);
    assert_eq!(f.platform.settings().tenant_concurrency, 11);
    let stored = store::settings::load(&f.pool).await.unwrap();
    assert_eq!(stored, *f.platform.settings());
    Arc::into_inner(f).expect("update finished").finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn the_reason_limit_counts_characters_in_validation_and_storage() {
    let f = Fixture::setup().await;
    let operator = f.operator();
    let current = f.ok(&operator, "GetSettings", json!({})).await;
    let mut settings = current.body["settings"].clone();
    settings["tenantConcurrency"] = json!(7);

    // 1000 Korean characters occupy 3000 bytes. The documented limit counts characters, so the
    // update must succeed and the audit record must keep the complete reason.
    let long = "한".repeat(1000);
    f.ok(
        &operator,
        "UpdateSettings",
        json!({"settings": settings.clone(), "reason": long}),
    )
    .await;
    assert_eq!(f.platform.settings().tenant_concurrency, 7);
    let stored: String = sqlx::query_scalar("SELECT reason FROM audit_records")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(stored.chars().count(), 1000);

    // One character more is rejected as invalid input before any storage write.
    settings["tenantConcurrency"] = json!(8);
    f.fails(
        Some(&operator),
        "UpdateSettings",
        json!({"settings": settings, "reason": "한".repeat(1001)}),
        "invalid_argument",
        reason::INVALID_INPUT,
    )
    .await;
    assert_eq!(f.platform.settings().tenant_concurrency, 7);

    // The storage constraint is the second line of defense: it also counts characters.
    let overlong = "한".repeat(1001);
    let rejected = sqlx::query(
        "INSERT INTO audit_records(id,actor,action,reason,request_id,details,created_at) VALUES($1,'operator:x','settings.update',$2,$1,'{}',now())",
    )
    .bind(Uuid::now_v7())
    .bind(overlong)
    .execute(&f.pool)
    .await;
    assert!(rejected.is_err());
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn request_inspection_exposes_only_safe_state_and_is_audited() {
    let f = Fixture::setup().await;
    let operator = f.operator();
    let tenant = id();
    let request = id();
    let now = store::now();
    sqlx::query("INSERT INTO requests(id,tenant_id,capability,state,error_reason,model,credential_kind,created_at,updated_at) VALUES($1,$2,1,3,15,'openai/gpt-6-luna',3,$3,$3)")
        .bind(request)
        .bind(tenant)
        .bind(now)
        .execute(&f.pool)
        .await
        .unwrap();
    let attempt = id();
    sqlx::query("INSERT INTO attempts(id,request_id,tenant_id,capability,model,generation_id,price_effective_at,catalog_price,occurred_at,measured_usage,retry_at,delivery_failures,created_at,updated_at) VALUES($1,$2,$3,1,'openai/gpt-6-luna','gen_1',$4,'{\"input_usd_per_token\":\"0.000001\"}',$4,'{\"input_tokens\":\"12\"}',$4,2,$4,$4)")
        .bind(attempt)
        .bind(request)
        .bind(tenant)
        .bind(now)
        .execute(&f.pool)
        .await
        .unwrap();
    let pending = id();
    sqlx::query("INSERT INTO attempts(id,request_id,tenant_id,capability,model,price_effective_at,catalog_price,occurred_at,created_at,updated_at) VALUES($1,$2,$3,1,'openai/gpt-6-luna',$4,'{}',$4,$4,$4)")
        .bind(pending)
        .bind(request)
        .bind(tenant)
        .bind(now + chrono::Duration::seconds(1))
        .execute(&f.pool)
        .await
        .unwrap();

    let reply = f
        .ok(
            &operator,
            "InspectRequest",
            json!({"requestId": request.to_string()}),
        )
        .await;
    let body = &reply.body;
    assert_eq!(body["inspectedRequestId"], request.to_string());
    assert_eq!(body["tenantId"], tenant.to_string());
    assert_eq!(body["capability"], "CAPABILITY_TRANSLATOR");
    assert_eq!(body["state"], "REQUEST_STATE_UNKNOWN");
    assert_eq!(body["errorReason"], "ERROR_REASON_TIMEOUT");
    assert_eq!(body["model"], "openai/gpt-6-luna");
    let attempts = body["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["id"], attempt.to_string());
    assert_eq!(attempts[0]["generationId"], "gen_1");
    assert_eq!(attempts[0]["usageKnown"], true);
    assert!(attempts[0].get("delivered").is_none_or(|v| v == false));
    assert_eq!(attempts[0]["deliveryFailures"], 2);
    assert!(
        attempts[0]["catalogPriceJson"]
            .as_str()
            .unwrap()
            .contains("input_usd_per_token")
    );
    assert!(
        attempts[0]["measuredUsageJson"]
            .as_str()
            .unwrap()
            .contains("input_tokens")
    );
    assert!(attempts[0]["priceEffectiveAt"].is_string());
    assert!(attempts[0]["occurredAt"].is_string());
    assert!(attempts[1].get("usageKnown").is_none_or(|v| v == false));
    assert_eq!(attempts[1]["id"], pending.to_string());

    // Inspection is read-only but audited, with no reason and the inspected target.
    let rows = audit_rows(&f).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String, _>("action"), "request.inspect");
    assert_eq!(rows[0].get::<String, _>("reason"), "");
    assert_eq!(rows[0].get::<Option<Uuid>, _>("target_id"), Some(request));
    assert_eq!(rows[0].get::<Option<Uuid>, _>("tenant_id"), Some(tenant));

    f.fails(
        Some(&operator),
        "InspectRequest",
        json!({"requestId": id().to_string()}),
        "not_found",
        reason::INVALID_INPUT,
    )
    .await;
    f.fails(
        Some(&operator),
        "InspectRequest",
        json!({"requestId": "nope"}),
        "invalid_argument",
        reason::INVALID_INPUT,
    )
    .await;
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn execution_rpcs_are_registered_but_unimplemented_and_request_ids_are_server_issued() {
    let f = Fixture::setup().await;
    let (user, _) = f.user("user", None);
    for (method, body) in [
        (
            "TranslateTextArray",
            json!({"tenantId": id().to_string(), "outputLanguage": "ko", "texts": ["a"]}),
        ),
        (
            "ExtractDataFromText",
            json!({"tenantId": id().to_string(), "text": "a", "jsonSchema": "{}"}),
        ),
        (
            "AssociateGeneration",
            json!({"attemptId": id().to_string(), "generationId": "g", "reason": "recovery"}),
        ),
    ] {
        let reply = f.fails(Some(&user), method, body, "unimplemented", 0).await;
        assert!(reply.body["message"].as_str().unwrap().contains(method));
        assert!(
            !reply.body["message"].as_str().unwrap().contains("INTERNAL"),
            "{}",
            reply.body
        );
    }
    // Every response, success or failure, carries a fresh UUID v7 request ID and ignores any
    // client-supplied value.
    let a = f.call(None, "GetSettings", json!({})).await;
    let b = f.call(None, "GetSettings", json!({})).await;
    assert!(server_common::canonical_id(&a.request_id_header));
    assert_ne!(a.request_id_header, b.request_id_header);
    assert_ne!(a.request_id_header, "client-supplied-value");
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn health_readiness_and_metrics_endpoints() {
    let f = Fixture::setup().await;
    let get = |path: &'static str| {
        let url = format!("{}{path}", f.base);
        let http = f.http.clone();
        async move { http.get(url).send().await.unwrap() }
    };
    assert_eq!(get("/healthz").await.status(), 200);
    // Ready only with a compatible schema and a synchronized catalog price.
    assert_eq!(get("/readyz").await.status(), 503);
    sqlx::query("INSERT INTO catalog_prices(id,model,effective_at,input_usd_per_token,output_usd_per_token,synced_at,created_at) VALUES($1,'openai/gpt-6-luna',now(),0.000001,0.000002,now(),now())")
        .bind(id())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(get("/readyz").await.status(), 200);
    // Additional requirements registered by later layers participate in readiness.
    struct Never;
    #[async_trait::async_trait]
    impl ReadinessCheck for Never {
        fn name(&self) -> &'static str {
            "never"
        }
        async fn ready(&self, _: &sqlx::PgPool) -> bool {
            false
        }
    }
    f.platform.add_readiness_check(Arc::new(Never));
    assert_eq!(get("/readyz").await.status(), 503);

    // Metrics use bounded labels and never carry tenants, requests, keys or messages.
    let tenant = id();
    let (owner, _) = f.user("owner", Some((tenant, TenantRole::Owner)));
    let created = f
        .ok(
            &owner,
            "CreateKey",
            json!({"tenantId": tenant.to_string(), "kind": "KEY_KIND_SERVICE", "capabilities": ["CAPABILITY_TRANSLATOR"]}),
        )
        .await;
    f.call(None, "ListKeys", json!({"tenantId": tenant.to_string()}))
        .await;
    let text = get("/metrics").await.text().await.unwrap();
    assert!(
        text.contains("delitools_requests_total{outcome=\"ok\",procedure=\"CreateKey\"} 1"),
        "{text}"
    );
    assert!(text.contains(
        "delitools_requests_total{outcome=\"unauthenticated\",procedure=\"ListKeys\"} 1"
    ));
    assert!(text.contains("delitools_failures_total{kind=\"authentication\"}"));
    for forbidden in [
        tenant.to_string(),
        created.body["secret"].as_str().unwrap().to_owned(),
        created.request_id_header.clone(),
        "owner".to_owned(),
    ] {
        assert!(!text.contains(&forbidden), "metrics leak {forbidden}");
    }
    f.finish().await;
}

struct CountingTask {
    name: TaskName,
    runs: Arc<AtomicU32>,
    fail: bool,
}
#[async_trait::async_trait]
impl BackgroundTask for CountingTask {
    fn name(&self) -> TaskName {
        self.name
    }
    fn interval(&self, _: &delitools_api::settings::Settings) -> Duration {
        Duration::from_millis(30)
    }
    async fn run_once(&self, _: &TaskContext) -> anyhow::Result<()> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            anyhow::bail!("simulated failure")
        }
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn worker_runs_registered_tasks_survives_failures_and_stops_gracefully() {
    let f = Fixture::setup().await;
    let now = store::now();
    let expired = now - chrono::Duration::days(31);
    sqlx::query("INSERT INTO audit_records(id,actor,action,reason,request_id,created_at) VALUES($1,'operator:x','settings.update','r',$2,$3)")
        .bind(id())
        .bind(id())
        .bind(expired)
        .execute(&f.pool)
        .await
        .unwrap();
    let healthy = Arc::new(AtomicU32::new(0));
    let failing = Arc::new(AtomicU32::new(0));
    let mut worker = Worker::new(f.platform.clone());
    worker.register(Arc::new(CountingTask {
        name: TaskName::UsageDelivery,
        runs: healthy.clone(),
        fail: false,
    }));
    worker.register(Arc::new(CountingTask {
        name: TaskName::PriceSync,
        runs: failing.clone(),
        fail: true,
    }));
    assert!(worker.task_names().contains(&TaskName::Retention));
    assert!(worker.task_names().contains(&TaskName::BacklogSampler));
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(async move { worker.run(stopped).await });
    for _ in 0..100 {
        if healthy.load(Ordering::SeqCst) >= 3 && failing.load(Ordering::SeqCst) >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        healthy.load(Ordering::SeqCst) >= 3,
        "healthy task keeps running"
    );
    assert!(
        failing.load(Ordering::SeqCst) >= 3,
        "a failing task is retried, not abandoned"
    );
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_records")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0, "retention ran at startup");
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("worker stops after shutdown")
        .unwrap()
        .unwrap();
    let metrics = f.platform.metrics.encode();
    assert!(metrics.contains("delitools_worker_runs_total{outcome=\"error\",task=\"price_sync\"}"));
    assert!(metrics.contains("delitools_worker_runs_total{outcome=\"ok\",task=\"retention\"}"));
    assert!(metrics.contains("delitools_retention_deleted_total{table=\"audit_records\"} 1"));
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn an_installed_execution_service_replaces_the_placeholder() {
    struct Echo;
    #[async_trait::async_trait]
    impl ExecutionService for Echo {
        async fn translate_text_array(
            &self,
            call: CallContext,
            request: delino_protocol::delitools::v1::TranslateTextArrayRequest,
        ) -> Result<
            delino_protocol::delitools::v1::TranslateTextArrayResponse,
            delitools_api::error::Fault,
        > {
            let tenant: Uuid = request
                .tenant_id
                .parse()
                .map_err(|_| delitools_api::error::Fault::invalid_input("tenantId"))?;
            let principal = call
                .platform
                .authenticate_execution(
                    &call.authorization,
                    tenant,
                    delino_protocol::delitools::v1::Capability::CAPABILITY_TRANSLATOR,
                )
                .await?;
            assert!(principal.user_id.is_some());
            Ok(delino_protocol::delitools::v1::TranslateTextArrayResponse {
                request_id: call.request_id,
                texts: request.texts,
                ..Default::default()
            })
        }
        async fn extract_data_from_text(
            &self,
            _: CallContext,
            _: delino_protocol::delitools::v1::ExtractDataFromTextRequest,
        ) -> Result<
            delino_protocol::delitools::v1::ExtractDataFromTextResponse,
            delitools_api::error::Fault,
        > {
            Err(delitools_api::error::Fault::not_implemented("x"))
        }
        async fn associate_generation(
            &self,
            _: CallContext,
            _: delino_protocol::delitools::v1::AssociateGenerationRequest,
        ) -> Result<
            delino_protocol::delitools::v1::AssociateGenerationResponse,
            delitools_api::error::Fault,
        > {
            Err(delitools_api::error::Fault::not_implemented("x"))
        }
    }
    let f = Fixture::setup().await;
    let app = delitools_api::http::router(Arc::new(delitools_api::Server::with_execution(
        f.platform.clone(),
        Arc::new(Echo),
    )));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let tenant = id();
    let (user, _) = f.user("user", Some((tenant, TenantRole::Member)));
    let response: Value = f
        .http
        .post(format!(
            "{base}/delitools.v1.DeliToolsService/TranslateTextArray"
        ))
        .bearer_auth(&user)
        .json(&json!({"tenantId": tenant.to_string(), "outputLanguage": "ko", "texts": ["a", "b"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["texts"], json!(["a", "b"]));
    assert!(server_common::canonical_id(
        response["requestId"].as_str().unwrap()
    ));
    server.abort();
    f.finish().await;
}
