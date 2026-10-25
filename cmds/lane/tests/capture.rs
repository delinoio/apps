// SPDX-License-Identifier: Apache-2.0
use base64::{Engine, engine::general_purpose::STANDARD};
use lane::{
    protocol::{self, id},
    store::Store,
};
use serde_json::{Value, json};
fn call(s: &Store, method: &str, mut q: Value) -> lane::error::Result<Value> {
    q = protocol::request(method, &q)?;
    protocol::attribute(
        protocol::method(method).unwrap().input(),
        &mut q,
        &json!({"kind":"ORIGIN_KIND_APP","deviceId":s.device,"deviceName":"Lane","accountId":s.identity()?.account}),
        &mut vec![],
    )?;
    s.invoke(method, &q)
}
fn png() -> Vec<u8> {
    STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==").unwrap()
}
#[test]
fn atomic_capture_replay_history_move_and_deletion() {
    let root = tempfile::tempdir().unwrap();
    let s = Store::open(root.path(), "", false).unwrap();
    let repository = id();
    let destination = id();
    for (rid, url) in [
        (&repository, "https://example.com/one"),
        (&destination, "https://example.com/two"),
    ] {
        call(&s, "CreateRepository", json!({"id":rid,"url":url})).unwrap();
    }
    let image = png();
    let attachment = id();
    let task = id();
    let q = json!({"meta":{"requestId":id()},"id":task,"repositoryId":repository,"title":"Fix the button","description":"The label is clipped.","images":[{"capture":{"attachmentId":attachment,"width":1,"height":1,"sha256":protocol::hash(&image),"byteSize":image.len().to_string(),"pageTitle":"Example"},"png":STANDARD.encode(&image)}]});
    let first = call(&s, "CreateCaptureTask", q.clone()).unwrap();
    let replay = call(&s, "CreateCaptureTask", q.clone()).unwrap();
    assert_eq!(
        first["result"]["version"]["versionId"],
        replay["result"]["version"]["versionId"]
    );
    let read = call(
        &s,
        "GetAttachment",
        json!({"taskId":task,"attachmentId":attachment}),
    )
    .unwrap();
    assert_eq!(read["png"], STANDARD.encode(&image));
    let mut edited = first["result"]["version"]["task"].clone();
    edited["captures"] = json!([]);
    edited["repositoryId"] = destination.into();
    call(&s,"SaveTask",json!({"id":task,"expectedVersionId":first["result"]["version"]["versionId"],"task":edited})).unwrap();
    call(&s, "DeleteRepository", json!({"id":repository})).unwrap();
    assert!(
        call(
            &s,
            "GetAttachment",
            json!({"taskId":task,"attachmentId":attachment})
        )
        .is_ok()
    );
    call(&s, "DeleteTask", json!({"id":task})).unwrap();
    assert_eq!(
        call(&s, "CreateCaptureTask", q).unwrap()["result"]["result"],
        "RESULT_KIND_DELETED"
    );
    assert_eq!(
        call(
            &s,
            "GetAttachment",
            json!({"taskId":task,"attachmentId":attachment})
        )
        .unwrap()["state"],
        "ATTACHMENT_STATE_DELETED"
    );
}
#[test]
fn invalid_image_rolls_back_the_entire_task() {
    let root = tempfile::tempdir().unwrap();
    let s = Store::open(root.path(), "", false).unwrap();
    let repository = id();
    let task = id();
    call(
        &s,
        "CreateRepository",
        json!({"id":repository,"url":"https://example.com/one"}),
    )
    .unwrap();
    let q = json!({"id":task,"repositoryId":repository,"title":"Test","images":[{"capture":{"attachmentId":id(),"width":1,"height":1,"byteSize":"3","sha256":"bad"},"png":STANDARD.encode(b"bad")}]});
    assert!(call(&s, "CreateCaptureTask", q).is_err());
    assert!(call(&s, "GetTask", json!({"id":task})).is_err());
}
#[test]
fn rules_normalize_deduplicate_and_cascade() {
    let root = tempfile::tempdir().unwrap();
    let s = Store::open(root.path(), "", false).unwrap();
    let repository = id();
    call(
        &s,
        "CreateRepository",
        json!({"id":repository,"url":"https://example.com/one"}),
    )
    .unwrap();
    let q = json!({"id":id(),"connection":{"prefix":"https://EXAMPLE.COM:443/app?x=1","repositoryId":repository}});
    let saved = call(&s, "CreateURLConnection", q).unwrap();
    assert_eq!(
        saved["result"]["version"]["urlConnection"]["prefix"],
        "https://example.com/app"
    );
    assert!(call(&s,"CreateURLConnection",json!({"id":id(),"connection":{"prefix":"https://example.com/app#x","repositoryId":repository}})).is_err());
    assert_eq!(
        call(&s, "ListURLConnections", json!({})).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    call(&s, "DeleteRepository", json!({"id":repository})).unwrap();
    assert_eq!(
        call(&s, "ListURLConnections", json!({})).unwrap()["items"],
        json!([])
    );
}

fn saved_capture(s: &Store) -> Value {
    let repository = call(
        s,
        "CreateRepository",
        json!({"url":format!("https://example.com/{}",id())}),
    )
    .unwrap()["result"]["version"]["entityId"]
        .clone();
    let image = png();
    call(s,"CreateCaptureTask",json!({"repositoryId":repository,"title":"Capture","images":[{"capture":{"attachmentId":id(),"width":1,"height":1,"sha256":protocol::hash(&image),"byteSize":image.len().to_string()},"png":STANDARD.encode(image)}]})).unwrap()["result"]["version"].clone()
}

#[test]
fn import_copies_history_attachments_and_rules_without_account_relabeling() {
    let root = tempfile::tempdir().unwrap();
    let source = Store::open(root.path(), "", false).unwrap();
    let version = saved_capture(&source);
    let task = version["entityId"].clone();
    let attachment = version["task"]["captures"][0]["attachmentId"].clone();
    let rule = call(&source,"CreateURLConnection",json!({"connection":{"prefix":"https://example.com/app","repositoryId":version["task"]["repositoryId"]}})).unwrap();
    let mut edited = version["task"].clone();
    edited["captures"] = json!([]);
    call(
        &source,
        "SaveTask",
        json!({"id":task,"expectedVersionId":version["versionId"],"task":edited}),
    )
    .unwrap();
    let destination = source.select_account("issuer", &id(), "tenant").unwrap();
    for _ in 0..2 {
        lane::syncclient::import(&source, &destination).unwrap();
        assert_eq!(
            call(
                &destination,
                "GetAttachment",
                json!({"taskId":task,"attachmentId":attachment})
            )
            .unwrap()["png"],
            STANDARD.encode(png())
        );
        assert_eq!(
            call(&destination, "ListURLConnections", json!({})).unwrap()["items"][0]["version"],
            rule["result"]["version"]
        );
    }
    let other = source.select_account("issuer", &id(), "tenant").unwrap();
    assert!(lane::syncclient::import(&destination, &other).is_err());
    assert!(
        call(
            &other,
            "GetAttachment",
            json!({"taskId":task,"attachmentId":attachment})
        )
        .is_err()
    );
    call(&destination, "DeleteTask", json!({"id":task})).unwrap();
    assert_eq!(
        call(
            &source,
            "GetAttachment",
            json!({"taskId":task,"attachmentId":attachment})
        )
        .unwrap()["state"],
        "ATTACHMENT_STATE_AVAILABLE"
    );
}

#[test]
fn synchronized_manifests_reject_content_changes_and_cross_task_ownership() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path(), "", false).unwrap();
    let version = saved_capture(&store);
    for field in ["sha256", "width", "byteSize", "entityId"] {
        let mut next = version.clone();
        next["versionId"] = id().into();
        next["parentVersionIds"] = json!([version["versionId"]]);
        if field == "entityId" {
            next[field] = id().into();
            next["parentVersionIds"] = json!([]);
        } else {
            next["task"]["captures"][0][field] = match field {
                "sha256" => "0".repeat(64).into(),
                "width" => 2.into(),
                _ => "999".into(),
            };
        }
        assert!(
            store
                .with(true, |c| lane::store::sync::apply(
                    c,
                    &json!({"version":next})
                ))
                .is_err()
        );
    }
    let mut legacy = version.clone();
    legacy["formatVersion"] = 1.into();
    assert!(lane::store::sync::validate(&legacy).is_err());
    let request = json!({"taskId":version["entityId"],"attachmentId":version["task"]["captures"][0]["attachmentId"]});
    assert_eq!(
        call(&store, "GetAttachment", request).unwrap()["png"],
        STANDARD.encode(png())
    );
}

#[test]
fn account_deletion_erases_image_bytes_and_rules() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path(), "", false).unwrap();
    let version = saved_capture(&store);
    call(&store,"CreateURLConnection",json!({"connection":{"prefix":"https://example.com/","repositoryId":version["task"]["repositoryId"]}})).unwrap();
    let account = id();
    store.with(true,|c| lane::store::sync::change(c,&json!({"deletion":{"kind":"ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT","id":account,"deletionVersionId":id()}}),&account)).unwrap();
    store
        .with(false, |c| {
            for table in ["attachments", "entities", "versions", "outbox", "receipts"] {
                let count: i64 =
                    c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
                assert_eq!(count, 0);
            }
            assert_eq!(
                c.query_row::<i64, _, _>("SELECT count(*) FROM attachment_identities", [], |r| r
                    .get(0))?,
                1
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn pending_manifests_obey_png_and_sqlite_integer_bounds() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path(), "", false).unwrap();
    let version = saved_capture(&store);
    for (field, value) in [
        ("width", json!(2_147_483_648_u64)),
        ("height", json!(u32::MAX)),
        ("byteSize", json!("9223372036854775808")),
        ("width", json!(0)),
    ] {
        let mut invalid = version.clone();
        invalid["task"]["captures"][0][field] = value;
        assert!(lane::store::sync::validate(&invalid).is_err());
    }
    let mut boundary = version;
    boundary["task"]["captures"][0]["width"] = json!(i32::MAX);
    boundary["task"]["captures"][0]["height"] = json!(i32::MAX);
    boundary["task"]["captures"][0]["byteSize"] = json!(i64::MAX.to_string());
    lane::store::sync::validate(&boundary).unwrap();
}

#[test]
fn deleted_attachment_identity_cannot_be_reassigned_locally_or_by_sync() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path(), "", false).unwrap();
    let version = saved_capture(&store);
    let capture = version["task"]["captures"][0].clone();
    call(&store, "DeleteTask", json!({"id":version["entityId"]})).unwrap();
    let next_task = id();
    let counts = || {
        store
            .with(false, |db| {
                [
                    "entities",
                    "versions",
                    "receipts",
                    "outbox",
                    "attachments",
                    "attachment_identities",
                ]
                .into_iter()
                .map(|table| {
                    Ok(db.query_row::<i64, _, _>(
                        &format!("SELECT count(*) FROM {table}"),
                        [],
                        |r| r.get(0),
                    )?)
                })
                .collect::<lane::error::Result<Vec<_>>>()
            })
            .unwrap()
    };
    let before = counts();
    assert_eq!(*before.last().unwrap(), 1);
    let mut fresh_capture = capture.clone();
    fresh_capture["attachmentId"] = id().into();
    let attempt = json!({"id":next_task,"repositoryId":version["task"]["repositoryId"],"title":"Must fail","images":[{"capture":fresh_capture,"png":STANDARD.encode(png())},{"capture":capture,"png":STANDARD.encode(png())}]});
    assert!(call(&store, "CreateCaptureTask", attempt).is_err());
    assert_eq!(counts(), before);
    assert!(call(&store, "GetTask", json!({"id":next_task})).is_err());
    let mut incoming = version;
    incoming["entityId"] = next_task.into();
    incoming["versionId"] = id().into();
    assert!(
        store
            .with(true, |db| lane::store::sync::apply(
                db,
                &json!({"version":incoming})
            ))
            .is_err()
    );
    assert_eq!(counts(), before);
}

#[test]
fn standalone_import_preserves_identity_without_owner_or_content() {
    let root = tempfile::tempdir().unwrap();
    let source = Store::open(root.path(), "", false).unwrap();
    let version = saved_capture(&source);
    let capture = version["task"]["captures"][0].clone();
    call(&source, "DeleteTask", json!({"id":version["entityId"]})).unwrap();
    source.with(true,|db| {
        // Model a retained identity ledger after all content-bearing records
        // have been removed. Identity import cannot depend on version scans.
        db.execute_batch("DELETE FROM heads; DELETE FROM versions; DELETE FROM receipts; DELETE FROM outbox; DELETE FROM entities;")?;
        Ok(())
    }).unwrap();
    let dest = source.select_account("issuer", &id(), "tenant").unwrap();
    lane::syncclient::import(&source, &dest).unwrap();
    dest.with(false, |db| {
        assert_eq!(
            db.query_row::<i64, _, _>("SELECT count(*) FROM attachment_identities", [], |r| r
                .get(0))?,
            1
        );
        assert_eq!(
            db.query_row::<i64, _, _>("SELECT count(*) FROM attachments", [], |r| r.get(0))?,
            0
        );
        assert_eq!(
            db.query_row::<i64, _, _>("SELECT count(*) FROM entities", [], |r| r.get(0))?,
            0
        );
        Ok(())
    })
    .unwrap();
    let repo = call(
        &dest,
        "CreateRepository",
        json!({"url":"https://example.com/imported"}),
    )
    .unwrap()["result"]["version"]["entityId"]
        .clone();
    assert!(call(&dest,"CreateCaptureTask",json!({"repositoryId":repo,"title":"Must fail","images":[{"capture":capture,"png":STANDARD.encode(png())}]})).is_err());
}

#[test]
fn import_rejects_conflicting_retained_attachment_identity_atomically() {
    let root = tempfile::tempdir().unwrap();
    let source = Store::open(root.path(), "", false).unwrap();
    let original = saved_capture(&source);
    call(&source, "DeleteTask", json!({"id":original["entityId"]})).unwrap();
    let dest = source.select_account("issuer", &id(), "tenant").unwrap();
    let repo = call(
        &dest,
        "CreateRepository",
        json!({"url":"https://example.com/destination"}),
    )
    .unwrap()["result"]["version"]["entityId"]
        .clone();
    // Independent spaces can have the same UUID before an explicit import.
    let existing = call(&dest,"CreateCaptureTask",json!({"repositoryId":repo,"title":"Existing","images":[{"capture":original["task"]["captures"][0],"png":STANDARD.encode(png())}]})).unwrap();
    assert!(lane::syncclient::import(&source, &dest).is_err());
    assert_eq!(
        call(&dest, "ListRepositories", json!({})).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(call(&dest,"GetAttachment",json!({"taskId":existing["result"]["version"]["entityId"],"attachmentId":original["task"]["captures"][0]["attachmentId"]})).unwrap()["png"],STANDARD.encode(png()));
    dest.with(false, |db| {
        assert_eq!(
            lane::store::meta(db, &format!("standalone_import_source_{}", source.space))?,
            ""
        );
        Ok(())
    })
    .unwrap();
}
