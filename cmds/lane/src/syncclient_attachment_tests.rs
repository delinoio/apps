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
                // Model the accepted task push; uploads wait for an unsent creation.
                db.execute("DELETE FROM outbox", [])?;
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
    fn state(&self, attachment: &str) -> (Option<String>, i64) {
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
    assert_eq!(f.state(&f.attachments[0]), (Some(PNG.into()), 0));
    f.run(&io).unwrap();
    assert_eq!(f.state(&f.attachments[0]), (Some(PNG.into()), 1));
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
        let result = f.run(&AttachmentIo {
            client: &client,
            rpc: &rpc,
            object: &object,
        });
        // A payload-size refusal is permanent and blocks only that image; transport
        // failures remain retryable cycle failures with the local bytes preserved.
        let blocked = status == Some(413);
        assert_eq!(result.is_ok(), blocked);
        assert_eq!(
            f.state(&f.attachments[0]),
            (
                Some(PNG.into()),
                if blocked {
                    ATTACHMENT_BLOCKED_TOO_LARGE
                } else {
                    ATTACHMENT_PENDING
                }
            )
        );
        assert_eq!(f.state(&f.attachments[1]), (Some(PNG.into()), 1));
        assert_eq!(*completed.borrow(), vec![f.attachments[1].clone()]);
    }
}
fn status(f: &Fixture) -> Value {
    f.store.invoke("GetLocalStatus", &json!({})).unwrap()
}
#[test]
fn oversized_upload_blocks_only_that_item_and_is_not_retried_implicitly() {
    let f = Fixture::new(2, false);
    let client = object_client().unwrap();
    let begins = RefCell::new(Vec::new());
    let rpc = |method: &str, q: &Value| {
        let attachment = strv(q, "attachmentId").to_owned();
        if method == "BeginAttachmentUpload" {
            begins.borrow_mut().push(attachment.clone());
            if attachment == f.attachments[0] {
                return Err(Failure::new("ITEM_TOO_LARGE", "Mock byte limit."));
            }
            let mut begin = access("uploadUrl", &attachment);
            begin["state"] = "ATTACHMENT_STATE_WAITING_FOR_SYNC".into();
            begin["uploadId"] = attachment.into();
            Ok(begin)
        } else {
            Ok(json!({"state":"ATTACHMENT_STATE_AVAILABLE"}))
        }
    };
    let object = |_: reqwest::RequestBuilder, _| Ok(response(200, vec![]));
    let io = AttachmentIo {
        client: &client,
        rpc: &rpc,
        object: &object,
    };
    f.run(&io).unwrap();
    assert_eq!(
        f.state(&f.attachments[0]),
        (Some(PNG.into()), ATTACHMENT_BLOCKED_TOO_LARGE)
    );
    assert_eq!(f.state(&f.attachments[1]), (Some(PNG.into()), 1));
    let reported = status(&f);
    assert_eq!(reported["pendingItemCount"], "1");
    assert_eq!(reported["itemErrors"].as_array().unwrap().len(), 1);
    assert_eq!(
        reported["itemErrors"][0]["attachmentId"],
        f.attachments[0].as_str()
    );
    assert_eq!(
        reported["itemErrors"][0]["reason"],
        "ERROR_REASON_ITEM_TOO_LARGE"
    );
    // Locally saved content stays readable while synchronization is blocked.
    let task: String = f
        .store
        .with(false, |db| {
            Ok(db.query_row(
                "SELECT task_id FROM attachments WHERE id=?",
                [&f.attachments[0]],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(
        f.store
            .invoke(
                "GetAttachment",
                &json!({"taskId":task,"attachmentId":f.attachments[0]})
            )
            .unwrap()["png"],
        PNG
    );
    f.run(&io).unwrap();
    assert_eq!(
        *begins.borrow(),
        vec![f.attachments[0].clone(), f.attachments[1].clone()]
    );
}
#[test]
fn invalid_local_content_is_blocked_without_network_access() {
    let f = Fixture::new(1, false);
    f.store
        .with(true, |db| {
            db.execute(
                "UPDATE attachments SET png_base64=? WHERE id=?",
                params![STANDARD.encode(b"not a png"), f.attachments[0]],
            )?;
            Ok(())
        })
        .unwrap();
    let client = object_client().unwrap();
    let rpc =
        |_: &str, _: &Value| -> Result<Value> { panic!("Invalid content reached the server") };
    let object = |_: reqwest::RequestBuilder, _| -> Result<Response> {
        panic!("Invalid content reached object storage")
    };
    f.run(&AttachmentIo {
        client: &client,
        rpc: &rpc,
        object: &object,
    })
    .unwrap();
    assert_eq!(f.state(&f.attachments[0]).1, ATTACHMENT_BLOCKED_INVALID);
    assert_eq!(
        status(&f)["itemErrors"][0]["reason"],
        "ERROR_REASON_INVALID_INPUT"
    );
}
#[test]
fn content_waiting_on_another_device_or_task_push_is_pending_not_failed() {
    // Download: another device has not uploaded the bytes yet.
    for answer in ["waiting", "not-found"] {
        let f = Fixture::new(1, true);
        let client = object_client().unwrap();
        let rpc = |method: &str, _: &Value| {
            assert_eq!(method, "GetAttachment");
            if answer == "waiting" {
                Ok(json!({"state":"ATTACHMENT_STATE_WAITING_FOR_SYNC"}))
            } else {
                Err(Failure::new("NOT_FOUND", "Mock unknown task."))
            }
        };
        let object = |_: reqwest::RequestBuilder, _| -> Result<Response> {
            panic!("Waiting content has no object access")
        };
        f.run(&AttachmentIo {
            client: &client,
            rpc: &rpc,
            object: &object,
        })
        .unwrap();
        assert_eq!(f.state(&f.attachments[0]), (None, 1));
        let reported = status(&f);
        assert_eq!(reported["itemErrors"], json!([]));
        assert_eq!(reported["pendingItemCount"], "1");
    }
    // Upload: the owning task's creation has not been accepted by the server.
    let f = Fixture::new(1, false);
    let task: String = f
        .store
        .with(false, |db| {
            Ok(db.query_row("SELECT task_id FROM attachments", [], |r| r.get(0))?)
        })
        .unwrap();
    let creation = json!({"meta":{"requestId":id()},"kind":"MUTATION_KIND_CREATE","version":{"entityId":task}});
    f.store
        .with(true, |db| {
            db.execute(
                "INSERT INTO outbox(request_id,entity_id,request_json,state,created_at_us) VALUES(?,?,?,2,0)",
                params![id(), task, creation.to_string()],
            )?;
            Ok(())
        })
        .unwrap();
    let client = object_client().unwrap();
    let rpc = |_: &str, _: &Value| -> Result<Value> { panic!("Upload ran before its task") };
    let object = |_: reqwest::RequestBuilder, _| -> Result<Response> {
        panic!("Upload ran before its task")
    };
    f.run(&AttachmentIo {
        client: &client,
        rpc: &rpc,
        object: &object,
    })
    .unwrap();
    assert_eq!(f.state(&f.attachments[0]), (Some(PNG.into()), 0));
}
#[test]
fn pulled_attachment_in_the_default_state_is_downloaded() {
    for answer in ["waiting", "available"] {
        let f = Fixture::new(1, false);
        // Model the pull path: `store::sync::insert_version` inserts the manifest
        // without bytes or a synchronization state.
        f.store
            .with(true, |db| {
                let row: (String, String, i64, i64, i64) = db.query_row(
                    "SELECT task_id,sha256,width,height,byte_size FROM attachments",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )?;
                db.execute("DELETE FROM attachments", [])?;
                db.execute(
                    "INSERT INTO attachments(id,task_id,sha256,width,height,byte_size) VALUES(?,?,?,?,?,?)",
                    params![f.attachments[0], row.0, row.1, row.2, row.3, row.4],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(f.state(&f.attachments[0]), (None, 0));
        let client = object_client().unwrap();
        let rpc = |method: &str, _: &Value| {
            assert_eq!(method, "GetAttachment");
            if answer == "waiting" {
                Ok(json!({"state":"ATTACHMENT_STATE_WAITING_FOR_SYNC"}))
            } else {
                Ok(access("downloadUrl", &f.attachments[0]))
            }
        };
        let object = |builder: reqwest::RequestBuilder, _| {
            assert_ne!(answer, "waiting", "Waiting content has no object access");
            let request = builder.build().unwrap();
            assert_eq!(request.method(), reqwest::Method::GET);
            Ok(response(200, STANDARD.decode(PNG).unwrap()))
        };
        f.run(&AttachmentIo {
            client: &client,
            rpc: &rpc,
            object: &object,
        })
        .unwrap();
        if answer == "waiting" {
            assert_eq!(f.state(&f.attachments[0]), (None, 0));
            assert_eq!(status(&f)["pendingItemCount"], "1");
        } else {
            assert_eq!(f.state(&f.attachments[0]), (Some(PNG.into()), 1));
        }
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
        assert_eq!(f.state(&f.attachments[0]), (Some(PNG.into()), 0));
    }
}
