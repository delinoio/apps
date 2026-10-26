// SPDX-License-Identifier: Apache-2.0
//! Behavior of the local planning store beyond the shared vectors: persistence across restarts,
//! idempotent retries, atomicity, deletion cleanup and history erasure, conflicts and the
//! explicit development reset.
mod support;
use lane::{
    protocol::{id, strv},
    store::{self, Store},
};
use rusqlite::Connection;
use serde_json::{Value, json};
use support::{call, norm};

fn open() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path(), "", false).unwrap();
    (dir, store)
}
fn repository(store: &Store, url: &str) -> String {
    let created = call(store, "CreateRepository", &json!({"url":url})).unwrap();
    strv(&created["result"]["version"], "entityId").to_owned()
}
fn planning(store: &Store, repo: &str, field: &str, name: &str) -> Value {
    let mut body = json!({"repositoryId":repo,"name":name});
    match field {
        "label" => body["color"] = "#1d76db".into(),
        "cycle" => {
            body["startDate"] = "2026-01-01".into();
            body["endDate"] = "2026-01-14".into();
        }
        _ => {}
    }
    call(
        store,
        "CreatePlanningItem",
        &json!({"item":{ field: body }}),
    )
    .unwrap()["result"]["version"]
        .clone()
}
fn task(store: &Store, repo: &str, title: &str, extra: &Value) -> Value {
    let mut request = json!({"repository":{"id":repo},"title":title});
    for (k, v) in extra.as_object().unwrap() {
        request[k] = v.clone();
    }
    call(store, "CreateTask", &request).unwrap()
}
fn current(store: &Store, id: &str) -> Value {
    store.invoke("GetTask", &json!({"id":id})).unwrap()["result"]["version"].clone()
}
fn count(store: &Store, sql: &str) -> i64 {
    store
        .with(false, |c| Ok(c.query_row(sql, [], |r| r.get(0))?))
        .unwrap()
}

#[test]
fn schema_three_persists_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let (space, repo_id, label_id, task_id) = {
        let store = Store::open(dir.path(), "", false).unwrap();
        let repo = repository(&store, "https://example.com/persist");
        let label = planning(&store, &repo, "label", "Backend");
        let milestone = planning(&store, &repo, "milestone", "M1");
        let created = task(
            &store,
            &repo,
            "Persisted",
            &json!({
                "priority":"TASK_PRIORITY_HIGH","type":"TASK_TYPE_BUG",
                "labelIds":[label["entityId"]],"startDate":"2026-02-01","dueDate":"2026-02-02",
                "storyPoints":0,"milestoneId":milestone["entityId"]
            }),
        );
        assert_eq!(created["result"]["version"]["formatVersion"], 3);
        assert_eq!(label["formatVersion"], 3);
        (
            store.space,
            repo,
            strv(&label, "entityId").to_owned(),
            strv(&created["result"]["version"], "entityId").to_owned(),
        )
    };
    let store = Store::open(dir.path(), &space, false).unwrap();
    let t = current(&store, &task_id);
    assert_eq!(t["task"]["priority"], "TASK_PRIORITY_HIGH");
    assert_eq!(t["task"]["storyPoints"], 0);
    assert_eq!(t["task"]["labelIds"], json!([label_id]));
    let listed = store
        .invoke(
            "ListPlanningItems",
            &json!({"repositoryId":repo_id,"page":{}}),
        )
        .unwrap();
    assert_eq!(listed["items"].as_array().unwrap().len(), 2);
    store
        .with(false, |c| {
            assert_eq!(store::meta(c, "format_version")?, "3");
            assert_eq!(
                c.query_row::<i64, _, _>("PRAGMA user_version", [], |r| r.get(0))?,
                3
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn unsupported_schema_names_the_reset_command_and_is_never_modified() {
    let dir = tempfile::tempdir().unwrap();
    let space = Store::open(dir.path(), "", false).unwrap().space;
    let path = dir.path().join("spaces").join(&space).join("lane.sqlite");
    for version in [1, 2, 4] {
        Connection::open(&path)
            .unwrap()
            .execute_batch(&format!("PRAGMA user_version={version}"))
            .unwrap();
        let error = Store::open(dir.path(), &space, false).err().unwrap();
        assert_eq!(error.reason, "ERROR_REASON_UNSUPPORTED_FORMAT");
        assert!(
            error
                .message
                .contains("data reset --confirm-development-reset")
                && error.message.contains(&space),
            "{}",
            error.message
        );
        let after: i64 = Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            after, version,
            "an unsupported store is never migrated or reset"
        );
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "One scenario covers edit, conflict, resolution and deletion of an item."
)]
fn planning_items_validate_edit_conflict_and_delete_with_history_erasure() {
    let (_dir, store) = open();
    let repo = repository(&store, "https://example.com/items");
    let label = planning(&store, &repo, "label", "Backend");
    let label_id = strv(&label, "entityId").to_owned();
    assert_eq!(label["label"]["color"], "#1D76DB");
    let created = task(&store, &repo, "Subject", &json!({"labelIds":[label_id]}));
    let task_id = strv(&created["result"]["version"], "entityId").to_owned();

    // Edit: history gains a version of the item's own.
    let saved = call(
        &store,
        "SavePlanningItem",
        &json!({"id":label_id,"expectedVersionId":label["versionId"],
            "item":{"label":{"repositoryId":repo,"name":"Back end","color":"#ff0000","description":"d"}}}),
    )
    .unwrap();
    assert_eq!(saved["result"]["version"]["label"]["color"], "#FF0000");
    let history = store
        .invoke("ListHistory", &json!({"id":label_id,"page":{}}))
        .unwrap();
    assert_eq!(history["versions"].as_array().unwrap().len(), 2);

    // A stale save is preserved as a whole-item conflict and resolves like any entity.
    let stale = call(
        &store,
        "SavePlanningItem",
        &json!({"id":label_id,"expectedVersionId":label["versionId"],
            "item":{"label":{"repositoryId":repo,"name":"Alt","color":"#00ff00"}}}),
    )
    .unwrap_err();
    assert_eq!(stale.reason, "ERROR_REASON_STALE_VERSION");
    let conflicted = store
        .invoke("GetPlanningItem", &json!({"id":label_id}))
        .unwrap();
    assert_eq!(conflicted["result"]["result"], "RESULT_KIND_CONFLICT");
    let heads: Vec<Value> = conflicted["result"]["alternatives"]
        .as_array()
        .unwrap()
        .clone();
    let mut expected: Vec<String> = heads
        .iter()
        .map(|h| strv(h, "versionId").to_owned())
        .collect();
    expected.sort();
    assert_eq!(
        call(
            &store,
            "SavePlanningItem",
            &json!({"id":label_id,"expectedVersionId":heads[0]["versionId"],
                "item":{"label":{"repositoryId":repo,"name":"X","color":"#000000"}}}),
        )
        .unwrap_err()
        .reason,
        "ERROR_REASON_CONFLICT_READ_ONLY"
    );
    let resolved = call(
        &store,
        "ResolveConflict",
        &json!({"id":label_id,"expectedHeadIds":expected,"resolved":{
            "formatVersion":3,"entityId":label_id,"kind":"ENTITY_KIND_LABEL",
            "label":{"repositoryId":repo,"name":"Merged","color":"#123456"}}}),
    )
    .unwrap();
    assert_eq!(resolved["result"]["version"]["label"]["name"], "Merged");

    // Delete detaches the label from the task (a new task version) and erases its history.
    let head = store
        .invoke("GetPlanningItem", &json!({"id":label_id}))
        .unwrap();
    let before_task = current(&store, &task_id)["versionId"].clone();
    let deleted = call(&store, "DeletePlanningItem", &json!({"id":label_id})).unwrap();
    assert_eq!(deleted["result"]["result"], "RESULT_KIND_DELETED");
    let after = current(&store, &task_id);
    assert_ne!(after["versionId"], before_task);
    assert_eq!(after["task"]["labelIds"], json!([]));
    assert_eq!(after["origin"]["kind"], "ORIGIN_KIND_CLI");
    assert_eq!(
        count(
            &store,
            &format!("SELECT count(*) FROM versions WHERE entity_id='{label_id}'")
        ),
        1
    );
    assert_eq!(
        count(
            &store,
            &format!("SELECT count(*) FROM outbox WHERE entity_id='{label_id}'")
        ),
        1,
        "only the tombstone remains pending"
    );
    let serialized = store
        .with(false, |c| {
            Ok(store::rows(
                c,
                "SELECT envelope_json || request_json FROM versions JOIN outbox ON 1=1",
                [],
            )?
            .join(""))
        })
        .unwrap();
    for name in ["Backend", "Back end", "Merged", "Alt"] {
        assert!(!serialized.contains(name), "{name} survived deletion");
    }
    let _ = head;
    // Replaying the delete returns DELETED without content; a deleted item cannot be edited.
    let replay = call(&store, "DeletePlanningItem", &json!({"id":label_id})).unwrap();
    assert_eq!(replay["result"]["result"], "RESULT_KIND_DELETED");
    // Its name is reusable by a new item.
    planning(&store, &repo, "label", "Backend");
}

#[test]
fn retries_are_idempotent_and_changed_content_is_rejected() {
    let (_dir, store) = open();
    let repo = repository(&store, "https://example.com/retry");
    let other = id();
    let request_id = id();
    let body = json!({"meta":{"requestId":request_id},"id":other,
        "item":{"cycle":{"repositoryId":repo,"name":"Sprint","startDate":"2026-03-01","endDate":"2026-03-14"}}});
    let first = call(&store, "CreatePlanningItem", &body).unwrap();
    let replay = call(&store, "CreatePlanningItem", &body).unwrap();
    assert_eq!(replay["result"]["result"], "RESULT_KIND_REPLAYED");
    assert_eq!(
        replay["result"]["version"]["versionId"],
        first["result"]["version"]["versionId"]
    );
    assert_eq!(
        count(
            &store,
            "SELECT count(*) FROM versions WHERE entity_id IN (SELECT id FROM entities WHERE kind=6)"
        ),
        1
    );
    let mut changed = body;
    changed["item"]["cycle"]["endDate"] = "2026-03-15".into();
    assert_eq!(
        call(&store, "CreatePlanningItem", &changed)
            .unwrap_err()
            .reason,
        "ERROR_REASON_REQUEST_ID_REUSED"
    );

    // Relationship edits replay with the same affected versions and no duplicate history.
    let a = task(&store, &repo, "A", &json!({}));
    let b = task(&store, &repo, "B", &json!({}));
    let (a_id, b_id) = (
        strv(&a["result"]["version"], "entityId").to_owned(),
        strv(&b["result"]["version"], "entityId").to_owned(),
    );
    let save = json!({"meta":{"requestId":id()},"id":a_id,
        "expectedVersionId":a["result"]["version"]["versionId"],
        "task":a["result"]["version"]["task"],
        "relationshipEdits":{"addSubtaskIds":[b_id,b_id]}});
    let applied = call(&store, "SaveTask", &save).unwrap();
    assert_eq!(applied["affected"].as_array().unwrap().len(), 1);
    let versions_before = count(&store, "SELECT count(*) FROM versions");
    let outbox_before = count(&store, "SELECT count(*) FROM outbox");
    let replayed = call(&store, "SaveTask", &save).unwrap();
    assert_eq!(replayed["result"]["result"], "RESULT_KIND_REPLAYED");
    assert_eq!(
        replayed["affected"][0]["version"]["versionId"],
        applied["affected"][0]["version"]["versionId"]
    );
    assert_eq!(
        count(&store, "SELECT count(*) FROM versions"),
        versions_before
    );
    assert_eq!(count(&store, "SELECT count(*) FROM outbox"), outbox_before);
    assert_eq!(current(&store, &b_id)["task"]["parentTaskId"], a_id);
    assert_eq!(current(&store, &b_id)["origin"]["kind"], "ORIGIN_KIND_CLI");
}

#[test]
fn failed_relationship_edit_leaves_no_partial_state_and_conflicted_peers_are_read_only() {
    let (_dir, store) = open();
    let repo = repository(&store, "https://example.com/atomic");
    let a = task(&store, &repo, "A", &json!({}));
    let b = task(&store, &repo, "B", &json!({}));
    let c = task(&store, &repo, "C", &json!({}));
    let ids: Vec<String> = [&a, &b, &c]
        .iter()
        .map(|t| strv(&t["result"]["version"], "entityId").to_owned())
        .collect();
    // B depends on A; making A depend on B again is a cycle even when another peer edit
    // in the same request would have succeeded.
    call(
        &store,
        "SaveTask",
        &json!({"id":ids[1],"expectedVersionId":b["result"]["version"]["versionId"],
            "task":{"repositoryId":repo,"title":"B","progress":"PROGRESS_TODO","proposal":"PROPOSAL_REGULAR","dependsOnTaskIds":[ids[0]]}}),
    )
    .unwrap();
    let versions = count(&store, "SELECT count(*) FROM versions");
    let outbox = count(&store, "SELECT count(*) FROM outbox");
    let changes = count(&store, "SELECT count(*) FROM changes");
    let error = call(
        &store,
        "SaveTask",
        &json!({"id":ids[0],"expectedVersionId":a["result"]["version"]["versionId"],
            "task":{"repositoryId":repo,"title":"A","progress":"PROGRESS_TODO","proposal":"PROPOSAL_REGULAR","dependsOnTaskIds":[ids[1]]},
            "relationshipEdits":{"addSubtaskIds":[ids[2]]}}),
    )
    .unwrap_err();
    assert_eq!(error.reason, "ERROR_REASON_CONSTRAINT_VIOLATION");
    assert_eq!(
        error.violation.map(lane::error::Violation::as_str),
        Some("CONSTRAINT_VIOLATION_DEPENDENCY_CYCLE")
    );
    assert_eq!(count(&store, "SELECT count(*) FROM versions"), versions);
    assert_eq!(count(&store, "SELECT count(*) FROM outbox"), outbox);
    assert_eq!(count(&store, "SELECT count(*) FROM changes"), changes);
    assert!(current(&store, &ids[2])["task"]["parentTaskId"].is_null());

    // A conflicted peer cannot be edited through a relationship edit: whole operation fails.
    let version = current(&store, &ids[2]);
    let alternative = id();
    store
        .with(true, |db| {
            let mut v = version.clone();
            v["versionId"] = alternative.clone().into();
            v["parentVersionIds"] = json!([]);
            v["task"]["title"] = "C2".into();
            db.execute(
                "INSERT INTO versions(version_id,entity_id,envelope_json,created_at_us) VALUES(?,?,?,2)",
                rusqlite::params![alternative, ids[2], v.to_string()],
            )?;
            db.execute("INSERT INTO heads VALUES(?,?)", rusqlite::params![ids[2], alternative])?;
            Ok(())
        })
        .unwrap();
    let error = call(
        &store,
        "SaveTask",
        &json!({"id":ids[0],"expectedVersionId":current(&store, &ids[0])["versionId"],
            "task":current(&store, &ids[0])["task"],
            "relationshipEdits":{"addSubtaskIds":[ids[2]]}}),
    )
    .unwrap_err();
    assert_eq!(error.reason, "ERROR_REASON_CONFLICT_READ_ONLY", "{error:?}");
}

#[test]
fn progress_proposal_prompt_and_deletes_preserve_and_clean_planning_fields() {
    let (_dir, store) = open();
    let repo = repository(&store, "https://example.com/preserve");
    let label = planning(&store, &repo, "label", "L");
    let parent = task(&store, &repo, "Parent", &json!({}));
    let parent_id = strv(&parent["result"]["version"], "entityId").to_owned();
    let child = task(
        &store,
        &repo,
        "Child",
        &json!({"prompt":"keep","labelIds":[label["entityId"]],"parentTaskId":parent_id,
            "priority":"TASK_PRIORITY_URGENT","storyPoints":3,"proposal":"PROPOSAL_PENDING"}),
    );
    let child_version = &child["result"]["version"];
    let child_id = strv(child_version, "entityId");
    let progressed = call(
        &store,
        "SetProgress",
        &json!({"id":child_id,"expectedVersionId":child_version["versionId"],"progress":"PROGRESS_DONE"}),
    )
    .unwrap();
    let after = &progressed["result"]["version"]["task"];
    assert_eq!(after["parentTaskId"], parent_id);
    assert_eq!(after["priority"], "TASK_PRIORITY_URGENT");
    assert_eq!(after["storyPoints"], 3);
    assert_eq!(after["labelIds"], json!([label["entityId"]]));
    let accepted = call(
        &store,
        "SetProposal",
        &json!({"id":child_id,"expectedVersionId":progressed["result"]["version"]["versionId"],"proposal":"PROPOSAL_ACCEPTED"}),
    )
    .unwrap();
    assert_eq!(
        accepted["result"]["version"]["task"]["parentTaskId"],
        parent_id
    );
    // Relationships never alter progress or proposal state.
    assert_eq!(
        accepted["result"]["version"]["task"]["progress"],
        "PROGRESS_DONE"
    );
    let restored = call(
        &store,
        "RestorePrompt",
        &json!({"id":child_id,"expectedVersionId":accepted["result"]["version"]["versionId"],
            "sourceVersionId":child_version["versionId"]}),
    )
    .unwrap();
    assert_eq!(restored["result"]["version"]["task"]["storyPoints"], 3);
    assert_eq!(
        restored["result"]["version"]["task"]["parentTaskId"],
        parent_id
    );

    // Deleting the parent removes the reference from the child with the deleting origin.
    call(&store, "DeleteTask", &json!({"id":parent_id})).unwrap();
    let child_now = current(&store, child_id);
    assert!(child_now["task"]["parentTaskId"].is_null());
    assert_eq!(child_now["task"]["priority"], "TASK_PRIORITY_URGENT");

    // Repository deletion cascades tasks and planning items alike.
    call(&store, "DeleteRepository", &json!({"id":repo})).unwrap();
    assert_eq!(
        count(
            &store,
            "SELECT count(*) FROM entities WHERE deleted=0 AND kind<>3"
        ),
        0
    );
    assert_eq!(
        store
            .invoke("GetPlanningItem", &json!({"id":label["entityId"]}))
            .unwrap()["result"]["result"],
        "RESULT_KIND_DELETED"
    );
}

#[test]
fn moving_a_task_cleans_repository_owned_references() {
    let (_dir, store) = open();
    let repo = repository(&store, "https://example.com/from");
    let target = repository(&store, "https://example.com/to");
    let label = planning(&store, &repo, "label", "L");
    let peer = task(&store, &repo, "Peer", &json!({}));
    let peer_id = strv(&peer["result"]["version"], "entityId").to_owned();
    let subject = task(
        &store,
        &repo,
        "Subject",
        &json!({"labelIds":[label["entityId"]],"relatedTaskIds":[peer_id],"priority":"TASK_PRIORITY_LOW"}),
    );
    let subject_id = strv(&subject["result"]["version"], "entityId").to_owned();
    assert_eq!(
        current(&store, &peer_id)["task"]["relatedTaskIds"],
        json!([subject_id])
    );
    let mut moved = subject["result"]["version"]["task"].clone();
    moved["repositoryId"] = target.into();
    let saved = call(
        &store,
        "SaveTask",
        &json!({"id":subject_id,"expectedVersionId":subject["result"]["version"]["versionId"],"task":moved}),
    )
    .unwrap();
    let task_after = &saved["result"]["version"]["task"];
    assert_eq!(task_after["labelIds"], json!([]));
    assert_eq!(task_after["relatedTaskIds"], json!([]));
    assert_eq!(task_after["priority"], "TASK_PRIORITY_LOW");
    assert_eq!(
        current(&store, &peer_id)["task"]["relatedTaskIds"],
        json!([])
    );
    // The label stays in its repository.
    assert_eq!(
        store
            .invoke("GetPlanningItem", &json!({"id":label["entityId"]}))
            .unwrap()["result"]["result"],
        "RESULT_KIND_APPLIED"
    );
}

#[test]
fn resolve_conflict_validates_planning_rules_for_tasks() {
    let (_dir, store) = open();
    let repo = repository(&store, "https://example.com/resolve");
    let other_repo = repository(&store, "https://example.com/resolve-other");
    let foreign = planning(&store, &other_repo, "label", "Foreign");
    let created = task(&store, &repo, "Base", &json!({}));
    let version = created["result"]["version"].clone();
    let task_id = strv(&version, "entityId").to_owned();
    // A stale edit becomes a second head.
    let first = call(
        &store,
        "SaveTask",
        &json!({"id":task_id,"expectedVersionId":version["versionId"],
            "task":{"repositoryId":repo,"title":"One","progress":"PROGRESS_TODO","proposal":"PROPOSAL_REGULAR"}}),
    )
    .unwrap();
    let _ = first;
    let _ = call(
        &store,
        "SaveTask",
        &json!({"id":task_id,"expectedVersionId":version["versionId"],
            "task":{"repositoryId":repo,"title":"Two","progress":"PROGRESS_TODO","proposal":"PROPOSAL_REGULAR"}}),
    )
    .unwrap_err();
    let heads = store.invoke("GetTask", &json!({"id":task_id})).unwrap();
    let mut head_ids: Vec<String> = heads["result"]["alternatives"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| strv(h, "versionId").to_owned())
        .collect();
    head_ids.sort();
    let error = call(
        &store,
        "ResolveConflict",
        &json!({"id":task_id,"expectedHeadIds":head_ids,"resolved":{
            "formatVersion":3,"entityId":task_id,"kind":"ENTITY_KIND_TASK",
            "task":{"repositoryId":repo,"title":"Merged","progress":"PROGRESS_TODO","proposal":"PROPOSAL_REGULAR","labelIds":[foreign["entityId"]]}}}),
    )
    .unwrap_err();
    assert_eq!(
        error.violation.map(lane::error::Violation::as_str),
        Some("CONSTRAINT_VIOLATION_CROSS_REPOSITORY")
    );
    let ok = call(
        &store,
        "ResolveConflict",
        &json!({"id":task_id,"expectedHeadIds":head_ids,"resolved":{
            "formatVersion":3,"entityId":task_id,"kind":"ENTITY_KIND_TASK",
            "task":{"repositoryId":repo,"title":"Merged","progress":"PROGRESS_TODO","proposal":"PROPOSAL_REGULAR","dueDate":"2026-05-05"}}}),
    )
    .unwrap();
    assert_eq!(ok["result"]["version"]["task"]["dueDate"], "2026-05-05");
}

#[test]
fn development_reset_recreates_an_empty_store_for_the_same_space() {
    let dir = tempfile::tempdir().unwrap();
    let (space, device, other_space) = {
        let store = Store::open(dir.path(), "", false).unwrap();
        let repo = repository(&store, "https://example.com/reset");
        planning(&store, &repo, "label", "Gone");
        task(
            &store,
            &repo,
            "Gone",
            &json!({"prompt":"secret prompt text"}),
        );
        let other = store.select_account("issuer", &id(), "tenant").unwrap();
        let other_repo = repository(&other, "https://example.com/other");
        task(&other, &other_repo, "Stays", &json!({}));
        (store.space, store.device, other.space)
    };
    let lock = dir
        .path()
        .join("locks")
        .join(format!("{space}.handles.lock"));
    assert!(lock.is_file());
    // A held handle makes the reset fail, retryably, without changing anything.
    {
        let holder = Store::open(dir.path(), &space, false).unwrap();
        let before = count(&holder, "SELECT count(*) FROM entities");
        let error = store::reset(dir.path(), &space).unwrap_err();
        assert_eq!(error.reason, "ERROR_REASON_STORAGE_BUSY");
        assert!(error.retryable);
        assert_eq!(count(&holder, "SELECT count(*) FROM entities"), before);
    }
    let result = store::reset(dir.path(), &space).unwrap();
    assert_eq!(result["completed"], true);
    assert_eq!(result["reconnectRequired"], false);
    assert!(lock.is_file(), "the rendezvous lock file is never removed");
    let store = Store::open(dir.path(), &space, false).unwrap();
    assert_eq!(
        (store.space.as_str(), store.device.as_str()),
        (space.as_str(), device.as_str())
    );
    for table in [
        "entities",
        "versions",
        "heads",
        "receipts",
        "outbox",
        "changes",
        "attachments",
    ] {
        assert_eq!(
            count(&store, &format!("SELECT count(*) FROM {table}")),
            0,
            "{table}"
        );
    }
    store
        .with(false, |c| {
            assert_eq!(store::meta(c, "format_version")?, "3");
            assert_eq!(store::meta(c, "space_id")?, space);
            assert_eq!(store::meta(c, "reconnect_required")?, "");
            Ok(())
        })
        .unwrap();
    // The raw files no longer contain the discarded content.
    let mut bytes = vec![];
    for entry in std::fs::read_dir(dir.path().join("spaces").join(&space)).unwrap() {
        bytes.extend(std::fs::read(entry.unwrap().path()).unwrap());
    }
    assert!(!String::from_utf8_lossy(&bytes).contains("secret prompt text"));
    // Another space is untouched, and the reset space accepts new work.
    let other = Store::open(dir.path(), &other_space, false).unwrap();
    assert_eq!(
        count(&other, "SELECT count(*) FROM entities WHERE kind=2"),
        1
    );
    let repo = repository(&store, "https://example.com/after");
    task(&store, &repo, "Fresh", &json!({}));
    // Local-only validation.
    assert_eq!(
        store::reset(dir.path(), "not-a-uuid").unwrap_err().reason,
        "ERROR_REASON_INVALID_INPUT"
    );
    assert_eq!(
        store::reset(dir.path(), &id()).unwrap_err().reason,
        "ERROR_REASON_NOT_FOUND"
    );
}

#[test]
fn account_space_reset_keeps_the_binding_and_requires_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let local = Store::open(dir.path(), "", false).unwrap();
    let account = local.select_account("issuer", &id(), "tenant").unwrap();
    let space = account.space.clone();
    let repo = repository(&account, "https://example.com/account");
    task(&account, &repo, "Gone", &json!({}));
    let identity = account.identity().unwrap();
    drop((account, local));
    store::reset(dir.path(), &space).unwrap();
    let account = Store::open(dir.path(), &space, false).unwrap();
    assert_eq!(account.identity().unwrap().account, identity.account);
    assert_eq!(count(&account, "SELECT count(*) FROM entities"), 0);
    account
        .with(false, |c| {
            assert_eq!(store::meta(c, "reconnect_required")?, "1");
            Ok(())
        })
        .unwrap();
}

#[test]
fn interrupted_reset_is_never_opened_as_valid_data() {
    let dir = tempfile::tempdir().unwrap();
    let space = Store::open(dir.path(), "", false).unwrap().space;
    let registry = Connection::open(dir.path().join("registry.sqlite")).unwrap();
    registry
        .execute(
            "INSERT INTO registry_meta VALUES(?,'1')",
            [format!("reset_pending:{space}")],
        )
        .unwrap();
    let error = Store::open(dir.path(), &space, false).err().unwrap();
    assert_eq!(error.reason, "ERROR_REASON_UNSUPPORTED_FORMAT");
    assert!(error.message.contains("data reset"));
    // Repeating the command completes the reset and clears the marker.
    store::reset(dir.path(), &space).unwrap();
    assert!(Store::open(dir.path(), &space, false).is_ok());
}

#[test]
fn normalization_of_stored_planning_bodies_is_stable() {
    let (_dir, store) = open();
    let repo = repository(&store, "https://example.com/stable");
    let a = planning(&store, &repo, "milestone", "  Release  ");
    assert_eq!(a["milestone"]["name"], "Release");
    assert_eq!(a["milestone"]["completed"], false);
    assert_eq!(
        norm("PlanningBody", &json!({"milestone":a["milestone"]}))["milestone"]["repositoryId"],
        repo
    );
}

#[test]
fn pulled_planning_versions_and_tombstones_apply_and_old_formats_are_refused() {
    let (_dir, store) = open();
    let repo = repository(&store, "https://example.com/pull");
    let version = |entity: &str, kind: &str, body_field: &str, body: Value, format: u32| {
        json!({"formatVersion":format,"entityId":entity,"versionId":id(),"kind":kind,
            "parentVersionIds":[],"createdAt":"2026-04-01T00:00:00.000000Z",
            "origin":{"kind":"ORIGIN_KIND_APP","deviceId":id(),"deviceName":"Other"},
            body_field:body})
    };
    let label_id = id();
    let label = version(
        &label_id,
        "ENTITY_KIND_LABEL",
        "label",
        json!({"repositoryId":repo,"name":"Pulled","color":"#ABCDEF"}),
        3,
    );
    let task_id = id();
    let task = version(
        &task_id,
        "ENTITY_KIND_TASK",
        "task",
        json!({"repositoryId":repo,"title":"Pulled task","progress":"PROGRESS_TODO","proposal":"PROPOSAL_REGULAR","labelIds":[label_id],"priority":"TASK_PRIORITY_LOW"}),
        3,
    );
    store
        .with(true, |c| {
            store::sync::apply(c, &json!({"version":label}))?;
            store::sync::apply(c, &json!({"version":task}))?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store
            .invoke("GetPlanningItem", &json!({"id":label_id}))
            .unwrap()["result"]["version"]["label"]["name"],
        "Pulled"
    );
    assert_eq!(
        current(&store, &task_id)["task"]["labelIds"],
        json!([label_id])
    );
    assert_eq!(
        store
            .invoke(
                "ListTasks",
                &json!({"filter":{"labelIds":[label_id]},"page":{}})
            )
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // A server tombstone removes the item and its history.
    let tombstone = json!({"formatVersion":3,"entityId":label_id,"versionId":id(),"kind":"ENTITY_KIND_LABEL","deleted":true});
    store
        .with(true, |c| {
            store::sync::apply(c, &json!({"version":tombstone}))
        })
        .unwrap();
    assert_eq!(
        store
            .invoke("GetPlanningItem", &json!({"id":label_id}))
            .unwrap()["result"]["result"],
        "RESULT_KIND_DELETED"
    );
    // Planning data cannot hide in an earlier envelope format; other formats are refused.
    let mut old = version(
        &id(),
        "ENTITY_KIND_LABEL",
        "label",
        json!({"repositoryId":repo,"name":"Old","color":"#000000"}),
        2,
    );
    assert!(store::sync::validate(&old).is_err());
    old["formatVersion"] = 4.into();
    assert_eq!(
        store::sync::validate(&old).unwrap_err().reason,
        "ERROR_REASON_UNSUPPORTED_FORMAT"
    );
    let mut task_v2 = task;
    task_v2["formatVersion"] = 2.into();
    assert!(store::sync::validate(&task_v2).is_err());
    task_v2["task"] = json!({"repositoryId":repo,"title":"Plain","progress":"PROGRESS_TODO","proposal":"PROPOSAL_REGULAR"});
    assert!(store::sync::validate(&task_v2).is_ok());
}

#[test]
fn correcting_a_rejected_version_replaces_it_and_keeps_dependents_after_it() {
    let (_dir, store) = open();
    let repo = repository(&store, "https://example.com/supersede");
    let label = planning(&store, &repo, "label", "Dup");
    let label_id = strv(&label, "entityId").to_owned();
    let created = task(&store, &repo, "Uses label", &json!({"labelIds":[label_id]}));
    let task_version = created["result"]["version"]["versionId"].clone();
    store
        .with(true, |c| {
            c.execute(
                "UPDATE outbox SET state=2,error_reason='CONSTRAINT_VIOLATION:DUPLICATE_NAME' WHERE entity_id=?",
                [&label_id],
            )?;
            Ok(())
        })
        .unwrap();
    let renamed = call(
        &store,
        "SavePlanningItem",
        &json!({"id":label_id,"expectedVersionId":label["versionId"],
            "item":{"label":{"repositoryId":repo,"name":"Unique","color":"#112233"}}}),
    )
    .unwrap()["result"]["version"]
        .clone();
    assert_eq!(renamed["parentVersionIds"], json!([]));
    // The rejected version stays in local history but is no longer pending; the corrected
    // one takes its upload position, ahead of the task that references the label.
    assert_eq!(
        store
            .invoke("ListHistory", &json!({"id":label_id,"page":{}}))
            .unwrap()["versions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let queue: Vec<(String, i64, String)> = store
        .with(false, |c| {
            let mut stmt = c.prepare(
                "SELECT entity_id,state,request_json FROM outbox ORDER BY created_at_us,rowid",
            )?;
            Ok(stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        })
        .unwrap();
    let positions: Vec<&str> = queue.iter().map(|q| q.0.as_str()).collect();
    assert_eq!(
        positions[1..],
        [
            label_id.as_str(),
            created["result"]["version"]["entityId"].as_str().unwrap()
        ]
    );
    assert!(queue.iter().all(|q| q.1 == 1));
    assert!(queue[1].2.contains("MUTATION_KIND_CREATE"));
    // Rebuilding the upload order after a reconnect keeps referenced entities first.
    let ordered = store.with(false, store::sync::ordered).unwrap();
    let index = |version: &Value| {
        ordered
            .iter()
            .position(|v| v["versionId"] == *version)
            .unwrap()
    };
    assert!(index(&renamed["versionId"]) < index(&task_version));
}

#[test]
fn stale_save_validates_the_graph_that_is_persisted() {
    let (_dir, store) = open();
    let repo = repository(&store, "https://example.com/stale-cycle");
    let a = task(&store, &repo, "A", &json!({}));
    let a_version = a["result"]["version"].clone();
    let a_id = strv(&a_version, "entityId").to_owned();
    let b = task(&store, &repo, "B", &json!({"parentTaskId":a_id}));
    let b_id = strv(&b["result"]["version"], "entityId").to_owned();
    // A moves on so the next save of A is stale.
    let mut renamed = a_version["task"].clone();
    renamed["title"] = "A2".into();
    call(
        &store,
        "SaveTask",
        &json!({"id":a_id,"expectedVersionId":a_version["versionId"],"task":renamed}),
    )
    .unwrap();
    // The stale snapshot makes B the parent of A and "removes" B's parent edge, but a stale
    // save never edits other tasks, so B keeps A as parent and the alternative closes a cycle.
    let mut alternative = a_version["task"].clone();
    alternative["parentTaskId"] = b_id.clone().into();
    let error = call(
        &store,
        "SaveTask",
        &json!({"id":a_id,"expectedVersionId":a_version["versionId"],"task":alternative,
            "relationshipEdits":{"removeSubtaskIds":[b_id]}}),
    )
    .unwrap_err();
    assert_eq!(
        error.violation.map(lane::error::Violation::as_str),
        Some("CONSTRAINT_VIOLATION_PARENT_CYCLE")
    );
    let heads = store.invoke("GetTask", &json!({"id":a_id})).unwrap();
    assert_eq!(
        heads["result"]["alternatives"]
            .as_array()
            .map_or(0, Vec::len),
        0
    );
    assert_eq!(current(&store, &b_id)["task"]["parentTaskId"], a_id);
}
