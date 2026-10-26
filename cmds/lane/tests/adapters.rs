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
            json!({"formatVersion":3,"accountId":server_account,"tenantId":"tenant","epoch":epoch})
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

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "The route audit keeps every local and online command check together."
)]
fn realqa_cli_local_routes_and_mcp_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::new(
        Some(dir.path().to_path_buf()),
        Auth::default(),
        "ORIGIN_KIND_CLI",
    );
    let opt = Options::default();
    let repo = runtime
        .invoke(
            &opt,
            "CreateRepository",
            json!({"url":"https://example.com/app"}),
        )
        .unwrap();
    let args = [
        "--json",
        "url-rule",
        "create",
        "--prefix",
        "https://example.com/app",
        "--repository-id",
        repo["result"]["version"]["entityId"].as_str().unwrap(),
    ]
    .map(str::to_owned);
    let mut output = vec![];
    assert_eq!(
        cli::run(
            &args,
            &mut runtime,
            &mut Cursor::new(""),
            &mut output,
            &mut vec![]
        ),
        0
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output).unwrap()["ok"],
        true
    );
    for method in [
        "CreateCaptureTask",
        "GetAttachment",
        "ListURLConnections",
        "CreateURLConnection",
        "SaveURLConnection",
        "DeleteURLConnection",
    ] {
        let tool = lane::mcp::tools()
            .into_iter()
            .find(|(name, _)| name == method)
            .unwrap()
            .1;
        assert!(
            !tool["description"]
                .as_str()
                .unwrap()
                .contains("Online-only")
        );
    }
    for args in [
        vec!["data", "reset"],
        vec![
            "data",
            "reset",
            "--space",
            "invalid",
            "--confirm-development-reset",
        ],
        vec![
            "data",
            "reset",
            "--space",
            "01900100-0000-7000-8000-000000000001",
        ],
        vec![
            "data",
            "reset",
            "--space",
            "01900100-0000-7000-8000-000000000001",
            "--confirm-development-reset",
            "--request-json",
            "{\"x\":1}",
        ],
        vec!["data", "reset", "--remote", "--confirm-development-reset"],
        vec![
            "data",
            "migrate",
            "--space",
            "01900100-0000-7000-8000-000000000001",
        ],
    ] {
        assert_eq!(
            cli::run(
                &args.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                &mut runtime,
                &mut Cursor::new(""),
                &mut vec![],
                &mut vec![]
            ),
            2
        );
    }
}

#[test]
fn attachment_sync_uses_epoch_bound_upload_and_validates_download() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let dir = tempfile::tempdir().unwrap();
    let source = lane::store::Store::open(dir.path(), "", false).unwrap();
    let mut runtime = Runtime::new(
        Some(dir.path().to_path_buf()),
        Auth::default(),
        "ORIGIN_KIND_CLI",
    );
    let repo = runtime
        .invoke(
            &Options::default(),
            "CreateRepository",
            json!({"url":"https://example.com/capture"}),
        )
        .unwrap()["result"]["version"]["entityId"]
        .clone();
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
    let bytes = STANDARD.decode(png).unwrap();
    let attachment = protocol::id();
    let version = runtime.invoke(&Options::default(),"CreateCaptureTask",json!({"repositoryId":repo,"title":"Test","images":[{"capture":{"attachmentId":attachment,"width":1,"height":1,"byteSize":bytes.len().to_string(),"sha256":protocol::hash(&bytes)},"png":png}]})).unwrap()["result"]["version"].clone();
    drop(runtime);
    let account = protocol::id();
    let epoch = protocol::id();
    let dest = source.select_account("issuer", &account, "tenant").unwrap();
    lane::syncclient::import(&source, &dest).unwrap();
    let server_account = account.clone();
    let (endpoint, worker) = http_server(7, move |request| {
        let q: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        if request.contains("/GetSyncInfo ") {
            json!({"formatVersion":3,"accountId":server_account,"tenantId":"tenant","epoch":epoch})
        } else if request.contains("/PushChanges ") {
            assert_eq!(q["formatVersion"], 3);
            json!({"results":q["mutations"].as_array().unwrap().iter().map(|m|json!({"requestId":m["meta"]["requestId"],"result":{"result":"RESULT_KIND_APPLIED","version":m["version"]}})).collect::<Vec<_>>()})
        } else if request.contains("/PullChanges ") {
            assert_eq!(q["formatVersion"], 3);
            json!({"changes":[],"nextCursor":"page-1","hasMore":false})
        } else if request.contains("/BeginAttachmentUpload ") {
            assert_eq!(q["epoch"], epoch);
            assert_eq!(q["attachmentId"], attachment);
            json!({"state":"ATTACHMENT_STATE_AVAILABLE"})
        } else {
            assert!(request.contains("/GetAttachment "));
            assert_eq!(q["attachmentId"], attachment);
            json!({"state":"ATTACHMENT_STATE_AVAILABLE","png":png})
        }
    });
    let auth = Auth {
        vault: Arc::new(MemoryVault::default()),
        credential_lock_root: Some(dir.path().join("credential-locks")),
        ..Auth::default()
    };
    auth.save(&dest,&json!({"endpoint":endpoint,"access_token":"access","refresh_token":"refresh","issuer":"issuer","account_id":account,"tenant_id":"tenant","device_id":source.device,"session_id":"session","expires_at":(chrono::Utc::now()+chrono::Duration::hours(1)).to_rfc3339()})).unwrap();
    dest.set_enabled(true).unwrap();
    lane::syncclient::run(&auth, &dest, "", true, false).unwrap();
    dest.with(true, |c| {
        assert_eq!(
            c.query_row::<i64, _, _>("SELECT synchronized FROM attachments", [], |r| r.get(0))?,
            1
        );
        c.execute("UPDATE attachments SET png_base64=NULL", [])?;
        Ok(())
    })
    .unwrap();
    lane::syncclient::run(&auth, &dest, "", true, false).unwrap();
    worker.join().unwrap();
    assert_eq!(dest.invoke("GetAttachment",&json!({"taskId":version["entityId"],"attachmentId":version["task"]["captures"][0]["attachmentId"]})).unwrap()["png"],png);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "The scenario records one complete rejection, hold-back, correction and retry flow."
)]
fn constraint_rejection_blocks_items_until_corrected_and_retried() {
    use std::sync::atomic::AtomicUsize;
    let dir = tempfile::tempdir().unwrap();
    let source = lane::store::Store::open(dir.path(), "", false).unwrap();
    let call = |store: &lane::store::Store, method: &str, request: Value| -> Value {
        let mut q = protocol::request(method, &request).unwrap();
        protocol::attribute(
            protocol::method(method).unwrap().input(),
            &mut q,
            &json!({"kind":"ORIGIN_KIND_CLI","deviceId":store.device,"deviceName":"Lane","accountId":store.identity().unwrap().account}),
            &mut vec![],
        )
        .unwrap();
        store.invoke(method, &q).unwrap()
    };
    let repo = call(
        &source,
        "CreateRepository",
        json!({"url":"https://example.com/blocked"}),
    )["result"]["version"]["entityId"]
        .clone();
    let label = call(
        &source,
        "CreatePlanningItem",
        json!({"item":{"label":{"repositoryId":repo,"name":"Docs","color":"#112233"}}}),
    )["result"]["version"]
        .clone();
    let label_id = label["entityId"].clone();
    let first = call(
        &source,
        "CreateTask",
        json!({"repository":{"id":repo},"title":"First","labelIds":[label_id]}),
    )["result"]["version"]
        .clone();
    let account = protocol::id();
    let epoch = protocol::id();
    let dest = source.select_account("issuer", &account, "tenant").unwrap();
    lane::syncclient::import(&source, &dest).unwrap();

    let phase = Arc::new(AtomicUsize::new(0));
    let pushes = Arc::new(Mutex::new(Vec::<Vec<Value>>::new()));
    let (server_phase, server_pushes, server_account) =
        (phase.clone(), pushes.clone(), account.clone());
    let (endpoint, worker) = http_server(11, move |request| {
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(
            body.get("formatVersion").map_or(3, |v| v.as_i64().unwrap()),
            3
        );
        if request.contains("/GetSyncInfo ") {
            json!({"formatVersion":3,"accountId":server_account,"tenantId":"tenant","epoch":epoch})
        } else if request.contains("/PushChanges ") {
            let mutations = body["mutations"].as_array().unwrap().clone();
            let first_phase = server_phase.load(Ordering::SeqCst) == 0;
            server_pushes.lock().unwrap().push(mutations.clone());
            json!({"results":mutations.iter().map(|m| {
                let version = &m["version"];
                let id = &m["meta"]["requestId"];
                // The first push rejects the label (a duplicate name created elsewhere) and,
                // because the server never accepted it, the task that references it.
                let rejection = match version["kind"].as_str().unwrap() {
                    "ENTITY_KIND_LABEL" if first_phase => Some("CONSTRAINT_VIOLATION_DUPLICATE_NAME"),
                    "ENTITY_KIND_TASK" if first_phase => Some("CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND"),
                    _ => None,
                };
                rejection.map_or_else(
                    || json!({"requestId":id,"result":{"result":"RESULT_KIND_APPLIED","version":version}}),
                    |violation| json!({"requestId":id,"error":{"reason":"ERROR_REASON_CONSTRAINT_VIOLATION","violation":violation,"entityId":version["entityId"],"retryable":false}}),
                )
            }).collect::<Vec<_>>()})
        } else {
            assert!(request.contains("/PullChanges "));
            json!({"changes":[],"nextCursor":"cursor","hasMore":false})
        }
    });
    let auth = Auth {
        vault: Arc::new(MemoryVault::default()),
        credential_lock_root: Some(dir.path().join("credential-locks")),
        ..Auth::default()
    };
    auth.save(&dest,&json!({"endpoint":endpoint,"access_token":"access","refresh_token":"refresh","issuer":"issuer","account_id":account,"tenant_id":"tenant","device_id":source.device,"session_id":"session","expires_at":(chrono::Utc::now()+chrono::Duration::hours(1)).to_rfc3339()})).unwrap();
    dest.set_enabled(true).unwrap();

    // First run: the rejected items stay local, blocked with the named violation.
    lane::syncclient::run(&auth, &dest, "", true, false).unwrap();
    let blocked = |store: &lane::store::Store| -> Vec<(String, i64, Option<String>)> {
        store
            .with(false, |c| {
                let mut stmt = c.prepare(
                    "SELECT entity_id,state,error_reason FROM outbox ORDER BY created_at_us,rowid",
                )?;
                Ok(stmt
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<std::result::Result<Vec<_>, _>>()?)
            })
            .unwrap()
    };
    let rows = blocked(&dest);
    assert_eq!(
        rows.iter()
            .map(|r| (r.1, r.2.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            (2, Some("CONSTRAINT_VIOLATION:DUPLICATE_NAME")),
            (2, Some("CONSTRAINT_VIOLATION:REFERENCE_NOT_FOUND")),
        ]
    );
    let status = dest.invoke("GetLocalStatus", &json!({})).unwrap();
    assert_eq!(status["itemErrors"].as_array().unwrap().len(), 2);
    assert_eq!(
        status["itemErrors"][0]["reason"],
        "CONSTRAINT_VIOLATION:DUPLICATE_NAME"
    );
    assert_eq!(status["pendingItemCount"], "2");
    // The local data is neither discarded nor reported as synchronized.
    assert_eq!(
        call(&dest, "GetPlanningItem", json!({"id":label_id}))["result"]["version"]["label"]["name"],
        "Docs"
    );
    assert_eq!(
        call(&dest, "GetTask", json!({"id":first["entityId"]}))["result"]["version"]["task"]["title"],
        "First"
    );

    // A new task that references the rejected label is held back instead of racing ahead.
    phase.store(1, Ordering::SeqCst);
    let second = call(
        &dest,
        "CreateTask",
        json!({"repository":{"id":repo},"title":"Second","labelIds":[label_id]}),
    )["result"]["version"]
        .clone();
    lane::syncclient::run(&auth, &dest, "", true, false).unwrap();
    assert_eq!(
        pushes.lock().unwrap().len(),
        1,
        "nothing was uploaded while the label is blocked"
    );
    // Correct the label with an ordinary edit.
    let renamed = call(
        &dest,
        "SavePlanningItem",
        json!({"id":label_id,"expectedVersionId":label["versionId"],
        "item":{"label":{"repositoryId":repo,"name":"Docs 2","color":"#112233"}}}),
    )["result"]["version"]
        .clone();
    assert_eq!(
        renamed["parentVersionIds"],
        json!([]),
        "the rejected version is superseded, not stranded as a parent"
    );
    let rows = blocked(&dest);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(
        rows[0].0,
        label_id.as_str().unwrap(),
        "the corrected label keeps its upload position"
    );
    assert_eq!(rows[0].1, 1);

    // Without --retry-blocked the corrected label and its dependent task are uploaded in
    // causal order; the still-blocked task stays blocked and is never reported as synchronized.
    lane::syncclient::run(&auth, &dest, "", true, false).unwrap();
    {
        let pushes = pushes.lock().unwrap();
        let sent: Vec<&str> = pushes[1]
            .iter()
            .map(|m| m["version"]["versionId"].as_str().unwrap())
            .collect();
        assert_eq!(
            sent,
            vec![
                renamed["versionId"].as_str().unwrap(),
                second["versionId"].as_str().unwrap()
            ]
        );
        assert_eq!(pushes[1][0]["kind"], "MUTATION_KIND_CREATE");
        assert_eq!(pushes[1][0]["version"]["label"]["name"], "Docs 2");
    }
    assert_eq!(
        blocked(&dest).iter().map(|r| r.1).collect::<Vec<_>>(),
        vec![2],
        "only the first task remains blocked"
    );
    // retry-blocked resends it and the server now accepts it.
    lane::syncclient::run(&auth, &dest, "", true, true).unwrap();
    worker.join().unwrap();
    let pushes = pushes.lock().unwrap();
    assert_eq!(pushes.len(), 3);
    assert_eq!(pushes[2][0]["version"]["versionId"], first["versionId"]);
    assert_eq!(
        pushes[2][0]["meta"]["requestId"], first["versionId"],
        "original request ID is preserved"
    );
    assert!(
        blocked(&dest).is_empty(),
        "everything acknowledged and removed"
    );
}
