// SPDX-License-Identifier: Apache-2.0
//! The local IPC service of the shared collector.
//!
//! `anydash.v1.LocalCollectorService` runs over the Connect protocol (HTTP/1.1, binary
//! Protobuf) on a Unix domain socket or a Windows named pipe restricted to the current OS user.
//! Requests carry no bearer token; the OS user boundary is the access control.
use crate::{
    connect,
    core::Core,
    error::{Failure, Result},
    keys::DataKey,
    pb::{
        AddConnectionCredentialRequest, AddConnectionCredentialResponse, AddSourcePathRequest,
        AddSourcePathResponse, DisableSourcePathRequest, DisableSourcePathResponse,
        EnableSourcePathRequest, EnableSourcePathResponse, ErrorReason, GetCollectorStatusResponse,
        ListNativeWidgetSnapshotsResponse, ListSourcePathsResponse, LocalTool, LogoutResponse,
        PutNativeWidgetSnapshotsRequest, PutNativeWidgetSnapshotsResponse, QuitResponse,
        RemoveConnectionCredentialRequest, RemoveConnectionCredentialResponse,
        RequestLocalConnectionRefreshRequest, RequestLocalConnectionRefreshResponse,
        RequestLocalSyncResponse, RotateKeyResponse, StartLoginRequest, StartLoginResponse,
    },
    settings::Dirs,
    time,
};
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use prost::Message;
use std::{path::PathBuf, sync::Arc};
pub const SERVICE: &str = "anydash.v1.LocalCollectorService";
const MAX_REQUEST: usize = 1 << 20;
#[must_use]
pub fn socket_path(dirs: &Dirs) -> PathBuf {
    dirs.runtime.join("collector.sock")
}
fn decode<T: Message + Default>(body: &[u8]) -> Result<T> {
    T::decode(body)
        .map_err(|_| Failure::invalid("body", "The request body is not a valid message."))
}
fn tool(value: i32) -> Result<LocalTool> {
    LocalTool::try_from(value)
        .ok()
        .filter(|t| *t != LocalTool::Unspecified)
        .ok_or_else(|| Failure::invalid("tool", "Choose codex or claude-code."))
}
/// A connection ID: a lowercase UUID v7, checked before any API call.
fn connection_id(value: &str) -> Result<&str> {
    uuid::Uuid::try_parse(value)
        .ok()
        .filter(|id| id.get_version_num() == 7 && id.hyphenated().to_string() == value)
        .map(|_| value)
        .ok_or_else(|| {
            Failure::invalid(
                "connection_id",
                "The connection ID must be a lowercase UUID v7.",
            )
        })
}
/// Execute one `LocalCollectorService` method. `running` is false for direct CLI execution.
/// `None` means the method is unknown.
pub const METHODS: [&str; 15] = [
    "GetCollectorStatus",
    "StartLogin",
    "Logout",
    "AddConnectionCredential",
    "RemoveConnectionCredential",
    "ListSourcePaths",
    "AddSourcePath",
    "DisableSourcePath",
    "EnableSourcePath",
    "RotateKey",
    "RequestLocalSync",
    "RequestLocalConnectionRefresh",
    "PutNativeWidgetSnapshots",
    "ListNativeWidgetSnapshots",
    "Quit",
];
pub async fn dispatch(
    core: &Arc<Core>,
    method: &str,
    body: &[u8],
    running: bool,
) -> Option<Result<Vec<u8>>> {
    if !METHODS.contains(&method) {
        return None;
    }
    let now = time::now();
    let result = async {
        Ok(match method {
            "GetCollectorStatus" => GetCollectorStatusResponse { status: Some(core.status(running)?) }.encode_to_vec(),
            "StartLogin" => {
                let r: StartLoginRequest = decode(body)?;
                StartLoginResponse { login: Some(core.start_login(&r.device_name).await?) }.encode_to_vec()
            }
            "Logout" => {
                core.logout().await?;
                LogoutResponse {}.encode_to_vec()
            }
            "AddConnectionCredential" => {
                let r: AddConnectionCredentialRequest = decode(body)?;
                AddConnectionCredentialResponse { connection: Some(core.add_connection(r).await?) }.encode_to_vec()
            }
            "RemoveConnectionCredential" => {
                let r: RemoveConnectionCredentialRequest = decode(body)?;
                core.remove_connection(&r.connection_id).await?;
                RemoveConnectionCredentialResponse {}.encode_to_vec()
            }
            "ListSourcePaths" => ListSourcePathsResponse { paths: core.list_paths()? }.encode_to_vec(),
            "AddSourcePath" => {
                let r: AddSourcePathRequest = decode(body)?;
                AddSourcePathResponse { path: Some(core.add_path(tool(r.tool)?, &r.path)?) }.encode_to_vec()
            }
            "DisableSourcePath" => {
                let r: DisableSourcePathRequest = decode(body)?;
                DisableSourcePathResponse { path: Some(core.set_path_enabled(tool(r.tool)?, &r.path, false)?) }.encode_to_vec()
            }
            "EnableSourcePath" => {
                let r: EnableSourcePathRequest = decode(body)?;
                EnableSourcePathResponse { path: Some(core.set_path_enabled(tool(r.tool)?, &r.path, true)?) }.encode_to_vec()
            }
            "RotateKey" => {
                if core.key_source(true)? == crate::pb::KeySource::Environment {
                    return Err(Failure::invalid(
                        "new-key-env",
                        "The key comes from ANYDASH_ENCRYPTION_KEY; stop the collector and run anydash key rotate --new-key-env VAR.",
                    ));
                }
                let count = core.rotate_key(None, None)?;
                RotateKeyResponse { rotated_at: Some(time::timestamp(now)), secret_count: count }.encode_to_vec()
            }
            "RequestLocalSync" => {
                let next = core.request_local_sync(now)?;
                if !running {
                    core.local_sync(now)?;
                    if core.logged_in()? {
                        core.upload(now).await?;
                    }
                }
                RequestLocalSyncResponse { next_allowed_at: Some(time::timestamp(next)) }.encode_to_vec()
            }
            "RequestLocalConnectionRefresh" => {
                let r: RequestLocalConnectionRefreshRequest = decode(body)?;
                let id = connection_id(&r.connection_id)?;
                // The collector calls the API with its own device token, so a device token
                // refresh stays under its refresh lock. API errors pass through unchanged.
                let next = core.request_connection_refresh(id).await?;
                tracing::info!(event = "connection_refresh_requested", connection_id = %id, next_allowed_at = %time::rfc3339(next));
                RequestLocalConnectionRefreshResponse { next_allowed_at: Some(time::timestamp(next)) }.encode_to_vec()
            }
            "PutNativeWidgetSnapshots" => {
                let r: PutNativeWidgetSnapshotsRequest = decode(body)?;
                core.put_widgets(&r.snapshots, r.replace_all)?;
                PutNativeWidgetSnapshotsResponse {}.encode_to_vec()
            }
            "ListNativeWidgetSnapshots" => ListNativeWidgetSnapshotsResponse { snapshots: core.widgets()? }.encode_to_vec(),
            "Quit" => {
                if !running {
                    return Err(Failure::local(ErrorReason::NotFound, "No collector is running."));
                }
                core.quit()?;
                QuitResponse {}.encode_to_vec()
            }
            _ => return Err(Failure::internal("Unknown method.")),
        })
    }
    .await;
    Some(result)
}
/// Rotate with a key supplied by `--new-key-env`, used only in direct execution.
pub fn rotate_with(core: &Core, key: Option<DataKey>) -> Result<RotateKeyResponse> {
    let count = core.rotate_key(key, None)?;
    Ok(RotateKeyResponse {
        rotated_at: Some(time::timestamp(time::now())),
        secret_count: count,
    })
}
type Response = hyper::Response<Full<Bytes>>;
fn respond(status: u16, body: Vec<u8>, content_type: &str) -> Response {
    hyper::Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .body(Full::new(Bytes::from(body)))
        .unwrap_or_default()
}
async fn handle(
    core: Arc<Core>,
    request: hyper::Request<hyper::body::Incoming>,
) -> std::result::Result<Response, std::convert::Infallible> {
    let path = request.uri().path().to_owned();
    let method = path
        .strip_prefix(&format!("/{SERVICE}/"))
        .map(str::to_owned);
    let (Some(method), true) = (method, request.method() == hyper::Method::POST) else {
        let (status, body) = connect::unimplemented();
        return Ok(respond(status, body, "application/json"));
    };
    let Ok(body) = http_body_util::Limited::new(request.into_body(), MAX_REQUEST)
        .collect()
        .await
    else {
        let (status, body) = connect::encode_error(&Failure::local(
            ErrorReason::PayloadTooLarge,
            "The request is too large.",
        ));
        return Ok(respond(status, body, "application/json"));
    };
    let body = body.to_bytes();
    let started = std::time::Instant::now();
    let result = dispatch(&core, &method, &body, true).await;
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(match result {
        None => {
            let (status, body) = connect::unimplemented();
            respond(status, body, "application/json")
        }
        Some(Ok(bytes)) => {
            tracing::debug!(event = "ipc_call", method, result = "ok", duration_ms);
            respond(200, bytes, connect::CONTENT_TYPE)
        }
        Some(Err(f)) => {
            tracing::info!(event = "ipc_call", method, result = f.reason.as_str_name(), correlation_id = %f.correlation_id, duration_ms);
            let (status, body) = connect::encode_error(&f);
            respond(status, body, "application/json")
        }
    })
}
pub(crate) async fn serve_stream<S>(core: Arc<Core>, stream: S)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = hyper::service::service_fn(move |r| handle(Arc::clone(&core), r));
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
        .await;
}
#[cfg(unix)]
pub struct Listener {
    inner: tokio::net::UnixListener,
    path: PathBuf,
}
#[cfg(unix)]
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
/// Bind the socket. The caller holds the collector lock, so a leftover socket is stale.
#[cfg(unix)]
pub fn bind(dirs: &Dirs) -> Result<Listener> {
    use std::os::unix::fs::PermissionsExt as _;
    crate::store::private_dir(&dirs.runtime)?;
    let path = socket_path(dirs);
    let _ = std::fs::remove_file(&path);
    let inner = tokio::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(Listener { inner, path })
}
#[cfg(unix)]
pub async fn serve(core: Arc<Core>, listener: Listener) {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let me = unsafe { libc::geteuid() };
    loop {
        let Ok((stream, _)) = listener.inner.accept().await else {
            continue;
        };
        if stream.peer_cred().map(|c| c.uid()).ok() != Some(me) {
            tracing::warn!(
                event = "ipc_rejected",
                "A local IPC client of another user was rejected."
            );
            continue;
        }
        tokio::spawn(serve_stream(Arc::clone(&core), stream));
    }
}
#[cfg(windows)]
pub use crate::windows::{Listener, bind, pipe_name, serve};
#[cfg(unix)]
type Stream = tokio::net::UnixStream;
#[cfg(windows)]
type Stream = tokio::net::windows::named_pipe::NamedPipeClient;
/// A client of the running collector.
pub struct Client {
    dirs: Dirs,
    /// The connection opened by `connect`, used by the first call, so that the call does not
    /// depend on opening the endpoint a second time.
    probe: std::sync::Mutex<Option<Stream>>,
}
#[cfg(unix)]
async fn open_stream(dirs: &Dirs) -> std::io::Result<Stream> {
    tokio::net::UnixStream::connect(socket_path(dirs)).await
}
/// How long a client retries opening a busy pipe.
#[cfg(windows)]
const PIPE_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
/// The wait between attempts to open a busy pipe.
#[cfg(windows)]
const PIPE_BUSY_RETRY: std::time::Duration = std::time::Duration::from_millis(50);
#[cfg(windows)]
async fn open_stream(dirs: &Dirs) -> std::io::Result<Stream> {
    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;
    let _ = dirs;
    let name = pipe_name()?;
    // The collector creates its next pipe instance only after a client connects to the current
    // one, so an open in between, or while other clients use every instance, fails with
    // `ERROR_PIPE_BUSY` although the collector runs. Retry it for a bounded time, as Tokio's
    // named pipe client documentation describes; a missing pipe fails immediately.
    let deadline = tokio::time::Instant::now() + PIPE_BUSY_TIMEOUT;
    loop {
        match tokio::net::windows::named_pipe::ClientOptions::new().open(&name) {
            Err(e)
                if e.raw_os_error().and_then(|code| u32::try_from(code).ok())
                    == Some(ERROR_PIPE_BUSY)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(PIPE_BUSY_RETRY).await;
            }
            result => return result,
        }
    }
}
impl Client {
    /// Connect to a running collector, or `None` when none answers.
    pub async fn connect(dirs: &Dirs) -> Option<Self> {
        let probe = open_stream(dirs).await.ok()?;
        Some(Self {
            dirs: dirs.clone(),
            probe: std::sync::Mutex::new(Some(probe)),
        })
    }
    /// Call a method. Unknown methods return Connect `unimplemented`, which asks the user to
    /// update the older component.
    pub async fn call<Req: Message, Resp: Message + Default>(
        &self,
        method: &str,
        request: &Req,
    ) -> Result<Resp> {
        let unavailable = || {
            Failure::local(
                ErrorReason::DependencyUnavailable,
                "The local collector is unreachable.",
            )
        };
        let probe = self
            .probe
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let stream = match probe {
            Some(stream) => stream,
            None => open_stream(&self.dirs).await.map_err(|_| unavailable())?,
        };
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
                .await
                .map_err(|_| unavailable())?;
        tokio::spawn(connection);
        let http = hyper::Request::builder()
            .method("POST")
            .uri(format!("/{SERVICE}/{method}"))
            .header("Host", "localhost")
            .header("Content-Type", connect::CONTENT_TYPE)
            .header("Connect-Protocol-Version", "1")
            .body(Full::new(Bytes::from(request.encode_to_vec())))
            .map_err(|_| unavailable())?;
        let response = sender.send_request(http).await.map_err(|_| unavailable())?;
        let status = response.status().as_u16();
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|_| unavailable())?
            .to_bytes();
        if status == 200 {
            return Resp::decode(body.as_ref()).map_err(|_| unavailable());
        }
        Err(connect::decode_error(status, &body, None, None))
    }
}
