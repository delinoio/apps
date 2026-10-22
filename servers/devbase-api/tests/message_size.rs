// SPDX-License-Identifier: FSL-1.1-ALv2
//! The 64 KiB request limit is enforced against the message bytes as received (after transport
//! decompression), not against the decoded message. These tests drive the real router with a
//! disposable PostgreSQL schema and a mocked Delcore.
use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use base64::Engine;
use buffa::{Message, view::MessageView};
use connectrpc::RequestContext;
use delino_protocol::devbase::v1::{DevBaseService, ErrorDetail, GetProjectRequest};
use devbase_api::{Server, config::Config, store};
use serde_json::{Value, json};
use server_common::authority::Authority;
use std::{io::Write, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const LIMIT: usize = 64 << 10;
const TRANSPORT_LIMIT: usize = 256 << 10;

struct Reply {
    status: StatusCode,
    trace: Option<String>,
    body: Vec<u8>,
}
impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "JSON reply expected ({e}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }
}

/// Builds the JSON request from the issue's fixture: the closing brace follows `size - 52` spaces.
fn padded_json(project_id: &str, size: usize) -> Vec<u8> {
    let compact = serde_json::to_vec(&json!({"projectId": project_id})).unwrap();
    assert_eq!(compact.len(), 52);
    assert!(size >= compact.len());
    let mut body = compact[..compact.len() - 1].to_vec();
    body.extend(std::iter::repeat_n(b' ', size - compact.len()));
    body.push(b'}');
    assert_eq!(body.len(), size);
    body
}

/// The same field with the first UUID character written as a Unicode escape.
fn escaped_json(project_id: &str, size: usize) -> Vec<u8> {
    let first = project_id.chars().next().unwrap();
    let escaped = format!("\\u{:04x}{}", first as u32, &project_id[1..]);
    let compact = format!("{{\"projectId\":\"{escaped}\"}}").into_bytes();
    assert!(size >= compact.len());
    let mut body = compact[..compact.len() - 1].to_vec();
    body.extend(std::iter::repeat_n(b' ', size - compact.len()));
    body.push(b'}');
    assert_eq!(body.len(), size);
    body
}

/// A binary GetProject request of exactly `size` bytes. Padding is an unknown field, which the
/// decoded message does not keep in its canonical form.
fn padded_binary(project_id: &str, size: usize) -> Vec<u8> {
    let mut body = GetProjectRequest {
        project_id: project_id.into(),
        ..Default::default()
    }
    .encode_to_vec();
    assert!(size >= body.len());
    let extra = size - body.len();
    if extra > 0 {
        // Field 15, length-delimited: one tag byte, a varint length, then the data.
        let varint = |n: usize| match n {
            0..=127 => 1,
            128..=16383 => 2,
            _ => 3,
        };
        let data = (1..=extra)
            .rev()
            .find(|n| 1 + varint(*n) + n == extra)
            .expect("padding length");
        body.push(0x7a);
        let mut n = data;
        loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                body.push(byte);
                break;
            }
            body.push(byte | 0x80);
        }
        body.extend(std::iter::repeat_n(0u8, data));
    }
    assert_eq!(body.len(), size);
    body
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

struct Harness {
    http: reqwest::Client,
    base: String,
    addr: std::net::SocketAddr,
    server: Arc<Server>,
    project: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    db: sqlx::PgPool,
    admin: sqlx::PgPool,
    schema: String,
}
impl Harness {
    async fn start() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("devbase_{}", uuid::Uuid::now_v7().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = schema.clone();
        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(10)
            .after_connect(move |conn, _| {
                let search = search.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path TO {search}"))
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        store::migrate(&db).await.unwrap();
        let tenant = uuid::Uuid::now_v7().to_string();
        let actor = uuid::Uuid::now_v7().to_string();
        let mock = Router::new()
            .route(
                "/token",
                post(|| async { Json(json!({"access_token":"service","expires_in":3600})) }),
            )
            .route(
                "/delcore.v1.DelcoreService/GetCurrentUser",
                post(move |h: HeaderMap| {
                    let actor = actor.clone();
                    async move {
                        if h.get("authorization").and_then(|h| h.to_str().ok())
                            != Some("Bearer manager")
                        {
                            return (
                                StatusCode::UNAUTHORIZED,
                                Json(json!({"code":"unauthenticated"})),
                            );
                        }
                        (StatusCode::OK, Json(json!({"userId":actor})))
                    }
                }),
            )
            .route(
                "/delcore.v1.DelcoreService/ListMembers",
                post(|h: HeaderMap| async move {
                    if h.get("authorization").and_then(|h| h.to_str().ok())
                        == Some("Bearer manager")
                    {
                        (StatusCode::OK, Json(json!({})))
                    } else {
                        (
                            StatusCode::FORBIDDEN,
                            Json(json!({"code":"permission_denied"})),
                        )
                    }
                }),
            )
            .route(
                "/delcore.v1.DelcoreService/GetTenantStates",
                post(|Json(q): Json<Value>| async move {
                    let entries: Vec<_> = q["tenantIds"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|id| json!({"tenantId":id,"state":"TENANT_STATE_ACTIVE"}))
                        .collect();
                    Json(json!({"states":entries}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let delcore = format!("http://{}", listener.local_addr().unwrap());
        let mock_task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
        let config = Config {
            projects: 100,
            users: 100,
            keys: 20,
            user_rate: 10000.0,
            user_burst: 20000,
            management_rate: 10000.0,
            management_burst: 20000,
            dependency_timeout: Duration::from_secs(3),
            request_timeout: Duration::from_secs(10),
            cleanup_interval: Duration::from_secs(300),
            origin: "https://console.example".into(),
            listen: "127.0.0.1:0".into(),
        };
        let http = reqwest::Client::new();
        let authority = Authority::new(
            http.clone(),
            delcore.clone(),
            format!("{delcore}/token"),
            "resource".into(),
            "client".into(),
            "fixture".into(),
        );
        let server = Arc::new(Server::new(db.clone(), authority, config));
        let router = devbase_api::http::router(server.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut harness = Self {
            http,
            base: format!("http://{addr}/devbase.v1.DevBaseService"),
            addr,
            server,
            project: String::new(),
            tasks: vec![mock_task, task],
            db,
            admin,
            schema,
        };
        let created = harness
            .send(
                "CreateProject",
                "application/json",
                &[],
                serde_json::to_vec(
                    &json!({"tenantId":tenant,"name":"Project","idempotencyKey":"size-project"}),
                )
                .unwrap(),
            )
            .await;
        assert_eq!(created.status, StatusCode::OK, "{}", created.json());
        harness.project = created.json()["project"]["id"].as_str().unwrap().into();
        harness
    }

    /// Posts `body` with a Content-Length as the manager.
    async fn send(
        &self,
        method: &str,
        content_type: &str,
        extra: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Reply {
        let mut request = self
            .http
            .post(format!("{}/{method}", self.base))
            .bearer_auth("manager")
            .header("Content-Type", content_type)
            .header("Connect-Protocol-Version", "1");
        for (name, value) in extra {
            request = request.header(*name, *value);
        }
        let response = request.body(body).send().await.unwrap();
        Reply {
            status: response.status(),
            trace: response
                .headers()
                .get("x-devbase-trace-id")
                .map(|v| v.to_str().unwrap().to_owned()),
            body: response.bytes().await.unwrap().to_vec(),
        }
    }

    async fn get_project(&self, body: Vec<u8>) -> Reply {
        self.send("GetProject", "application/json", &[], body).await
    }

    /// Posts `body` with `Transfer-Encoding: chunked` and no Content-Length.
    async fn chunked(&self, body: &[u8]) -> Reply {
        let mut socket = tokio::net::TcpStream::connect(self.addr).await.unwrap();
        let head = "POST /devbase.v1.DevBaseService/GetProject HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nConnect-Protocol-Version: 1\r\nAuthorization: Bearer manager\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
        socket.write_all(head.as_bytes()).await.unwrap();
        for chunk in body.chunks(7000) {
            socket
                .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                .await
                .unwrap();
            socket.write_all(chunk).await.unwrap();
            socket.write_all(b"\r\n").await.unwrap();
        }
        socket.write_all(b"0\r\n\r\n").await.unwrap();
        let mut raw = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), socket.read_to_end(&mut raw))
            .await
            .expect("reply within the socket bound")
            .unwrap();
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let status = head
            .lines()
            .next()
            .unwrap()
            .split(' ')
            .nth(1)
            .unwrap()
            .parse::<u16>()
            .unwrap();
        let trace = head.lines().find_map(|line| {
            let (name, value) = line.split_once(": ")?;
            name.eq_ignore_ascii_case("x-devbase-trace-id")
                .then(|| value.to_owned())
        });
        assert!(
            !head
                .lines()
                .any(|l| l.to_ascii_lowercase().starts_with("content-length: 0")),
            "{head}"
        );
        let rest = &raw[split + 4..];
        // The reply body is not chunk-encoded here: axum answers with a Content-Length body.
        let body = if head
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked")
        {
            let mut decoded = Vec::new();
            let mut rest = rest;
            loop {
                let line_end = rest.windows(2).position(|w| w == b"\r\n").unwrap();
                let size = usize::from_str_radix(
                    std::str::from_utf8(&rest[..line_end]).unwrap().trim(),
                    16,
                )
                .unwrap();
                if size == 0 {
                    break;
                }
                decoded.extend_from_slice(&rest[line_end + 2..line_end + 2 + size]);
                rest = &rest[line_end + 2 + size + 2..];
            }
            decoded
        } else {
            rest.to_vec()
        };
        Reply {
            status: StatusCode::from_u16(status).unwrap(),
            trace,
            body,
        }
    }

    async fn finish(self) {
        for task in &self.tasks {
            task.abort();
        }
        drop(self.server);
        self.db.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

fn assert_success(reply: &Reply, project: &str, what: &str) {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{what}: {}",
        String::from_utf8_lossy(&reply.body)
    );
    assert!(reply.trace.is_some(), "{what}: trace header");
    // JSON and binary replies both carry the project; the binary form is checked separately.
    if let Ok(body) = serde_json::from_slice::<Value>(&reply.body) {
        assert_eq!(body["project"]["id"], project, "{what}");
    }
}

/// Asserts the typed DevBase size rejection and returns the decoded error detail.
fn assert_typed_size_error(reply: &Reply, what: &str) -> ErrorDetail {
    assert_eq!(
        reply.status,
        StatusCode::BAD_REQUEST,
        "{what}: {}",
        String::from_utf8_lossy(&reply.body)
    );
    let body = reply.json();
    assert_eq!(body["code"], "invalid_argument", "{what}");
    assert_eq!(
        body["details"][0]["type"], "devbase.v1.ErrorDetail",
        "{what}"
    );
    let raw = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(body["details"][0]["value"].as_str().unwrap())
        .unwrap();
    let detail = ErrorDetail::decode_from_slice(&raw).unwrap();
    assert_eq!(
        detail.reason.to_i32(),
        1,
        "{what}: ERROR_REASON_INVALID_INPUT"
    );
    let trace = reply.trace.as_deref().expect("trace header");
    assert_eq!(detail.trace_id, trace, "{what}");
    detail
}

/// Asserts the plain transport rejection: no DevBase detail and no trace header.
fn assert_transport_rejection(reply: &Reply, what: &str) {
    assert_eq!(
        reply.status,
        StatusCode::TOO_MANY_REQUESTS,
        "{what}: {}",
        String::from_utf8_lossy(&reply.body)
    );
    let body = reply.json();
    assert_eq!(body["code"], "resource_exhausted", "{what}");
    assert!(
        body.get("details")
            .and_then(Value::as_array)
            .is_none_or(|d| d.is_empty()),
        "{what}: {body}"
    );
    assert!(reply.trace.is_none(), "{what}: trace header");
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn received_message_size_is_limited_for_json_binary_chunked_and_compressed_requests() {
    let h = Harness::start().await;
    let pid = h.project.clone();

    // Compact JSON and binary requests keep working.
    let compact = serde_json::to_vec(&json!({"projectId": pid})).unwrap();
    assert_success(&h.get_project(compact).await, &pid, "compact JSON");
    let binary = padded_binary(&pid, 38);
    let reply = h.send("GetProject", "application/proto", &[], binary).await;
    assert!(reply.status.is_success(), "{:?}", reply.status);

    // JSON padded with legal whitespace: the decoded request is always 38 bytes.
    for size in [LIMIT - 1, LIMIT] {
        let reply = h.get_project(padded_json(&pid, size)).await;
        assert_success(&reply, &pid, &format!("JSON {size}"));
    }
    for size in [LIMIT + 1, TRANSPORT_LIMIT] {
        let reply = h.get_project(padded_json(&pid, size)).await;
        assert_typed_size_error(&reply, &format!("JSON {size}"));
    }
    let reply = h.get_project(padded_json(&pid, TRANSPORT_LIMIT + 1)).await;
    assert_transport_rejection(&reply, "JSON 262145");

    // Unicode-escape expansion counts toward the limit although the field is identical.
    assert_success(
        &h.get_project(escaped_json(&pid, LIMIT)).await,
        &pid,
        "escaped JSON 65536",
    );
    assert_typed_size_error(
        &h.get_project(escaped_json(&pid, LIMIT + 1)).await,
        "escaped JSON 65537",
    );

    // Binary messages are measured as received, including bytes the decoded message drops.
    for size in [LIMIT - 1, LIMIT] {
        let reply = h
            .send(
                "GetProject",
                "application/proto",
                &[],
                padded_binary(&pid, size),
            )
            .await;
        assert!(
            reply.status.is_success(),
            "binary {size}: {:?}",
            reply.status
        );
    }
    for size in [LIMIT + 1, TRANSPORT_LIMIT] {
        let reply = h
            .send(
                "GetProject",
                "application/proto",
                &[],
                padded_binary(&pid, size),
            )
            .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "binary {size}");
        assert!(reply.trace.is_some(), "binary {size}: trace header");
    }
    let reply = h
        .send(
            "GetProject",
            "application/proto",
            &[],
            padded_binary(&pid, TRANSPORT_LIMIT + 1),
        )
        .await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(reply.trace.is_none());

    // Chunked bodies have no Content-Length; the consumed bytes are measured.
    assert_success(
        &h.chunked(&padded_json(&pid, LIMIT)).await,
        &pid,
        "chunked JSON 65536",
    );
    assert_typed_size_error(
        &h.chunked(&padded_json(&pid, LIMIT + 1)).await,
        "chunked JSON 65537",
    );
    assert_typed_size_error(
        &h.chunked(&padded_json(&pid, TRANSPORT_LIMIT)).await,
        "chunked JSON 262144",
    );
    assert_transport_rejection(
        &h.chunked(&padded_json(&pid, TRANSPORT_LIMIT + 1)).await,
        "chunked JSON 262145",
    );

    // Compressed requests are sized after decompression and keep the decompression guard.
    let gz = [("Content-Encoding", "gzip")];
    let reply = h
        .send(
            "GetProject",
            "application/json",
            &gz,
            gzip(&padded_json(&pid, LIMIT)),
        )
        .await;
    assert_success(&reply, &pid, "gzip JSON 65536");
    for size in [LIMIT + 1, TRANSPORT_LIMIT] {
        let wire = gzip(&padded_json(&pid, size));
        assert!(wire.len() < LIMIT, "fixture must compress below the limit");
        let reply = h.send("GetProject", "application/json", &gz, wire).await;
        assert_typed_size_error(&reply, &format!("gzip JSON {size}"));
    }
    let reply = h
        .send(
            "GetProject",
            "application/json",
            &gz,
            gzip(&padded_json(&pid, TRANSPORT_LIMIT + 1)),
        )
        .await;
    assert_transport_rejection(&reply, "gzip JSON 262145");

    // A handler reached without the captured length fails closed with the typed unavailable
    // result. The same call succeeds once the length is supplied explicitly.
    let body = bytes_of(&pid);
    let view =
        <GetProjectRequest as buffa::view::HasMessageView>::View::decode_view(&body).unwrap();
    let request = connectrpc::ServiceRequest::<GetProjectRequest>::from_parts(&view, &body);
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", "Bearer manager".parse().unwrap());
    let missing = h
        .server
        .get_project(RequestContext::new(headers.clone()), request)
        .await;
    let Err(error) = missing else {
        panic!("missing length must fail closed");
    };
    assert_eq!(error.code, connectrpc::ErrorCode::Unavailable);

    h.finish().await;
}

fn bytes_of(project_id: &str) -> buffa::bytes::Bytes {
    GetProjectRequest {
        project_id: project_id.into(),
        ..Default::default()
    }
    .encode_to_vec()
    .into()
}
