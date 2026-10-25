// SPDX-License-Identifier: Apache-2.0
use lane::{protocol, store::Store};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

fn legacy(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            if fields.get("captures").is_some_and(|v| v == &json!([])) {
                fields.remove("captures");
            }
            if fields.contains_key("formatVersion") {
                fields.insert("formatVersion".into(), 1.into());
            }
            for value in fields.values_mut() {
                legacy(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                legacy(value);
            }
        }
        _ => {}
    }
}
fn request(store: &Store, method: &str, value: &Value) -> Value {
    let mut value = protocol::request(method, value).unwrap();
    protocol::attribute(protocol::method(method).unwrap().input(),&mut value,&json!({"kind":"ORIGIN_KIND_CLI","deviceId":store.device,"deviceName":"Lane","accountId":""}),&mut vec![]).unwrap();
    value
}
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "The regression records the complete legacy fixture, explicit migration, replay and import in one audit trail."
)]
fn schema_one_save_receipt_replays_after_migration_and_protojson_import() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path(), "", false).unwrap();
    let space = store.space.clone();
    let created = store
        .invoke(
            "CreateTask",
            &request(
                &store,
                "CreateTask",
                &json!({"repository":{"url":"https://example.com/legacy"},"title":"Before"}),
            ),
        )
        .unwrap();
    let before = &created["result"]["version"];
    let mut task = before["task"].clone();
    task["title"] = "After".into();
    let mut saved_request = request(
        &store,
        "SaveTask",
        &json!({"id":before["entityId"],"expectedVersionId":before["versionId"],"task":task}),
    );
    let saved = store.invoke("SaveTask", &saved_request).unwrap();
    legacy(&mut saved_request);
    // This is the schema-1 algorithm: RFC 8785 hash of the request with no
    // captures field, independent of the new compatibility fingerprint helper.
    let old_fingerprint = protocol::hash(protocol::canonical(&saved_request));
    let mut old_result = saved["result"].clone();
    legacy(&mut old_result);
    store
        .with(true, |db| {
            for (table, id_column, json_column) in [
                ("versions", "version_id", "envelope_json"),
                ("outbox", "request_id", "request_json"),
            ] {
                let mut stmt =
                    db.prepare(&format!("SELECT {id_column},{json_column} FROM {table}"))?;
                let records = stmt
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                for (id, raw) in records {
                    let mut value: Value = serde_json::from_str(&raw)?;
                    legacy(&mut value);
                    db.execute(
                        &format!("UPDATE {table} SET {json_column}=? WHERE {id_column}=?"),
                        params![value.to_string(), id],
                    )?;
                }
            }
            db.execute(
                "UPDATE receipts SET fingerprint=?,result_json=? WHERE method='SaveTask'",
                params![old_fingerprint, old_result.to_string()],
            )?;
            db.execute(
                "UPDATE store_meta SET value='1' WHERE key='format_version'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    drop(store);
    let path = root.path().join("spaces").join(&space).join("lane.sqlite");
    let previous = root.path().join("previous.sqlite");
    std::fs::rename(&path, &previous).unwrap();
    let db = Connection::open(&path).unwrap();
    let schema = include_str!("../../../apps/lane/native/store/schema.sql")
        .split("CREATE TABLE attachments")
        .next()
        .unwrap()
        .replace("kind IN (1,2,3)", "kind IN (1,2)");
    db.execute_batch(&schema).unwrap();
    db.execute(
        "ATTACH DATABASE ? AS previous",
        [previous.to_str().unwrap()],
    )
    .unwrap();
    for table in [
        "store_meta",
        "entities",
        "versions",
        "heads",
        "receipts",
        "outbox",
    ] {
        db.execute(
            &format!("INSERT INTO {table} SELECT * FROM previous.{table}"),
            [],
        )
        .unwrap();
    }
    db.execute_batch(
        "DETACH DATABASE previous; PRAGMA application_id=1279348293; PRAGMA user_version=1;",
    )
    .unwrap();
    let raw_before: String = db
        .query_row(
            "SELECT envelope_json FROM versions WHERE version_id=?",
            [saved["result"]["version"]["versionId"].as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    drop(db);
    lane::store::migrate(root.path(), &space).unwrap();
    let store = Store::open(root.path(), &space, false).unwrap();
    let wire = protocol::request("SaveTask", &saved_request).unwrap();
    assert_eq!(wire["task"]["captures"], json!([]));
    assert_eq!(protocol::fingerprint(&wire), old_fingerprint);
    assert_eq!(
        store.invoke("SaveTask", &wire).unwrap()["result"]["version"]["versionId"],
        saved["result"]["version"]["versionId"]
    );
    store
        .with(false, |db| {
            assert_eq!(
                db.query_row::<String, _, _>(
                    "SELECT fingerprint FROM receipts WHERE method='SaveTask'",
                    [],
                    |r| r.get(0)
                )?,
                old_fingerprint
            );
            assert_eq!(
                db.query_row::<String, _, _>(
                    "SELECT envelope_json FROM versions WHERE version_id=?",
                    [saved["result"]["version"]["versionId"].as_str().unwrap()],
                    |r| r.get(0)
                )?,
                raw_before
            );
            Ok(())
        })
        .unwrap();
    let dest = store
        .select_account("issuer", &protocol::id(), "tenant")
        .unwrap();
    lane::syncclient::import(&store, &dest).unwrap();
    dest.with(true, |db| {
        let raws = lane::store::rows(
            db,
            "SELECT request_json FROM outbox ORDER BY created_at_us,rowid",
            [],
        )?;
        for raw in raws {
            let wire = protocol::message("SyncMutation", &serde_json::from_str(&raw)?)?;
            lane::store::sync::apply(db, &json!({"version":wire["version"]}))?;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(
        dest.invoke("GetTask", &json!({"id":before["entityId"]}))
            .unwrap()["result"]["version"]["task"]["title"],
        "After"
    );
}
