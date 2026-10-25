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
    let mut inspect = Some(inspect);
    http_server(1, move |request| {
        inspect.take().unwrap()(request);
        body.clone()
    })
}
fn http_server(
    count: usize,
    mut respond: impl FnMut(String) -> Value + Send + 'static,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        for _ in 0..count {
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
            let body = respond(String::from_utf8(bytes).unwrap()).to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
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
        credential_lock_root: Some(dir.path().join("credential-locks")),
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

#[cfg(unix)]
#[test]
fn mcp_idle_session_stops_on_sigterm() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_lane"))
        .args(["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(input, "{}", initialize()).unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["id"], 1);
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("MCP remained blocked on stdin after SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn standalone_import_and_sync_once_preserve_versions_and_drain_outbox() {
    let dir = tempfile::tempdir().unwrap();
    let source = lane::store::Store::open(dir.path(), "", false).unwrap();
    let mut request = protocol::request("CreateTask", &json!({"repository":{"url":"https://example.com/sync"},"title":"Imported","prompt":"keep"})).unwrap();
    protocol::attribute(
        protocol::method("CreateTask").unwrap().input(),
        &mut request,
        &json!({"kind":"ORIGIN_KIND_CLI","deviceId":source.device,"deviceName":"Lane"}),
        &mut vec![],
    )
    .unwrap();
    let created = source.invoke("CreateTask", &request).unwrap();
    let version = created["result"]["version"].clone();
    let account = protocol::id();
    let epoch = protocol::id();
    let account_space = source.select_account("issuer", &account, "tenant").unwrap();
    lane::syncclient::import(&source, &account_space).unwrap();
    lane::syncclient::import(&source, &account_space).unwrap();
    let uploaded = Arc::new(Mutex::new(Vec::new()));
    let observed = uploaded.clone();
    let server_account = account.clone();
    let (endpoint, worker) = http_server(3, move |request| {
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        if request.starts_with("POST /lane.v1.SyncService/GetSyncInfo ") {
            json!({"formatVersion":1,"accountId":server_account,"tenantId":"tenant","epoch":epoch})
        } else if request.starts_with("POST /lane.v1.SyncService/PushChanges ") {
            let mutations = body["mutations"].as_array().unwrap();
            assert_eq!(mutations.len(), 2);
            *observed.lock().unwrap() = mutations.clone();
            json!({"results":mutations.iter().map(|m| json!({"requestId":m["meta"]["requestId"],"result":{"result":"RESULT_KIND_APPLIED","version":m["version"]}})).collect::<Vec<_>>()})
        } else {
            assert!(request.starts_with("POST /lane.v1.SyncService/PullChanges "));
            json!({"changes":[],"nextCursor":"page-1","hasMore":false})
        }
    });
    let auth = Auth {
        vault: Arc::new(MemoryVault::default()),
        credential_lock_root: Some(dir.path().join("locks-credentials")),
        ..Auth::default()
    };
    auth.save(&account_space, &json!({"endpoint":endpoint,"access_token":"access","refresh_token":"refresh","issuer":"issuer","account_id":account,"tenant_id":"tenant","device_id":source.device,"session_id":"session","expires_at":(chrono::Utc::now()+chrono::Duration::hours(1)).to_rfc3339()})).unwrap();
    account_space.set_enabled(true).unwrap();
    lane::syncclient::run(&auth, &account_space, "", true, false).unwrap();
    worker.join().unwrap();
    account_space
        .with(false, |connection| {
            assert_eq!(
                connection
                    .query_row::<i64, _, _>("SELECT count(*) FROM outbox", [], |row| row.get(0))?,
                0
            );
            assert_eq!(lane::store::meta(connection, "pull_cursor")?, "page-1");
            assert_eq!(lane::store::meta(connection, "sync_owner")?, "");
            Ok(())
        })
        .unwrap();
    assert!(
        uploaded
            .lock()
            .unwrap()
            .iter()
            .any(|mutation| mutation["version"]["versionId"] == version["versionId"])
    );
    assert_eq!(
        account_space
            .invoke("GetTask", &json!({"id":version["entityId"]}))
            .unwrap()["result"]["version"]["task"]["prompt"],
        "keep"
    );
}

#[test]
fn remote_cancellation_interrupts_stalled_body() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let auth = Auth::default();
    let cancelled = auth.cancelled.clone();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let mut reader = std::io::BufReader::new(&mut stream);
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse::<usize>().unwrap();
            }
        }
        reader.read_exact(&mut vec![0; length]).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{{"
        )
        .unwrap();
        stream.flush().unwrap();
        cancelled.store(true, Ordering::Relaxed);
        assert_eq!(stream.read(&mut [0]).unwrap(), 0);
    });
    let start = std::time::Instant::now();
    assert!(
        lane::remote::with_credential(
            &auth,
            &json!({"endpoint":endpoint,"access_token":"synthetic"}),
            "ListTasks",
            &json!({})
        )
        .is_err()
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    server.join().unwrap();
}
