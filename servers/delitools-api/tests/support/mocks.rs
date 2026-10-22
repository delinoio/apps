// SPDX-License-Identifier: FSL-1.1-ALv2
//! Mocked Vercel AI Gateway, delcore and Logto token endpoint for the execution tests. They are
//! in-process HTTP servers with programmable behavior and call recorders; nothing talks to a live
//! provider.
#![allow(dead_code)]
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use bytes::Bytes;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    convert::Infallible,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------------------------
// Gateway
// ---------------------------------------------------------------------------------------------

/// Usage block of a streamed completion. `None` fields are omitted from the wire.
#[derive(Clone, Debug)]
pub struct UsageSpec {
    pub prompt: u64,
    pub completion: u64,
    pub cached: Option<u64>,
    pub created: Option<u64>,
}
impl UsageSpec {
    pub fn full(prompt: u64, completion: u64, cached: u64, created: u64) -> Self {
        Self {
            prompt,
            completion,
            cached: Some(cached),
            created: Some(created),
        }
    }
    fn json(&self) -> Value {
        let mut usage = json!({"prompt_tokens": self.prompt, "completion_tokens": self.completion});
        if let Some(cached) = self.cached {
            usage["prompt_tokens_details"] = json!({"cached_tokens": cached});
        }
        if let Some(created) = self.created {
            usage["cache_creation_input_tokens"] = json!(created);
        }
        usage
    }
}

#[derive(Clone, Debug)]
pub struct StreamReply {
    pub content: String,
    pub generation: String,
    pub usage: Option<UsageSpec>,
    pub finish: &'static str,
}

#[derive(Clone, Debug)]
pub enum Script {
    /// A complete stream.
    Reply(StreamReply),
    /// A non-200 answer before any stream.
    Status(u16),
    /// The first chunk (carrying the generation ID) and then silence until the client leaves.
    Hang { generation: String },
    /// Partial content, then the connection closes without `[DONE]`.
    Truncated { generation: String },
    /// A provider error event inside the stream.
    StreamError { generation: String },
    /// Wait before answering with the inner script.
    Slow(Duration, Box<Script>),
}

pub fn reply(content: &str, generation: &str, usage: Option<UsageSpec>) -> Script {
    Script::Reply(StreamReply {
        content: content.to_owned(),
        generation: generation.to_owned(),
        usage,
        finish: "stop",
    })
}

pub type Responder = Arc<dyn Fn(usize, &Value) -> Script + Send + Sync>;

#[derive(Clone, Debug)]
pub struct RecordedChat {
    pub body: Value,
    pub authorization: String,
}

#[derive(Default)]
pub struct GatewayMock {
    pub url: String,
    responder: Mutex<Option<Responder>>,
    chats: Mutex<Vec<RecordedChat>>,
    pub disconnects: AtomicUsize,
    models: Mutex<Value>,
    endpoints: Mutex<HashMap<String, Value>>,
    pub models_fail: AtomicBool,
    models_auth: Mutex<Vec<String>>,
    generations: Mutex<HashMap<String, Value>>,
    generation_lookups: Mutex<Vec<String>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

struct DisconnectGuard {
    mock: Arc<GatewayMock>,
    completed: bool,
}
impl Drop for DisconnectGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.mock.disconnects.fetch_add(1, Ordering::SeqCst);
        }
    }
}

pub fn catalog_entry(
    id: &str,
    input: &str,
    output: &str,
    read: Option<&str>,
    write: Option<&str>,
) -> Value {
    let mut pricing = json!({"input": input, "output": output});
    if let Some(read) = read {
        pricing["input_cache_read"] = json!(read);
    }
    if let Some(write) = write {
        pricing["input_cache_write"] = json!(write);
    }
    json!({"id": id, "object": "model", "type": "language", "pricing": pricing})
}

pub fn endpoint_entry(
    provider: &str,
    input: &str,
    output: &str,
    read: Option<&str>,
    write: Option<&str>,
) -> Value {
    let mut pricing = json!({"prompt": input, "completion": output});
    if let Some(read) = read {
        pricing["input_cache_read"] = json!(read);
    }
    if let Some(write) = write {
        pricing["input_cache_write"] = json!(write);
    }
    json!({"provider_name": provider, "status": 0, "pricing": pricing})
}

impl GatewayMock {
    pub async fn start() -> Arc<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let mock = Arc::new(Self {
            url,
            models: Mutex::new(json!({"data": []})),
            ..Default::default()
        });
        let app = Router::new()
            .route("/v1/chat/completions", post(chat))
            .route("/v1/models", get(models))
            .route("/v1/models/{creator}/{model}/endpoints", get(endpoints))
            .route("/v1/generation", get(generation))
            .with_state(mock.clone());
        *lock(&mock.task) = Some(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        }));
        mock
    }

    pub fn stop(&self) {
        if let Some(task) = lock(&self.task).take() {
            task.abort();
        }
    }

    /// Program the chat behavior: the closure receives the zero-based call number and the
    /// parsed request body.
    pub fn respond(&self, responder: impl Fn(usize, &Value) -> Script + Send + Sync + 'static) {
        *lock(&self.responder) = Some(Arc::new(responder));
    }
    /// Answer every call from a queue of scripts, repeating the last one.
    pub fn sequence(&self, scripts: Vec<Script>) {
        let queue = Mutex::new(VecDeque::from(scripts));
        self.respond(move |_, _| {
            let mut queue = lock(&queue);
            if queue.len() > 1 {
                queue.pop_front().unwrap()
            } else {
                queue.front().cloned().unwrap_or(Script::Status(500))
            }
        });
    }
    pub fn chats(&self) -> Vec<RecordedChat> {
        lock(&self.chats).clone()
    }
    pub fn chat_count(&self) -> usize {
        lock(&self.chats).len()
    }
    pub fn disconnected(&self) -> usize {
        self.disconnects.load(Ordering::SeqCst)
    }
    /// Publish the catalog (`data` entries) and per-model provider endpoints.
    pub fn set_catalog(&self, entries: Vec<Value>, endpoints: Vec<(&str, Vec<Value>)>) {
        *lock(&self.models) = json!({"object": "list", "data": entries});
        let mut map = lock(&self.endpoints);
        map.clear();
        for (model, list) in endpoints {
            map.insert(
                model.to_owned(),
                json!({"data": {"id": model, "endpoints": list}}),
            );
        }
    }
    pub fn models_auth_headers(&self) -> Vec<String> {
        lock(&self.models_auth).clone()
    }
    pub fn set_generation(&self, id: &str, data: Option<Value>) {
        match data {
            Some(data) => lock(&self.generations).insert(id.to_owned(), data),
            None => lock(&self.generations).remove(id),
        };
    }
    pub fn generation_lookups(&self) -> Vec<String> {
        lock(&self.generation_lookups).clone()
    }
}

/// Gateway `GET /generation` data for a generation with the given native counts.
pub fn generation_data(
    id: &str,
    model: &str,
    prompt: u64,
    completion: u64,
    cached: u64,
    created: u64,
) -> Value {
    json!({
        "id": id, "model": model, "total_cost": 0.5, "market_cost": 0.4, "provider_name": "openai",
        "native_tokens_prompt": prompt, "native_tokens_completion": completion,
        "native_tokens_cached": cached, "native_tokens_cache_creation": created,
    })
}

fn chunk(value: &Value) -> Bytes {
    Bytes::from(format!("data: {value}\n\n"))
}

async fn chat(State(mock): State<Arc<GatewayMock>>, headers: HeaderMap, body: String) -> Response {
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let number = {
        let mut chats = lock(&mock.chats);
        chats.push(RecordedChat {
            body: parsed.clone(),
            authorization: headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned(),
        });
        chats.len() - 1
    };
    let responder = lock(&mock.responder).clone();
    let mut script = responder.map_or(Script::Status(500), |f| f(number, &parsed));
    let mut guard = DisconnectGuard {
        mock: mock.clone(),
        completed: false,
    };
    while let Script::Slow(delay, inner) = script {
        tokio::time::sleep(delay).await;
        script = *inner;
    }
    let response = match script {
        Script::Status(status) => {
            guard.completed = true;
            (
                StatusCode::from_u16(status).unwrap(),
                Json(json!({"error": {"message": "upstream body that must never be logged", "type": "x"}})),
            )
                .into_response()
        }
        Script::Reply(reply) => stream(&mock, move |tx| async move {
            let id = reply.generation;
            let half = reply.content.len() / 2;
            let split = (0..=half)
                .rev()
                .find(|i| reply.content.is_char_boundary(*i))
                .unwrap_or(0);
            let (first, second) = reply.content.split_at(split);
            for (index, piece) in [first, second].into_iter().enumerate() {
                let finish = (index == 1).then_some(reply.finish);
                let value = json!({"id": id, "choices": [{"index": 0, "delta": {"content": piece}, "finish_reason": finish}]});
                if tx.send(Ok(chunk(&value))).await.is_err() {
                    return false;
                }
            }
            if let Some(usage) = &reply.usage {
                let value = json!({"id": id, "choices": [], "usage": usage.json()});
                if tx.send(Ok(chunk(&value))).await.is_err() {
                    return false;
                }
            }
            tx.send(Ok(Bytes::from("data: [DONE]\n\n"))).await.is_ok()
        }),
        Script::Hang { generation } => stream(&mock, move |tx| async move {
            let value = json!({"id": generation, "choices": [{"index": 0, "delta": {"content": "{"}, "finish_reason": null}]});
            let _ = tx.send(Ok(chunk(&value))).await;
            tx.closed().await;
            false
        }),
        Script::Truncated { generation } => stream(&mock, move |tx| async move {
            let value = json!({"id": generation, "choices": [{"index": 0, "delta": {"content": "{\"x\""}, "finish_reason": null}]});
            tx.send(Ok(chunk(&value))).await.is_ok()
        }),
        Script::StreamError { generation } => stream(&mock, move |tx| async move {
            let value = json!({"id": generation, "choices": [{"index": 0, "delta": {"content": "{"}, "finish_reason": null}]});
            let _ = tx.send(Ok(chunk(&value))).await;
            tx.send(Ok(chunk(
                &json!({"error": {"message": "provider overloaded"}}),
            )))
            .await
            .is_ok()
        }),
        Script::Slow(..) => unreachable!("resolved above"),
    };
    guard.completed = true;
    response
}

type Sender = tokio::sync::mpsc::Sender<Result<Bytes, Infallible>>;

/// Stream a body produced by `produce`; it returns `false` when the client went away, which is
/// counted as a disconnect.
fn stream<F, Fut>(mock: &Arc<GatewayMock>, produce: F) -> Response
where
    F: FnOnce(Sender) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = bool> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    let mock = mock.clone();
    tokio::spawn(async move {
        if !produce(tx).await {
            mock.disconnects.fetch_add(1, Ordering::SeqCst);
        }
    });
    Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(ReceiverStream::new(rx)))
        .unwrap()
}

async fn models(State(mock): State<Arc<GatewayMock>>, headers: HeaderMap) -> Response {
    lock(&mock.models_auth).push(
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned(),
    );
    if mock.models_fail.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    Json(lock(&mock.models).clone()).into_response()
}

async fn endpoints(
    State(mock): State<Arc<GatewayMock>>,
    Path((creator, model)): Path<(String, String)>,
) -> Response {
    match lock(&mock.endpoints).get(&format!("{creator}/{model}")) {
        Some(value) => Json(value.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn generation(
    State(mock): State<Arc<GatewayMock>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let id = query.get("id").cloned().unwrap_or_default();
    lock(&mock.generation_lookups).push(id.clone());
    match lock(&mock.generations).get(&id) {
        Some(data) => Json(json!({"data": data})).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"message": "Usage event not found"}})),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------------------------
// Delcore and the Logto token endpoint
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberRole {
    Owner,
    Admin,
    Member,
}

#[derive(Default)]
struct DelcoreState {
    users: HashMap<String, Uuid>,
    members: HashMap<(Uuid, Uuid), MemberRole>,
    tenants: HashSet<Uuid>,
    blocked: HashSet<Uuid>,
    denied: HashSet<(Uuid, String)>,
    authorize_down: bool,
    authorize_calls: Vec<Value>,
    ingest_calls: Vec<Value>,
    accepted: HashMap<String, Value>,
    ingest_failures: u32,
    /// Ingestions to refuse as unauthenticated, as for a revoked or rotated credential.
    ingest_auth_failures: u32,
    /// Ingestions to answer with a success status and an unrelated body.
    ingest_malformed: u32,
    token_scopes: Vec<String>,
    token_down: bool,
    ingest_authorizations: Vec<String>,
    /// Paid checks to let through immediately, then the delay applied to every later paid check.
    paid_delay: Option<(usize, Duration)>,
}

pub struct DelcoreMock {
    pub url: String,
    pub token_url: String,
    state: Mutex<DelcoreState>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl DelcoreMock {
    pub async fn start() -> Arc<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mock = Arc::new(Self {
            url: base.clone(),
            token_url: format!("{base}/oidc/token"),
            state: Mutex::default(),
            task: Mutex::default(),
        });
        let app = Router::new()
            .route("/oidc/token", post(token))
            .route("/delcore.v1.DelcoreService/{method}", post(rpc))
            .with_state(mock.clone());
        *lock(&mock.task) = Some(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        }));
        mock
    }

    pub fn stop(&self) {
        if let Some(task) = lock(&self.task).take() {
            task.abort();
        }
    }

    /// Register a Logto user bearer token for a platform user.
    pub fn add_user(&self, bearer: &str, user: Uuid) {
        lock(&self.state).users.insert(bearer.to_owned(), user);
    }
    pub fn add_tenant(&self, tenant: Uuid) {
        lock(&self.state).tenants.insert(tenant);
    }
    pub fn set_role(&self, tenant: Uuid, user: Uuid, role: MemberRole) {
        let mut state = lock(&self.state);
        state.tenants.insert(tenant);
        state.members.insert((tenant, user), role);
    }
    pub fn remove_member(&self, tenant: Uuid, user: Uuid) {
        lock(&self.state).members.remove(&(tenant, user));
    }
    pub fn set_billing_blocked(&self, tenant: Uuid, blocked: bool) {
        let mut state = lock(&self.state);
        if blocked {
            state.blocked.insert(tenant);
        } else {
            state.blocked.remove(&tenant);
        }
    }
    pub fn deny_capability(&self, tenant: Uuid, capability: &str) {
        lock(&self.state)
            .denied
            .insert((tenant, capability.to_owned()));
    }
    pub fn set_authorize_down(&self, down: bool) {
        lock(&self.state).authorize_down = down;
    }
    /// Let the next `skip` paid authorization checks answer at once, then delay every later paid
    /// check by `delay`. Management checks are never delayed.
    pub fn delay_paid_authorizations_after(&self, skip: usize, delay: Duration) {
        lock(&self.state).paid_delay = Some((skip, delay));
    }
    pub fn set_token_down(&self, down: bool) {
        lock(&self.state).token_down = down;
    }
    /// Fail the next `count` ingestions with a 503.
    pub fn fail_ingests(&self, count: u32) {
        lock(&self.state).ingest_failures = count;
    }
    /// Refuse the next `count` ingestions as unauthenticated (a revoked or rotated credential).
    pub fn fail_ingest_auth(&self, count: u32) {
        lock(&self.state).ingest_auth_failures = count;
    }
    /// Answer the next `count` ingestions with HTTP 200 and a body without acceptance fields.
    pub fn malformed_ingests(&self, count: u32) {
        lock(&self.state).ingest_malformed = count;
    }
    pub fn authorize_calls(&self) -> Vec<Value> {
        lock(&self.state).authorize_calls.clone()
    }
    pub fn ingest_calls(&self) -> Vec<Value> {
        lock(&self.state).ingest_calls.clone()
    }
    /// Distinct accepted events by event key.
    pub fn accepted(&self) -> HashMap<String, Value> {
        lock(&self.state).accepted.clone()
    }
    pub fn token_scopes(&self) -> Vec<String> {
        lock(&self.state).token_scopes.clone()
    }
    pub fn ingest_authorizations(&self) -> Vec<String> {
        lock(&self.state).ingest_authorizations.clone()
    }
}

async fn token(State(mock): State<Arc<DelcoreMock>>, headers: HeaderMap, body: String) -> Response {
    let mut state = lock(&mock.state);
    if state.token_down {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if !headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("Basic "))
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let scope = body
        .split('&')
        .find_map(|pair| pair.strip_prefix("scope="))
        .unwrap_or("")
        .replace("%3A", ":");
    state.token_scopes.push(scope.clone());
    Json(
        json!({"access_token": format!("m2m-{scope}"), "expires_in": 3600, "token_type": "Bearer"}),
    )
    .into_response()
}

fn connect_error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"code": code, "message": "mock"}))).into_response()
}

async fn rpc(
    State(mock): State<Arc<DelcoreMock>>,
    Path(method): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_owned();
    if method == "Authorize" && body["paid"] == true {
        let delay = {
            let mut state = lock(&mock.state);
            match state.paid_delay.as_mut() {
                Some((skip, _)) if *skip > 0 => {
                    *skip -= 1;
                    None
                }
                Some((_, delay)) => Some(*delay),
                None => None,
            }
        };
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
    }
    let mut state = lock(&mock.state);
    match method.as_str() {
        "GetCurrentUser" => match state.users.get(&bearer) {
            Some(user) => Json(json!({"userId": user.to_string(), "tenants": []})).into_response(),
            None => connect_error(StatusCode::UNAUTHORIZED, "unauthenticated"),
        },
        "Authorize" => {
            if bearer != "m2m-authorize" {
                return connect_error(StatusCode::FORBIDDEN, "permission_denied");
            }
            state.authorize_calls.push(body.clone());
            if state.authorize_down {
                return connect_error(StatusCode::SERVICE_UNAVAILABLE, "unavailable");
            }
            authorize(&state, &body)
        }
        "IngestEvent" => {
            state.ingest_authorizations.push(bearer.clone());
            if state.ingest_auth_failures > 0 {
                state.ingest_auth_failures -= 1;
                return connect_error(StatusCode::UNAUTHORIZED, "unauthenticated");
            }
            if bearer != "m2m-usage:write" {
                return connect_error(StatusCode::FORBIDDEN, "permission_denied");
            }
            state.ingest_calls.push(body.clone());
            if state.ingest_failures > 0 {
                state.ingest_failures -= 1;
                return connect_error(StatusCode::SERVICE_UNAVAILABLE, "unavailable");
            }
            if state.ingest_malformed > 0 {
                state.ingest_malformed -= 1;
                return Json(json!({})).into_response();
            }
            let event = &body["event"];
            let key = event["eventKey"].as_str().unwrap_or("").to_owned();
            if key.is_empty() || body["requestKey"] != event["eventKey"] {
                return connect_error(StatusCode::BAD_REQUEST, "invalid_argument");
            }
            if let Some(existing) = state.accepted.get(&key) {
                if existing == &body {
                    // A replay is answered from the retained identity.
                    return Json(json!({"eventId": format!("event-{key}"), "revision": 1}))
                        .into_response();
                }
                return connect_error(StatusCode::CONFLICT, "already_exists");
            }
            state.accepted.insert(key.clone(), body.clone());
            Json(json!({"eventId": format!("event-{key}"), "revision": 1})).into_response()
        }
        _ => connect_error(StatusCode::NOT_FOUND, "unimplemented"),
    }
}

fn authorize(state: &DelcoreState, body: &Value) -> Response {
    let tenant = body["tenantId"]
        .as_str()
        .and_then(|t| Uuid::parse_str(t).ok());
    let user = body["userId"]
        .as_str()
        .and_then(|t| Uuid::parse_str(t).ok());
    let capability = body["capability"].as_str().unwrap_or("");
    let paid = body["paid"].as_bool().unwrap_or(false);
    let mode = body["consumerAuthorization"].as_str().unwrap_or("");
    let deny = |reason: &str| Json(json!({"allowed": false, "reasons": [reason]})).into_response();
    let Some(tenant) = tenant else {
        return connect_error(StatusCode::BAD_REQUEST, "invalid_argument");
    };
    let permitted = match mode {
        "" => {
            user.is_some_and(|u| state.members.contains_key(&(tenant, u)))
                && !state.denied.contains(&(tenant, capability.to_owned()))
        }
        "CONSUMER_AUTHORIZATION_TENANT_SERVICE_KEY" => {
            if user.is_some() || capability.is_empty() {
                return connect_error(StatusCode::BAD_REQUEST, "invalid_argument");
            }
            state.tenants.contains(&tenant)
                && !state.denied.contains(&(tenant, capability.to_owned()))
        }
        "CONSUMER_AUTHORIZATION_PERSONAL_KEY_MANAGEMENT" => {
            if paid || !capability.is_empty() || user.is_none() {
                return connect_error(StatusCode::BAD_REQUEST, "invalid_argument");
            }
            user.is_some_and(|u| state.members.contains_key(&(tenant, u)))
        }
        "CONSUMER_AUTHORIZATION_SERVICE_KEY_MANAGEMENT" => {
            if paid || !capability.is_empty() || user.is_none() {
                return connect_error(StatusCode::BAD_REQUEST, "invalid_argument");
            }
            user.is_some_and(|u| {
                matches!(
                    state.members.get(&(tenant, u)),
                    Some(MemberRole::Owner | MemberRole::Admin)
                )
            })
        }
        _ => return connect_error(StatusCode::BAD_REQUEST, "invalid_argument"),
    };
    if !permitted {
        return deny("BLOCK_REASON_PERMISSION");
    }
    if paid && state.blocked.contains(&tenant) {
        return deny("BLOCK_REASON_BALANCE");
    }
    Json(json!({"allowed": true, "reasons": []})).into_response()
}
