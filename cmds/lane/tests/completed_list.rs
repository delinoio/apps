// SPDX-License-Identifier: Apache-2.0
mod support;
use lane::{protocol::strv, store::Store};
use rusqlite::params;
use serde_json::{Value, json};
use support::{call, fixture};

fn seeded() -> (tempfile::TempDir, Store, Value) {
    let suite = fixture("completed-list");
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path(), "", false).unwrap();
    store.with(true, |c| {
        for row in suite["dataset"]["rows"].as_array().unwrap() {
            let selected = suite["dataset"]["versions"]
                .as_array().unwrap().iter()
                .find(|v| v["versionId"] == row["selectedVersionId"]).unwrap();
            let entity = strv(selected, "entityId");
            c.execute(
                "INSERT INTO entities(id,kind,repository_id,deleted) VALUES (?,2,?,0)",
                params![entity, strv(&selected["task"], "repositoryId")],
            )?;
        }
        for version in suite["dataset"]["versions"].as_array().unwrap() {
            c.execute(
                "INSERT INTO versions(version_id,entity_id,envelope_json,created_at_us) VALUES (?,?,?,1)",
                params![strv(version, "versionId"), strv(version, "entityId"), version.to_string()],
            )?;
        }
        for row in suite["dataset"]["rows"].as_array().unwrap() {
            let selected = suite["dataset"]["versions"]
                .as_array().unwrap().iter()
                .find(|v| v["versionId"] == row["selectedVersionId"]).unwrap();
            let entity = strv(selected, "entityId");
            for head in row["headVersionIds"].as_array().unwrap() {
                c.execute("INSERT INTO heads VALUES (?,?)", params![entity, head.as_str().unwrap()])?;
            }
        }
        Ok(())
    }).unwrap();
    (dir, store, suite)
}

#[test]
fn shared_completed_list_vectors() {
    let (_dir, store, suite) = seeded();
    for case in suite["listCases"].as_array().unwrap() {
        let mut request = case["input"]["request"].clone();
        let mut pages = Vec::new();
        loop {
            let response = call(&store, "ListTasks", &request)
                .unwrap_or_else(|error| panic!("{}: {error:?}", case["id"]));
            pages.push(
                response["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| {
                        if item["version"].is_object() {
                            strv(&item["version"], "entityId").to_owned()
                        } else {
                            strv(&item["alternatives"][0], "entityId").to_owned()
                        }
                    })
                    .collect::<Vec<_>>(),
            );
            let next = strv(&response, "nextCursor");
            if next.is_empty() {
                break;
            }
            request["page"]["cursor"] = next.into();
            assert!(pages.len() < 100, "{}", case["id"]);
        }
        let expected: Vec<Vec<String>> =
            serde_json::from_value(case["expected"]["pages"].clone()).unwrap();
        assert_eq!(pages, expected, "{}", case["id"]);
    }
    for case in suite["invalidCases"].as_array().unwrap() {
        let error = call(&store, "ListTasks", &case["request"]).unwrap_err();
        assert_eq!(error.reason, "ERROR_REASON_INVALID_INPUT", "{}", case["id"]);
    }
}

#[test]
fn completed_cursor_binds_cutoff_and_selection() {
    let (_dir, store, _) = seeded();
    let first = call(&store, "ListTasks", &json!({
        "page":{"size":1},
        "completed":{"selection":"COMPLETED_SELECTION_LAST_24_HOURS","cutoff":"2026-10-27T12:00:00Z"}
    })).unwrap();
    let cursor = strv(&first, "nextCursor");
    assert!(!cursor.is_empty());
    for completed in [
        json!({"selection":"COMPLETED_SELECTION_LAST_24_HOURS","cutoff":"2026-10-27T12:00:00.000001Z"}),
        json!({"selection":"COMPLETED_SELECTION_LAST_7_DAYS","cutoff":"2026-10-27T12:00:00Z"}),
        json!({"selection":"COMPLETED_SELECTION_ALL"}),
    ] {
        let error = call(
            &store,
            "ListTasks",
            &json!({"page":{"size":1,"cursor":cursor},"completed":completed}),
        )
        .unwrap_err();
        assert_eq!(error.reason, "ERROR_REASON_INVALID_INPUT");
    }
}

#[test]
fn a_full_raw_page_of_terminal_tasks_does_not_hide_later_active_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path(), "", false).unwrap();
    store.with(true, |c| {
        for number in 1..=121 {
            let entity = format!("019c0000-0000-7000-8001-{number:012x}");
            let version = format!("019c0000-0000-7000-8002-{number:012x}");
            let progress = if number == 121 { "PROGRESS_TODO" } else { "PROGRESS_DONE" };
            let body = json!({
                "formatVersion":3,"entityId":entity,"kind":"ENTITY_KIND_TASK",
                "versionId":version,"createdAt":"2026-10-28T12:00:00Z",
                "task":{"repositoryId":"019c0000-0000-7000-8000-00000000a001",
                        "title":"task","progress":progress,"proposal":"PROPOSAL_REGULAR"}
            });
            c.execute("INSERT INTO entities(id,kind,deleted) VALUES (?,2,0)", [&entity])?;
            c.execute(
                "INSERT INTO versions(version_id,entity_id,envelope_json,created_at_us) VALUES (?,?,?,1)",
                params![version, entity, body.to_string()],
            )?;
            c.execute("INSERT INTO heads VALUES (?,?)", params![entity, version])?;
        }
        Ok(())
    }).unwrap();
    let response = call(
        &store,
        "ListTasks",
        &json!({
            "page":{"size":1},"completed":{"selection":"COMPLETED_SELECTION_NONE"}
        }),
    )
    .unwrap();
    assert_eq!(response["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        strv(&response["items"][0]["version"], "entityId"),
        "019c0000-0000-7000-8001-000000000079"
    );
    assert_eq!(strv(&response, "nextCursor"), "");
}
