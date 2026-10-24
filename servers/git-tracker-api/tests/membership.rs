// SPDX-License-Identifier: FSL-1.1-ALv2
//! Mocked-provider coverage for pull request commit membership. The mock honours GitHub's
//! documented 250-commit cap on `GET pulls/{n}/commits` and serves the full list through the
//! GraphQL `pullRequest.commits` connection. No network, database or credential is involved.
use axum::{
    Json, Router,
    extract::{Request, State},
    http::StatusCode,
};
use delino_protocol::gittracker::v1::{Context, Repository};
use git_tracker_api::github::GitHub;
use jsonwebtoken::EncodingKey;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const KEY: &[u8] = include_bytes!("fixtures/github-test-key.pem");
const REST_CAP: usize = 250;

/// Lowercase 40-character hexadecimal representation of `i`.
fn sha(i: usize) -> String {
    format!("{i:040x}")
}

/// Mutable provider behaviour for one test.
struct Provider {
    /// Current PR head returned by `GET pulls/1`.
    head: String,
    /// Full ordered PR commit list; REST serves at most `REST_CAP` entries.
    commits: Vec<String>,
    /// Overrides REST rows entirely when set (for example to hide the target).
    rest_rows: Option<Vec<String>>,
    state: &'static str,
    /// Replaces the generated GraphQL response when set. Receives the request variables.
    graphql: Option<GraphqlReply>,
    /// Pages returned by the generated GraphQL response, as `(oids, hasNextPage)`.
    pages: Option<Vec<(Vec<String>, bool)>>,
    total: Option<usize>,
    delay: Duration,
    /// Recorded `(path, authorization, body)` for every request.
    calls: Vec<(String, String, Value)>,
}

type GraphqlReply = Box<dyn Fn(&Value) -> (StatusCode, Value) + Send + Sync>;
type Shared = Arc<Mutex<Provider>>;

fn provider(commits: Vec<String>, head: &str) -> Shared {
    Arc::new(Mutex::new(Provider {
        head: head.into(),
        commits,
        rest_rows: None,
        state: "OPEN",
        graphql: None,
        pages: None,
        total: None,
        delay: Duration::ZERO,
        calls: vec![],
    }))
}

fn graphql_page(p: &Provider, variables: &Value) -> (StatusCode, Value) {
    let after = variables["after"].as_str();
    let pages: Vec<(Vec<String>, bool)> = p.pages.clone().unwrap_or_else(|| {
        let n = p.commits.len();
        p.commits
            .chunks(100)
            .enumerate()
            .map(|(i, c)| (c.to_vec(), (i + 1) * 100 < n))
            .collect()
    });
    let index = after.map_or(0, |c| c.trim_start_matches('c').parse::<usize>().unwrap());
    let (oids, more) = pages.get(index).cloned().unwrap_or_default();
    let total = p.total.unwrap_or(p.commits.len());
    (
        StatusCode::OK,
        json!({"data":{"repository":{"databaseId":42,"pullRequest":{
            "id":"PR_node","number":1,"state":p.state,"baseRefName":"main","headRefOid":p.head,
            "commits":{
                "totalCount":total,
                "nodes":oids.iter().map(|o| json!({"commit":{"oid":o}})).collect::<Vec<_>>(),
                "pageInfo":{"hasNextPage":more,"endCursor":format!("c{}", index + 1)}
            }
        }}}}),
    )
}

async fn handle(State(shared): State<Shared>, request: Request) -> (StatusCode, Json<Value>) {
    let path = request.uri().path().to_owned();
    let query = request.uri().query().unwrap_or("").to_owned();
    let auth = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let body = axum::body::to_bytes(request.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let delay = {
        let mut p = shared.lock().unwrap();
        p.calls.push((path.clone(), auth, body.clone()));
        p.delay
    };
    if path == "/graphql" && !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    let p = shared.lock().unwrap();
    let auth = p.calls.last().unwrap().1.clone();
    let (status, out) = if path == "/app/installations/1/access_tokens" {
        (StatusCode::OK, json!({"token":"installation"}))
    } else if let Some(out) = lifecycle_route(&path, &auth) {
        (StatusCode::OK, out)
    } else if let Some(sha) = path.strip_prefix("/repos/owner/repo/commits/") {
        (StatusCode::OK, json!({"sha":sha,"parents":[]}))
    } else if path == "/repos/owner/repo/pulls/1" {
        (
            StatusCode::OK,
            json!({"base":{"ref":"main","sha":sha(0)},"head":{"sha":p.head}}),
        )
    } else if path == "/repos/owner/repo/pulls/1/commits" {
        let page: usize = query
            .split('&')
            .find_map(|v| v.strip_prefix("page="))
            .unwrap()
            .parse()
            .unwrap();
        let rows: Vec<String> = p
            .rest_rows
            .clone()
            .unwrap_or_else(|| p.commits.iter().take(REST_CAP).cloned().collect());
        let rows: Vec<Value> = rows
            .chunks(100)
            .nth(page - 1)
            .unwrap_or_default()
            .iter()
            .map(|s| json!({"sha":s}))
            .collect();
        (StatusCode::OK, Value::Array(rows))
    } else if path.starts_with("/repos/owner/repo/compare/") {
        (
            StatusCode::OK,
            json!({"merge_base_commit":{"sha":sha(0)},"commits":[]}),
        )
    } else if path == "/graphql" {
        match &p.graphql {
            Some(f) => f(&body["variables"]),
            None => graphql_page(&p, &body["variables"]),
        }
    } else {
        (StatusCode::NOT_FOUND, json!({}))
    };
    (status, Json(out))
}

struct Fixture {
    github: GitHub,
    repository: Repository,
    shared: Shared,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn fixture(shared: Shared) -> Fixture {
    let router = Router::new().fallback(handle).with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    Fixture {
        github: GitHub::new(
            http,
            base,
            "1".into(),
            EncodingKey::from_rsa_pem(KEY).unwrap(),
        ),
        repository: Repository {
            id: "repository".into(),
            github_repository_id: "42".into(),
            installation_id: "1".into(),
            full_name: "owner/repo".into(),
            connected: true,
            ..Default::default()
        },
        shared,
        task,
    }
}

fn context(commit: &str) -> Context {
    Context {
        repository_id: "repository".into(),
        execution_id: "run-t".into(),
        commit: commit.into(),
        branch: "main".into(),
        pull_request: 1,
        ..Default::default()
    }
}

impl Fixture {
    async fn resolve(&self, commit: &str) -> Result<(Vec<String>, String), String> {
        self.github
            .resolve(&self.repository, &context(commit))
            .await
            .map_err(|f| f.to_string())
    }
    fn graphql_calls(&self) -> Vec<(String, String, Value)> {
        self.shared
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| c.0 == "/graphql")
            .cloned()
            .collect()
    }
    fn rest_pages(&self) -> usize {
        self.shared
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| c.0 == "/repos/owner/repo/pulls/1/commits")
            .count()
    }
}

const UNAVAILABLE: &str = "a dependency is unavailable";

fn chain(n: usize) -> Vec<String> {
    (1..=n).map(sha).collect()
}

#[tokio::test]
async fn omitted_member_beyond_rest_cap_is_accepted_through_graphql() {
    // C1..C300 with head C300; REST returns C1..C250 in pages of 100/100/50 and omits C251.
    let f = fixture(provider(chain(300), &sha(300))).await;
    let target = sha(251);
    let (parents, base) = f.resolve(&target).await.unwrap();
    assert!(parents.is_empty());
    assert_eq!(base, sha(0));
    assert_eq!(f.rest_pages(), 3);
    let calls = f.graphql_calls();
    assert_eq!(calls.len(), 3);
    let afters: Vec<Value> = calls
        .iter()
        .map(|c| c.2["variables"]["after"].clone())
        .collect();
    assert_eq!(afters, vec![Value::Null, json!("c1"), json!("c2")]);
    for (_, auth, body) in &calls {
        assert_eq!(auth, "Bearer installation");
        assert_eq!(body["variables"]["owner"], "owner");
        assert_eq!(body["variables"]["name"], "repo");
        assert_eq!(body["variables"]["number"], 1);
    }
}

#[tokio::test]
async fn cap_boundaries_and_target_positions() {
    for count in [249, 250, 251, 300, 301] {
        let f = fixture(provider(chain(count), &sha(count))).await;
        // Last non-head member: C(count-1) sits at the end of the list.
        let target = sha(count - 1);
        f.resolve(&target).await.unwrap();
        // Only members beyond the REST cap need the GraphQL connection.
        assert_eq!(
            !f.graphql_calls().is_empty(),
            count - 1 > REST_CAP,
            "count {count}"
        );
    }
    // Membership does not depend on provider ordering.
    let mut commits = chain(300);
    commits.reverse();
    let f = fixture(provider(commits, &sha(300))).await;
    f.shared.lock().unwrap().rest_rows = Some(chain(250).into_iter().rev().collect());
    f.resolve(&sha(260)).await.unwrap();
    assert!(!f.graphql_calls().is_empty());
}

#[tokio::test]
async fn rest_positive_and_head_shortcut_skip_graphql() {
    let f = fixture(provider(chain(300), &sha(300))).await;
    f.resolve(&sha(10)).await.unwrap();
    assert!(f.graphql_calls().is_empty());
    f.resolve(&sha(300)).await.unwrap();
    assert!(f.graphql_calls().is_empty());
    // Head shortcut does not even list PR commits.
    let rest = f.rest_pages();
    f.resolve(&sha(300)).await.unwrap();
    assert_eq!(f.rest_pages(), rest);
}

#[tokio::test]
async fn non_members_are_rejected_after_complete_traversal() {
    let f = fixture(provider(chain(300), &sha(300))).await;
    // Retrievable repository commits (base ancestor, side parent, unrelated, force-pushed-away).
    for excluded in [sha(0), sha(1000), sha(2000), sha(3000)] {
        assert_eq!(f.resolve(&excluded).await.unwrap_err(), UNAVAILABLE);
    }
    assert_eq!(f.graphql_calls().len(), 12);
}

#[tokio::test]
async fn membership_survives_lifecycle_states_and_base_advance() {
    // The provider keeps T in the PR connection for every state while the compare result omits it.
    for state in ["OPEN", "CLOSED", "MERGED"] {
        let f = fixture(provider(chain(300), &sha(300))).await;
        f.shared.lock().unwrap().state = state;
        f.resolve(&sha(251)).await.unwrap();
    }
}

#[tokio::test]
async fn baseline_override_skips_compare() {
    let f = fixture(provider(chain(300), &sha(300))).await;
    let mut c = context(&sha(251));
    c.baseline_commit = sha(7);
    let (_, base) = f.github.resolve(&f.repository, &c).await.unwrap();
    assert_eq!(base, sha(7));
    assert!(
        !f.shared
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|c| c.0.contains("/compare/"))
    );
}

#[tokio::test]
async fn short_nonterminal_and_empty_terminal_pages_are_consistent() {
    let commits = chain(120);
    // Pages of 50/50/20 where every page but the last reports another page.
    let f = fixture(provider(commits.clone(), &sha(120))).await;
    f.shared.lock().unwrap().rest_rows = Some(vec![]);
    f.shared.lock().unwrap().pages = Some(vec![
        (commits[..50].to_vec(), true),
        (commits[50..100].to_vec(), true),
        (commits[100..].to_vec(), false),
    ]);
    f.resolve(&sha(115)).await.unwrap();
    assert_eq!(f.graphql_calls().len(), 3);

    // An empty terminal page agrees with totalCount; non-membership is then proven.
    let f = fixture(provider(commits.clone(), &sha(120))).await;
    f.shared.lock().unwrap().rest_rows = Some(vec![]);
    f.shared.lock().unwrap().pages = Some(vec![
        (commits[..100].to_vec(), true),
        (commits[100..].to_vec(), true),
        (vec![], false),
    ]);
    assert_eq!(f.resolve(&sha(5000)).await.unwrap_err(), UNAVAILABLE);
    assert_eq!(f.graphql_calls().len(), 3);
}

fn pull(v: &mut Value) -> &mut Value {
    &mut v["data"]["repository"]["pullRequest"]
}

fn mutate(
    commits: Vec<String>,
    edit: impl Fn(usize, &mut Value) + Send + Sync + 'static,
) -> Shared {
    let shared = provider(commits, &sha(999));
    // Separate provider so the generator does not re-lock the mutex held while it runs.
    let inner = provider(shared.lock().unwrap().commits.clone(), &sha(999));
    shared.lock().unwrap().rest_rows = Some(vec![]);
    shared.lock().unwrap().graphql = Some(Box::new(move |variables| {
        let (status, mut value) = graphql_page(&inner.lock().unwrap(), variables);
        let page = variables["after"]
            .as_str()
            .map_or(0, |c| c.trim_start_matches('c').parse().unwrap());
        edit(page, &mut value);
        (status, value)
    }));
    shared
}

#[tokio::test]
async fn provider_uncertainty_fails_closed() {
    type Edit = Box<dyn Fn(usize, &mut Value) + Send + Sync>;
    let cases: Vec<(&str, Edit)> = vec![
        (
            "graphql errors",
            Box::new(|_, v| v["errors"] = json!([{"message":"boom"}])),
        ),
        ("null data", Box::new(|_, v| v["data"] = Value::Null)),
        (
            "missing pull request",
            Box::new(|_, v| v["data"]["repository"]["pullRequest"] = Value::Null),
        ),
        (
            "malformed node",
            Box::new(|p, v| {
                if p == 1 {
                    pull(v)["commits"]["nodes"][3] = json!({"commit":null})
                }
            }),
        ),
        (
            "non-hex oid",
            Box::new(|p, v| {
                if p == 1 {
                    pull(v)["commits"]["nodes"][3]["commit"]["oid"] = json!("XYZ")
                }
            }),
        ),
        (
            "duplicate oid",
            Box::new(|p, v| {
                if p == 1 {
                    pull(v)["commits"]["nodes"][3]["commit"]["oid"] = json!(sha(1))
                }
            }),
        ),
        (
            "repeated cursor",
            Box::new(|_, v| pull(v)["commits"]["pageInfo"]["endCursor"] = json!("c1")),
        ),
        (
            "missing cursor",
            Box::new(|p, v| {
                if p == 1 {
                    pull(v)["commits"]["pageInfo"]["endCursor"] = Value::Null
                }
            }),
        ),
        (
            "missing hasNextPage",
            Box::new(|p, v| {
                if p == 1 {
                    pull(v)["commits"]["pageInfo"]["hasNextPage"] = Value::Null
                }
            }),
        ),
        (
            "total count too large",
            Box::new(|_, v| pull(v)["commits"]["totalCount"] = json!(301)),
        ),
        (
            "total count too small",
            Box::new(|_, v| pull(v)["commits"]["totalCount"] = json!(299)),
        ),
        (
            "head changed between pages",
            Box::new(|p, v| {
                if p == 2 {
                    pull(v)["headRefOid"] = json!(sha(500))
                }
            }),
        ),
        (
            "state changed between pages",
            Box::new(|p, v| {
                if p == 2 {
                    pull(v)["state"] = json!("MERGED")
                }
            }),
        ),
        (
            "pull request identity changed between pages",
            Box::new(|p, v| {
                if p == 2 {
                    pull(v)["id"] = json!("OTHER")
                }
            }),
        ),
        (
            "different base ref",
            Box::new(|_, v| pull(v)["baseRefName"] = json!("release")),
        ),
        (
            "different repository",
            Box::new(|_, v| v["data"]["repository"]["databaseId"] = json!(43)),
        ),
        (
            "different pull request number",
            Box::new(|_, v| pull(v)["number"] = json!(2)),
        ),
    ];
    for (name, edit) in cases {
        let f = fixture(mutate(chain(300), edit)).await;
        // sha(5000) is never listed, so success could only come from a skipped consistency check.
        assert_eq!(
            f.resolve(&sha(5000)).await.unwrap_err(),
            UNAVAILABLE,
            "{name}"
        );
        // A listed member after the broken page also never succeeds on an inconsistent snapshot.
        let listed = f.resolve(&sha(250)).await;
        if [
            "graphql errors",
            "null data",
            "missing pull request",
            "different base ref",
            "different repository",
            "different pull request number",
        ]
        .contains(&name)
        {
            assert_eq!(listed.unwrap_err(), UNAVAILABLE, "{name}");
        }
    }
}

#[tokio::test]
async fn provider_failures_and_deadline_fail_closed() {
    for status in [403, 404, 429, 500, 502] {
        let f = fixture(provider(chain(300), &sha(300))).await;
        f.shared.lock().unwrap().rest_rows = Some(vec![]);
        f.shared.lock().unwrap().graphql = Some(Box::new(move |_| {
            (
                StatusCode::from_u16(status).unwrap(),
                json!({"message":"no"}),
            )
        }));
        assert_eq!(
            f.resolve(&sha(251)).await.unwrap_err(),
            UNAVAILABLE,
            "{status}"
        );
    }
    // Failing on the second page never yields partial success for a target on a later page.
    let f = fixture(provider(chain(300), &sha(300))).await;
    f.shared.lock().unwrap().graphql = Some(Box::new(|v| {
        if v["after"].is_null() {
            graphql_empty_first()
        } else {
            (StatusCode::BAD_GATEWAY, json!({}))
        }
    }));
    assert_eq!(f.resolve(&sha(251)).await.unwrap_err(), UNAVAILABLE);

    // Deadline expiry is unavailable, not a membership verdict.
    let mut f = fixture(provider(chain(300), &sha(300))).await;
    f.github.membership_deadline = Duration::from_millis(100);
    f.shared.lock().unwrap().delay = Duration::from_millis(1500);
    assert_eq!(f.resolve(&sha(251)).await.unwrap_err(), UNAVAILABLE);
}

fn graphql_empty_first() -> (StatusCode, Value) {
    let commits = chain(300);
    let p = provider(commits.clone(), &sha(300));
    let mut p = p.lock().unwrap();
    p.pages = Some(vec![(commits[..100].to_vec(), true)]);
    graphql_page(&p, &json!({"after":null}))
}

#[tokio::test]
async fn closed_membership_does_not_come_from_repository_existence() {
    // The mock serves every `commits/{sha}` lookup, i.e. the object exists in the repository,
    // but a SHA absent from the PR connection must still be rejected.
    let f = fixture(provider(chain(10), &sha(10))).await;
    assert_eq!(f.resolve(&sha(77)).await.unwrap_err(), UNAVAILABLE);
    assert!(f.resolve(&sha(5)).await.is_ok());
}

/// Identity, Delcore and repository-connection routes used only by the PostgreSQL lifecycle test.
fn lifecycle_route(path: &str, auth: &str) -> Option<Value> {
    Some(match path {
        "/token" => json!({"access_token":"service","expires_in":3600}),
        "/delcore.v1.DelcoreService/GetCurrentUser" if auth == "Bearer manager" => {
            json!({"userId":ACTOR})
        }
        "/delcore.v1.DelcoreService/ListMembers" if auth == "Bearer manager" => json!({}),
        "/delcore.v1.DelcoreService/Authorize" => json!({"allowed":true}),
        "/delcore.v1.DelcoreService/CheckTenantLifecycle" => json!({"finalizedTenantIds":[]}),
        "/user" => json!({"id":7}),
        "/repos/owner/repo" | "/repositories/42" => {
            json!({"id":42,"full_name":"owner/repo","private":false,"permissions":{"admin":true}})
        }
        "/repos/owner/repo/installation" => json!({"id":1}),
        _ => return None,
    })
}
const ACTOR: &str = "018f0000-0000-7000-8000-000000000001";

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn postgres_commit_beyond_rest_cap_uploads_appends_and_completes_after_head_advance() {
    use git_tracker_api::{Server, auth::Actions};
    use server_common::{
        authority::Authority,
        telemetry::{Metrics, Service},
    };
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("tracker_{}", uuid::Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = schema.clone();
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |c, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path TO {search}"))
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/00001_git_tracker.sql"))
        .execute(&db)
        .await
        .unwrap();

    // t0: the PR head is T = C251, which the capped REST list omits once the head advances.
    let target = sha(251);
    let shared = provider(chain(300), &target);
    let f = fixture(shared.clone()).await;
    let base = f.github.base.clone();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let jwks = serde_json::from_value(json!({"keys":[]})).unwrap();
    let server = Arc::new(Server {
        db: db.clone(),
        authority: Authority::new(
            http.clone(),
            base.clone(),
            format!("{base}/token"),
            "resource".into(),
            "client".into(),
            "fixture".into(),
        ),
        github: GitHub::new(
            http.clone(),
            base.clone(),
            "1".into(),
            EncodingKey::from_rsa_pem(KEY).unwrap(),
        ),
        actions: Actions::new(http.clone(), format!("{base}/jwks"), jwks),
        audit_retention: chrono::Duration::days(90),
        webhook_secret: "fixture-only".into(),
        metrics: Arc::new(Metrics::new(Service::GitTracker)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = format!(
        "http://{}/gittracker.v1.GitTrackerService",
        listener.local_addr().unwrap()
    );
    let router = Router::new().fallback_service(
        connectrpc::Router::new()
            .add_service(server.clone())
            .into_axum_service(),
    );
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    async fn call(
        http: &reqwest::Client,
        api: &str,
        method: &str,
        auth: &str,
        q: Value,
    ) -> (StatusCode, Value) {
        let r = http
            .post(format!("{api}/{method}"))
            .header("Authorization", auth)
            .header("Connect-Protocol-Version", "1")
            .json(&q)
            .send()
            .await
            .unwrap();
        (r.status(), r.json().await.unwrap())
    }
    let space = uuid::Uuid::now_v7().to_string();
    let (status, connected) = call(
        &http,
        &api,
        "ConnectRepository",
        "Bearer manager",
        json!({"spaceId":space,"fullName":"owner/repo","githubAccessToken":"fixture"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{connected}");
    let rid = connected["repository"]["id"].as_str().unwrap().to_owned();
    let (_, issued) = call(
        &http,
        &api,
        "IssueToken",
        "Bearer manager",
        json!({"repositoryId":rid,"expiresAt":(chrono::Utc::now()+chrono::Duration::days(1)).to_rfc3339()}),
    )
    .await;
    let auth = format!("Bearer {}", issued["token"].as_str().unwrap());
    let context = json!({"repositoryId":rid,"executionId":"run-t","commit":target,"branch":"main","pullRequest":"1"});
    let dataset = json!({"id":"size","name":"Size","columns":[{"id":"bytes","unit":"B","direction":"DIRECTION_LOWER"}]});
    let upload = |id: &str, value: &str| json!({"context":context,"dataset":dataset,"rows":[{"id":id,"values":{"bytes":value}}]});

    // t0: head shortcut.
    let (status, first) = call(&http, &api, "Upload", &auth, upload("one", "1")).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert!(f.graphql_calls().is_empty());

    // t0 + 1 minute: ordinary fast-forward to C300. T stays in the PR but beyond the REST cap.
    shared.lock().unwrap().head = sha(300);
    let (status, second) = call(&http, &api, "Upload", &auth, upload("two", "2")).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(f.graphql_calls().len(), 3);
    // Exact retransmission needs no provider call.
    let calls = f.graphql_calls().len();
    let (status, _) = call(&http, &api, "Upload", &auth, upload("two", "2")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(f.graphql_calls().len(), calls);

    let (status, complete) = call(&http, &api, "Complete", &auth, json!({"context":context})).await;
    assert_eq!(status, StatusCode::OK, "{complete}");
    assert_eq!(complete["execution"]["state"], "EXECUTION_STATE_COMPLETED");
    let (_, execution) = call(
        &http,
        &api,
        "GetExecution",
        "",
        json!({"repositoryId":rid,"executionId":"run-t"}),
    )
    .await;
    assert_eq!(
        execution["execution"]["measurements"][0]["rows"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let (_, again) = call(&http, &api, "Complete", &auth, json!({"context":context})).await;
    assert_eq!(complete, again);
    let (_, closed) = call(&http, &api, "Upload", &auth, upload("three", "3")).await;
    assert_eq!(closed["code"], "failed_precondition");

    // A provider outage leaves a new execution unregistered instead of persisting partial data.
    shared.lock().unwrap().graphql = Some(Box::new(|_| (StatusCode::BAD_GATEWAY, json!({}))));
    let mut other = context.clone();
    other["executionId"] = "run-u".into();
    let (_, failed) = call(
        &http,
        &api,
        "Upload",
        &auth,
        json!({"context":other,"dataset":dataset,"rows":[{"id":"one","values":{"bytes":"1"}}]}),
    )
    .await;
    assert_eq!(failed["code"], "unavailable");
    let (_, missing) = call(
        &http,
        &api,
        "GetExecution",
        "",
        json!({"repositoryId":rid,"executionId":"run-u"}),
    )
    .await;
    assert!(missing.get("execution").is_none(), "{missing}");

    task.abort();
    drop(server);
    db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
