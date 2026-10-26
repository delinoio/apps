// SPDX-License-Identifier: Apache-2.0
#![expect(
    clippy::needless_pass_by_value,
    reason = "Test requests are consumed once at the driver boundary."
)]
use base64::{Engine, engine::general_purpose::STANDARD};
use lane::{protocol, store::Store};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

struct Native {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Native {
    fn new(root: &std::path::Path, space: &str) -> Self {
        let mut child =
            Command::new(std::env::var_os("LANE_NATIVE_STORE_DRIVER").expect("set native driver"))
                .arg(root)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut native = Self {
            child,
            input,
            output,
        };
        native.call(json!({"method":"open","request":{"formatVersion":2,"spaceId":space}}));
        native
    }
    fn call(&mut self, request: Value) -> Value {
        writeln!(self.input, "{request}").unwrap();
        self.input.flush().unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["ok"], true, "{response}");
        response["response"].clone()
    }
    fn invoke(&mut self, space: &str, method: &str, q: Value) -> Value {
        self.call(json!({"method":"invoke","operation":method,"spaceId":space,"request":q}))
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        let _ = writeln!(self.input, "{{\"method\":\"exit\"}}");
        let _ = self.input.flush();
        let _ = self.child.wait();
    }
}
fn request(store: &Store, method: &str, q: Value) -> Value {
    let mut q = protocol::request(method, &q).unwrap();
    protocol::attribute(protocol::method(method).unwrap().input(),&mut q,
        &json!({"kind":"ORIGIN_KIND_APP","deviceId":store.device,"deviceName":"Lane","accountId":""}),&mut vec![]).unwrap();
    q
}
#[test]
#[ignore = "requires LANE_NATIVE_STORE_DRIVER"]
fn capture_receipts_and_history_interoperate_in_both_writer_directions() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path(), "", false).unwrap();
    let mut native = Native::new(root.path(), &store.space);
    let repo = protocol::id();
    let q = request(
        &store,
        "CreateRepository",
        json!({"id":repo,"url":"https://example.com/repo"}),
    );
    store.invoke("CreateRepository", &q).unwrap();
    let png=STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==").unwrap();
    for native_first in [true, false] {
        let task = protocol::id();
        let attachment = protocol::id();
        let q = request(
            &store,
            "CreateCaptureTask",
            json!({"id":task,"repositoryId":repo,"title":"Capture","description":"Problem","images":[{"capture":{"attachmentId":attachment,"width":1,"height":1,"byteSize":png.len().to_string(),"sha256":protocol::hash(&png),"pageTitle":"Metadata"},"png":STANDARD.encode(&png)}]}),
        );
        let (first, replay) = if native_first {
            (
                native.invoke(&store.space, "CreateCaptureTask", q.clone()),
                store.invoke("CreateCaptureTask", &q).unwrap(),
            )
        } else {
            (
                store.invoke("CreateCaptureTask", &q).unwrap(),
                native.invoke(&store.space, "CreateCaptureTask", q),
            )
        };
        assert_eq!(
            first["result"]["version"]["versionId"],
            replay["result"]["version"]["versionId"]
        );
        assert_eq!(replay["result"]["result"], "RESULT_KIND_REPLAYED");
        let original = &first["result"]["version"];
        let mut body = original["task"].clone();
        body["captures"] = json!([]);
        let q = request(
            &store,
            "SaveTask",
            json!({"id":task,"expectedVersionId":original["versionId"],"task":body}),
        );
        let saved = store.invoke("SaveTask", &q).unwrap();
        let q = request(
            &store,
            "RestorePrompt",
            json!({"id":task,"expectedVersionId":saved["result"]["version"]["versionId"],"sourceVersionId":original["versionId"]}),
        );
        native.invoke(&store.space, "RestorePrompt", q);
        let task_read = store.invoke("GetTask", &json!({"id":task})).unwrap();
        assert_eq!(
            task_read["result"]["version"]["task"]["captures"],
            original["task"]["captures"]
        );
        let bytes = native.invoke(
            &store.space,
            "GetAttachment",
            json!({"taskId":task,"attachmentId":attachment}),
        );
        assert_eq!(bytes["png"], STANDARD.encode(&png));
        native.invoke(
            &store.space,
            "DeleteTask",
            request(&store, "DeleteTask", json!({"id":task})),
        );
        assert_eq!(
            store
                .invoke(
                    "GetAttachment",
                    &json!({"taskId":task,"attachmentId":attachment})
                )
                .unwrap()["state"],
            "ATTACHMENT_STATE_DELETED"
        );
    }
    let q = request(
        &store,
        "CreateURLConnection",
        json!({"id":protocol::id(),"connection":{"prefix":"https://EXAMPLE.com:443/app/?q=1","repositoryId":repo}}),
    );
    let first = native.invoke(&store.space, "CreateURLConnection", q.clone());
    let replay = store.invoke("CreateURLConnection", &q).unwrap();
    assert_eq!(
        first["result"]["version"]["versionId"],
        replay["result"]["version"]["versionId"]
    );
    assert_eq!(replay["result"]["result"], "RESULT_KIND_REPLAYED");
}
