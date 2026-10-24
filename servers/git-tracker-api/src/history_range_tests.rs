// SPDX-License-Identifier: FSL-1.1-ALv2
//! Connected explicit-branch `GetHistory` commit-range regressions.
//!
//! These tests run `Server::history` against a mocked GitHub first-parent chain and synthetic
//! repository state, so they need neither PostgreSQL nor live GitHub credentials.
use crate::{Server, auth::Actions, github::GitHub, measurement::State};
use axum::{
    Json, Router,
    extract::{Request, State as AxumState},
    http::StatusCode,
};
use delino_protocol::gittracker::v1::Repository;
use jsonwebtoken::{EncodingKey, jwk::JwkSet};
use serde_json::{Value, json};
use server_common::{
    authority::Authority,
    telemetry::{Metrics, Service},
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const KEY: &[u8] = include_bytes!("../tests/fixtures/github-test-key.pem");

/// Expands a commit label (`A` through `F`) to a full lowercase SHA such as 40 repeated `a`.
fn sha(c: char) -> String {
    c.to_ascii_lowercase().to_string().repeat(40)
}

type MockState = (Vec<String>, Arc<Mutex<Vec<String>>>);

/// Mocked GitHub with a first-parent chain, newest first; the last commit is the root.
struct Fixture {
    server: Server,
    requested: Arc<Mutex<Vec<String>>>,
    _task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new(chain: &str) -> Self {
        let chain: Vec<String> = chain.chars().map(sha).collect();
        let requested = Arc::new(Mutex::new(Vec::<String>::new()));
        let shared = (chain.clone(), requested.clone());
        let mock = Router::new()
            .fallback(
                |AxumState((chain, requested)): AxumState<MockState>, r: Request| async move {
                    let path = r.uri().path().to_owned();
                    let out = if path == "/app/installations/1/access_tokens" {
                        json!({"token":"installation"})
                    } else if path == "/repos/owner/repo/git/ref/heads/main" {
                        json!({"ref":"refs/heads/main","object":{"type":"commit","sha":chain[0]}})
                    } else if let Some(c) = path.strip_prefix("/repos/owner/repo/commits/") {
                        requested.lock().unwrap().push(c.to_owned());
                        match chain.iter().position(|x| x == c) {
                            Some(i) => {
                                let parents: Vec<Value> = chain
                                    .get(i + 1)
                                    .map(|p| json!({"sha":p}))
                                    .into_iter()
                                    .collect();
                                json!({"sha":c,"parents":parents})
                            }
                            None => {
                                return (StatusCode::NOT_FOUND, Json(json!({"message":"missing"})));
                            }
                        }
                    } else {
                        return (StatusCode::NOT_FOUND, Json(json!({"message":"missing"})));
                    };
                    (StatusCode::OK, Json(out))
                },
            )
            .with_state(shared);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let server = Server {
            // History never touches the database; the lazy pool never connects.
            db: sqlx::PgPool::connect_lazy("postgres://unused@127.0.0.1:1/unused").unwrap(),
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
            actions: Actions::new(http, format!("{base}/jwks"), JwkSet { keys: vec![] }),
            audit_retention: chrono::Duration::days(90),
            webhook_secret: "fixture-only".into(),
            metrics: Arc::new(Metrics::new(Service::GitTracker)),
        };
        Self {
            server,
            requested,
            _task: task,
        }
    }

    /// Runs `history` for `branch=main` plus `extra` request fields and returns the response.
    async fn history(&self, executions: &[(&str, char, u64, &str)], extra: Value) -> Value {
        let records: serde_json::Map<String, Value> = executions
            .iter()
            .map(|(id, commit, order, registered_at)| {
                ((*id).to_owned(), record(id, *commit, *order, registered_at))
            })
            .collect();
        let state: State = serde_json::from_value(json!({"next": 1000, "records": records}))
            .expect("fixture state");
        let repo = Repository {
            full_name: "owner/repo".into(),
            installation_id: "1".into(),
            connected: true,
            ..Default::default()
        };
        let mut q = json!({"branch":"main"});
        q.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        self.requested.lock().unwrap().clear();
        self.server.history(&repo, &state, &q).await.unwrap()
    }
}

fn record(id: &str, commit: char, order: u64, registered_at: &str) -> Value {
    json!({
        "execution": {
            "context": {"repositoryId":"r","executionId":id,"commit":sha(commit),"branch":branch_of(id)},
            "state": "EXECUTION_STATE_COMPLETED",
            // Storage keeps the order as a zero-padded decimal string, as `State::upload` writes it.
            "registrationOrder": format!("{order:020}"),
            "registeredAt": registered_at,
            "measurements": [{
                "dataset": {"id":"size","name":"Size","columns":[{"id":"bytes","unit":"B","direction":"DIRECTION_LOWER"}]},
                "rows": [{"id":"bundle","values":{"bytes":"40"}}]
            }]
        },
        "lastData": registered_at,
        "cutoff": registered_at,
        "base": {"execution":"","commit":""},
        "push": true
    })
}

/// Fixture executions whose ID starts with `other-` belong to the `other` branch.
fn branch_of(id: &str) -> &'static str {
    if id.starts_with("other-") {
        "other"
    } else {
        "main"
    }
}

const T0: &str = "2026-01-01T00:00:00Z";

fn ids(v: &Value) -> Vec<&str> {
    v["executions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["context"]["executionId"].as_str().unwrap())
        .collect()
}

fn commits(v: &Value) -> String {
    v["commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            c.as_str()
                .unwrap()
                .chars()
                .next()
                .unwrap()
                .to_ascii_uppercase()
        })
        .collect()
}

fn bounds(from: Option<char>, to: Option<char>) -> Value {
    json!({
        "fromCommit": from.map(sha).unwrap_or_default(),
        "toCommit": to.map(sha).unwrap_or_default(),
    })
}

#[tokio::test]
async fn unmeasured_endpoints_older_than_the_only_measurement_bound_the_range() {
    let f = Fixture::new("DCBA").await;
    let runs = [("run-d", 'D', 1, T0)];
    let v = f.history(&runs, bounds(Some('A'), Some('B'))).await;
    assert!(ids(&v).is_empty(), "{v}");
    assert_eq!(commits(&v), "BA");
    // The walk reached the unmeasured upper bound B instead of stopping at D.
    assert!(f.requested.lock().unwrap().contains(&sha('B')));
    // Unbounded history is unchanged: only the measured tip.
    let v = f.history(&runs, json!({})).await;
    assert_eq!(ids(&v), ["run-d"]);
    assert_eq!(commits(&v), "D");
}

#[tokio::test]
async fn five_commit_chain_honors_valid_unmeasured_and_measured_endpoints() {
    let f = Fixture::new("EDCBA").await;
    let runs = [("run-c", 'C', 1, T0), ("run-e", 'E', 2, T0)];
    for (name, from, to, want_ids, want_commits) in [
        ("to=B", None, Some('B'), vec![], "B"),
        ("from=A,to=D", Some('A'), Some('D'), vec!["run-c"], "DCBA"),
        ("to=C", None, Some('C'), vec!["run-c"], "C"),
        ("from=C,to=C", Some('C'), Some('C'), vec!["run-c"], "C"),
        ("to=D", None, Some('D'), vec!["run-c"], "DC"),
        ("from=B", Some('B'), None, vec!["run-e", "run-c"], "EDCB"),
        ("from=B,to=D", Some('B'), Some('D'), vec!["run-c"], "DCB"),
        ("none", None, None, vec!["run-e", "run-c"], "EDC"),
    ] {
        let v = f.history(&runs, bounds(from, to)).await;
        assert_eq!(ids(&v), want_ids, "{name}: {v}");
        assert_eq!(commits(&v), want_commits, "{name}: {v}");
    }
}

#[tokio::test]
async fn unmeasured_bound_without_any_execution_creates_no_measurements() {
    let f = Fixture::new("EDCBA").await;
    let v = f.history(&[], bounds(None, Some('B'))).await;
    assert!(ids(&v).is_empty(), "{v}");
    assert_eq!(commits(&v), "B");
}

#[tokio::test]
async fn date_filter_does_not_remove_range_bounds() {
    let f = Fixture::new("EDCBA").await;
    let runs = [
        ("run-a", 'A', 1, "2025-01-01T00:00:00Z"),
        ("run-c", 'C', 2, T0),
    ];
    let v = f.history(&runs, bounds(Some('A'), Some('B'))).await;
    assert_eq!(ids(&v), ["run-a"], "{v}");
    let mut q = bounds(Some('A'), Some('B'));
    q["since"] = json!("2025-06-01T00:00:00Z");
    let v = f.history(&runs, q).await;
    assert!(ids(&v).is_empty(), "{v}");
    assert_eq!(commits(&v), "BA");
}

#[tokio::test]
async fn reversed_and_absent_endpoints_keep_existing_slicing_behavior() {
    let f = Fixture::new("EDCBA").await;
    let runs = [("run-c", 'C', 1, T0), ("run-e", 'E', 2, T0)];
    // A reversed range keeps the lower slice and ignores the unmatched upper endpoint.
    let v = f.history(&runs, bounds(Some('D'), Some('B'))).await;
    assert_eq!(commits(&v), "B");
    assert!(ids(&v).is_empty(), "{v}");
    // An endpoint that is not on the first-parent path is ignored, so on-path executions stay.
    let mut off_branch = bounds(None, None);
    off_branch["toCommit"] = json!(sha('F'));
    let v = f.history(&runs, off_branch).await;
    assert_eq!(ids(&v), ["run-e", "run-c"], "{v}");
}

#[tokio::test]
async fn range_filtering_precedes_pagination() {
    let f = Fixture::new("EDCBA").await;
    let mut owned: Vec<(String, char, u64)> =
        (1..=130).map(|i| (format!("in-{i}"), 'C', i)).collect();
    owned.extend((131..=135).map(|i| (format!("out-{i}"), 'E', i)));
    let runs: Vec<(&str, char, u64, &str)> = owned
        .iter()
        .map(|(id, c, o)| (id.as_str(), *c, *o, T0))
        .collect();
    let v = f.history(&runs, bounds(Some('B'), Some('D'))).await;
    let first = ids(&v);
    assert_eq!(first.len(), 100);
    assert_eq!(first[0], "in-130");
    assert!(first.iter().all(|id| id.starts_with("in-")));
    assert_eq!(v["nextCursor"], "100");
    let mut q = bounds(Some('B'), Some('D'));
    q["cursor"] = json!("100");
    let v = f.history(&runs, q).await;
    assert_eq!(ids(&v).len(), 30);
    assert!(ids(&v).iter().all(|id| id.starts_with("in-")));
    assert_eq!(v["nextCursor"], "");
}

#[test]
fn history_page_window_cursor_is_empty_on_every_final_page() {
    use crate::history_page_window;
    let w = |offset, len| history_page_window(offset, len);
    assert_eq!(w(0, 0), (0, 0, String::new()));
    assert_eq!(w(0, 99), (0, 99, String::new()));
    // An exactly full first page is also the final page.
    assert_eq!(w(0, 100), (0, 100, String::new()));
    assert_eq!(w(0, 101), (0, 100, "100".into()));
    assert_eq!(w(100, 101), (100, 101, String::new()));
    assert_eq!(w(0, 200), (0, 100, "100".into()));
    assert_eq!(w(100, 200), (100, 200, String::new()));
    assert_eq!(w(100, 201), (100, 200, "200".into()));
    // Cursors at or beyond the collection length clamp to an empty final page.
    assert_eq!(w(100, 100), (100, 100, String::new()));
    assert_eq!(w(150, 100), (100, 100, String::new()));
    assert_eq!(w(usize::MAX, 100), (100, 100, String::new()));
}

/// Follows `nextCursor` from an empty cursor and returns every page's execution IDs and cursor.
async fn pages(
    f: &Fixture,
    runs: &[(&str, char, u64, &str)],
    mut q: Value,
) -> Vec<(Vec<String>, String)> {
    let mut out = Vec::new();
    let mut cursor = String::new();
    loop {
        q["cursor"] = json!(cursor);
        let v = f.history(runs, q.clone()).await;
        let next = v["nextCursor"].as_str().unwrap().to_owned();
        out.push((
            ids(&v).into_iter().map(str::to_owned).collect(),
            next.clone(),
        ));
        if next.is_empty() {
            return out;
        }
        assert!(out.len() < 10, "cursor never terminated: {out:?}");
        cursor = next;
    }
}

#[tokio::test]
async fn history_cursor_terminates_on_exact_page_boundaries() {
    let f = Fixture::new("A").await;
    for (count, sizes, cursors) in [
        (0usize, vec![0usize], vec![""]),
        (99, vec![99], vec![""]),
        (100, vec![100], vec![""]),
        (101, vec![100, 1], vec!["100", ""]),
        (200, vec![100, 100], vec!["100", ""]),
    ] {
        let owned: Vec<(String, char, u64)> = (1..=count as u64)
            .map(|i| (format!("run-{i:03}"), 'A', i))
            .collect();
        let runs: Vec<(&str, char, u64, &str)> = owned
            .iter()
            .map(|(id, c, o)| (id.as_str(), *c, *o, T0))
            .collect();
        let got = pages(&f, &runs, json!({})).await;
        assert_eq!(
            got.iter().map(|p| p.0.len()).collect::<Vec<_>>(),
            sizes,
            "{count}"
        );
        assert_eq!(
            got.iter().map(|p| p.1.as_str()).collect::<Vec<_>>(),
            cursors,
            "{count}"
        );
        // Every execution is returned once, newest registration first.
        let all: Vec<String> = got.into_iter().flat_map(|p| p.0).collect();
        let expected: Vec<String> = owned.iter().rev().map(|(id, _, _)| id.clone()).collect();
        assert_eq!(all, expected, "{count}");
        // A cursor at or beyond the collection length is an empty final page.
        for cursor in [count.to_string(), (count + 50).to_string()] {
            let v = f.history(&runs, json!({"cursor": cursor})).await;
            assert!(ids(&v).is_empty(), "{count}: {v}");
            assert_eq!(v["nextCursor"], "", "{count}");
        }
    }
}

#[tokio::test]
async fn history_cursor_ignores_executions_on_other_branches() {
    let f = Fixture::new("A").await;
    let mut owned: Vec<(String, char, u64)> =
        (1..=100).map(|i| (format!("run-{i:03}"), 'A', i)).collect();
    owned.extend((101..=130).map(|i| (format!("other-{i}"), 'A', i)));
    let runs: Vec<(&str, char, u64, &str)> = owned
        .iter()
        .map(|(id, c, o)| (id.as_str(), *c, *o, T0))
        .collect();
    // Exactly 100 executions remain for `main`, so the first page is also the final page.
    let got = pages(&f, &runs, json!({"branch": "main"})).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0.len(), 100);
    assert!(got[0].0.iter().all(|id| id.starts_with("run-")));
    assert_eq!(got[0].1, "");
}
