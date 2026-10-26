// SPDX-License-Identifier: Apache-2.0
use lane::{protocol, store::Store};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};
struct Native {
    process: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Native {
    fn new(root: &std::path::Path) -> Self {
        let mut process = Command::new(
            std::env::var_os("LANE_NATIVE_STORE_DRIVER").expect("set LANE_NATIVE_STORE_DRIVER"),
        )
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
        let input = process.stdin.take().unwrap();
        let output = BufReader::new(process.stdout.take().unwrap());
        Self {
            process,
            input,
            output,
        }
    }
    fn exchange(&mut self, request: &Value) -> Value {
        writeln!(self.input, "{request}").unwrap();
        self.input.flush().unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        let _ = writeln!(self.input, "{}", json!({"method":"exit"}));
        let _ = self.input.flush();
        let _ = self.process.wait();
    }
}
#[test]
#[ignore = "requires the real native store driver; run with LANE_NATIVE_STORE_DRIVER"]
fn native_and_rust_share_data_and_removal_locks() {
    for native_removes in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path(), "", false).unwrap();
        let mut native = Native::new(dir.path());
        assert_eq!(
            native.exchange(
                &json!({"method":"open","request":{"formatVersion":2,"spaceId":store.space}})
            )["ok"],
            true
        );
        let created = native.exchange(&json!({"method":"invoke","operation":"CreateTask","spaceId":store.space,"request":{"id":protocol::id(),"meta":{"requestId":protocol::id(),"origin":{"kind":"ORIGIN_KIND_APP","deviceId":store.device,"deviceName":"Native fixture"}},"repository":{"url":"https://example.com/shared"},"title":"Native task","prompt":"retained"}}));
        assert_eq!(created["ok"], true, "{created}");
        let version = &created["response"]["result"]["version"];
        let id = version["entityId"].clone();
        let read = store.invoke("GetTask", &json!({"id":id})).unwrap();
        assert_eq!(read["result"]["version"]["task"]["title"], "Native task");
        let mut request = protocol::request(
            "SetProgress",
            &json!({"id":id,"expectedVersionId":version["versionId"],"progress":"PROGRESS_DONE"}),
        )
        .unwrap();
        protocol::attribute(
            protocol::method("SetProgress").unwrap().input(),
            &mut request,
            &json!({"kind":"ORIGIN_KIND_CLI","deviceId":store.device,"deviceName":"Lane"}),
            &mut vec![],
        )
        .unwrap();
        store.invoke("SetProgress", &request).unwrap();
        let read = native.exchange(&json!({"method":"invoke","operation":"GetTask","spaceId":store.space,"request":{"id":id}}));
        assert_eq!(
            read["response"]["result"]["version"]["task"]["progress"],
            "PROGRESS_DONE"
        );
        let path = dir.path().join("spaces").join(&store.space);
        if native_removes {
            let request = json!({"method":"invoke","operation":"RemoveDeviceData","spaceId":store.space,"request":{}});
            assert_eq!(
                native.exchange(&request)["error"]["reason"],
                "ERROR_REASON_STORAGE_BUSY"
            );
            assert!(path.join("lane.sqlite").exists());
            assert_eq!(
                store
                    .invoke("GetTask", &json!({"id":id}))
                    .unwrap_err()
                    .reason,
                "ERROR_REASON_DELETED"
            );
            assert_eq!(native.exchange(&request)["ok"], true);
        } else {
            assert_eq!(
                store.remove().unwrap_err().reason,
                "ERROR_REASON_STORAGE_BUSY"
            );
            assert!(path.join("lane.sqlite").exists());
            native.exchange(&json!({"method":"poll"}));
            Store::open(dir.path(), &store.space, true)
                .unwrap()
                .remove()
                .unwrap();
        }
        assert!(!path.exists());
    }
}
