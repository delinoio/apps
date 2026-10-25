// SPDX-License-Identifier: Apache-2.0
use lane::{
    auth::{Auth, Vault},
    cli::{self, Dispatcher, Options},
    error::{Failure, Result},
    protocol,
    runtime::Runtime,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, Cursor, Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex, atomic::Ordering},
};

#[derive(Default)]
struct MemoryVault(Mutex<BTreeMap<String, Value>>);
impl Vault for MemoryVault {
    fn get(&self, key: &str) -> Result<Value> {
        self.0
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or_else(|| Failure::new("UNAUTHENTICATED", "Missing test credential."))
    }
    fn set(&self, key: &str, value: &Value) -> Result<()> {
        self.0.lock().unwrap().insert(key.into(), value.clone());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}
fn initialize() -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test"}}})
}
#[test]
fn mcp_discovery_does_not_create_storage() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("absent");
    let mut runtime = Runtime::new(Some(root.clone()), Auth::default(), "ORIGIN_KIND_MCP");
    let mut input = Cursor::new(format!(
        "{}\n{}\n",
        initialize(),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})
    ));
    let mut output = vec![];
    runtime
        .mcp(&Options::default(), &mut input, &mut output)
        .unwrap();
    assert!(!root.exists());
    let messages: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        messages[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "lane_create_task")
    );
}
#[test]
fn mcp_cancellation_and_validation() {
    let messages = [
        initialize(),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"lane_get_local_status","arguments":{}}}),
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"lane_create_task","arguments":{"meta":{"origin":{"kind":"ORIGIN_KIND_CLI"}}}}}),
    ];
    let mut input = Cursor::new(
        messages
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    );
    let mut output = vec![];
    lane::mcp::serve(&mut input, &mut output, |method, _, cancelled| {
        assert_eq!(method, "GetLocalStatus");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !cancelled.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(cancelled.load(Ordering::Relaxed));
        Ok(json!({}))
    })
    .unwrap();
    let messages: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1]["id"], 3);
    assert_eq!(messages[1]["error"]["code"], -32602);
    assert!(lane::mcp::decode(br#"{"x":1,"x":2}"#).is_err());
}
#[test]
fn mcp_eof_drains_accepted_calls() {
    let mut messages = vec![initialize()];
    for id in 2..10 {
        messages.push(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"lane_get_local_status"}}));
    }
    let mut output = vec![];
    lane::mcp::serve(
        &mut Cursor::new(
            messages
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        ),
        &mut output,
        |_, _, _| Ok(json!({"ready":true})),
    )
    .unwrap();
    assert_eq!(String::from_utf8(output).unwrap().lines().count(), 9);
}
struct Capture {
    calls: Vec<(Options, String, Value)>,
}
impl Dispatcher for Capture {
    fn invoke(&mut self, options: &Options, method: &str, request: Value) -> Result<Value> {
        self.calls.push((options.clone(), method.into(), request));
        if method == "GetTask" {
            return Ok(
                json!({"result":{"version":{"task":{"title":"old","prompt":"keep","repositoryId":"repo","progress":"PROGRESS_BLOCKED","proposal":"PROPOSAL_PENDING"}}}}),
            );
        }
        Ok(json!({}))
    }
    fn mcp(&mut self, _: &Options, _: &mut dyn BufRead, _: &mut (dyn Write + Send)) -> Result<()> {
        panic!("unexpected MCP")
    }
}
#[test]
fn cli_preserves_snapshots_and_private_flags() {
    let mut capture = Capture { calls: vec![] };
    let args = [
        "--json",
        "task",
        "edit",
        "id",
        "--expected-version",
        "old",
        "--title",
        "new",
    ]
    .map(str::to_owned);
    assert_eq!(
        cli::run(
            &args,
            &mut capture,
            &mut Cursor::new(""),
            &mut vec![],
            &mut vec![]
        ),
        0
    );
    let request = &capture.calls[1].2;
    assert_eq!(request["expectedVersionId"], "old");
    assert_eq!(request["task"]["prompt"], "keep");
    assert_eq!(request["task"]["title"], "new");
    assert_eq!(request["task"]["progress"], "PROGRESS_BLOCKED");
    for args in [
        vec!["--json", "--remote", "--space", "x", "task", "list"],
        vec!["--json", "task", "list", "--bogus", "x"],
        vec!["--json", "rpc", "Bogus"],
        vec!["--json", "task", "edit", "id"],
    ] {
        let mut output = vec![];
        assert_eq!(
            cli::run(
                &args.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                &mut capture,
                &mut Cursor::new(""),
                &mut output,
                &mut vec![]
            ),
            2
        );
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("ERROR_REASON_INVALID_INPUT")
        );
    }
}
fn http_once(
    body: Value,
    inspect: impl FnOnce(String) + Send + 'static,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let mut bytes = vec![];
        let mut buffer = [0; 4096];
        loop {
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buffer[..n]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length:")
                            .map(|n| n.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
        }
        inspect(String::from_utf8(bytes).unwrap());
        let body = body.to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    (endpoint, worker)
}
#[test]
fn typed_connect_transport_and_endpoint_binding() {
    let auth = Auth {
        vault: Arc::new(MemoryVault::default()),
        ..Auth::default()
    };
    let (endpoint, worker) = http_once(json!({"items":[]}), |request| {
        assert!(request.starts_with("POST /lane.v1.LaneService/ListTasks HTTP/1.1"));
        assert!(
            request
                .to_lowercase()
                .contains("connect-protocol-version: 1")
        );
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer test-token")
        );
    });
    let response = lane::remote::with_credential(
        &auth,
        &json!({"endpoint":endpoint,"access_token":"test-token"}),
        "ListTasks",
        &json!({}),
    )
    .unwrap();
    assert_eq!(response["items"], json!([]));
    worker.join().unwrap();
    for endpoint in [
        "http://example.com",
        "https://user:password@example.com",
        "https://example.com/path",
        "https://example.com?token=x",
    ] {
        assert!(lane::auth::endpoint(endpoint).is_err());
    }
}
#[test]
fn browser_approval_binds_state_device_and_pkce() {
    let dir = tempfile::tempdir().unwrap();
    let store = lane::store::Store::open(dir.path(), "", false).unwrap();
    let vault = Arc::new(MemoryVault::default());
    let auth = Auth {
        vault,
        ..Auth::default()
    };
    let account = protocol::id();
    let device = store.device.clone();
    let body = json!({"access_token":"access","refresh_token":"refresh","token_type":"Bearer","expires_in":3600,"issuer":"issuer","account_id":account,"tenant_id":"tenant","device_id":device,"session_id":"session","epoch":protocol::id()});
    let (endpoint, worker) = http_once(body, |request| {
        assert!(request.starts_with("POST /oauth/token "));
        assert!(request.contains("code_verifier="));
        assert!(request.contains("grant_type=authorization_code"));
    });
    let approval = auth
        .begin(&store, &endpoint, "lane://auth/callback", &account, false)
        .unwrap();
    let url = url::Url::parse(approval["authorizationUrl"].as_str().unwrap()).unwrap();
    let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["device_id"], store.device);
    assert_eq!(query["code_challenge_method"], "S256");
    assert!(query["code_challenge"].len() >= 43);
    assert!(
        auth.complete(&store, "lane://auth/callback?state=wrong&code=code")
            .is_err()
    );
    let callback = format!("lane://auth/callback?state={}&code=code", query["state"]);
    let credentials = auth.complete(&store, &callback).unwrap();
    assert_eq!(credentials["account_id"], account);
    assert!(auth.complete(&store, &callback).is_err());
    worker.join().unwrap();
}
