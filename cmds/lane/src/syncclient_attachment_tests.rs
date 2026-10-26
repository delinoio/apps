// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::{network::Response, protocol};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::cell::{Cell, RefCell};

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
struct Fixture {
    _root: tempfile::TempDir,
    store: Store,
    identity: Identity,
    epoch: String,
    owner: String,
    attachments: Vec<String>,
}
impl Fixture {
    fn new(count: usize, downloaded: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let standalone = Store::open(root.path(), "", false).unwrap();
        let store = standalone
            .select_account("issuer", &id(), "tenant")
            .unwrap();
        store.set_enabled(true).unwrap();
        let identity = store.identity().unwrap();
        let call = |method: &str, q: Value| {
            let mut q = protocol::request(method, &q).unwrap();
            protocol::attribute(protocol::method(method).unwrap().input(), &mut q,
                &json!({"kind":"ORIGIN_KIND_CLI","deviceId":store.device,"deviceName":"fixture","accountId":identity.account}), &mut vec![]).unwrap();
            store.invoke(method, &q).unwrap()
        };
        let repository = call(
            "CreateRepository",
            json!({"url":"https://example.com/repo"}),
        )["result"]["version"]["entityId"]
            .clone();
        let attachments: Vec<_> = (0..count).map(|_| id()).collect();
        let png = STANDARD.decode(PNG).unwrap();
        let images: Vec<_> = attachments.iter().map(|attachment| json!({"capture":{"attachmentId":attachment,"width":1,"height":1,"byteSize":png.len().to_string(),"sha256":protocol::hash(&png)},"png":PNG})).collect();
        call(
            "CreateCaptureTask",
            json!({"repositoryId":repository,"title":"fixture","images":images}),
        );
        let epoch = id();
        store
            .with(true, |db| {
                put_meta(db, "account_epoch", &epoch)?;
                if downloaded {
                    db.execute("UPDATE attachments SET png_base64=NULL,synchronized=1", [])?;
                }
                Ok(())
            })
            .unwrap();
        Self {
            _root: root,
            store,
            identity,
            epoch,
            owner: id(),
            attachments,
        }
    }
    fn state(&self, attachment: &str) -> (Option<String>, bool) {
        self.store
            .with(false, |db| {
                Ok(db.query_row(
                    "SELECT png_base64,synchronized FROM attachments WHERE id=?",
                    [attachment],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })
            .unwrap()
    }
    fn run(&self, io: &AttachmentIo<'_>) -> Result<()> {
        attachments_with_io(&self.store, &self.owner, &self.identity, &self.epoch, io)
    }
}
fn access(field: &str, attachment: &str) -> Value {
    let mut response = json!({"state":"ATTACHMENT_STATE_AVAILABLE","expiresAt":(chrono::Utc::now()+chrono::Duration::minutes(5)).to_rfc3339()});
    response[field] = format!("https://objects.example.test/{attachment}?signature=private").into();
    response
}
fn response(status: u16, body: Vec<u8>) -> Response {
    Response {
        status: reqwest::StatusCode::from_u16(status).unwrap(),
        body,
    }
}
fn assert_private(request: &reqwest::Request) {
    assert_eq!(request.url().scheme(), "https");
    for header in ["authorization", "proxy-authorization", "cookie"] {
        assert!(!request.headers().contains_key(header));
    }
}
#[test]
fn put_headers_bytes_and_complete_failure_preserve_retry_content() {
    let f = Fixture::new(1, false);
    let client = object_client().unwrap();
    let completions = Cell::new(0);
    let uploads = RefCell::new(Vec::new());
    let rpc = |method: &str, q: &Value| {
        assert_eq!(q["epoch"], f.epoch);
        assert_eq!(q["attachmentId"], f.attachments[0]);
        if method == "BeginAttachmentUpload" {
            let mut begin = access("uploadUrl", &f.attachments[0]);
            begin["state"] = "ATTACHMENT_STATE_WAITING_FOR_SYNC".into();
            begin["uploadId"] = format!("attempt-{}", completions.get()).into();
            begin["headers"] = json!({"content-type":"image/png","x-amz-meta-checksum":"expected"});
            Ok(begin)
        } else {
            assert_eq!(method, "CompleteAttachmentUpload");
            assert_eq!(q["uploadId"], format!("attempt-{}", completions.get()));
            completions.set(completions.get() + 1);
            if completions.get() == 1 {
                Err(Failure::new(
                    "DEPENDENCY_UNAVAILABLE",
                    "Mock completion lost.",
                ))
            } else {
                Ok(json!({"state":"ATTACHMENT_STATE_AVAILABLE"}))
            }
        }
    };
    let object = |builder: reqwest::RequestBuilder, _| {
        let request = builder.build().unwrap();
        assert_private(&request);
        assert_eq!(request.method(), reqwest::Method::PUT);
        assert_eq!(request.headers()["content-type"], "image/png");
        assert_eq!(request.headers()["x-amz-meta-checksum"], "expected");
        uploads
            .borrow_mut()
            .push(request.body().unwrap().as_bytes().unwrap().to_vec());
        Ok(response(200, vec![]))
    };
    let io = AttachmentIo {
        client: &client,
        rpc: &rpc,
        object: &object,
    };
    assert!(f.run(&io).is_err());
    assert_eq!(f.state(&f.attachments[0]), (Some(PNG.into()), false));
    f.run(&io).unwrap();
    assert_eq!(f.state(&f.attachments[0]), (Some(PNG.into()), true));
    assert_eq!(*uploads.borrow(), vec![STANDARD.decode(PNG).unwrap(); 2]);
    assert_eq!(completions.get(), 2);
}
#[test]
fn rejected_or_disconnected_put_does_not_block_another_image() {
    for status in [None, Some(413), Some(503)] {
        let f = Fixture::new(2, false);
        let client = object_client().unwrap();
        let completed = RefCell::new(Vec::new());
        let rpc = |method: &str, q: &Value| {
            let attachment = strv(q, "attachmentId");
            if method == "BeginAttachmentUpload" {
                let mut begin = access("uploadUrl", attachment);
                begin["state"] = "ATTACHMENT_STATE_WAITING_FOR_SYNC".into();
                begin["uploadId"] = attachment.into();
                Ok(begin)
            } else {
                assert_eq!(method, "CompleteAttachmentUpload");
                completed.borrow_mut().push(attachment.to_owned());
                Ok(json!({"state":"ATTACHMENT_STATE_AVAILABLE"}))
            }
        };
        let object = |builder: reqwest::RequestBuilder, _| {
            let request = builder.build().unwrap();
            assert_private(&request);
            if request.url().path().ends_with(&f.attachments[0]) {
                status.map_or_else(
                    || {
                        Err(Failure::new(
                            "DEPENDENCY_UNAVAILABLE",
                            "Mock network disconnect.",
                        ))
                    },
                    |status| Ok(response(status, vec![])),
                )
            } else {
                Ok(response(200, vec![]))
            }
        };
        assert!(
            f.run(&AttachmentIo {
                client: &client,
                rpc: &rpc,
                object: &object
            })
            .is_err()
        );
        assert_eq!(f.state(&f.attachments[0]), (Some(PNG.into()), false));
        assert_eq!(f.state(&f.attachments[1]), (Some(PNG.into()), true));
        assert_eq!(*completed.borrow(), vec![f.attachments[1].clone()]);
    }
}
#[test]
fn signed_get_validates_exact_size_digest_and_status_before_cache() {
    let original = STANDARD.decode(PNG).unwrap();
    let mut bad_hash = original.clone();
    bad_hash[42] ^= 1;
    let mut oversized = original.clone();
    oversized.push(0);
    for (bytes, status, success) in [
        (bad_hash, 200, false),
        (original[..original.len() - 1].to_vec(), 200, false),
        (oversized, 200, false),
        (original.clone(), 503, false),
        (original.clone(), 200, true),
    ] {
        let f = Fixture::new(1, true);
        let client = object_client().unwrap();
        let rpc = |method: &str, _: &Value| {
            assert_eq!(method, "GetAttachment");
            Ok(access("downloadUrl", &f.attachments[0]))
        };
        let object = |builder: reqwest::RequestBuilder, limit| {
            let request = builder.build().unwrap();
            assert_private(&request);
            assert_eq!(request.method(), reqwest::Method::GET);
            assert!(request.body().is_none());
            assert_eq!(limit, original.len());
            // Deliberately bypass the reader limit in the mock: validation must
            // still reject a transport returning more than the manifest size.
            Ok(response(status, bytes.clone()))
        };
        assert_eq!(
            f.run(&AttachmentIo {
                client: &client,
                rpc: &rpc,
                object: &object
            })
            .is_ok(),
            success
        );
        assert_eq!(f.state(&f.attachments[0]).0, success.then(|| PNG.into()));
    }
}
#[test]
fn epoch_identity_disconnect_or_owner_deletion_during_get_cannot_cache() {
    for change in ["epoch", "account", "disconnect", "delete"] {
        let f = Fixture::new(1, true);
        let client = object_client().unwrap();
        let rpc = |_: &str, _: &Value| Ok(access("downloadUrl", &f.attachments[0]));
        let object = |_: reqwest::RequestBuilder, _| {
            match change {
                "epoch" => f
                    .store
                    .with(true, |db| put_meta(db, "account_epoch", &id()))
                    .unwrap(),
                "account" => {
                    f.store
                        .registry
                        .execute(
                            "UPDATE spaces SET account_id=? WHERE id=?",
                            params![id(), f.store.space],
                        )
                        .unwrap();
                }
                "disconnect" => f.store.set_enabled(false).unwrap(),
                _ => {
                    let task: String = f
                        .store
                        .with(false, |db| {
                            Ok(db.query_row(
                                "SELECT task_id FROM attachments LIMIT 1",
                                [],
                                |r| r.get(0),
                            )?)
                        })
                        .unwrap();
                    f.store.invoke("DeleteTask", &json!({"id":task,"meta":{"requestId":id(),"origin":{"kind":"ORIGIN_KIND_CLI","deviceId":f.store.device,"deviceName":"fixture","accountId":f.identity.account}}})).unwrap();
                }
            }
            Ok(response(200, STANDARD.decode(PNG).unwrap()))
        };
        let result = f.run(&AttachmentIo {
            client: &client,
            rpc: &rpc,
            object: &object,
        });
        if change == "delete" {
            assert!(result.is_ok());
            f.store
                .with(false, |db| {
                    assert_eq!(
                        db.query_row::<i64, _, _>("SELECT count(*) FROM attachments", [], |r| r
                            .get(0))?,
                        0
                    );
                    Ok(())
                })
                .unwrap();
        } else {
            assert!(result.is_err());
            assert_eq!(f.state(&f.attachments[0]).0, None);
        }
    }
}

#[test]
fn upload_rejects_auth_cookie_and_host_headers_before_object_request() {
    for header in ["Authorization", "Proxy-Authorization", "Cookie", "Host"] {
        let f = Fixture::new(1, false);
        let client = object_client().unwrap();
        let rpc = |method: &str, _: &Value| {
            assert_eq!(method, "BeginAttachmentUpload");
            let mut begin = access("uploadUrl", &f.attachments[0]);
            begin["state"] = "ATTACHMENT_STATE_WAITING_FOR_SYNC".into();
            begin["headers"] = json!({header:"must-not-send"});
            Ok(begin)
        };
        let object = |_: reqwest::RequestBuilder, _| -> Result<Response> {
            panic!("Forbidden object header reached transport")
        };
        assert!(
            f.run(&AttachmentIo {
                client: &client,
                rpc: &rpc,
                object: &object
            })
            .is_err()
        );
        assert_eq!(f.state(&f.attachments[0]), (Some(PNG.into()), false));
    }
}
