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
