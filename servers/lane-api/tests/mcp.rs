// SPDX-License-Identifier: FSL-1.1-ALv2
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use lane_api::{
    Fault, Result,
    mcp::{Backend, router_with_backend},
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;

#[derive(Default)]
struct MockDispatch {
    authenticated: bool,
    auth_calls: AtomicUsize,
    calls: Mutex<Vec<(String, String, Value)>>,
    fail: bool,
    revoked: bool,
}
impl Backend for MockDispatch {
    fn max_request_bytes(&self) -> usize {
        1 << 20
    }
    async fn authenticate(&self, headers: &HeaderMap) -> Result<()> {
        self.auth_calls.fetch_add(1, Ordering::SeqCst);
        if self.authenticated
            && headers
                .get("authorization")
                .is_some_and(|v| v == "Bearer test")
        {
            Ok(())
        } else {
            Err(Fault::unauthenticated())
        }
    }
    async fn execute(
        &self,
        _: &HeaderMap,
        service: &'static str,
        method: &'static str,
        arguments: Value,
    ) -> Result<Value> {
        self.calls
            .lock()
            .unwrap()
            .push((service.into(), method.into(), arguments));
        if self.revoked {
            return Err(Fault::unauthenticated());
        }
        if self.fail {
            return Err(Fault::stale());
        }
        Ok(json!({"items":[], "nextCursor":""}))
    }
    fn resource_metadata_url(&self) -> String {
        "https://lane.example/.well-known/oauth-protected-resource/mcp".into()
    }
}
fn setup() -> (Router, Arc<MockDispatch>) {
    let backend = Arc::new(MockDispatch {
        authenticated: true,
        ..Default::default()
    });
    (router_with_backend(backend.clone()), backend)
}
fn request(method: &str, body: String) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri("/mcp")
        .header("authorization", "Bearer test")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from(body))
        .unwrap()
}
async fn send(app: Router, value: Value) -> (StatusCode, HeaderMap, Value) {
    response(
        app.oneshot(request("POST", value.to_string()))
            .await
            .unwrap(),
    )
    .await
}
async fn response(response: axum::response::Response) -> (StatusCode, HeaderMap, Value) {
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 32 << 20).await.unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, headers, value)
}
fn rpc(method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0", "id":"test-id", "method":method, "params":params})
}

#[tokio::test]
async fn every_http_operation_authenticates_before_body_handling() {
    let backend = Arc::new(MockDispatch::default());
    let app = router_with_backend(backend.clone());
    for method in ["POST", "GET", "DELETE", "OPTIONS"] {
        let (status, headers, _) = response(
            app.clone()
                .oneshot(request(method, "malformed".into()))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            headers["www-authenticate"],
            "Bearer resource_metadata=\"https://lane.example/.well-known/oauth-protected-resource/mcp\""
        );
    }
    for method in [
        "initialize",
        "tools/list",
        "ping",
        "tools/call",
        "notifications/initialized",
    ] {
        assert_eq!(
            send(app.clone(), rpc(method, json!({}))).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(backend.auth_calls.load(Ordering::SeqCst), 9);
    assert!(backend.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn initialization_negotiates_supported_version_without_session() {
    let (app, backend) = setup();
    for (requested, expected) in [
        ("2025-03-26", "2025-06-18"),
        ("2025-06-18", "2025-06-18"),
        ("future", "2025-06-18"),
    ] {
        let (status, headers, value) = send(app.clone(), rpc("initialize", json!({"protocolVersion":requested, "capabilities":{}, "clientInfo":{"name":"fixture", "version":"1"}}))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["result"]["protocolVersion"], expected);
        assert_eq!(
            value["result"]["capabilities"]["tools"]["listChanged"],
            false
        );
        assert!(headers.get("mcp-session-id").is_none());
    }
    let (_, _, bad) = send(
        app.clone(),
        rpc("initialize", json!({"protocolVersion":"2025-06-18"})),
    )
    .await;
    assert_eq!(bad["error"]["code"], -32602);
    let (status, _, notification) = send(
        app.clone(),
        json!({"jsonrpc":"2.0", "method":"notifications/initialized"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(notification.is_null());
    assert_eq!(
        send(app, rpc("ping", json!({}))).await.2["result"],
        json!({})
    );
    assert!(backend.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn transport_rejects_unsupported_methods_and_versions() {
    let (app, _) = setup();
    for method in ["GET", "DELETE"] {
        let result = app
            .clone()
            .oneshot(request(method, String::new()))
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(result.headers()["allow"], "POST");
    }
    for (header, value, expected) in [
        (
            "content-type",
            "text/plain",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        ("accept", "application/json", StatusCode::NOT_ACCEPTABLE),
        ("mcp-protocol-version", "future", StatusCode::BAD_REQUEST),
    ] {
        let mut req = request("POST", rpc("ping", json!({})).to_string());
        req.headers_mut().insert(header, value.parse().unwrap());
        assert_eq!(app.clone().oneshot(req).await.unwrap().status(), expected);
    }
}

#[tokio::test]
async fn malformed_rpc_and_notifications_never_dispatch_mutations() {
    let (app, backend) = setup();
    let malformed = app
        .clone()
        .oneshot(request("POST", "{".into()))
        .await
        .unwrap();
    assert_eq!(response(malformed).await.2["error"]["code"], -32700);
    for value in [
        json!([]),
        json!(null),
        json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":null,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":true,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":1,"result":{}}),
    ] {
        assert_eq!(send(app.clone(), value).await.0, StatusCode::BAD_REQUEST);
    }
    let (_, _, unknown) = send(
        app.clone(),
        rpc("lane.v1.LaneService/CreateTask", json!({})),
    )
    .await;
    assert_eq!(unknown["error"]["code"], -32601);
    let (_, _, unknown) = send(
        app.clone(),
        rpc("tools/call", json!({"name":"CreateTask", "arguments":{}})),
    )
    .await;
    assert_eq!(unknown["error"]["code"], -32602);
    let mut notification = rpc("tools/call", json!({"name":"lane_create_task"}));
    notification.as_object_mut().unwrap().remove("id");
    assert_eq!(
        send(app.clone(), notification).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(
            app,
            json!({"jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":1}})
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    assert!(backend.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn catalog_matches_all_canonical_management_methods_and_dispatches_typed_requests() {
    let (app, backend) = setup();
    let (_, _, listed) = send(app.clone(), rpc("tools/list", json!({}))).await;
    let tools = listed["result"]["tools"].as_array().unwrap();
    let proto = include_str!("../../../protos/lane/v1/lane.proto");
    let mut expected = Vec::new();
    let mut service = "";
    for line in proto.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix("service ") {
            service = name.split_whitespace().next().unwrap();
        }
        if service == "SyncService" {
            continue;
        }
        if let Some(method) = line.strip_prefix("rpc ") {
            expected.push((service, method.split('(').next().unwrap()));
        }
    }
    assert_eq!(tools.len(), expected.len());
    assert_eq!(tools.len(), 48);
    for (service, method) in expected {
        let snake = method
            .chars()
            .enumerate()
            .flat_map(|(i, c)| {
                let mut s = String::new();
                if i > 0 && c.is_uppercase() {
                    s.push('_');
                }
                s.push(c.to_ascii_lowercase());
                s.chars().collect::<Vec<_>>()
            })
            .collect::<String>();
        let name = format!("lane_{snake}").replace("_u_r_l_", "_url_");
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert!(
            tool["description"]
                .as_str()
                .unwrap()
                .starts_with("Online-only.")
        );
        assert!(!tool["inputSchema"].to_string().contains("\"origin\""));
        let (_, _, result) = send(
            app.clone(),
            rpc("tools/call", json!({"name":name,"arguments":{}})),
        )
        .await;
        assert_eq!(result["result"]["isError"], false, "{method}: {result}");
        let call = backend.calls.lock().unwrap().last().unwrap().clone();
        assert_eq!(call.0, format!("lane.v1.{service}"));
        assert_eq!(call.1, method);
    }
}

#[tokio::test]
async fn strict_nested_schema_and_generated_decoder_reject_invalid_inputs() {
    let (app, backend) = setup();
    for (name, args) in [
        ("lane_create_task", json!({"origin":{}})),
        (
            "lane_create_task",
            json!({"meta":{"origin":{"kind":"ORIGIN_KIND_APP"}}}),
        ),
        (
            "lane_complete_event",
            json!({"tasks":[{"meta":{"origin":{}}}]}),
        ),
        ("lane_resolve_conflict", json!({"resolved":{"origin":{}}})),
        ("lane_create_task", json!({"progress":"TYPO"})),
        ("lane_create_task", json!({"progress":999})),
        (
            "lane_create_task",
            json!({"repository":{"id":"a", "url":"b"}}),
        ),
        ("lane_list_tasks", json!({"page":{"size":-1}})),
        ("lane_list_tasks", json!({"page":{"size":4294967296_u64}})),
        ("lane_list_tasks", json!({"page":{"unknown":1}})),
        ("lane_list_tasks", json!({"progress":"PROGRESS_TODO"})),
        ("lane_register_push", json!({"publicKey":"%%%"})),
        (
            "lane_reopen_event",
            json!({"expectedRevision":"18446744073709551616"}),
        ),
        ("lane_reopen_event", json!({"expectedRevision":"01"})),
        (
            "lane_resolve_conflict",
            json!({"resolved":{"createdAt":"not-a-timestamp"}}),
        ),
        ("lane_save_task", json!({"task":{"title":42}})),
    ] {
        let (_, _, value) = send(
            app.clone(),
            rpc("tools/call", json!({"name":name, "arguments":args})),
        )
        .await;
        assert_eq!(value["error"]["code"], -32602, "{name}: {value}");
    }
    assert!(backend.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn typed_round_trip_preserves_optional_prompt_and_large_revision() {
    let (app, backend) = setup();
    for args in [json!({"prompt":""}), json!({})] {
        let (_, _, value) = send(
            app.clone(),
            rpc(
                "tools/call",
                json!({"name":"lane_create_task", "arguments":args}),
            ),
        )
        .await;
        assert_eq!(value["result"]["isError"], false);
        assert_eq!(
            backend
                .calls
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .2
                .get("prompt"),
            args.get("prompt")
        );
        assert_eq!(
            serde_json::from_str::<Value>(value["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap(),
            value["result"]["structuredContent"]
        );
    }
    let (_, _, value) = send(app, rpc("tools/call", json!({"name":"lane_reopen_event", "arguments":{"expectedRevision":"18446744073709551615"}}))).await;
    assert_eq!(value["result"]["isError"], false);
    assert_eq!(
        backend.calls.lock().unwrap().last().unwrap().2["expectedRevision"],
        "18446744073709551615"
    );
}

#[tokio::test]
async fn domain_errors_are_tool_results_but_revocation_is_http_401() {
    for revoked in [false, true] {
        let backend = Arc::new(MockDispatch {
            authenticated: true,
            fail: true,
            revoked,
            ..Default::default()
        });
        let (status, headers, value) = send(
            router_with_backend(backend),
            rpc(
                "tools/call",
                json!({"name":"lane_get_task", "arguments":{"id":"fixture"}}),
            ),
        )
        .await;
        if revoked {
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert!(headers.contains_key("www-authenticate"));
        } else {
            assert_eq!(status, StatusCode::OK);
            assert_eq!(value["result"]["isError"], true);
            assert_eq!(
                value["result"]["structuredContent"]["error"]["code"],
                "aborted"
            );
            assert!(value["result"]["structuredContent"]["error"]["correlationId"].is_string());
        }
    }
}

#[tokio::test]
async fn planning_tools_accept_typed_fields_filters_and_edits_and_reject_the_rest() {
    let (app, backend) = setup();
    let valid = [
        (
            "lane_create_task",
            json!({"id":"a","priority":"TASK_PRIORITY_URGENT","type":3,"labelIds":["x"],"startDate":"2026-01-01","dueDate":"2026-01-02","storyPoints":0,"milestoneId":"m","cycleId":"c","parentTaskId":"p","dependsOnTaskIds":["d"],"relatedTaskIds":["r"],"duplicateOfTaskId":"o","relationshipEdits":{"addSubtaskIds":["s"],"removeBlockedTaskIds":["b"]}}),
        ),
        (
            "lane_save_task",
            json!({"id":"a","task":{"priority":"TASK_PRIORITY_LOW","storyPoints":4294967295_u64,"labelIds":[]},"relationshipEdits":{"removeSubtaskIds":["s"],"addBlockedTaskIds":["b"]}}),
        ),
        (
            "lane_list_tasks",
            json!({"filter":{"priorities":["TASK_PRIORITY_HIGH"],"includeUnassignedPriority":true,"types":["TASK_TYPE_BUG"],"labelIds":["l"],"includeUnlabeled":true,"startDate":{"from":"2026-01-01","includeUnassigned":true},"dueDate":{"to":"2026-12-31"},"storyPoints":{"min":0,"max":8},"milestoneIds":["m"],"includeNoMilestone":true,"cycleIds":["c"],"includeNoCycle":true,"relationships":["RELATIONSHIP_KIND_BLOCKS","RELATIONSHIP_KIND_SUBTASKS"],"includeNoRelationships":true},"sortKey":"TASK_SORT_KEY_MILESTONE_NAME","sortDirection":"SORT_DIRECTION_DESCENDING"}),
        ),
        (
            "lane_create_planning_item",
            json!({"id":"l","item":{"label":{"repositoryId":"r","name":"Docs","color":"#AABBCC","description":"d"}}}),
        ),
        (
            "lane_save_planning_item",
            json!({"id":"l","expectedVersionId":"v","item":{"milestone":{"repositoryId":"r","name":"M","targetDate":"2026-01-01","completed":true}}}),
        ),
        (
            "lane_create_planning_item",
            json!({"id":"l","item":{"cycle":{"repositoryId":"r","name":"C","startDate":"2026-01-01","endDate":"2026-01-02"}}}),
        ),
        (
            "lane_list_planning_items",
            json!({"repositoryId":"r","kind":"ENTITY_KIND_CYCLE"}),
        ),
        ("lane_get_planning_item", json!({"id":"l"})),
        ("lane_delete_planning_item", json!({"id":"l"})),
        (
            "lane_resolve_conflict",
            json!({"resolved":{"label":{"repositoryId":"r","name":"Docs","color":"#AABBCC"}}}),
        ),
    ];
    for (name, args) in valid {
        let (_, _, value) = send(
            app.clone(),
            rpc("tools/call", json!({"name":name, "arguments":args})),
        )
        .await;
        assert_eq!(value["result"]["isError"], false, "{name}: {value}");
        // The generated decoder round-trips the arguments, so the backend sees the same content.
        let seen = backend.calls.lock().unwrap().last().unwrap().2.clone();
        assert_eq!(seen["id"], args["id"], "{name}");
    }
    {
        let list = backend.calls.lock().unwrap();
        let filter = &list.iter().find(|c| c.1 == "ListTasks").unwrap().2;
        assert_eq!(filter["sortKey"], "TASK_SORT_KEY_MILESTONE_NAME");
        assert_eq!(filter["filter"]["storyPoints"]["min"], 0);
    }
    for (name, args) in [
        ("lane_list_tasks", json!({"sortKey":"TASK_SORT_KEY_TYPO"})),
        ("lane_list_tasks", json!({"filter":{"unknown":true}})),
        (
            "lane_list_tasks",
            json!({"filter":{"startDate":{"from":1}}}),
        ),
        (
            "lane_list_tasks",
            json!({"filter":{"relationships":["PARENT"]}}),
        ),
        (
            "lane_create_task",
            json!({"priority":"TASK_PRIORITY_EXTREME"}),
        ),
        ("lane_create_task", json!({"storyPoints":-1})),
        (
            "lane_create_task",
            json!({"relationshipEdits":{"addSubtaskIds":"x"}}),
        ),
        (
            "lane_create_task",
            json!({"relationshipEdits":{"origin":{}}}),
        ),
        (
            "lane_create_planning_item",
            json!({"item":{"label":{}, "cycle":{}}}),
        ),
        (
            "lane_create_planning_item",
            json!({"meta":{"origin":{"kind":"ORIGIN_KIND_APP"}}}),
        ),
        (
            "lane_save_planning_item",
            json!({"item":{"label":{"origin":{}}}}),
        ),
        (
            "lane_list_planning_items",
            json!({"kind":"ENTITY_KIND_NOPE"}),
        ),
    ] {
        let calls = backend.calls.lock().unwrap().len();
        let (_, _, value) = send(
            app.clone(),
            rpc("tools/call", json!({"name":name, "arguments":args})),
        )
        .await;
        assert_eq!(value["error"]["code"], -32602, "{name}: {args}: {value}");
        assert_eq!(backend.calls.lock().unwrap().len(), calls);
    }
    // The published schemas expose the planning fields and never an origin.
    let (_, _, listed) = send(app, rpc("tools/list", json!({}))).await;
    let tools = listed["result"]["tools"].as_array().unwrap();
    let schema =
        |name: &str| tools.iter().find(|t| t["name"] == name).unwrap()["inputSchema"].clone();
    let create = schema("lane_create_task");
    for field in [
        "priority",
        "type",
        "labelIds",
        "startDate",
        "dueDate",
        "storyPoints",
        "milestoneId",
        "cycleId",
        "parentTaskId",
        "dependsOnTaskIds",
        "relatedTaskIds",
        "duplicateOfTaskId",
        "relationshipEdits",
    ] {
        assert!(create["properties"].get(field).is_some(), "{field}");
    }
    assert!(
        schema("lane_list_tasks")["properties"]
            .get("filter")
            .is_some()
    );
    assert!(
        schema("lane_list_tasks")["properties"]
            .get("sortKey")
            .is_some()
    );
    assert!(
        schema("lane_save_task")["properties"]
            .get("relationshipEdits")
            .is_some()
    );
    assert!(
        schema("lane_create_planning_item")["properties"]["item"]["properties"]
            .get("milestone")
            .is_some()
    );
    for tool in tools {
        assert!(!tool["inputSchema"].to_string().contains("\"origin\""));
    }
}

// ---- Format-2 capture and URL connection inputs ----

const T: &str = "01900000-0000-7000-8000-000000000001";
const A: &str = "01900000-0000-7000-8000-000000000002";
const R: &str = "01900000-0000-7000-8000-000000000003";
const V: &str = "01900000-0000-7000-8000-000000000004";
const PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

fn camel(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in name.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}
/// Canonical ProtoJSON field names of a oneof-free message in `lane.proto`.
fn proto_fields(message: &str) -> Vec<String> {
    let proto = include_str!("../../../protos/lane/v1/lane.proto");
    let header = format!("message {message} {{");
    let start = proto.find(&header).unwrap_or_else(|| panic!("{message}"));
    let mut names = Vec::new();
    for line in proto[start + header.len()..].lines().skip(1) {
        let line = line.split("//").next().unwrap().trim();
        if line.starts_with('}') {
            break;
        }
        let Some((declaration, _)) = line.split_once('=') else {
            continue;
        };
        names.push(camel(declaration.split_whitespace().last().unwrap()));
    }
    names.sort();
    names
}
fn property_names(schema: &Value) -> Vec<String> {
    let mut names = schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    names.sort();
    names
}
async fn tool_schemas(app: Router) -> Value {
    let (_, _, listed) = send(app, rpc("tools/list", json!({}))).await;
    listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| (tool["name"].clone(), tool["inputSchema"].clone()))
        .fold(json!({}), |mut all, (name, schema)| {
            all[name.as_str().unwrap()] = schema;
            all
        })
}
fn advertised_accepts(schemas: &Value, name: &str, arguments: &Value) -> bool {
    jsonschema::draft202012::new(&schemas[name])
        .unwrap()
        .is_valid(arguments)
}
async fn call(app: &Router, name: &str, arguments: &Value) -> Value {
    send(
        app.clone(),
        rpc("tools/call", json!({"name":name, "arguments":arguments})),
    )
    .await
    .2
}
fn capture_reference(sha256: &str, byte_size: usize) -> Value {
    json!({"attachmentId":A, "width":1, "height":1, "sha256":sha256,
        "byteSize":byte_size.to_string(), "pageUrl":"https://example.com/app/page",
        "pageTitle":"Example page", "cssSelector":"main > img.hero"})
}
fn png_fixture() -> (String, usize) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use sha2::{Digest, Sha256};
    let bytes = STANDARD.decode(PNG_BASE64).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    (hex::encode(Sha256::digest(&bytes)), bytes.len())
}
fn version_fixture(kind: Value, body: (&str, Value)) -> Value {
    let mut version = json!({"formatVersion":2, "entityId":T, "kind":kind, "versionId":V,
        "parentVersionIds":[R, A], "createdAt":"2026-01-02T03:04:05Z", "deleted":false});
    version[body.0] = body.1;
    version
}

#[tokio::test]
async fn capture_tools_advertise_exact_proto_derived_request_properties() {
    let (app, _) = setup();
    let schemas = tool_schemas(app).await;
    for (tool, request) in [
        (
            "lane_begin_attachment_upload",
            "BeginAttachmentUploadRequest",
        ),
        (
            "lane_complete_attachment_upload",
            "CompleteAttachmentUploadRequest",
        ),
        ("lane_create_capture_task", "CreateCaptureTaskRequest"),
        ("lane_get_attachment", "GetAttachmentRequest"),
        ("lane_list_url_connections", "ListURLConnectionsRequest"),
        ("lane_create_url_connection", "CreateURLConnectionRequest"),
        ("lane_save_url_connection", "SaveURLConnectionRequest"),
        ("lane_delete_url_connection", "DeleteURLConnectionRequest"),
    ] {
        assert_eq!(
            property_names(&schemas[tool]),
            proto_fields(request),
            "{tool}"
        );
        assert_eq!(schemas[tool]["additionalProperties"], false, "{tool}");
    }
    let images = &schemas["lane_create_capture_task"]["properties"]["images"];
    assert_eq!(images["type"], "array");
    assert_eq!(
        property_names(&images["items"]),
        proto_fields("CapturedImage")
    );
    let capture = &images["items"]["properties"]["capture"];
    assert_eq!(property_names(capture), proto_fields("CaptureReference"));
    assert_eq!(capture["properties"]["byteSize"]["type"], "string");
    assert_eq!(
        images["items"]["properties"]["png"]["contentEncoding"],
        "base64"
    );
    assert_eq!(
        property_names(&schemas["lane_create_url_connection"]["properties"]["connection"]),
        proto_fields("URLConnection")
    );
    for tool in ["lane_save_task", "lane_resolve_conflict"] {
        let task = if tool == "lane_save_task" {
            &schemas[tool]["properties"]["task"]
        } else {
            &schemas[tool]["properties"]["resolved"]["properties"]["task"]
        };
        assert_eq!(task["properties"]["captures"]["items"], *capture, "{tool}");
    }
    // Every proto field of EntityVersion except the server-owned origin is advertised.
    let resolved = &schemas["lane_resolve_conflict"]["properties"]["resolved"];
    for field in ["urlConnection", "kind", "task", "repository"] {
        assert!(resolved["properties"].get(field).is_some(), "{field}");
    }
    assert!(resolved["properties"].get("origin").is_none());
    assert!(
        resolved["properties"]["kind"]["oneOf"][0]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("ENTITY_KIND_URL_CONNECTION"))
    );
    assert!(
        resolved["properties"]["kind"]["oneOf"][1]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!(3))
    );
}

#[tokio::test]
async fn nonempty_capture_service_inputs_reach_the_backend_with_decoded_values() {
    let (app, backend) = setup();
    let schemas = tool_schemas(app.clone()).await;
    let (sha256, size) = png_fixture();
    let image = json!({"capture":capture_reference(&sha256, size), "png":PNG_BASE64});
    let connection = json!({"prefix":"https://example.com/app/", "repositoryId":A});
    let vectors = [
        (
            "BeginAttachmentUpload",
            "lane_begin_attachment_upload",
            json!({"taskId":T, "attachmentId":A, "epoch":V}),
        ),
        (
            "CompleteAttachmentUpload",
            "lane_complete_attachment_upload",
            json!({"taskId":T, "attachmentId":A, "uploadId":R, "epoch":V}),
        ),
        (
            "CreateCaptureTask",
            "lane_create_capture_task",
            json!({"meta":{"requestId":R}, "id":T, "repositoryId":A, "title":"Capture",
                "description":"Body", "images":[image]}),
        ),
        (
            "GetAttachment",
            "lane_get_attachment",
            json!({"taskId":T, "attachmentId":A}),
        ),
        (
            "ListURLConnections",
            "lane_list_url_connections",
            json!({"page":{"size":1, "cursor":"c"}}),
        ),
        (
            "CreateURLConnection",
            "lane_create_url_connection",
            json!({"meta":{"requestId":R}, "id":T, "connection":connection}),
        ),
        (
            "SaveURLConnection",
            "lane_save_url_connection",
            json!({"meta":{"requestId":R}, "id":T, "expectedVersionId":V, "connection":connection}),
        ),
        (
            "DeleteURLConnection",
            "lane_delete_url_connection",
            json!({"meta":{"requestId":R}, "id":T}),
        ),
    ];
    for (method, name, arguments) in vectors {
        assert!(advertised_accepts(&schemas, name, &arguments), "{name}");
        let before = backend.calls.lock().unwrap().len();
        let result = call(&app, name, &arguments).await;
        assert_eq!(result["result"]["isError"], false, "{name}: {result}");
        let calls = backend.calls.lock().unwrap();
        assert_eq!(calls.len(), before + 1, "{name}");
        let seen = calls.last().unwrap();
        assert_eq!(seen.0, "lane.v1.CaptureService", "{name}");
        assert_eq!(seen.1, method, "{name}");
        assert_eq!(seen.2, arguments, "{name}");
    }
    // The nonempty image keeps every capture field, including optional browser
    // metadata and the uint64 decimal-string byte size, and the PNG bytes.
    let calls = backend.calls.lock().unwrap();
    let created = &calls.iter().find(|c| c.1 == "CreateCaptureTask").unwrap().2;
    let capture = &created["images"][0]["capture"];
    assert_eq!(capture["attachmentId"], A);
    assert_eq!(capture["width"], 1);
    assert_eq!(capture["height"], 1);
    assert_eq!(capture["sha256"], sha256);
    assert_eq!(capture["byteSize"], size.to_string());
    assert_eq!(capture["pageUrl"], "https://example.com/app/page");
    assert_eq!(capture["pageTitle"], "Example page");
    assert_eq!(capture["cssSelector"], "main > img.hero");
    assert_eq!(created["images"][0]["png"], PNG_BASE64);
}

#[tokio::test]
async fn optional_capture_metadata_may_be_absent_or_empty() {
    let (app, backend) = setup();
    let minimal = json!({"attachmentId":A, "width":1, "height":1, "sha256":"00", "byteSize":"5"});
    let empty = json!({"attachmentId":A, "width":1, "height":1, "sha256":"00", "byteSize":"18446744073709551615",
        "pageUrl":"", "pageTitle":"", "cssSelector":""});
    for capture in [minimal, empty] {
        let arguments = json!({"images":[{"capture":capture, "png":""}]});
        let result = call(&app, "lane_create_capture_task", &arguments).await;
        assert_eq!(result["result"]["isError"], false, "{result}");
        // Absent optional strings stay absent and present empty strings stay present.
        let seen = backend.calls.lock().unwrap().last().unwrap().2.clone();
        for key in ["pageUrl", "pageTitle", "cssSelector"] {
            assert_eq!(
                seen["images"][0]["capture"].get(key),
                arguments["images"][0]["capture"].get(key),
                "{key}"
            );
        }
        assert_eq!(
            seen["images"][0]["capture"]["byteSize"],
            arguments["images"][0]["capture"]["byteSize"]
        );
    }
}

#[tokio::test]
async fn task_captures_are_accepted_by_save_task_and_conflict_resolution() {
    let (app, backend) = setup();
    let schemas = tool_schemas(app.clone()).await;
    let (sha256, size) = png_fixture();
    let reference = capture_reference(&sha256, size);
    for captures in [json!([reference.clone()]), json!([])] {
        let task = json!({"repositoryId":A, "title":"T", "captures":captures});
        let resolved = version_fixture(json!("ENTITY_KIND_TASK"), ("task", task.clone()));
        for (name, arguments) in [
            (
                "lane_save_task",
                json!({"id":T, "expectedVersionId":V, "task":task}),
            ),
            (
                "lane_resolve_conflict",
                json!({"id":T, "expectedHeadIds":[V], "resolved":resolved}),
            ),
        ] {
            assert!(advertised_accepts(&schemas, name, &arguments), "{name}");
            let result = call(&app, name, &arguments).await;
            assert_eq!(result["result"]["isError"], false, "{name}: {result}");
            let seen = backend.calls.lock().unwrap().last().unwrap().clone();
            assert_eq!(seen.0, "lane.v1.LaneService");
            let task = if name == "lane_save_task" {
                &seen.2["task"]
            } else {
                &seen.2["resolved"]["task"]
            };
            if captures.as_array().unwrap().is_empty() {
                assert!(
                    task.get("captures")
                        .is_none_or(|c| c.as_array().unwrap().is_empty()),
                    "{name}: {task}"
                );
            } else {
                assert_eq!(task["captures"], captures, "{name}");
            }
        }
    }
    // Tasks that omit captures stay valid.
    let result = call(&app, "lane_save_task", &json!({"task":{"title":"T"}})).await;
    assert_eq!(result["result"]["isError"], false);
}

#[tokio::test]
async fn url_connection_conflict_resolution_accepts_kind_name_and_number() {
    let (app, backend) = setup();
    let schemas = tool_schemas(app.clone()).await;
    for kind in [json!("ENTITY_KIND_URL_CONNECTION"), json!(3)] {
        let resolved = version_fixture(
            kind,
            (
                "urlConnection",
                json!({"prefix":"https://example.com/app/", "repositoryId":A}),
            ),
        );
        let arguments = json!({"id":T, "expectedHeadIds":[V, R], "resolved":resolved});
        assert!(advertised_accepts(
            &schemas,
            "lane_resolve_conflict",
            &arguments
        ));
        let result = call(&app, "lane_resolve_conflict", &arguments).await;
        assert_eq!(result["result"]["isError"], false, "{result}");
        let seen = backend.calls.lock().unwrap().last().unwrap().clone();
        assert_eq!(
            (seen.0.as_str(), seen.1.as_str()),
            ("lane.v1.LaneService", "ResolveConflict")
        );
        let resolved = &seen.2["resolved"];
        assert_eq!(resolved["formatVersion"], 2);
        assert_eq!(resolved["entityId"], T);
        assert_eq!(resolved["versionId"], V);
        assert_eq!(resolved["parentVersionIds"], json!([R, A]));
        assert_eq!(resolved["createdAt"], "2026-01-02T03:04:05Z");
        assert_eq!(
            resolved["urlConnection"],
            json!({"prefix":"https://example.com/app/", "repositoryId":A})
        );
        assert!(
            resolved["kind"] == "ENTITY_KIND_URL_CONNECTION" || resolved["kind"] == 3,
            "{resolved}"
        );
    }
}

#[tokio::test]
async fn entity_version_body_members_are_mutually_exclusive_in_schema_and_runtime() {
    let (app, backend) = setup();
    let schemas = tool_schemas(app.clone()).await;
    let bodies = [
        ("repository", json!({"normalizedUrl":"example.com/a"})),
        ("task", json!({"title":"T"})),
        ("urlConnection", json!({"prefix":"https://example.com/"})),
        ("label", json!({"name":"L"})),
        ("milestone", json!({"name":"M"})),
        ("cycle", json!({"name":"C"})),
    ];
    // Each single body is valid; every pair, and every group of three or more, is not.
    let mut groups = 0;
    for mask in 1_u32..(1 << bodies.len()) {
        let mut resolved = json!({"formatVersion":2, "entityId":T});
        let mut members = 0;
        for (index, (key, body)) in bodies.iter().enumerate() {
            if mask & (1 << index) != 0 {
                resolved[*key] = body.clone();
                members += 1;
            }
        }
        let arguments = json!({"resolved":resolved});
        let before = backend.calls.lock().unwrap().len();
        let result = call(&app, "lane_resolve_conflict", &arguments).await;
        let schema_valid = advertised_accepts(&schemas, "lane_resolve_conflict", &arguments);
        if members == 1 {
            assert_eq!(result["result"]["isError"], false, "{arguments}: {result}");
            assert!(schema_valid, "{arguments}");
            assert_eq!(backend.calls.lock().unwrap().len(), before + 1);
        } else {
            assert_eq!(result["error"]["code"], -32602, "{arguments}: {result}");
            assert!(!schema_valid, "advertised schema accepts {arguments}");
            assert_eq!(backend.calls.lock().unwrap().len(), before);
        }
        groups += 1;
    }
    assert_eq!(groups, 63);
    // The absent oneof stays valid in both layers.
    let none = json!({"resolved":{"formatVersion":2}});
    assert!(advertised_accepts(&schemas, "lane_resolve_conflict", &none));
    assert_eq!(
        call(&app, "lane_resolve_conflict", &none).await["result"]["isError"],
        false
    );
    // The three format-2 bodies named by the capture contract.
    for (first, second, third) in [("repository", "task", "urlConnection")] {
        let all = json!({"resolved":{first:{}, second:{}, third:{}}});
        assert!(!advertised_accepts(&schemas, "lane_resolve_conflict", &all));
        assert_eq!(
            call(&app, "lane_resolve_conflict", &all).await["error"]["code"],
            -32602
        );
    }
}

#[tokio::test]
async fn capture_and_url_connection_inputs_reject_invalid_values_before_dispatch() {
    let (app, backend) = setup();
    let schemas = tool_schemas(app.clone()).await;
    let (sha256, size) = png_fixture();
    let reference = capture_reference(&sha256, size);
    let image = json!({"capture":reference, "png":PNG_BASE64});
    let with = |mut base: Value, key: &str, value: Value| {
        base[key] = value;
        base
    };
    // (tool, arguments, whether the advertised schema must also reject it)
    let cases = vec![
        // Unknown fields at the request, connection, image and capture levels.
        (
            "lane_get_attachment",
            json!({"taskId":T, "unknown":1}),
            true,
        ),
        (
            "lane_create_url_connection",
            json!({"connection":{"prefix":"p", "unknown":1}}),
            true,
        ),
        (
            "lane_create_capture_task",
            json!({"images":[with(image.clone(), "unknown", json!(1))]}),
            true,
        ),
        (
            "lane_create_capture_task",
            json!({"images":[{"capture":with(reference.clone(), "unknown", json!(1))}]}),
            true,
        ),
        (
            "lane_save_task",
            json!({"task":{"captures":[with(reference.clone(), "unknown", json!(1))]}}),
            true,
        ),
        (
            "lane_resolve_conflict",
            json!({"resolved":{"task":{"captures":[with(reference.clone(), "unknown", json!(1))]}}}),
            true,
        ),
        (
            "lane_resolve_conflict",
            json!({"resolved":{"urlConnection":{"unknown":1}}}),
            true,
        ),
        // Server-owned Origin through meta or resolved, at every nesting level.
        (
            "lane_create_url_connection",
            json!({"meta":{"requestId":R, "origin":{"kind":"ORIGIN_KIND_MCP"}}}),
            true,
        ),
        (
            "lane_create_capture_task",
            json!({"meta":{"origin":{"kind":"ORIGIN_KIND_APP"}}}),
            true,
        ),
        (
            "lane_save_url_connection",
            json!({"meta":{"origin":{}}}),
            true,
        ),
        (
            "lane_delete_url_connection",
            json!({"meta":{"origin":{}}}),
            true,
        ),
        ("lane_create_capture_task", json!({"origin":{}}), true),
        (
            "lane_resolve_conflict",
            json!({"resolved":{"kind":3, "origin":{"kind":"ORIGIN_KIND_APP"}, "urlConnection":{}}}),
            true,
        ),
        // Unknown enum names and numbers.
        (
            "lane_resolve_conflict",
            json!({"resolved":{"kind":"ENTITY_KIND_NOPE"}}),
            true,
        ),
        (
            "lane_resolve_conflict",
            json!({"resolved":{"kind":7}}),
            true,
        ),
        // Wrong scalar shapes, including a numeric byteSize and out-of-range values.
        (
            "lane_create_capture_task",
            json!({"images":[{"capture":with(reference.clone(), "byteSize", json!(1))}]}),
            true,
        ),
        (
            "lane_create_capture_task",
            json!({"images":[{"capture":with(reference.clone(), "byteSize", json!("01"))}]}),
            true,
        ),
        (
            "lane_create_capture_task",
            json!({"images":[{"capture":with(reference.clone(), "byteSize", json!("18446744073709551616"))}]}),
            // The pattern cannot express the uint64 maximum; runtime validation does.
            false,
        ),
        (
            "lane_create_capture_task",
            json!({"images":[{"capture":with(reference.clone(), "width", json!(4294967296_u64))}]}),
            true,
        ),
        (
            "lane_create_capture_task",
            json!({"images":[{"capture":with(reference.clone(), "pageUrl", json!(5))}]}),
            true,
        ),
        (
            "lane_create_capture_task",
            json!({"images":{"capture":reference}}),
            true,
        ),
        ("lane_save_task", json!({"task":{"captures":{}}}), true),
        (
            "lane_list_url_connections",
            json!({"page":{"size":-1}}),
            true,
        ),
        // Malformed base64 passes the structural schema and fails typed decoding.
        (
            "lane_create_capture_task",
            json!({"images":[{"png":"%%%not base64"}]}),
            false,
        ),
    ];
    for (name, arguments, schema_rejects) in cases {
        let result = call(&app, name, &arguments).await;
        assert_eq!(
            result["error"]["code"], -32602,
            "{name}: {arguments}: {result}"
        );
        if schema_rejects {
            assert!(
                !advertised_accepts(&schemas, name, &arguments),
                "advertised schema accepts {name}: {arguments}"
            );
        }
    }
    assert!(backend.calls.lock().unwrap().is_empty());
}
