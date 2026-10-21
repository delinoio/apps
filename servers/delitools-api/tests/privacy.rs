// SPDX-License-Identifier: FSL-1.1-ALv2
//! Raw content and secrets must be absent from database rows, logs and metrics. The test runs
//! every execution path with distinctive sentinels and then searches the captured evidence.
//! It needs a disposable PostgreSQL database in `TEST_DATABASE_URL`.
mod support;
use serde_json::json;
use std::{
    io::Write,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use support::{
    Env, echo_translations, extraction_content,
    mocks::{MemberRole, Script, UsageSpec, reply},
    reason,
};

fn captured() -> &'static Mutex<Vec<u8>> {
    static LOGS: OnceLock<Mutex<Vec<u8>>> = OnceLock::new();
    LOGS.get_or_init(|| Mutex::new(Vec::new()))
}

struct Capture;
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        captured()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

const SOURCE: &str = "zxq-source-sentinel-7781 personal@example.com";
const FIELD: &str = "zxqfield7782";
const IDEMPOTENCY: &str = "idem-zxq-7783";
const GATEWAY_KEY: &str = "gateway-key-zxq-7784";
const CLIENT_SECRET: &str = "client-secret-zxq-7785";

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn content_and_secrets_never_reach_rows_logs_or_metrics() {
    // The global subscriber captures every task's output, including spawned pipeline tasks.
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(|| Capture)
            .with_env_filter(tracing_subscriber::EnvFilter::new(
                "delitools_api=trace,warn",
            ))
            .finish(),
    )
    .expect("this test binary owns the global subscriber");

    let env = Env::start(
        &[
            ("GATEWAY_KEY", GATEWAY_KEY),
            ("CLIENT_SECRET", CLIENT_SECRET),
        ],
        false,
    )
    .await;
    let (owner, _) = env.user("owner", Some(MemberRole::Owner));
    let (service, _) = env
        .create_key(
            &owner,
            "KEY_KIND_SERVICE",
            &["CAPABILITY_TRANSLATOR", "CAPABILITY_EXTRACTOR"],
        )
        .await;
    let schema = format!(
        r#"{{"type":"object","properties":{{"{FIELD}":{{"type":"string"}}}},"required":["{FIELD}"],"additionalProperties":false}}"#
    );

    // Success, malformed output, provider failure, unknown usage and an unsupported schema.
    env.gateway.respond(
        |n, body| match body["response_format"]["json_schema"]["name"].as_str() {
            Some("delitools_extraction") => reply(
                &extraction_content("extracted", json!({FIELD: "value zxq-result-7786"})),
                &format!("gen_p{n}"),
                Some(UsageSpec::full(10, 2, 0, 0)),
            ),
            _ => reply(&echo_translations(body), &format!("gen_p{n}"), None),
        },
    );
    let translated = env
        .fx
        .ok(
            &service,
            "TranslateTextArray",
            env.translate_body(&[SOURCE], Some(IDEMPOTENCY)),
        )
        .await;
    assert!(
        translated.body["texts"][0]
            .as_str()
            .unwrap()
            .contains("zxq-source-sentinel-7781")
    );
    env.fx
        .ok(
            &service,
            "ExtractDataFromText",
            env.extract_body(SOURCE, &schema, None),
        )
        .await;
    env.fx
        .fails(
            Some(&service),
            "TranslateTextArray",
            env.translate_body(&[SOURCE], Some(IDEMPOTENCY)),
            "already_exists",
            reason::ALREADY_PROCESSED,
        )
        .await;
    env.gateway.sequence(vec![reply(
        "not json zxq-bad-7787",
        "gen_bad",
        Some(UsageSpec::full(5, 1, 0, 0)),
    )]);
    env.fx
        .fails(
            Some(&service),
            "TranslateTextArray",
            env.translate_body(&[SOURCE], None),
            "data_loss",
            reason::INVALID_OUTPUT,
        )
        .await;
    env.gateway.sequence(vec![Script::Status(503)]);
    env.fx
        .fails(
            Some(&service),
            "TranslateTextArray",
            env.translate_body(&[SOURCE], None),
            "unavailable",
            reason::PROVIDER,
        )
        .await;
    env.fx
        .fails(
            Some(&service),
            "ExtractDataFromText",
            env.extract_body(
                SOURCE,
                &format!(r#"{{"type":"object","x-{FIELD}":1}}"#),
                None,
            ),
            "invalid_argument",
            reason::INVALID_SCHEMA,
        )
        .await;
    env.make_due().await;
    env.fx
        .usage
        .as_ref()
        .unwrap()
        .run_deliveries()
        .await
        .unwrap();
    env.fx
        .usage
        .as_ref()
        .unwrap()
        .run_reconciliation()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let operator = env.fx.operator();
    let request: String =
        sqlx::query_scalar("SELECT id::text FROM requests ORDER BY created_at LIMIT 1")
            .fetch_one(&env.fx.pool)
            .await
            .unwrap();
    let inspected = env
        .fx
        .ok(&operator, "InspectRequest", json!({"requestId": request}))
        .await;

    // Assemble all evidence.
    let mut evidence = String::new();
    for table in [
        "settings",
        "api_keys",
        "audit_records",
        "operational_events",
        "catalog_prices",
        "requests",
        "attempts",
        "idempotency_records",
    ] {
        let dump: Option<String> = sqlx::query_scalar(&format!(
            "SELECT string_agg(t::text, E'\\n') FROM {table} t"
        ))
        .fetch_one(&env.fx.pool)
        .await
        .unwrap();
        evidence.push_str(&format!("[{table}]\n{}\n", dump.unwrap_or_default()));
    }
    let database = evidence.clone();
    let logs =
        String::from_utf8_lossy(&captured().lock().unwrap_or_else(|e| e.into_inner())).into_owned();
    assert!(logs.contains("rpc completed"), "the capture works");
    let metrics = env.fx.platform.metrics.encode();
    let inspection = inspected.body.to_string();
    let key_secret = env
        .fx
        .call(
            Some(&owner),
            "ListKeys",
            json!({"tenantId": env.tenant.to_string(), "kind": "KEY_KIND_SERVICE"}),
        )
        .await
        .body
        .to_string();

    let forbidden = [
        "zxq-source-sentinel-7781",
        "personal@example.com",
        FIELD,
        "zxq-result-7786",
        "zxq-bad-7787",
        GATEWAY_KEY,
        CLIENT_SECRET,
        "upstream body that must never be logged",
        "[ko]",
    ];
    for needle in forbidden {
        assert!(!database.contains(needle), "database contains {needle}");
        assert!(!logs.contains(needle), "logs contain {needle}");
        assert!(!metrics.contains(needle), "metrics contain {needle}");
        assert!(!inspection.contains(needle), "inspection contains {needle}");
        assert!(
            !key_secret.contains(needle),
            "key listing contains {needle}"
        );
    }
    // The only caller-provided identifier that is stored is the idempotency key itself; it stays
    // out of logs, metrics and inspection.
    assert!(database.contains(IDEMPOTENCY));
    for text in [&logs, &metrics, &inspection] {
        assert!(!text.contains(IDEMPOTENCY));
    }
    // No credential is recoverable: neither the API key secret, the bearer tokens nor the
    // service tokens appear anywhere.
    let created = env
        .fx
        .ok(
            &owner,
            "CreateKey",
            json!({"tenantId": env.tenant.to_string(), "kind": "KEY_KIND_SERVICE", "capabilities": ["CAPABILITY_TRANSLATOR"]}),
        )
        .await;
    let secret = created.body["secret"].as_str().unwrap().to_owned();
    let secret_part = secret.rsplit('_').next().unwrap().to_owned();
    let after =
        String::from_utf8_lossy(&captured().lock().unwrap_or_else(|e| e.into_inner())).into_owned();
    assert!(!after.contains(&secret_part) && !after.contains(&secret));
    for credential in [
        &service,
        &owner,
        &secret_part,
        &"m2m-authorize".to_owned(),
        &"m2m-usage:write".to_owned(),
    ] {
        assert!(
            !database.contains(credential.as_str()),
            "database holds a credential"
        );
        assert!(
            !after.contains(credential.as_str()),
            "logs hold a credential"
        );
        assert!(!metrics.contains(credential.as_str()));
    }
    let stored: String = sqlx::query_scalar("SELECT string_agg(k::text, ' ') FROM api_keys k")
        .fetch_one(&env.fx.pool)
        .await
        .unwrap();
    assert!(!stored.contains(&secret_part));
    env.finish().await;
}
