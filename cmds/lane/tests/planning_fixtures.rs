// SPDX-License-Identifier: Apache-2.0
//! Replays the shared planning conformance vectors (`protos/lane/v1/fixtures/planning-*.json`)
//! against the Rust local store. The Go reference evaluator replays the same files.
mod support;
use lane::{error::Failure, protocol::strv, store::Store};
use serde_json::{Value, json};
use std::collections::HashMap;
use support::{call, fixture, norm, seed};

fn open() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path(), "", false).unwrap();
    (dir, store)
}
/// Fixture requests carry placeholder expected versions; point them at the seeded heads.
fn bind(request: &Value, heads: &HashMap<String, String>) -> Value {
    let mut request = request.clone();
    if request.get("expectedVersionId").is_some()
        && let Some(version) = heads.get(strv(&request, "id"))
    {
        request["expectedVersionId"] = version.clone().into();
    }
    request
}
fn assert_error(case: &Value, got: &Failure, want: &Value) {
    assert_eq!(got.reason, want["reason"], "{case}: {got:?}");
    assert_eq!(
        got.violation.map_or("", lane::error::Violation::as_str),
        strv(want, "violation"),
        "{case}"
    );
}
fn stored_task(store: &Store, id: &str) -> Value {
    let result = store.invoke("GetTask", &json!({"id":id})).unwrap();
    result["result"]["version"]["task"].clone()
}

#[test]
fn validation_vectors() {
    let suite = fixture("planning-validation");
    assert!(suite["cases"].as_array().unwrap().len() >= 78);
    for case in suite["cases"].as_array().unwrap() {
        let name = &case["id"];
        let (_dir, store) = open();
        let heads = seed(&store, &suite["state"]);
        let method = strv(&case["input"], "method");
        let request = bind(&case["input"]["request"], &heads);
        let result = call(&store, method, &request);
        let want = &case["expected"];
        if want["outcome"] == "error" {
            assert_error(name, &result.expect_err(&name.to_string()), want);
            // A rejected write leaves nothing behind.
            let count: i64 = store
                .with(false, |c| {
                    Ok(c.query_row("SELECT count(*) FROM outbox", [], |r| r.get(0))?)
                })
                .unwrap();
            assert_eq!(count, 0, "{name}");
            continue;
        }
        let version =
            result.unwrap_or_else(|e| panic!("{name}: {e:?}"))["result"]["version"].clone();
        if method == "SaveTask" {
            assert_eq!(
                norm("Task", &version["task"]),
                norm("Task", &want["normalized"]["task"]),
                "{name}"
            );
        } else {
            let field = ["label", "milestone", "cycle"]
                .into_iter()
                .find(|f| want["normalized"].get(*f).is_some())
                .unwrap();
            assert_eq!(
                norm("PlanningBody", &json!({field: version[field]})),
                norm("PlanningBody", &want["normalized"]),
                "{name}"
            );
        }
    }
}

#[test]
fn graph_vectors() {
    let suite = fixture("planning-graph");
    assert!(suite["cases"].as_array().unwrap().len() >= 48);
    for case in suite["cases"].as_array().unwrap() {
        let name = &case["id"];
        let (_dir, store) = open();
        let heads = seed(&store, &suite["state"]);
        let method = strv(&case["input"], "method");
        let request = bind(&case["input"]["request"], &heads);
        let result = call(&store, method, &request);
        let want = &case["expected"];
        if want["outcome"] == "error" {
            assert_error(name, &result.expect_err(&name.to_string()), want);
            let count: i64 = store
                .with(false, |c| {
                    Ok(c.query_row("SELECT count(*) FROM outbox", [], |r| r.get(0))?)
                })
                .unwrap();
            assert_eq!(
                count, 0,
                "{name}: a rejected write must leave no outbox rows"
            );
            continue;
        }
        let response = result.unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(
            norm("Task", &response["result"]["version"]["task"]),
            norm("Task", &want["normalized"]["task"]),
            "{name}"
        );
        let got: HashMap<String, Value> = response["affected"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|a| {
                (
                    strv(&a["version"], "entityId").to_owned(),
                    norm("Task", &a["version"]["task"]),
                )
            })
            .collect();
        let expected: HashMap<String, Value> = want["affected"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| (strv(a, "entityId").to_owned(), norm("Task", &a["task"])))
            .collect();
        assert_eq!(got, expected, "{name}: affected tasks");
        // Every affected task is stored with exactly the reported body.
        for (id, body) in &expected {
            assert_eq!(
                &norm("Task", &stored_task(&store, id)),
                body,
                "{name}: {id}"
            );
        }
    }
}

#[test]
fn cleanup_vectors() {
    let suite = fixture("planning-cleanup");
    for case in suite["cases"].as_array().unwrap() {
        let name = &case["id"];
        let (_dir, store) = open();
        let heads = seed(&store, &suite["state"]);
        let before: HashMap<String, Value> = suite["state"]["entities"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["kind"] == "ENTITY_KIND_TASK")
            .map(|e| (strv(e, "entityId").to_owned(), norm("Task", &e["task"])))
            .collect();
        let method = strv(&case["input"], "method");
        let request = bind(&case["input"]["request"], &heads);
        call(&store, method, &request).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let want = &case["expected"];
        let deleted: Vec<&str> = want["deleted"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let changed: HashMap<String, Value> = want["changed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (strv(e, "entityId").to_owned(), norm("Task", &e["task"])))
            .collect();
        for (id, original) in &before {
            if deleted.contains(&id.as_str()) {
                let result = store.invoke("GetTask", &json!({"id":id})).unwrap();
                assert_eq!(
                    result["result"]["result"], "RESULT_KIND_DELETED",
                    "{name}: {id}"
                );
                continue;
            }
            let expected = changed.get(id).unwrap_or(original);
            assert_eq!(
                &norm("Task", &stored_task(&store, id)),
                expected,
                "{name}: {id}"
            );
        }
        for id in deleted {
            let live: i64 = store
                .with(false, |c| {
                    Ok(c.query_row(
                        "SELECT count(*) FROM entities WHERE id=? AND deleted=0",
                        [id],
                        |r| r.get(0),
                    )?)
                })
                .unwrap();
            assert_eq!(live, 0, "{name}: {id} must be tombstoned");
        }
    }
}

#[test]
fn list_vectors() {
    let suite = fixture("planning-list");
    let cases = suite["cases"].as_array().unwrap();
    assert!(cases.len() >= 40);
    let (_dir, store) = open();
    seed(&store, &suite["dataset"]);
    for case in cases {
        let name = &case["id"];
        let mut request = case["input"]["request"].clone();
        let mut pages: Vec<Vec<String>> = vec![];
        loop {
            let response =
                call(&store, "ListTasks", &request).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            pages.push(
                response["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| strv(&i["version"], "entityId").to_owned())
                    .collect(),
            );
            let next = strv(&response, "nextCursor");
            if next.is_empty() {
                break;
            }
            request["page"]["cursor"] = next.into();
            assert!(pages.len() < 100, "{name}: cursor did not terminate");
        }
        let expected_pages: Vec<Vec<String>> = case["expected"]["pages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                p.as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect()
            })
            .collect();
        assert_eq!(pages, expected_pages, "{name}");
        let flat: Vec<String> = pages.concat();
        let expected: Vec<&str> = case["expected"]["taskIds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(flat, expected, "{name}: pages equal the unpaged order");
    }
}

#[test]
fn list_cursors_are_scoped_to_the_query() {
    let suite = fixture("planning-list");
    let (_dir, store) = open();
    seed(&store, &suite["dataset"]);
    let first = call(
        &store,
        "ListTasks",
        &json!({"page":{"size":2},"sortKey":"TASK_SORT_KEY_PRIORITY"}),
    )
    .unwrap();
    let cursor = strv(&first, "nextCursor");
    assert_ne!(cursor, "");
    for other in [
        json!({"page":{"size":2,"cursor":cursor},"sortKey":"TASK_SORT_KEY_DUE_DATE"}),
        json!({"page":{"size":2,"cursor":cursor},"sortKey":"TASK_SORT_KEY_PRIORITY","sortDirection":"SORT_DIRECTION_DESCENDING"}),
        json!({"page":{"size":2,"cursor":cursor},"sortKey":"TASK_SORT_KEY_PRIORITY","filter":{"includeUnlabeled":true}}),
        json!({"page":{"size":2,"cursor":cursor}}),
    ] {
        let error = call(&store, "ListTasks", &other).unwrap_err();
        assert_eq!(error.reason, "ERROR_REASON_INVALID_INPUT");
    }
}
