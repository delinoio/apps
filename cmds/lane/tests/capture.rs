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
        &json!({"kind":"ORIGIN_KIND_APP","deviceId":s.device,"deviceName":"Lane","accountId":""}),
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
    assert!(
        call(&s, "ListURLConnections", json!({})).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
