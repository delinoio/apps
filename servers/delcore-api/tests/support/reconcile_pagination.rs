// SPDX-License-Identifier: FSL-1.1-ALv2
//! Reconciliation of payments that sit beyond the first page of the provider's transaction list.
use super::{Fixture, key};
use delcore_api::{config::Config, worker::Worker};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

const BASE: &str = "{BASE}";

/// Loopback Paddle fixture. Pages are keyed by the `after` cursor of the request ("" is the first
/// page) and may contain `{BASE}`, which is replaced by the fixture origin. Every request is
/// recorded as `METHOD path?query` together with whether it carried the fixture token.
pub(super) struct Provider {
    url: String,
    pages: Arc<Mutex<HashMap<String, (u16, String)>>>,
    log: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Provider {
    pub(super) async fn start() -> Self {
        let pages: Arc<Mutex<HashMap<String, (u16, String)>>> = Arc::default();
        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (served, recorded, origin) = (pages.clone(), log.clone(), url.clone());
        let app = axum::Router::new().fallback(
            move |method: axum::http::Method,
                  uri: axum::http::Uri,
                  headers: axum::http::HeaderMap| {
                let (served, recorded, origin) = (served.clone(), recorded.clone(), origin.clone());
                async move {
                    let token = headers
                        .get("authorization")
                        .is_some_and(|v| v == "Bearer fixture");
                    recorded.lock().unwrap().push(format!(
                        "{method} {} token={token}",
                        uri.path_and_query().unwrap()
                    ));
                    let after = uri
                        .query()
                        .unwrap_or("")
                        .split('&')
                        .find_map(|pair| pair.strip_prefix("after="))
                        .unwrap_or("")
                        .to_string();
                    let (status, body) = served
                        .lock()
                        .unwrap()
                        .get(&after)
                        .cloned()
                        .unwrap_or((404, String::new()));
                    (
                        axum::http::StatusCode::from_u16(status).unwrap(),
                        body.replace(BASE, &origin),
                    )
                }
            },
        );
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            url,
            pages,
            log,
            task,
        }
    }
    pub(super) fn page(&self, after: &str, body: Value) {
        self.pages
            .lock()
            .unwrap()
            .insert(after.into(), (200, body.to_string()));
    }
    pub(super) fn raw(&self, after: &str, status: u16, body: &str) {
        self.pages
            .lock()
            .unwrap()
            .insert(after.into(), (status, body.into()));
    }
    pub(super) fn requests(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    pub(super) fn worker(&self, f: &Fixture) -> Worker {
        Worker::new(
            f.db.clone(),
            Config {
                paddle_url: self.url.clone(),
                paddle_token: "fixture".into(),
                email_url: self.url.clone(),
                ..f.config.clone()
            },
        )
        .unwrap()
    }
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// One list response. `next` is the `after` cursor of the following page and is rendered as the
/// provider does, with the filters of the request repeated.
pub(super) fn list(rows: Vec<Value>, filter: &str, next: Option<&str>) -> Value {
    let last = rows
        .last()
        .and_then(|row| row["id"].as_str())
        .map(String::from);
    let pagination = match next {
        Some(after) => json!({
            "per_page": 30,
            "has_more": true,
            "next": format!("{BASE}/transactions?per_page=30&order_by=created_at%5BDESC%5D&{filter}&after={after}"),
        }),
        None => json!({"per_page": 30, "has_more": false}),
    };
    assert!(
        next.is_none() || last.as_deref() == next,
        "fixture cursor must be the last row"
    );
    json!({"data": rows, "meta": {"pagination": pagination}})
}
pub(super) fn unrelated(id: &str, tenant: &uuid::Uuid) -> Value {
    json!({"id": id, "status": "completed", "currency_code": "USD",
        "custom_data": {"delcore_job_id": uuid::Uuid::now_v7().to_string(), "delcore_tenant_id": tenant.to_string()},
        "details": {"totals": {"subtotal": "1000"}}})
}
pub(super) fn payment(id: &str, tenant: &uuid::Uuid, job: &uuid::Uuid, status: &str) -> Value {
    json!({"id": id, "status": status, "currency_code": "USD", "customer_id": "ctm_fixture",
        "subscription_id": "sub_fixture",
        "custom_data": {"delcore_job_id": job.to_string(), "delcore_tenant_id": tenant.to_string()},
        "details": {"totals": {"subtotal": "1000"}}})
}

pub(super) struct Case {
    pub(super) tenant: uuid::Uuid,
    pub(super) job: uuid::Uuid,
    /// Query filter of every list request of this case.
    pub(super) filter: String,
}
impl Case {
    /// Provider transaction ID of the matching payment, unique per case because the ledger and
    /// provider transaction records are keyed by it across cases of one test schema.
    pub(super) fn txn(&self) -> String {
        format!("txn_{}", self.job.simple())
    }
}
/// Creates a tenant with a top-up job that waits for reconciliation. `external` is empty for the
/// customer-filtered list and `sub_fixture` for the subscription-filtered list.
pub(super) async fn case(f: &Fixture, customer: &str, external: &str) -> Case {
    let tenant: uuid::Uuid = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey": key(), "name": "Paginated"}),
        )
        .await["tenant"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    sqlx::query("UPDATE tenants SET external_customer_id=$2 WHERE id=$1")
        .bind(tenant)
        .bind(customer)
        .execute(&f.db)
        .await
        .unwrap();
    let job = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,state,attempts,external_id,available_at,created_at,updated_at) VALUES($1,$2,'topup',$1::text,'{\"amount\":\"10\",\"minor_units\":\"1000\",\"currency\":\"USD\"}','reconcile',0,$3,now(),now(),now())")
        .bind(job).bind(tenant).bind(external).execute(&f.db).await.unwrap();
    let filter = if !external.is_empty() {
        format!("subscription_id={external}")
    } else if !customer.is_empty() {
        format!("customer_id={customer}")
    } else {
        "per_page=30".into()
    };
    Case {
        tenant,
        job,
        filter,
    }
}
pub(super) async fn job_state(f: &Fixture, job: uuid::Uuid) -> (String, String, i32, Value) {
    sqlx::query_as("SELECT state,error_category,attempts,payload FROM jobs WHERE id=$1")
        .bind(job)
        .fetch_one(&f.db)
        .await
        .unwrap()
}
pub(super) async fn credits(f: &Fixture, tenant: uuid::Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM ledger WHERE tenant_id=$1 AND kind='credit'")
        .bind(tenant)
        .fetch_one(&f.db)
        .await
        .unwrap()
}
pub(super) async fn ready(f: &Fixture, job: uuid::Uuid) {
    sqlx::query("UPDATE jobs SET available_at=now() WHERE id=$1")
        .bind(job)
        .execute(&f.db)
        .await
        .unwrap();
}
pub(super) fn gets(provider: &Provider) -> usize {
    provider
        .requests()
        .iter()
        .filter(|r| r.starts_with("GET "))
        .count()
}
pub(super) fn assert_read_only_and_authenticated(provider: &Provider) {
    for request in provider.requests() {
        assert!(
            request.starts_with("GET "),
            "recovery must not mutate: {request}"
        );
        assert!(request.ends_with("token=true"), "{request}");
    }
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn payment_on_the_second_page_is_credited_once_in_both_list_branches() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    for (customer, external) in [
        ("", ""),
        ("ctm_fixture", ""),
        ("ctm_fixture", "sub_fixture"),
    ] {
        let provider = Provider::start().await;
        let worker = provider.worker(&f);
        let c = case(&f, customer, external).await;
        let first = vec![
            unrelated("txn_a1", &c.tenant),
            unrelated("txn_a2", &c.tenant),
        ];
        provider.page("", list(first, &c.filter, Some("txn_a2")));
        let mut target = payment(&c.txn(), &c.tenant, &c.job, "completed");
        if !external.is_empty() {
            // The subscription list also attributes a payment through its credit item.
            target["custom_data"] = Value::Null;
            target["items"] = json!([{"quantity": 1, "price": {
                "id": "pri_fixture",
                "product_id": f.config.paddle_credit_product_id,
                "custom_data": {"delcore_job_id": c.job.to_string(), "delcore_tenant_id": c.tenant.to_string()}}}]);
        }
        provider.page("txn_a2", list(vec![target], &c.filter, None));
        worker.step().await.unwrap();
        let (state, category, attempts, payload) = job_state(&f, c.job).await;
        assert_eq!(
            (state.as_str(), category.as_str(), attempts),
            ("reconcile", "reconcile_scan_pending", 0)
        );
        assert_eq!(payload["reconcile_scan"]["after"], "txn_a2");
        assert_eq!(
            payload["amount"], "10",
            "payment intent fields stay untouched"
        );
        assert_eq!(credits(&f, c.tenant).await, 0);
        assert_eq!(gets(&provider), 1, "one page per step");
        worker.step().await.unwrap();
        let (state, _, _, _) = job_state(&f, c.job).await;
        assert_eq!(state, "done", "{customer}/{external}");
        assert_eq!(credits(&f, c.tenant).await, 1);
        assert_eq!(gets(&provider), 2);
        // Replays neither request the provider again nor credit twice.
        worker.step().await.unwrap();
        worker.step().await.unwrap();
        assert_eq!(credits(&f, c.tenant).await, 1);
        assert_eq!(provider.requests().len(), 2);
        assert_read_only_and_authenticated(&provider);
        for request in provider.requests() {
            assert!(
                request.contains(&c.filter) || c.filter == "per_page=30",
                "{request}"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn first_page_match_does_not_read_further_pages() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let provider = Provider::start().await;
    let worker = provider.worker(&f);
    let c = case(&f, "ctm_fixture", "").await;
    let rows = vec![
        payment(&c.txn(), &c.tenant, &c.job, "completed"),
        unrelated("txn_a2", &c.tenant),
    ];
    provider.page("", list(rows, &c.filter, Some("txn_a2")));
    worker.step().await.unwrap();
    assert_eq!(job_state(&f, c.job).await.0, "done");
    assert_eq!(credits(&f, c.tenant).await, 1);
    assert_eq!(gets(&provider), 1);
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn scan_progress_survives_restart_and_never_consumes_attempts() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let provider = Provider::start().await;
    let c = case(&f, "ctm_fixture", "").await;
    for (id, next) in [("", "txn_p1"), ("txn_p1", "txn_p2"), ("txn_p2", "txn_p3")] {
        let row = unrelated(next, &c.tenant);
        provider.page(id, list(vec![row], &c.filter, Some(next)));
    }
    provider.page(
        "txn_p3",
        list(
            vec![payment(&c.txn(), &c.tenant, &c.job, "completed")],
            &c.filter,
            None,
        ),
    );
    provider.worker(&f).step().await.unwrap();
    assert_eq!(
        job_state(&f, c.job).await.3["reconcile_scan"]["after"],
        "txn_p1"
    );
    // A crash while the job runs returns it to reconciliation with its checkpoint.
    sqlx::query("UPDATE jobs SET state='running',attempts=1 WHERE id=$1")
        .bind(c.job)
        .execute(&f.db)
        .await
        .unwrap();
    let restarted = provider.worker(&f);
    restarted.recover().await.unwrap();
    let (state, _, _, payload) = job_state(&f, c.job).await;
    assert_eq!(state, "reconcile");
    assert_eq!(payload["reconcile_scan"]["after"], "txn_p1");
    sqlx::query("UPDATE jobs SET attempts=0 WHERE id=$1")
        .bind(c.job)
        .execute(&f.db)
        .await
        .unwrap();
    for expected in ["txn_p2", "txn_p3"] {
        restarted.step().await.unwrap();
        let (state, category, attempts, payload) = job_state(&f, c.job).await;
        assert_eq!(
            (state.as_str(), category.as_str(), attempts),
            ("reconcile", "reconcile_scan_pending", 0)
        );
        assert_eq!(payload["reconcile_scan"]["after"], expected);
    }
    restarted.step().await.unwrap();
    assert_eq!(job_state(&f, c.job).await.0, "done");
    assert_eq!(credits(&f, c.tenant).await, 1);
    let cursors: Vec<_> = provider
        .requests()
        .iter()
        .map(|r| {
            r.split("after=")
                .nth(1)
                .map(|v| v.split(' ').next().unwrap().to_string())
        })
        .collect();
    assert_eq!(
        cursors,
        [
            None,
            Some("txn_p1".into()),
            Some("txn_p2".into()),
            Some("txn_p3".into())
        ]
    );
    assert_read_only_and_authenticated(&provider);
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn exhausted_history_keeps_branch_results_and_a_fresh_scan_finds_new_payments() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let provider = Provider::start().await;
    let worker = provider.worker(&f);
    for external in ["", "sub_fixture"] {
        let c = case(&f, "ctm_fixture", external).await;
        provider.page(
            "",
            list(
                vec![unrelated("txn_a1", &c.tenant)],
                &c.filter,
                Some("txn_a1"),
            ),
        );
        provider.page(
            "txn_a1",
            list(vec![unrelated("txn_a2", &c.tenant)], &c.filter, None),
        );
        worker.step().await.unwrap();
        worker.step().await.unwrap();
        let (state, category, attempts, payload) = job_state(&f, c.job).await;
        let want = if external.is_empty() {
            ("failed", "unknown_payment_outcome")
        } else {
            ("reconcile", "awaiting_payment")
        };
        assert_eq!((state.as_str(), category.as_str()), want);
        assert_eq!(attempts, 1, "only the exhausted second step counts");
        assert!(
            payload.get("reconcile_scan").is_none(),
            "checkpoint is cleared"
        );
        assert_eq!(credits(&f, c.tenant).await, 0);
        // A new completed payment appears; an explicit retry or the next poll scans from the start.
        provider.page(
            "",
            list(
                vec![payment(&c.txn(), &c.tenant, &c.job, "completed")],
                &c.filter,
                None,
            ),
        );
        sqlx::query("UPDATE jobs SET state='reconcile',available_at=now() WHERE id=$1")
            .bind(c.job)
            .execute(&f.db)
            .await
            .unwrap();
        worker.step().await.unwrap();
        assert_eq!(job_state(&f, c.job).await.0, "done");
        assert_eq!(credits(&f, c.tenant).await, 1);
    }
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn invalid_pagination_is_a_bounded_error_and_never_leaves_the_provider_origin() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let evil = Provider::start().await;
    evil.raw("", 200, r#"{"data":[]}"#);
    let provider = Provider::start().await;
    let worker = provider.worker(&f);
    let filter = "customer_id=ctm_fixture";
    let ok = "per_page=30&order_by=created_at%5BDESC%5D&customer_id=ctm_fixture";
    let page = |next: String, has_more: Value| {
        json!({"data":[{"id":"txn_a1","status":"completed","currency_code":"USD"}],
            "meta":{"pagination":{"has_more":has_more,"next":next}}})
    };
    let cases = [
        (
            "untrusted origin",
            page(
                format!("{}/transactions?{ok}&after=txn_a1", evil.url),
                json!(true),
            ),
        ),
        (
            "other route",
            page(
                format!("{BASE}/subscriptions?{ok}&after=txn_a1"),
                json!(true),
            ),
        ),
        (
            "changed filter",
            page(
                format!(
                    "{BASE}/transactions?per_page=30&order_by=created_at%5BDESC%5D&after=txn_a1"
                ),
                json!(true),
            ),
        ),
        (
            "cursor not last row",
            page(
                format!("{BASE}/transactions?{ok}&after=txn_zz"),
                json!(true),
            ),
        ),
        (
            "malformed flag",
            page(
                format!("{BASE}/transactions?{ok}&after=txn_a1"),
                json!("yes"),
            ),
        ),
    ];
    for (name, body) in cases {
        let c = case(&f, "ctm_fixture", "").await;
        provider.page("", body);
        worker.step().await.unwrap();
        let (state, category, attempts, payload) = job_state(&f, c.job).await;
        assert_eq!(
            (state.as_str(), category.as_str(), attempts),
            ("reconcile", "payment_ambiguous", 1),
            "{name}"
        );
        assert!(payload.get("reconcile_scan").is_none(), "{name}");
        assert_eq!(credits(&f, c.tenant).await, 0, "{name}");
        sqlx::query("UPDATE jobs SET state='failed' WHERE id=$1")
            .bind(c.job)
            .execute(&f.db)
            .await
            .unwrap();
    }
    assert!(
        evil.requests().is_empty(),
        "the token must not reach another origin"
    );
    assert_read_only_and_authenticated(&provider);
    // A repeated cursor is rejected: the checkpoint says page "txn_a1" was read already.
    let c = case(&f, "ctm_fixture", "").await;
    sqlx::query("UPDATE jobs SET payload=payload||jsonb_build_object('reconcile_scan',jsonb_build_object('context','customer:ctm_fixture','after','txn_a1')) WHERE id=$1").bind(c.job).execute(&f.db).await.unwrap();
    provider.page(
        "txn_a1",
        page(
            format!("{BASE}/transactions?{ok}&after=txn_a1"),
            json!(true),
        ),
    );
    worker.step().await.unwrap();
    let (state, category, _, payload) = job_state(&f, c.job).await;
    assert_eq!(
        (state.as_str(), category.as_str()),
        ("reconcile", "payment_ambiguous")
    );
    assert_eq!(
        payload["reconcile_scan"]["after"], "txn_a1",
        "completed progress is kept"
    );
    // A terminal page whose next URL leads to another origin is not followed.
    let c = case(&f, "ctm_fixture", "").await;
    let before = provider.requests().len();
    provider.page("", json!({"data":[],"meta":{"pagination":{"has_more":false,"next":format!("{}/transactions?{ok}&after=x", evil.url)}}}));
    worker.step().await.unwrap();
    assert_eq!(job_state(&f, c.job).await.1, "unknown_payment_outcome");
    assert_eq!(provider.requests().len(), before + 1);
    assert!(evil.requests().is_empty());
    let _ = filter;
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn a_cycle_over_several_pages_ends_through_the_retry_policy() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let provider = Provider::start().await;
    let worker = provider.worker(&f);
    let c = case(&f, "ctm_fixture", "").await;
    // Each page is consistent on its own: its cursor names its last row and differs from the
    // requested one. Together the pages form the cycle txn_a -> txn_b -> txn_a.
    for (id, next) in [("", "txn_a"), ("txn_a", "txn_b"), ("txn_b", "txn_a")] {
        provider.page(
            id,
            list(vec![unrelated(next, &c.tenant)], &c.filter, Some(next)),
        );
    }
    for expected in ["txn_a", "txn_b"] {
        worker.step().await.unwrap();
        let (state, category, attempts, payload) = job_state(&f, c.job).await;
        assert_eq!(
            (state.as_str(), category.as_str(), attempts),
            ("reconcile", "reconcile_scan_pending", 0)
        );
        assert_eq!(payload["reconcile_scan"]["after"], expected);
    }
    // The page after txn_b points back to txn_a, which was requested already.
    worker.step().await.unwrap();
    let (state, category, attempts, payload) = job_state(&f, c.job).await;
    assert_eq!(
        (state.as_str(), category.as_str(), attempts),
        ("reconcile", "payment_ambiguous", 1)
    );
    assert_eq!(
        payload["reconcile_scan"]["after"], "txn_b",
        "completed progress is kept"
    );
    // Every further step spends an attempt, so the retry limit ends the scan with an error that
    // needs operator review instead of requeueing it for ever.
    let mut steps = 0;
    while job_state(&f, c.job).await.0 == "reconcile" {
        steps += 1;
        assert!(steps <= 50, "the cycle must end through the retry limit");
        ready(&f, c.job).await;
        worker.step().await.unwrap();
    }
    let (state, category, _, _) = job_state(&f, c.job).await;
    assert_eq!(
        (state.as_str(), category.as_str()),
        ("failed", "retry_exhausted")
    );
    assert_eq!(credits(&f, c.tenant).await, 0);
    assert_read_only_and_authenticated(&provider);
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn transient_error_on_a_later_page_keeps_the_completed_page() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let provider = Provider::start().await;
    let worker = provider.worker(&f);
    let c = case(&f, "ctm_fixture", "").await;
    provider.page(
        "",
        list(
            vec![unrelated("txn_a1", &c.tenant)],
            &c.filter,
            Some("txn_a1"),
        ),
    );
    provider.raw("txn_a1", 503, "");
    worker.step().await.unwrap();
    ready(&f, c.job).await;
    worker.step().await.unwrap();
    let (state, category, attempts, payload) = job_state(&f, c.job).await;
    assert_eq!(
        (state.as_str(), category.as_str(), attempts),
        ("reconcile", "payment_ambiguous", 1)
    );
    assert_eq!(payload["reconcile_scan"]["after"], "txn_a1");
    provider.page(
        "txn_a1",
        list(
            vec![payment(&c.txn(), &c.tenant, &c.job, "completed")],
            &c.filter,
            None,
        ),
    );
    ready(&f, c.job).await;
    worker.step().await.unwrap();
    assert_eq!(job_state(&f, c.job).await.0, "done");
    assert_eq!(credits(&f, c.tenant).await, 1);
    assert_eq!(gets(&provider), 3, "page one is not read again");
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn mismatched_or_unpaid_payments_on_later_pages_never_credit() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let provider = Provider::start().await;
    let worker = provider.worker(&f);
    // Another tenant's, another job's, and a currency mismatch are skipped or rejected; a pending
    // payment keeps waiting. None of them credits the tenant.
    for (name, want) in [
        ("wrong tenant", "unknown_payment_outcome"),
        ("wrong job", "unknown_payment_outcome"),
        ("pending", "awaiting_payment"),
        ("currency", "payment_mismatch"),
    ] {
        let c = case(&f, "ctm_fixture", "").await;
        let mut target = payment(&c.txn(), &c.tenant, &c.job, "completed");
        match name {
            "wrong tenant" => {
                target["custom_data"]["delcore_tenant_id"] = json!(uuid::Uuid::now_v7().to_string())
            }
            "wrong job" => {
                target["custom_data"]["delcore_job_id"] = json!(uuid::Uuid::now_v7().to_string())
            }
            "pending" => target["status"] = json!("billed"),
            _ => target["currency_code"] = json!("EUR"),
        }
        provider.page(
            "",
            list(
                vec![unrelated("txn_a1", &c.tenant)],
                &c.filter,
                Some("txn_a1"),
            ),
        );
        provider.page("txn_a1", list(vec![target], &c.filter, None));
        worker.step().await.unwrap();
        worker.step().await.unwrap();
        let (state, category, _, _) = job_state(&f, c.job).await;
        assert_eq!(category, want, "{name} ({state})");
        assert_eq!(credits(&f, c.tenant).await, 0, "{name}");
    }
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database"]
async fn operator_retry_starts_a_fresh_scan() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let c = case(&f, "ctm_fixture", "").await;
    sqlx::query("UPDATE jobs SET state='failed',error_category='retry_exhausted',payload=payload||jsonb_build_object('reconcile_scan',jsonb_build_object('context','customer:ctm_fixture','after','txn_a1')) WHERE id=$1")
        .bind(c.job)
        .execute(&f.db)
        .await
        .unwrap();
    f.rpc(
        "owner",
        "RetryJob",
        json!({"requestKey": key(), "jobId": c.job.to_string(), "reason": "retry stuck scan"}),
    )
    .await;
    let (state, _, _, payload) = job_state(&f, c.job).await;
    assert_eq!(state, "reconcile");
    assert!(payload.get("reconcile_scan").is_none());
    assert_eq!(payload["amount"], "10");
}
