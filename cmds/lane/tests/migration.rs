// SPDX-License-Identifier: Apache-2.0
use lane::{
    protocol::id,
    store::{Store, migrate},
};
use rusqlite::Connection;
use serde_json::json;

fn legacy(root: &std::path::Path) -> (String, String) {
    let store = Store::open(root, "", false).unwrap();
    let space = store.space.clone();
    let device = store.device.clone();
    drop(store);
    let path = root.join("spaces").join(&space).join("lane.sqlite");
    std::fs::remove_file(&path).unwrap();
    let db = Connection::open(path).unwrap();
    let sql = include_str!("../../../apps/lane/native/store/schema.sql")
        .split("CREATE TABLE attachments")
        .next()
        .unwrap()
        .replace("kind IN (1,2,3)", "kind IN (1,2)");
    db.execute_batch(&sql).unwrap();
    db.execute_batch("PRAGMA application_id=1279348293; PRAGMA user_version=1")
        .unwrap();
    for (key, value) in [
        ("space_id", space.as_str()),
        ("device_id", device.as_str()),
        ("format_version", "1"),
    ] {
        db.execute("INSERT INTO store_meta VALUES(?,?)", [key, value])
            .unwrap();
    }
    let entity = id();
    let version = id();
    let envelope = json!({"formatVersion":1,"entityId":entity,"versionId":version,"kind":"ENTITY_KIND_REPOSITORY","repository":{"normalizedUrl":"example.com/repo"}}).to_string();
    db.execute("INSERT INTO entities(id,kind) VALUES(?,1)", [&entity])
        .unwrap();
    db.execute(
        "INSERT INTO versions VALUES(?,?,?,1,0)",
        [&version, &entity, &envelope],
    )
    .unwrap();
    db.execute("INSERT INTO heads VALUES(?,?)", [&entity, &version])
        .unwrap();
    db.execute(
        "INSERT INTO receipts VALUES('CreateRepository','request','fingerprint',?,'result')",
        [&entity],
    )
    .unwrap();
    db.execute(
        "INSERT INTO outbox VALUES('request',?,'outbox',1,NULL,1)",
        [&entity],
    )
    .unwrap();
    (space, envelope)
}
#[test]
fn explicit_upgrade_preserves_history_receipts_and_outbox() {
    let root = tempfile::tempdir().unwrap();
    let (space, envelope) = legacy(root.path());
    assert!(Store::open(root.path(), &space, false).is_err());
    assert_eq!(migrate(root.path(), &space).unwrap()["migrated"], true);
    let db = Connection::open(root.path().join("spaces").join(&space).join("lane.sqlite")).unwrap();
    assert_eq!(
        db.query_row::<String, _, _>("SELECT envelope_json FROM versions", [], |r| r.get(0))
            .unwrap(),
        envelope
    );
    assert_eq!(
        db.query_row::<String, _, _>(
            "SELECT fingerprint || ':' || result_json FROM receipts",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        "fingerprint:result"
    );
    assert_eq!(
        db.query_row::<String, _, _>("SELECT request_json FROM outbox", [], |r| r.get(0))
            .unwrap(),
        "outbox"
    );
    db.execute("INSERT INTO entities(id,kind) VALUES(?,3)", [id()])
        .unwrap();
    assert_eq!(migrate(root.path(), &space).unwrap()["migrated"], false);
    assert!(Store::open(root.path(), &space, false).is_ok());
}
#[test]
fn unknown_and_corrupt_stores_are_not_partially_migrated() {
    let root = tempfile::tempdir().unwrap();
    let (space, _) = legacy(root.path());
    let db = Connection::open(root.path().join("spaces").join(&space).join("lane.sqlite")).unwrap();
    db.execute_batch("PRAGMA user_version=99").unwrap();
    assert!(migrate(root.path(), &space).is_err());
    db.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA user_version=1; INSERT INTO heads VALUES('missing','missing')")
        .unwrap();
    assert!(migrate(root.path(), &space).is_err());
    assert_eq!(
        db.query_row::<i64, _, _>("PRAGMA user_version", [], |r| r.get(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row::<i64, _, _>(
            "SELECT count(*) FROM sqlite_master WHERE name='attachments'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row::<String, _, _>(
            "SELECT value FROM store_meta WHERE key='format_version'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        "1"
    );
}
#[test]
fn open_clients_block_explicit_migration() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path(), "", false).unwrap();
    let error = migrate(root.path(), &store.space).unwrap_err();
    assert!(error.message.contains("busy"));
    assert!(migrate(root.path(), "").is_err());
}

#[test]
#[ignore = "requires LANE_NATIVE_STORE_DRIVER"]
fn native_and_rust_migrate_the_same_physical_schema() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    for native_migrates in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let (space, _) = legacy(root.path());
        if !native_migrates {
            migrate(root.path(), &space).unwrap();
        }
        let mut child = Command::new(std::env::var_os("LANE_NATIVE_STORE_DRIVER").unwrap())
            .arg(root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        {
            let mut input = child.stdin.take().unwrap();
            if native_migrates {
                writeln!(input, "{}", json!({"method":"migrate","spaceId":space})).unwrap();
            } else {
                writeln!(
                    input,
                    "{}",
                    json!({"method":"open","request":{"formatVersion":2,"spaceId":space}})
                )
                .unwrap();
            }
            writeln!(input, "{}", json!({"method":"exit"})).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let line = std::str::from_utf8(&output.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap();
        let result: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(result["ok"], true, "{result}");
        assert!(Store::open(root.path(), &space, false).is_ok());
    }
}
