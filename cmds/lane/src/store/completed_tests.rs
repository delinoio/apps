// SPDX-License-Identifier: Apache-2.0
use super::*;
use rusqlite::{
    hooks::{AuthAction, AuthContext, Authorization},
    params,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../protos/lane/v1/fixtures/completed-list.json"
    ))
    .unwrap()
}

#[test]
fn completed_entry_vectors() {
    let suite = fixture();
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE versions (entity_id TEXT, version_id TEXT, envelope_json TEXT);")
        .unwrap();
    let versions: HashMap<String, Value> = suite["dataset"]["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|version| {
            c.execute(
                "INSERT INTO versions VALUES (?,?,?)",
                params![
                    strv(version, "entityId"),
                    strv(version, "versionId"),
                    version.to_string()
                ],
            )
            .unwrap();
            (strv(version, "versionId").to_owned(), version.clone())
        })
        .collect();
    for case in suite["entryCases"].as_array().unwrap() {
        let input = &case["input"];
        let selected = &versions[strv(input, "selectedVersionId")];
        let heads: Vec<Value> = input["headVersionIds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| versions[id.as_str().unwrap()].clone())
            .collect();
        let got = entry_for_heads(
            &c,
            strv(selected, "entityId"),
            &heads,
            strv(&selected["task"], "progress"),
        )
        .unwrap();
        if case["expected"]["kind"] == "TERMINAL_ENTRY_KIND_UNKNOWN" {
            assert!(got.is_none(), "{}", case["id"]);
        } else {
            let (_, time) = got.unwrap_or_else(|| panic!("{}", case["id"]));
            assert_eq!(
                time,
                timestamp(strv(&case["expected"], "enteredAt")).unwrap(),
                "{}",
                case["id"]
            );
        }
    }
}

#[test]
fn diamond_history_reuses_terminal_entries_with_bounded_queries() {
    let mut c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE versions (entity_id TEXT, version_id TEXT, envelope_json TEXT);")
        .unwrap();
    let entity = "entity";
    let progress = "PROGRESS_DONE";
    let version = |id: String, parents: Vec<String>, created_at: &str| {
        serde_json::json!({
            "entityId": entity,
            "versionId": id,
            "parentVersionIds": parents,
            "createdAt": created_at,
            "deleted": false,
            "task": {"progress": progress}
        })
    };
    let insert = |version: &Value| {
        c.execute(
            "INSERT INTO versions VALUES (?,?,?)",
            params![entity, strv(version, "versionId"), version.to_string()],
        )
        .unwrap();
    };
    insert(&version(
        "root".to_owned(),
        Vec::new(),
        "2026-01-01T00:00:00.000000Z",
    ));

    let diamonds = 20;
    let mut previous = vec!["root".to_owned()];
    for layer in 1..=diamonds {
        let parents = if layer == 1 {
            previous.clone()
        } else {
            vec![previous[0].clone(), previous[1].clone()]
        };
        let left = version(
            format!("left-{layer}"),
            parents.clone(),
            "2026-01-01T00:00:00.000000Z",
        );
        let right = version(
            format!("right-{layer}"),
            parents,
            "2026-01-01T00:00:00.000000Z",
        );
        insert(&left);
        insert(&right);
        previous = vec![
            strv(&left, "versionId").to_owned(),
            strv(&right, "versionId").to_owned(),
        ];
    }
    let head = version(
        "head".to_owned(),
        vec![previous[0].clone(), previous[1].clone()],
        "2026-01-01T00:00:00.000000Z",
    );

    let query_count = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&query_count);
    c.authorizer(Some(move |context: AuthContext<'_>| {
        if matches!(context.action, AuthAction::Select) {
            observed.fetch_add(1, Ordering::Relaxed);
        }
        Authorization::Allow
    }))
    .unwrap();
    let got = entry_for_heads(&c, entity, &[head], progress).unwrap();
    assert_eq!(
        got.map(|(_, time)| time),
        timestamp("2026-01-01T00:00:00.000000Z")
    );
    assert!(
        query_count.load(Ordering::Relaxed) <= diamonds * 4 + 4,
        "diamond traversal exceeded the bounded query budget: {}",
        query_count.load(Ordering::Relaxed)
    );
}
