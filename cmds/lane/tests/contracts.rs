// SPDX-License-Identifier: Apache-2.0
#![allow(
    clippy::too_many_lines,
    reason = "Keep shared fixture branches together."
)]
use lane::{
    protocol::{self, id, strv},
    store::Store,
};
use serde_json::{Value, json};
fn fixture(name: &str) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/../../protos/lane/v1/fixtures/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap()
}
fn call(s: &Store, m: &str, mut q: Value) -> lane::error::Result<Value> {
    if protocol::method(m).is_some() {
        q = protocol::request(m, &q)?;
        let mut allocated = vec![];
        protocol::attribute(
            protocol::method(m).unwrap().input(),
            &mut q,
            &json!({"kind":"ORIGIN_KIND_CLI","deviceId":s.device,"deviceName":"Lane","accountId":s.identity()?.account}),
            &mut allocated,
        )?;
    }
    s.invoke(m, &q)
}
fn task(s: &Store, t: &Value) -> Value {
    let repo = "019a0000-0000-7000-8000-000000000002";
    call(
        s,
        "CreateRepository",
        json!({"id":repo,"url":"https://example.com/fixture"}),
    )
    .unwrap();
    let mut q = t.clone();
    q.as_object_mut().unwrap().remove("repositoryId");
    q["repository"] = json!({"id":repo});
    call(s, "CreateTask", q).unwrap()["result"]["version"].clone()
}
#[test]
fn normalization_vectors() {
    for case in fixture("normalization")["cases"].as_array().unwrap() {
        let result = lane::store::normalize::url(strv(&case["input"], "url"));
        if case["expected"].get("reason").is_some() {
            assert!(result.is_err(), "{}", case["id"]);
        } else {
            assert_eq!(
                result.unwrap(),
                strv(&case["expected"], "normalizedUrl"),
                "{}",
                case["id"]
            );
        }
    }
}
#[test]
fn fingerprint_vectors() {
    for case in fixture("fingerprints")["cases"].as_array().unwrap() {
        let request = protocol::request("CreateTask", &case["input"]["normalizedRequest"]).unwrap();
        assert_eq!(
            protocol::fingerprint(&request),
            strv(&case["expected"], "sha256"),
            "{}",
            case["id"]
        );
    }
}
#[test]
fn task_vectors() {
    for case in fixture("tasks")["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let s = Store::open(temp.path(), "", false).unwrap();
        let input = &case["input"];
        let want = &case["expected"];
        let method = strv(input, "method");
        match method {
            "CreateTask" => {
                let result = call(&s, method, input["request"].clone());
                if let Some(reason) = want.get("reason") {
                    assert_eq!(result.unwrap_err().reason, *reason, "{}", case["id"]);
                    let count = s
                        .with(false, |c| {
                            Ok(c.query_row::<i64, _, _>(
                                "SELECT count(*) FROM entities",
                                [],
                                |r| r.get(0),
                            )?)
                        })
                        .unwrap();
                    assert_eq!(count, 0);
                } else {
                    let got = result.unwrap();
                    let t = &got["result"]["version"]["task"];
                    assert_eq!(t["progress"], want["progress"]);
                    assert_eq!(t["proposal"], want["proposal"]);
                    assert_eq!(t.get("prompt").is_some(), want["promptPresent"] == true);
                }
            }
            "SetProgress" | "SaveTask" => {
                let v = task(&s, &input["before"]);
                let mut q = input["request"].clone();
                q["id"] = v["entityId"].clone();
                q["expectedVersionId"] = v["versionId"].clone();
                let result = call(&s, method, q);
                if let Some(reason) = want.get("reason") {
                    assert_eq!(result.unwrap_err().reason, *reason);
                } else {
                    let result = result.unwrap();
                    assert_eq!(
                        protocol::message("Task", &result["result"]["version"]["task"]).unwrap(),
                        protocol::message("Task", &want["task"]).unwrap(),
                        "{}",
                        case["id"]
                    );
                }
            }
            _ => {
                if let Some(decisions) = input["decisions"].as_array() {
                    let mut v = task(&s, &input["task"]);
                    for (i, p) in decisions.iter().enumerate() {
                        v=call(&s,"SetProposal",json!({"id":v["entityId"],"expectedVersionId":v["versionId"],"proposal":p})).unwrap()["result"]["version"].clone();
                        assert_eq!(v["task"]["progress"], want["progresses"][i]);
                        assert_eq!(v["task"]["proposal"], want["proposals"][i]);
                    }
                } else if let Some(snapshots) = input["snapshots"].as_array() {
                    let mut v = task(&s, &snapshots[0]);
                    let mut ids = vec![v["versionId"].clone()];
                    for t in &snapshots[1..] {
                        v = call(
                            &s,
                            "SaveTask",
                            json!({"id":v["entityId"],"expectedVersionId":v["versionId"],"task":t}),
                        )
                        .unwrap()["result"]["version"]
                            .clone();
                        ids.push(v["versionId"].clone());
                    }
                    v=call(&s,"RestorePrompt",json!({"id":v["entityId"],"expectedVersionId":v["versionId"],"sourceVersionId":ids[usize::try_from(input["restoreSourceIndex"].as_u64().unwrap()).unwrap()]})).unwrap()["result"]["version"].clone();
                    assert_eq!(v["task"]["prompt"], want["prompt"]);
                    assert_eq!(v["promptRestoration"], true);
                    let history = call(
                        &s,
                        "ListHistory",
                        json!({"id":v["entityId"],"promptsOnly":true}),
                    )
                    .unwrap();
                    if let Some(n) = want["promptHistoryCount"].as_u64() {
                        assert_eq!(
                            history["versions"].as_array().unwrap().len(),
                            usize::try_from(n).unwrap()
                        );
                    }
                } else if input["operation"] == "search" {
                    if strv(input, "title").is_empty() {
                        continue;
                    }
                    call(&s,"CreateTask",json!({"title":input["title"],"description":input["description"],"repository":{"url":"https://example.com/search"}})).unwrap();
                    let result = call(&s, "ListTasks", json!({"search":input["query"]})).unwrap();
                    assert_eq!(
                        !result["items"].as_array().unwrap().is_empty(),
                        want["matches"] == true,
                        "{}",
                        case["id"]
                    );
                } else if input["operation"] == "validateStoredTask" {
                    let v = task(
                        &s,
                        &json!({"repositoryId":"019a0000-0000-7000-8000-000000000002","title":"Initial","description":"","progress":"PROGRESS_TODO","proposal":"PROPOSAL_REGULAR"}),
                    );
                    assert!(call(&s,"SaveTask",json!({"id":v["entityId"],"expectedVersionId":v["versionId"],"task":input["task"]})).is_err());
                } else {
                    panic!("Unhandled vector {}", case["id"]);
                }
            }
        }
    }
}
#[test]
fn retry_conflict_delete_and_prompt_history() {
    let temp = tempfile::tempdir().unwrap();
    let s = Store::open(temp.path(), "", false).unwrap();
    let request = id();
    let q = json!({"meta":{"requestId":request},"title":"Initial","repository":{"url":"https://example.com/a/repo.git"}});
    let v = call(&s, "CreateTask", q.clone()).unwrap()["result"]["version"].clone();
    assert_eq!(
        call(&s, "CreateTask", q.clone()).unwrap()["result"]["result"],
        "RESULT_KIND_REPLAYED"
    );
    let mut changed = q.clone();
    changed["title"] = "Changed".into();
    assert_eq!(
        call(&s, "CreateTask", changed).unwrap_err().reason,
        "ERROR_REASON_REQUEST_ID_REUSED"
    );
    let mut t = v["task"].clone();
    t["prompt"] = "".into();
    let edit = call(
        &s,
        "SaveTask",
        json!({"id":v["entityId"],"expectedVersionId":v["versionId"],"task":t}),
    )
    .unwrap();
    let mut t = v["task"].clone();
    t["title"] = "Concurrent".into();
    let stale = call(
        &s,
        "SaveTask",
        json!({"id":v["entityId"],"expectedVersionId":v["versionId"],"task":t}),
    )
    .unwrap_err();
    assert_eq!(stale.reason, "ERROR_REASON_STALE_VERSION");
    assert_eq!(stale.current_head_ids.len(), 2);
    let current = call(&s, "GetTask", json!({"id":v["entityId"]})).unwrap();
    assert_eq!(
        current["result"]["alternatives"].as_array().unwrap().len(),
        2
    );
    call(&s,"ResolveConflict",json!({"id":v["entityId"],"expectedHeadIds":stale.current_head_ids,"resolved":edit["result"]["version"]})).unwrap();
    call(&s, "DeleteTask", json!({"id":v["entityId"]})).unwrap();
    assert_eq!(
        call(&s, "CreateTask", q).unwrap()["result"]["result"],
        "RESULT_KIND_DELETED"
    );
    assert_eq!(
        call(&s, "ListHistory", json!({"id":v["entityId"]})).unwrap()["versions"],
        json!([])
    );
}
#[test]
fn schema_and_space_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let s = Store::open(temp.path(), "", false).unwrap();
    let other = s
        .select_account("https://issuer.example", "account", "tenant")
        .unwrap();
    call(
        &s,
        "CreateTask",
        json!({"title":"Local","repository":{"url":"https://example.com/local"}}),
    )
    .unwrap();
    assert_eq!(
        call(&other, "ListTasks", json!({})).unwrap()["items"],
        json!([])
    );
    assert!(Store::open(temp.path(), &id(), false).is_err());
    s.with(true, |c| {
        c.execute_batch("PRAGMA user_version=99")?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        call(&s, "ListTasks", json!({})).unwrap_err().reason,
        "ERROR_REASON_UNSUPPORTED_FORMAT"
    );
}
#[test]
fn concurrent_writers_and_removal() {
    let temp = tempfile::tempdir().unwrap();
    let first = Store::open(temp.path(), "", false).unwrap();
    let space = first.space.clone();
    let root = temp.path().to_owned();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let root = &root;
            let space = &space;
            scope.spawn(move||{let s=Store::open(root,space,false).unwrap();call(&s,"CreateTask",json!({"title":"Concurrent","repository":{"url":format!("https://example.com/{}",id())}})).unwrap();});
        }
    });
    assert_eq!(
        call(&first, "ListTasks", json!({})).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        8
    );
    drop(first);
    let mut s = Store::open(temp.path(), &space, true).unwrap();
    s.remove().unwrap();
    assert!(!temp.path().join("spaces").join(&space).exists());
    assert!(s.lock_path().exists());
    assert!(Store::open(temp.path(), &space, false).is_err());
    Store::open(temp.path(), &space, true)
        .unwrap()
        .remove()
        .unwrap();
}
#[test]
fn sqlite_engine_has_wal_reset_fix() {
    assert!(rusqlite::version_number() >= 3_051_003);
}

#[test]
fn sync_apply_vectors() {
    let mut selected = std::collections::BTreeSet::from([
        "whole-task-conflict",
        "ancestor-replay",
        "edit-then-delete",
        "delete-then-edit",
        "delete-then-delayed-create",
        "immutable-version-id",
        "unsupported-format",
    ]);
    for case in fixture("sync")["cases"].as_array().unwrap() {
        let name = strv(case, "id");
        if !selected.remove(name) {
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path(), "", false).unwrap();
        let apply = |version: &Value| {
            store.with(true, |connection| {
                lane::store::sync::apply(connection, &json!({"version":version}))
            })
        };
        for version in case["input"]["initial"].as_array().into_iter().flatten() {
            apply(version).unwrap();
        }
        if name == "unsupported-format" {
            assert_eq!(
                apply(&case["input"]["version"]).unwrap_err().reason,
                "ERROR_REASON_UNSUPPORTED_FORMAT"
            );
            continue;
        }
        let mut rejected = false;
        for mutation in case["input"]["mutations"].as_array().into_iter().flatten() {
            if let Err(error) = apply(&mutation["version"]) {
                assert_eq!(name, "immutable-version-id", "{error}");
                rejected = true;
            }
        }
        if name == "immutable-version-id" {
            assert!(rejected);
            continue;
        }
        let heads = store
            .with(false, |connection| {
                let mut statement =
                    connection.prepare("SELECT version_id FROM heads ORDER BY version_id")?;
                let values = statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(values)
            })
            .unwrap();
        assert_eq!(json!(heads), case["expected"]["headIds"], "{name}");
    }
    assert_eq!(selected.len(), 0, "missing fixtures: {selected:?}");
}

#[test]
fn invalid_stored_enums_and_body_kinds_fail_closed() {
    for (field, value) in [
        ("progress", json!(99)),
        ("proposal", json!(99)),
        ("kind", json!("ENTITY_KIND_REPOSITORY")),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path(), "", false).unwrap();
        let mut version = task(
            &store,
            &json!({"title":"Task","progress":"PROGRESS_TODO","proposal":"PROPOSAL_REGULAR"}),
        );
        if field == "kind" {
            version[field] = value;
        } else {
            version["task"][field] = value;
        }
        store
            .with(true, |connection| {
                connection.execute(
                    "UPDATE versions SET envelope_json=? WHERE version_id=?",
                    rusqlite::params![version.to_string(), strv(&version, "versionId")],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            store
                .invoke("GetTask", &json!({"id":version["entityId"]}))
                .unwrap_err()
                .reason,
            "ERROR_REASON_INVALID_INPUT"
        );
    }
}

#[test]
fn ssh_hosts_share_https_repository_identity() {
    for host in ["GitHub.com", "BÜCHER.example"] {
        let expected =
            lane::store::normalize::url(&format!("https://{host}/Acme/Repo.git")).unwrap();
        for input in [
            format!("git@{host}:Acme/Repo.git"),
            format!("ssh://git@{host}:22/Acme/Repo.git"),
        ] {
            assert_eq!(lane::store::normalize::url(&input).unwrap(), expected);
            let temp = tempfile::tempdir().unwrap();
            let store = Store::open(temp.path(), "", false).unwrap();
            let repository = call(&store, "CreateRepository", json!({"url": input})).unwrap();
            assert_eq!(
                repository["result"]["version"]["repository"]["normalizedUrl"],
                expected
            );
        }
    }
}

#[test]
fn synchronized_deletions_have_empty_history() {
    for kind in [
        "ENTITY_KIND_OR_REMOTE_KIND_TASK",
        "ENTITY_KIND_OR_REMOTE_KIND_REPOSITORY",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let s = Store::open(temp.path(), "", false).unwrap();
        let v = call(&s, "CreateTask", json!({"title":"Deleted", "prompt":"private", "repository":{"url":"https://example.com/deleted"}})).unwrap()["result"]["version"].clone();
        let target = if kind == "ENTITY_KIND_OR_REMOTE_KIND_TASK" {
            &v["entityId"]
        } else {
            &v["task"]["repositoryId"]
        };
        s.with(true, |c| {
            lane::store::sync::change(
                c,
                &json!({"deletion":{"id":target,"kind":kind,"deletionVersionId":id()}}),
                "",
            )
        })
        .unwrap();
        for prompts in [false, true] {
            let history = call(
                &s,
                "ListHistory",
                json!({"id":v["entityId"], "promptsOnly":prompts}),
            )
            .unwrap();
            assert_eq!(history["versions"], json!([]));
            assert_eq!(history["nextCursor"], "");
        }
    }
}

#[test]
fn standalone_import_projects_only_current_repository_heads() {
    let temp = tempfile::tempdir().unwrap();
    let source = Store::open(temp.path(), "", false).unwrap();
    let original = call(
        &source,
        "CreateRepository",
        json!({"url":"https://example.com/old"}),
    )
    .unwrap()["result"]["version"]
        .clone();
    let current = call(&source, "SaveRepository", json!({"id":original["entityId"],"expectedVersionId":original["versionId"],"url":"https://example.com/current"})).unwrap()["result"]["version"].clone();
    let task = call(
        &source,
        "CreateTask",
        json!({"repository":{"id":original["entityId"]},"title":"Imported","prompt":"preserved"}),
    )
    .unwrap()["result"]["version"]
        .clone();
    let dest = source.select_account("issuer", &id(), "tenant").unwrap();
    let existing = call(
        &dest,
        "CreateRepository",
        json!({"url":"https://example.com/old"}),
    )
    .unwrap()["result"]["version"]
        .clone();
    for _ in 0..2 {
        lane::syncclient::import(&source, &dest).unwrap();
        assert_eq!(
            call(&dest, "GetRepository", json!({"id":original["entityId"]})).unwrap()["result"]["version"],
            current
        );
        assert_eq!(
            call(&dest, "GetRepository", json!({"id":existing["entityId"]})).unwrap()["result"]["version"],
            existing
        );
        assert_eq!(
            call(&dest, "GetTask", json!({"id":task["entityId"]})).unwrap()["result"]["version"],
            task
        );
        let history = call(&dest, "ListHistory", json!({"id":original["entityId"]})).unwrap();
        assert_eq!(history["versions"], json!([original, current]));
        dest.with(false, |c| {
            let pending: i64 = c.query_row(
                "SELECT count(*) FROM outbox WHERE entity_id=?",
                [strv(&original, "entityId")],
                |r| r.get(0),
            )?;
            assert_eq!(pending, 2);
            Ok(())
        })
        .unwrap();
    }
}
