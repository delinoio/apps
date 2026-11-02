// SPDX-License-Identifier: Apache-2.0
//! Collector operations shared by the local IPC service and direct CLI execution: login,
//! device credentials, connections, leases, provider polling, uploads and status.
use crate::{
    api::{self, Api},
    error::{Failure, Result, new_id},
    keys::{DataKey, Keys, SecureStore},
    pb::{
        self, AcquireLeaseRequest, AcquireLeaseResponse, AddConnectionCredentialRequest, BucketKey,
        BucketOutcome, ClientVersionStatus, CollectionStatus, CollectorKind, CollectorRunState,
        CollectorStatus, Connection, CpuArchitecture, CreateConnectionRequest,
        CreateConnectionResponse, DeviceCredentials, DevicePlatform, EnrollmentState, ErrorReason,
        FailureClass, HeartbeatRequest, HeartbeatResponse, LocalConnectionStatus, LocalNotice,
        LocalTool, LoginProgress, NativeWidgetSnapshot, ParserState, ParserStatus,
        PollEnrollmentRequest, PollEnrollmentResponse, Provider, RefreshDeviceTokenRequest,
        RefreshDeviceTokenResponse, RegisterConnectionDeviceRequest,
        RegisterConnectionDeviceResponse, ReleaseLeaseRequest, ReleaseLeaseResponse,
        RenewLeaseRequest, RenewLeaseResponse, ReportParserStatusRequest,
        ReportParserStatusResponse, ReportSnapshotRequest, ReportSnapshotResponse,
        RequestConnectionRefreshRequest, RequestConnectionRefreshResponse,
        RevokeDeviceCredentialsRequest, RevokeDeviceCredentialsResponse, Snapshot, SourcePath,
        StartEnrollmentRequest, StartEnrollmentResponse, TokenCounts,
        UnregisterConnectionDeviceRequest, UnregisterConnectionDeviceResponse, UploadUsageRequest,
        UploadUsageResponse, UsageBucket,
    },
    providers::{self, Ctx, Endpoints},
    scan, schedule,
    settings::{Dirs, Settings},
    store::{ConnectionRow, Store},
    time,
};
use prost::Message as _;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const CREDENTIALS: &str = "device_credentials";
const MAX_UPLOAD_BUCKETS: usize = 2000;
const MAX_UPLOAD_BYTES: usize = (1 << 20) - 4096;
const TOKEN_REFRESH_MARGIN: i64 = 300;
const MAX_WIDGETS: usize = 100;
/// `ReportSnapshot` limits of the AnyDash API: resources per snapshot and metrics including
/// resource metrics. The request body limit is the upload byte limit.
pub const MAX_SNAPSHOT_RESOURCES: usize = 1000;
pub const MAX_SNAPSHOT_METRICS: usize = 5000;
/// The process that owns the collector lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// A CLI command that holds the lock only for one operation.
    Direct,
    Foreground,
    Service,
    Desktop,
}
impl Owner {
    #[must_use]
    pub const fn kind(self) -> CollectorKind {
        match self {
            Self::Direct => CollectorKind::Unspecified,
            Self::Desktop => CollectorKind::DesktopApp,
            Self::Foreground | Self::Service => CollectorKind::Cli,
        }
    }
}
/// Construction inputs; tests replace the endpoints, the secure store and the API URL.
pub struct Options {
    pub settings: Settings,
    pub endpoints: Endpoints,
    pub os: Arc<dyn SecureStore>,
    pub owner: Owner,
}
#[derive(Default)]
struct State {
    login: Option<LoginProgress>,
    version: Option<ClientVersionStatus>,
    offline: bool,
    sync_requested: bool,
    next_sync: i64,
    next_heartbeat: i64,
    /// The server's heartbeat interval; held leases are renewed at this interval while a
    /// provider poll blocks the scheduler. `None` until the first heartbeat.
    heartbeat_interval: Option<Duration>,
    upload_failures: u32,
    next_upload: i64,
    /// `UploadUsage` rate-limit deadline (`Retry-After`). Local and scheduled syncs still
    /// aggregate before it but do not upload.
    upload_retry_at: i64,
    api_failures: u32,
    next_api: i64,
    passive: Option<(i64, serde_json::Value)>,
    quit: bool,
    /// Registration generation per connection, increased whenever `add_connection` or
    /// `remove_connection` replaces or deletes the connection row. Operations that read a row
    /// and write it back after an await (polls, lease renewal, heartbeats, reports) write only
    /// while the generation is unchanged, so an obsolete result never overwrites a newer
    /// registration, for example by restoring `auth_stopped` after a reconnect.
    registrations: HashMap<String, u64>,
}
/// How a provider poll ended.
enum PollEnd {
    Completed(providers::Outcome),
    /// The polled connection's lease was lost during the poll.
    LeaseLost,
    /// A quit or a stop signal was requested during the poll.
    Stopped,
}
pub struct Core {
    pub settings: Settings,
    pub dirs: Dirs,
    pub owner: Owner,
    pub started_at: i64,
    store: Mutex<Store>,
    keys: Mutex<Option<Keys>>,
    os: Arc<dyn SecureStore>,
    api: Api,
    ctx: Ctx,
    state: Mutex<State>,
    refresh: tokio::sync::Mutex<()>,
    pub wake: tokio::sync::Notify,
    pub stopped: tokio::sync::watch::Sender<bool>,
}
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
#[must_use]
pub const fn platform() -> DevicePlatform {
    if cfg!(target_os = "macos") {
        DevicePlatform::Macos
    } else if cfg!(windows) {
        DevicePlatform::Windows
    } else {
        DevicePlatform::Linux
    }
}
#[must_use]
pub fn architecture() -> CpuArchitecture {
    match std::env::consts::ARCH {
        "aarch64" => CpuArchitecture::Arm64,
        "x86_64" => CpuArchitecture::X64,
        _ => CpuArchitecture::Unspecified,
    }
}
#[must_use]
pub fn os_version() -> String {
    if cfg!(target_os = "linux") {
        let text = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let field = |k: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(k))
                .map(|v| v.trim_matches('"').to_owned())
        };
        return format!(
            "{} {}",
            field("ID=").unwrap_or_else(|| "linux".into()),
            field("VERSION_ID=").unwrap_or_default()
        )
        .trim()
        .to_owned();
    }
    if cfg!(target_os = "macos")
        && let Ok(out) = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
    {
        return format!("macOS {}", String::from_utf8_lossy(&out.stdout).trim());
    }
    std::env::consts::OS.to_owned()
}
#[must_use]
pub fn host_name(settings: &Settings) -> String {
    if let Some(name) = settings.var("COMPUTERNAME") {
        return name.to_owned();
    }
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        // SAFETY: The buffer is valid for its length and gethostname writes at most that many bytes.
        let rc = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
        if rc == 0 {
            let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
            let name = String::from_utf8_lossy(&buffer[..end]).trim().to_owned();
            if !name.is_empty() {
                return name.chars().take(100).collect();
            }
        }
    }
    "AnyDash device".to_owned()
}
fn stored_parsers(store: &Store) -> Result<Vec<ParserStatus>> {
    Ok(hex::decode(store.meta("parsers")?)
        .ok()
        .and_then(|b| ReportParserStatusRequest::decode(b.as_slice()).ok())
        .map_or_else(Vec::new, |r| r.statuses))
}
/// Keep a snapshot within the `ReportSnapshot` limits.
///
/// The API rejects a larger snapshot as a whole with `PAYLOAD_TOO_LARGE`. Account-level
/// metrics are kept first, then
/// resources in provider order while they fit; a `SUCCEEDED` snapshot that loses resources or
/// metrics becomes `PARTIALLY_SUCCEEDED`, so the server does not remove the omitted resources
/// from the latest view. Returns the omitted resource and metric counts.
#[must_use]
pub fn fit_snapshot(snapshot: &mut Snapshot) -> (usize, usize) {
    let mut omitted_metrics = snapshot.metrics.len().saturating_sub(MAX_SNAPSHOT_METRICS);
    snapshot.metrics.truncate(MAX_SNAPSHOT_METRICS);
    let resources = std::mem::take(&mut snapshot.resources);
    let mut metrics = snapshot.metrics.len();
    let mut size = snapshot.encoded_len();
    let mut omitted_resources = 0;
    for resource in resources {
        // A length-delimited field adds a 1-byte tag and a length of up to 3 bytes below 2 MiB.
        let len = resource.encoded_len() + 4;
        if snapshot.resources.len() < MAX_SNAPSHOT_RESOURCES
            && metrics + resource.metrics.len() <= MAX_SNAPSHOT_METRICS
            && size + len <= MAX_UPLOAD_BYTES
        {
            metrics += resource.metrics.len();
            size += len;
            snapshot.resources.push(resource);
        } else {
            omitted_resources += 1;
            omitted_metrics += resource.metrics.len();
        }
    }
    if (omitted_resources > 0 || omitted_metrics > 0)
        && snapshot.status == CollectionStatus::Succeeded as i32
    {
        snapshot.status = CollectionStatus::PartiallySucceeded.into();
    }
    (omitted_resources, omitted_metrics)
}
fn ts_seconds(t: Option<&prost_types::Timestamp>) -> i64 {
    t.map_or(0, |t| t.seconds)
}
impl Core {
    pub fn open(options: Options) -> Result<Arc<Self>> {
        let dirs = options.settings.dirs();
        let store = Store::open(&dirs.data.join("collector.db"))?;
        // An update-required flag belongs to the version that received it.
        let flagged = store.meta("update_required")?;
        if !flagged.is_empty() && flagged != VERSION {
            store.put_meta("update_required", "")?;
        }
        let api = Api::new(&options.settings.api_url())?;
        let ctx = Ctx::new(
            options.endpoints,
            options.settings.clone(),
            options.os.clone(),
        )?;
        let (stopped, _) = tokio::sync::watch::channel(false);
        Ok(Arc::new(Self {
            settings: options.settings,
            dirs,
            owner: options.owner,
            started_at: time::now(),
            store: Mutex::new(store),
            keys: Mutex::new(None),
            os: options.os,
            api,
            ctx,
            state: Mutex::new(State::default()),
            refresh: tokio::sync::Mutex::new(()),
            wake: tokio::sync::Notify::new(),
            stopped,
        }))
    }
    fn store(&self) -> MutexGuard<'_, Store> {
        lock(&self.store)
    }
    /// The data keys, loaded on first use. Lock order: keys before store.
    fn keys(&self) -> Result<MutexGuard<'_, Option<Keys>>> {
        let mut keys = lock(&self.keys);
        if keys.is_none() {
            let mut loaded = Keys::load(&self.settings, &self.dirs, &self.os)?;
            loaded.recover(&self.store())?;
            *keys = Some(loaded);
        }
        Ok(keys)
    }
    /// Run `f` with the data key and the store while the key lock is held, so a concurrent key
    /// rotation cannot replace and delete the key between encryption or decryption and the
    /// store operation. Lock order: keys before store.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "The key guard is held through `f` on purpose."
    )]
    fn with_key<T>(&self, f: impl FnOnce(&DataKey, &Store) -> Result<T>) -> Result<T> {
        let keys = self.keys()?;
        let key = &keys
            .as_ref()
            .ok_or_else(|| {
                Failure::local(
                    ErrorReason::KeyUnavailable,
                    "The encryption key is unavailable.",
                )
            })?
            .key;
        f(key, &self.store())
    }
    /// The registration generation of a connection. Lock order: store before state.
    fn registration(&self, id: &str) -> u64 {
        lock(&self.state)
            .registrations
            .get(id)
            .copied()
            .unwrap_or(0)
    }
    /// Mark a connection row as replaced or deleted. Call while the store lock is held.
    fn new_registration(&self, id: &str) {
        *lock(&self.state)
            .registrations
            .entry(id.to_owned())
            .or_default() += 1;
    }
    /// A connection row with its registration generation.
    fn connection_at(&self, id: &str) -> Result<Option<(ConnectionRow, u64)>> {
        let store = self.store();
        Ok(store
            .connection(id)?
            .map(|row| (row, self.registration(id))))
    }
    /// Every connection row with its registration generation.
    fn connections_at(&self) -> Result<Vec<(ConnectionRow, u64)>> {
        let store = self.store();
        Ok(store
            .connections()?
            .into_iter()
            .map(|row| {
                let generation = self.registration(&row.id);
                (row, generation)
            })
            .collect())
    }
    /// Whether the connection was registered again or removed since `generation` was read.
    fn superseded(&self, id: &str, generation: u64) -> bool {
        let changed = self.registration(id) != generation;
        if changed {
            tracing::info!(
                event = "connection_update_discarded",
                connection_id = %id,
                "The connection was registered again or removed meanwhile."
            );
        }
        changed
    }
    /// Store a row read at `generation` unless the connection was registered again or removed
    /// since then.
    fn save_connection(&self, row: &ConnectionRow, generation: u64) -> Result<()> {
        let store = self.store();
        if self.superseded(&row.id, generation) {
            return Ok(());
        }
        store.put_connection(row)
    }
    /// Delete a connection read at `generation` unless it was registered again since then.
    fn delete_connection_at(&self, id: &str, generation: u64) -> Result<()> {
        let store = self.store();
        if self.superseded(id, generation) {
            return Ok(());
        }
        self.new_registration(id);
        store.delete_connection(id)
    }
    /// Record the last error for `status` and app error screens.
    pub fn record_error(&self, f: &Failure) {
        let store = self.store();
        let _ = store.put_meta("last_error_correlation", &f.correlation_id);
        let _ = store.put_meta("last_error_reason", f.reason.as_str_name());
        let _ = store.put_meta("last_error_at", &time::now().to_string());
    }
    pub fn device_id(&self) -> Result<String> {
        self.store().meta("device_id")
    }
    fn credentials(&self) -> Result<Option<DeviceCredentials>> {
        let bytes = self.with_key(|key, store| store.secret(key, CREDENTIALS))?;
        Ok(bytes.and_then(|b| DeviceCredentials::decode(b.as_slice()).ok()))
    }
    fn save_credentials(&self, c: &DeviceCredentials) -> Result<()> {
        self.with_key(|key, store| store.put_secret(key, CREDENTIALS, &c.encode_to_vec()))
    }
    pub fn logged_in(&self) -> Result<bool> {
        let store = self.store();
        Ok(store.has_secret(CREDENTIALS)? && store.meta("credentials_inactive")?.is_empty())
    }
    fn update_required(&self) -> bool {
        self.store()
            .meta("update_required")
            .is_ok_and(|v| !v.is_empty())
    }
    /// Handle API failure side effects: update-required, inactive credentials and offline state.
    fn api_failure(&self, f: &Failure) {
        match f.reason {
            ErrorReason::UpdateRequired => {
                let _ = self.store().put_meta("update_required", VERSION);
                tracing::warn!(
                    event = "update_required",
                    minimum_version = %f.detail.minimum_client_version,
                    "Update the AnyDash collector through its install channel (install script, npm or desktop app update)."
                );
            }
            ErrorReason::DeviceCredentialsInactive | ErrorReason::RefreshTokenReused => {
                let _ = self
                    .store()
                    .put_meta("credentials_inactive", f.reason.as_str_name());
                tracing::warn!(
                    event = "credentials_inactive",
                    reason = f.reason.as_str_name(),
                    "Run anydash login again."
                );
            }
            _ => {}
        }
        lock(&self.state).offline = f.transport;
        self.record_error(f);
    }
    /// A valid device access token, refreshed shortly before expiry. The new refresh token is
    /// stored before the old one is discarded.
    pub async fn access_token(&self) -> Result<String> {
        let _guard = self.refresh.lock().await;
        let creds = self
            .credentials()?
            .filter(|_| {
                self.store()
                    .meta("credentials_inactive")
                    .is_ok_and(|v| v.is_empty())
            })
            .ok_or_else(|| Failure::local(ErrorReason::NotLoggedIn, "Run anydash login first."))?;
        let remaining = ts_seconds(creds.access_token_expires_at.as_ref()) - time::now();
        if remaining > TOKEN_REFRESH_MARGIN {
            return Ok(creds.access_token);
        }
        let request = RefreshDeviceTokenRequest {
            device_id: self.device_id()?,
            refresh_token: creds.refresh_token.clone(),
        };
        let response: RefreshDeviceTokenResponse = self
            .api
            .call("DeviceService", "RefreshDeviceToken", &request, None)
            .await
            .inspect_err(|f| self.api_failure(f))?;
        let fresh = response
            .credentials
            .ok_or_else(|| Failure::internal("The token refresh response has no credentials."))?;
        self.save_credentials(&fresh)?;
        tracing::info!(event = "device_token_refreshed");
        Ok(fresh.access_token)
    }
    /// Call a device-token method of the AnyDash API.
    pub async fn call<Req: prost::Message, Resp: prost::Message + Default>(
        &self,
        service: &str,
        method: &str,
        request: &Req,
    ) -> Result<Resp> {
        if self.update_required() {
            return Err(Failure::local(
                ErrorReason::UpdateRequired,
                "This collector version is no longer supported; update it through its install channel.",
            ));
        }
        let token = self.access_token().await?;
        let result = self.api.call(service, method, request, Some(&token)).await;
        match &result {
            Ok(_) => lock(&self.state).offline = false,
            Err(f) => self.api_failure(f),
        }
        result
    }
    // Login
    pub async fn start_login(self: &Arc<Self>, device_name: &str) -> Result<LoginProgress> {
        if self.logged_in()? {
            return Err(Failure::local(
                ErrorReason::AlreadyLoggedIn,
                "This device is already logged in.",
            ));
        }
        if self.update_required() {
            return Err(Failure::local(
                ErrorReason::UpdateRequired,
                "Update the collector before logging in.",
            ));
        }
        let name = if device_name.trim().is_empty() {
            host_name(&self.settings)
        } else {
            device_name.trim().chars().take(100).collect()
        };
        let request = StartEnrollmentRequest {
            device_name: name,
            platform: platform().into(),
            architecture: architecture().into(),
            collector_kind: if self.owner == Owner::Desktop {
                CollectorKind::DesktopApp
            } else {
                CollectorKind::Cli
            }
            .into(),
            collector_version: VERSION.to_owned(),
            os_version: os_version(),
            previous_device_id: self.device_id()?,
        };
        let response: StartEnrollmentResponse = self
            .api
            .call("DeviceService", "StartEnrollment", &request, None)
            .await
            .inspect_err(|f| self.api_failure(f))?;
        let progress = LoginProgress {
            state: EnrollmentState::Pending.into(),
            user_code: response.user_code.clone(),
            verification_url: response.verification_url.clone(),
            expires_at: response.expires_at,
        };
        lock(&self.state).login = Some(progress.clone());
        tracing::info!(
            event = "enrollment_started",
            reattach = !request.previous_device_id.is_empty()
        );
        let core = Arc::clone(self);
        tokio::spawn(async move { core.poll_enrollment(response).await });
        Ok(progress)
    }
    fn set_login(&self, state: EnrollmentState) {
        if let Some(login) = lock(&self.state).login.as_mut() {
            login.state = state.into();
        }
    }
    async fn poll_enrollment(&self, e: StartEnrollmentResponse) {
        // The server sets the poll interval (5 seconds); polling faster is rate limited.
        let interval = e
            .poll_interval
            .as_ref()
            .and_then(|d| {
                Some(Duration::new(
                    u64::try_from(d.seconds).ok()?,
                    u32::try_from(d.nanos).ok()?,
                ))
            })
            .filter(|d| !d.is_zero())
            .unwrap_or(Duration::from_secs(5));
        let expires = ts_seconds(e.expires_at.as_ref());
        let mut wait = interval;
        loop {
            tokio::time::sleep(wait).await;
            wait = interval;
            if *self.stopped.borrow() {
                return;
            }
            let request = PollEnrollmentRequest {
                enrollment_id: e.enrollment_id.clone(),
                device_code: e.device_code.clone(),
            };
            let result: Result<PollEnrollmentResponse> = self
                .api
                .call("DeviceService", "PollEnrollment", &request, None)
                .await;
            match result {
                Ok(r) if r.state == EnrollmentState::Consumed as i32 => {
                    let (Some(device), Some(creds)) = (r.device, r.credentials) else {
                        self.set_login(EnrollmentState::Invalidated);
                        return;
                    };
                    if self.save_credentials(&creds).is_err() {
                        self.set_login(EnrollmentState::Invalidated);
                        return;
                    }
                    let store = self.store();
                    let _ = store.put_meta("device_id", &device.id);
                    let _ = store.put_meta("credentials_inactive", "");
                    drop(store);
                    self.set_login(EnrollmentState::Consumed);
                    tracing::info!(event = "enrollment_completed", device_id = %device.id);
                    self.wake.notify_one();
                    return;
                }
                Ok(r) if r.state == EnrollmentState::Pending as i32 => {}
                Ok(r) => {
                    self.set_login(
                        EnrollmentState::try_from(r.state).unwrap_or(EnrollmentState::Invalidated),
                    );
                    return;
                }
                Err(f) => match f.reason {
                    ErrorReason::EnrollmentExpired => {
                        self.set_login(EnrollmentState::Expired);
                        return;
                    }
                    ErrorReason::EnrollmentInvalidated
                    | ErrorReason::EnrollmentAlreadyUsed
                    | ErrorReason::Unauthenticated => {
                        self.set_login(EnrollmentState::Invalidated);
                        return;
                    }
                    ErrorReason::RateLimited => wait = f.retry_after.unwrap_or(wait),
                    _ => self.record_error(&f),
                },
            }
            if expires > 0 && time::now() > expires {
                self.set_login(EnrollmentState::Expired);
                return;
            }
        }
    }
    #[must_use]
    pub fn login_progress(&self) -> Option<LoginProgress> {
        lock(&self.state).login.clone()
    }
    /// Revoke and delete the device credentials and release leases. The device ID, the local
    /// buffer and provider credentials remain for reattachment.
    pub async fn logout(&self) -> Result<()> {
        if self.credentials()?.is_none() {
            return Err(Failure::local(
                ErrorReason::NotLoggedIn,
                "This device is not logged in.",
            ));
        }
        self.release_all().await;
        // Releasing leases can rotate the device credentials, and the server revokes only the
        // current refresh token. Hold the refresh lock so no rotation happens between reading
        // the current credentials, revoking them and deleting them.
        let _guard = self.refresh.lock().await;
        let Some(creds) = self.credentials()? else {
            return Err(Failure::local(
                ErrorReason::NotLoggedIn,
                "This device is not logged in.",
            ));
        };
        let request = RevokeDeviceCredentialsRequest {
            device_id: self.device_id()?,
            refresh_token: creds.refresh_token,
        };
        let revoked: Result<RevokeDeviceCredentialsResponse> = self
            .api
            .call("DeviceService", "RevokeDeviceCredentials", &request, None)
            .await;
        if let Err(f) = &revoked {
            tracing::warn!(event = "revoke_failed", correlation_id = %f.correlation_id, "Local credentials are deleted anyway.");
        }
        let store = self.store();
        store.delete_secret(CREDENTIALS)?;
        store.put_meta("credentials_inactive", "")?;
        for mut c in store.connections()? {
            c.lease_held = false;
            store.put_connection(&c)?;
        }
        drop(store);
        lock(&self.state).login = None;
        tracing::info!(event = "logged_out");
        Ok(())
    }
    // Connections
    pub async fn add_connection(&self, r: AddConnectionCredentialRequest) -> Result<Connection> {
        if !self.logged_in()? {
            return Err(Failure::local(
                ErrorReason::NotLoggedIn,
                "Run anydash login first.",
            ));
        }
        let provider = Provider::try_from(r.provider)
            .ok()
            .filter(|p| *p != Provider::Unspecified)
            .ok_or_else(|| Failure::invalid("provider", "Choose a provider."))?;
        let local = providers::uses_local_cli(provider);
        if local && !r.secret.is_empty() {
            return Err(Failure::invalid(
                "secret",
                "This provider uses its own CLI login; do not supply a token.",
            ));
        }
        if !local && r.secret.trim().is_empty() {
            return Err(Failure::invalid(
                "secret",
                "Supply the provider token through stdin or an environment variable.",
            ));
        }
        if provider == Provider::Vercel && r.scope_id.trim().is_empty() {
            return Err(Failure::invalid(
                "scope_id",
                "Vercel connections require --team-id.",
            ));
        }
        let registered = self.registered_locally(&r.connection_id, provider)?;
        let secret = r.secret.trim().to_owned();
        let validation = providers::validate(&self.ctx, provider, &secret, r.scope_id.trim())
            .await
            .map_err(|fail| {
                let mut f = Failure::local(
                    ErrorReason::ProviderCredentialRejected,
                    if local {
                        "The local CLI credential is missing or unreadable; sign in to the provider CLI first."
                    } else {
                        "The provider rejected the credential."
                    },
                );
                (if local { "provider" } else { "secret" }).clone_into(&mut f.detail.field);
                tracing::info!(event = "credential_rejected", provider = providers::provider_name(provider), failure_class = fail.class.as_str_name());
                f
            })?;
        let connection = if r.connection_id.is_empty() {
            let request = CreateConnectionRequest {
                request_id: new_id(),
                provider: provider.into(),
                display_name: r.display_name.trim().to_owned(),
                account_label: validation.account_label.clone(),
                account_fingerprint: validation.fingerprint.clone(),
            };
            let response: CreateConnectionResponse = self
                .call("ConnectionService", "CreateConnection", &request)
                .await?;
            response.connection
        } else {
            let request = RegisterConnectionDeviceRequest {
                connection_id: r.connection_id.clone(),
                account_fingerprint: validation.fingerprint.clone(),
            };
            let response: RegisterConnectionDeviceResponse = self
                .call("ConnectionService", "RegisterConnectionDevice", &request)
                .await?;
            response.connection
        }
        .ok_or_else(|| Failure::internal("The connection response is empty."))?;
        self.check_registered_provider(provider, &connection, registered)
            .await?;
        self.with_key(|key, store| {
            // Polls, renewals and reports in progress must not overwrite the new registration.
            self.new_registration(&connection.id);
            if !local {
                store.put_secret(
                    key,
                    &format!("connection:{}", connection.id),
                    secret.as_bytes(),
                )?;
            }
            store.put_connection(&ConnectionRow {
                id: connection.id.clone(),
                provider: provider.into(),
                display_name: connection.display_name.clone(),
                scope_id: r.scope_id.trim().to_owned(),
                // The server keeps the fingerprint of the first registration; a registration
                // without one adopts it.
                account_fingerprint: if connection.account_fingerprint.is_empty() {
                    validation.fingerprint
                } else {
                    connection.account_fingerprint.clone()
                },
                ..ConnectionRow::default()
            })
        })?;
        tracing::info!(event = "connection_added", connection_id = %connection.id, provider = providers::provider_name(provider));
        self.wake.notify_one();
        Ok(connection)
    }
    /// Whether the connection `id` (empty for a new connection) is registered on this device.
    /// A registration for an existing connection must use the connection's provider: the server
    /// takes the provider from the connection, so another provider's token would be polled and
    /// reported under it.
    fn registered_locally(&self, id: &str, provider: Provider) -> Result<bool> {
        if id.is_empty() {
            return Ok(false);
        }
        let existing = self.store().connection(id)?;
        match existing {
            Some(existing) if existing.provider != provider as i32 => {
                Err(Self::provider_mismatch(provider, existing.provider))
            }
            existing => Ok(existing.is_some()),
        }
    }
    /// Reject a registration whose provider differs from the registered connection's. When this
    /// call made the registration (no local connection row existed), undo it, because the
    /// device stores no credential for the connection.
    async fn check_registered_provider(
        &self,
        provider: Provider,
        connection: &Connection,
        registered_before: bool,
    ) -> Result<()> {
        if connection.provider == provider as i32 {
            return Ok(());
        }
        if !registered_before {
            // A failed unregistration leaves a registration that never becomes ready, because
            // the device never lists the connection as ready.
            let undo = UnregisterConnectionDeviceRequest {
                connection_id: connection.id.clone(),
                device_id: String::new(),
            };
            if let Err(f) = self
                .call::<_, UnregisterConnectionDeviceResponse>(
                    "ConnectionService",
                    "UnregisterConnectionDevice",
                    &undo,
                )
                .await
            {
                tracing::warn!(event = "registration_undo_failed", connection_id = %connection.id, reason = f.reason.as_str_name(), correlation_id = %f.correlation_id);
            }
        }
        Err(Self::provider_mismatch(provider, connection.provider))
    }
    /// `INVALID_ARGUMENT` for a registration whose provider differs from the connection's.
    fn provider_mismatch(requested: Provider, actual: i32) -> Failure {
        let actual = Provider::try_from(actual).unwrap_or(Provider::Unspecified);
        tracing::info!(
            event = "connection_provider_mismatch",
            requested = providers::provider_name(requested),
            actual = providers::provider_name(actual)
        );
        Failure::invalid(
            "provider",
            &format!(
                "The connection belongs to the {} provider; connect it with that provider.",
                providers::provider_name(actual)
            ),
        )
    }
    pub async fn remove_connection(&self, id: &str) -> Result<()> {
        let row = self.store().connection(id)?.ok_or_else(|| {
            Failure::local(
                ErrorReason::NotFound,
                "The connection is not registered on this device.",
            )
        })?;
        if row.lease_held {
            let _: Result<ReleaseLeaseResponse> = self
                .call(
                    "CollectorService",
                    "ReleaseLease",
                    &ReleaseLeaseRequest {
                        connection_id: id.to_owned(),
                        epoch: row.lease_epoch,
                    },
                )
                .await;
        }
        let request = UnregisterConnectionDeviceRequest {
            connection_id: id.to_owned(),
            device_id: String::new(),
        };
        match self
            .call::<_, UnregisterConnectionDeviceResponse>(
                "ConnectionService",
                "UnregisterConnectionDevice",
                &request,
            )
            .await
        {
            Ok(_) => {}
            Err(f) if f.reason == ErrorReason::NotFound => {}
            Err(f) => return Err(f),
        }
        {
            let store = self.store();
            self.new_registration(id);
            store.delete_connection(id)?;
        }
        tracing::info!(event = "connection_removed", connection_id = %id);
        Ok(())
    }
    /// Ask the server for a manual connection refresh using the current access token.
    pub async fn request_connection_refresh(&self, id: &str) -> Result<i64> {
        let response: RequestConnectionRefreshResponse = self
            .call(
                "ConnectionService",
                "RequestConnectionRefresh",
                &RequestConnectionRefreshRequest {
                    connection_id: id.to_owned(),
                },
            )
            .await?;
        Ok(ts_seconds(response.next_allowed_at.as_ref()))
    }
    // Source paths
    pub fn list_paths(&self) -> Result<Vec<SourcePath>> {
        scan::list(&self.settings, &self.store())
    }
    pub fn add_path(&self, tool: LocalTool, path: &str) -> Result<SourcePath> {
        let p = scan::add(&self.settings, &self.store(), tool, path)?;
        self.wake.notify_one();
        Ok(p)
    }
    pub fn set_path_enabled(
        &self,
        tool: LocalTool,
        path: &str,
        enabled: bool,
    ) -> Result<SourcePath> {
        scan::set_enabled(&self.settings, &self.store(), tool, path, enabled)
    }
    // Keys
    pub fn rotate_key(
        &self,
        new_key: Option<DataKey>,
        fault: Option<crate::keys::RotateFault>,
    ) -> Result<u32> {
        let mut keys = self.keys()?;
        let mut store = self.store();
        keys.as_mut()
            .ok_or_else(|| {
                Failure::local(
                    ErrorReason::KeyUnavailable,
                    "The encryption key is unavailable.",
                )
            })?
            .rotate(&mut store, new_key, fault)
    }
    /// The key source; the key is loaded only when secrets exist or `load` is set.
    pub fn key_source(&self, load: bool) -> Result<pb::KeySource> {
        if load || self.store().secret_count()? > 0 {
            return Ok(self
                .keys()?
                .as_ref()
                .map_or(pb::KeySource::Unspecified, |k| k.source));
        }
        Ok(lock(&self.keys)
            .as_ref()
            .map_or(pb::KeySource::Unspecified, |k| k.source))
    }
    // Manual local sync
    pub fn request_local_sync(&self, now: i64) -> Result<i64> {
        let store = self.store();
        let last = store.meta_i64("last_manual_sync")?;
        if let Some(wait) = schedule::manual_wait(last, now) {
            return Err(Failure::rate_limited(
                wait,
                "A local sync was requested less than a minute ago.",
            ));
        }
        store.put_meta("last_manual_sync", &now.to_string())?;
        drop(store);
        lock(&self.state).sync_requested = true;
        self.wake.notify_one();
        Ok(now + schedule::seconds(schedule::MANUAL_REFRESH_INTERVAL))
    }
    // Native widget snapshots
    pub fn put_widgets(&self, snapshots: &[NativeWidgetSnapshot], replace_all: bool) -> Result<()> {
        if snapshots.len() > MAX_WIDGETS {
            return Err(Failure::invalid(
                "snapshots",
                "At most 100 widget snapshots can be stored.",
            ));
        }
        let items: Vec<(String, Vec<u8>)> = snapshots
            .iter()
            .map(|s| {
                let mut s = s.clone();
                if s.sharing_state == pb::SharingState::Ended as i32 {
                    s.summary = None;
                }
                (s.widget_id.clone(), s.encode_to_vec())
            })
            .collect();
        self.store().put_widgets(&items, replace_all)
    }
    pub fn widgets(&self) -> Result<Vec<NativeWidgetSnapshot>> {
        Ok(self
            .store()
            .widgets()?
            .iter()
            .filter_map(|b| NativeWidgetSnapshot::decode(b.as_slice()).ok())
            .collect())
    }
    /// Request a stop. A service-managed collector refuses.
    pub fn quit(&self) -> Result<()> {
        if self.owner == Owner::Service {
            return Err(Failure::local(
                ErrorReason::ServiceManaged,
                "The installed user service runs this collector; run anydash service uninstall to stop it.",
            ));
        }
        lock(&self.state).quit = true;
        let _ = self.stopped.send(true);
        self.wake.notify_one();
        Ok(())
    }
    // Status
    pub fn status(&self, running: bool) -> Result<CollectorStatus> {
        let key_source = self.key_source(false)?;
        let store = self.store();
        let logged_in = store.has_secret(CREDENTIALS)?;
        let inactive = !store.meta("credentials_inactive")?.is_empty();
        let update_required = !store.meta("update_required")?.is_empty();
        let state = lock(&self.state);
        let run_state = if !logged_in {
            CollectorRunState::NotLoggedIn
        } else if inactive {
            CollectorRunState::CredentialsInactive
        } else if update_required {
            CollectorRunState::UpdateRequired
        } else if state.offline || !running {
            CollectorRunState::Offline
        } else {
            CollectorRunState::Running
        };
        let parsers = stored_parsers(&store)?;
        let (pending, oldest) = store.pending_summary()?;
        let version = state.version.clone().or_else(|| {
            hex::decode(store.meta("version_status").ok()?)
                .ok()
                .and_then(|b| ClientVersionStatus::decode(b.as_slice()).ok())
        });
        let mut notices = vec![];
        if version.as_ref().is_some_and(|v| v.update_available) {
            notices.push(LocalNotice::UpdateAvailable.into());
        }
        if update_required || version.as_ref().is_some_and(|v| v.update_required) {
            notices.push(LocalNotice::UpdateRequired.into());
        }
        if parsers
            .iter()
            .any(|p| p.state == ParserState::Partial as i32)
        {
            notices.push(LocalNotice::UnsupportedRecords.into());
        }
        let connections = store
            .connections()?
            .into_iter()
            .map(|c| LocalConnectionStatus {
                credential_present: providers::uses_local_cli(
                    Provider::try_from(c.provider).unwrap_or(Provider::Unspecified),
                ) || store
                    .has_secret(&format!("connection:{}", c.id))
                    .unwrap_or(false),
                connection_id: c.id,
                provider: c.provider,
                display_name: c.display_name,
                lease_held: c.lease_held,
                lease_epoch: c.lease_epoch,
                last_status: c.last_status,
                failure_class: c.failure_class,
                last_success_at: time::optional(c.last_success),
                next_attempt_at: time::optional(c.next_attempt),
                last_error_correlation_id: c.last_error_correlation,
            })
            .collect();
        let reason = store.meta("last_error_reason")?;
        Ok(CollectorStatus {
            run_state: run_state.into(),
            device_id: store.meta("device_id")?,
            collector_version: VERSION.to_owned(),
            owner_kind: if running {
                self.owner.kind()
            } else {
                CollectorKind::Unspecified
            }
            .into(),
            started_at: if running {
                time::optional(self.started_at)
            } else {
                None
            },
            last_local_sync_at: time::optional(store.meta_i64("last_local_sync_at")?),
            last_upload_at: time::optional(store.meta_i64("last_upload_at")?),
            pending_bucket_count: pending,
            oldest_pending_bucket_start: if pending > 0 {
                Some(time::timestamp(oldest))
            } else {
                None
            },
            connections,
            parsers,
            last_error_correlation_id: store.meta("last_error_correlation")?,
            last_error_reason: ErrorReason::from_str_name(&reason)
                .unwrap_or(ErrorReason::Unspecified)
                .into(),
            last_error_at: time::optional(store.meta_i64("last_error_at")?),
            version_status: version,
            notices,
            key_source: key_source.into(),
            autostart_enabled: crate::service::installed(&self.settings),
            login: state.login.clone(),
        })
    }
    // Collection cycles
    /// Scan local session files, update buckets and remember the parser statuses.
    pub fn local_sync(&self, now: i64) -> Result<scan::SyncReport> {
        let store = self.store();
        let report = scan::sync(&self.settings, &store, now)?;
        store.put_meta("last_local_sync_at", &now.to_string())?;
        store.put_meta(
            "parsers",
            &hex::encode(
                ReportParserStatusRequest {
                    statuses: report.parsers.clone(),
                }
                .encode_to_vec(),
            ),
        )?;
        drop(store);
        if let Some(passive) = &report.codex_rate_limits {
            lock(&self.state).passive = Some(passive.clone());
        }
        for p in &report.parsers {
            if p.state == ParserState::Partial as i32 {
                tracing::warn!(
                    event = "parser_partial",
                    tool = scan::tool_name(
                        LocalTool::try_from(p.tool).unwrap_or(LocalTool::Unspecified)
                    ),
                    skipped_record_count = p.skipped_record_count,
                    "Some session records could not be read; update the collector."
                );
            }
        }
        tracing::debug!(
            event = "local_sync",
            files_read = report.files_read,
            changed_buckets = report.changed_buckets
        );
        Ok(report)
    }
    /// Upload changed buckets in chunks of at most 2,000 buckets and 1 MiB, then the parser
    /// statuses. Rejected uploads stay buffered.
    pub async fn upload(&self, now: i64) -> Result<usize> {
        let mut uploaded = 0;
        // A stop request ends a long backlog upload between chunks; the rest stays buffered.
        while !self.stopping() {
            let rows = self.store().pending_buckets(MAX_UPLOAD_BUCKETS)?;
            if rows.is_empty() {
                break;
            }
            let mut chunk = vec![];
            let mut size = 0;
            for row in rows {
                let bucket = UsageBucket {
                    tool: row.tool,
                    model: row.model.clone(),
                    bucket_start: Some(time::timestamp(row.bucket)),
                    version: row.version,
                    session_count: row.value.sessions,
                    tokens: Some(TokenCounts {
                        input_tokens: Some(row.value.tokens.input),
                        output_tokens: Some(row.value.tokens.output),
                        cache_read_tokens: Some(row.value.tokens.cache_read),
                        cache_write_tokens: Some(row.value.tokens.cache_write),
                    }),
                };
                let len = bucket.encoded_len() + 4;
                if size + len > MAX_UPLOAD_BYTES {
                    break;
                }
                size += len;
                chunk.push((row, bucket));
            }
            let request = UploadUsageRequest {
                buckets: chunk.iter().map(|(_, b)| b.clone()).collect(),
            };
            let response: UploadUsageResponse = self
                .call("CollectorService", "UploadUsage", &request)
                .await?;
            let store = self.store();
            for (index, (row, _)) in chunk.iter().enumerate() {
                let outcome = response.results.get(index).map_or(0, |r| r.outcome);
                if outcome == BucketOutcome::Invalid as i32 {
                    tracing::warn!(
                        event = "bucket_rejected",
                        tool = row.tool,
                        bucket_start = row.bucket
                    );
                }
                if let Some(result) = response.results.get(index)
                    && result
                        .key
                        .as_ref()
                        .is_some_and(|k: &BucketKey| k.model != row.model)
                {
                    tracing::warn!(event = "bucket_result_mismatch");
                }
                store.mark_uploaded(row)?;
            }
            store.put_meta("last_upload_at", &now.to_string())?;
            drop(store);
            uploaded += chunk.len();
        }
        let parsers = stored_parsers(&self.store())?;
        if !parsers.is_empty() {
            let _: ReportParserStatusResponse = self
                .call(
                    "CollectorService",
                    "ReportParserStatus",
                    &ReportParserStatusRequest { statuses: parsers },
                )
                .await?;
        }
        if uploaded > 0 {
            tracing::info!(event = "usage_uploaded", bucket_count = uploaded);
        }
        Ok(uploaded)
    }
    fn ready_ids(&self) -> Result<Vec<String>> {
        let rows: Vec<(ConnectionRow, bool)> = {
            let store = self.store();
            store
                .connections()?
                .into_iter()
                .map(|c| {
                    let has = store
                        .has_secret(&format!("connection:{}", c.id))
                        .unwrap_or(false);
                    (c, has)
                })
                .collect()
        };
        Ok(rows
            .into_iter()
            .filter(|(c, has)| providers::ready(&self.ctx, c, *has))
            .map(|(c, _)| c.id)
            .collect())
    }
    /// Send a heartbeat and apply lease assignments, refresh and sync requests.
    pub async fn heartbeat(&self, now: i64) -> Result<HeartbeatResponse> {
        // Only connections known when the request is built can be missing from its assignments.
        let known = self.connections_at()?;
        let request = HeartbeatRequest {
            collector_version: VERSION.to_owned(),
            os_version: os_version(),
            ready_connection_ids: self.ready_ids()?,
        };
        let response: HeartbeatResponse =
            self.call("CollectorService", "Heartbeat", &request).await?;
        if let Some(v) = &response.version_status {
            self.store()
                .put_meta("version_status", &hex::encode(v.encode_to_vec()))?;
            if v.update_available {
                tracing::info!(event = "update_available", latest_version = %v.latest_version, "A new AnyDash collector version is available through its install channel.");
            }
            lock(&self.state).version = Some(v.clone());
        }
        lock(&self.state).heartbeat_interval = Some(
            response
                .heartbeat_interval
                .and_then(|d| u64::try_from(d.seconds).ok())
                .filter(|s| *s > 0)
                .map_or(schedule::HEARTBEAT_INTERVAL, Duration::from_secs),
        );
        let device = self.device_id()?;
        let assigned: Vec<String> = response
            .assignments
            .iter()
            .map(|a| a.connection_id.clone())
            .collect();
        for a in &response.assignments {
            let Some((mut row, generation)) = self.connection_at(&a.connection_id)? else {
                continue;
            };
            match &a.lease {
                Some(l) if l.holder_device_id == device => {
                    row.lease_held = true;
                    row.lease_epoch = l.epoch;
                }
                _ => row.lease_held = false,
            }
            if a.acquire_recommended && !row.lease_held {
                let r: AcquireLeaseResponse = self
                    .call(
                        "CollectorService",
                        "AcquireLease",
                        &AcquireLeaseRequest {
                            connection_id: row.id.clone(),
                        },
                    )
                    .await?;
                if r.granted
                    && let Some(l) = r.lease
                {
                    tracing::info!(event = "lease_acquired", connection_id = %row.id, epoch = l.epoch);
                    row.lease_held = true;
                    row.lease_epoch = l.epoch;
                    row.next_attempt = row.next_attempt.min(now);
                }
            }
            if a.refresh_requested {
                Self::manual_due(&mut row, now);
            }
            self.save_connection(&row, generation)?;
        }
        // Connections deleted on the server or unregistered elsewhere lose their local credential.
        for (row, generation) in known {
            if !assigned.contains(&row.id) {
                tracing::info!(event = "connection_unassigned", connection_id = %row.id);
                self.delete_connection_at(&row.id, generation)?;
            }
        }
        if response.local_sync_requested {
            lock(&self.state).sync_requested = true;
        }
        Ok(response)
    }
    /// A manual refresh makes the connection due now, unless a provider rate limit applies.
    fn manual_due(row: &mut ConnectionRow, now: i64) {
        let rate_limited =
            row.failure_class == FailureClass::RateLimited as i32 && row.next_attempt > now;
        if !rate_limited && schedule::manual_wait(row.last_manual, now).is_none() {
            row.next_attempt = now;
            row.last_manual = now;
        }
    }
    /// Report a snapshot under the held lease epoch. `LEASE_EXPIRED` discards it and returns to
    /// candidate selection; `INVALID_ARGUMENT`, `NOT_FOUND` and `PAYLOAD_TOO_LARGE` discard it,
    /// because retrying the same payload cannot succeed; other failures keep it pending.
    async fn report(&self, row: &mut ConnectionRow, snapshot: Snapshot) -> Result<()> {
        self.store()
            .put_pending_snapshot(&row.id, row.lease_epoch, &snapshot.encode_to_vec())?;
        let request = ReportSnapshotRequest {
            connection_id: row.id.clone(),
            lease_epoch: row.lease_epoch,
            snapshot: Some(snapshot),
        };
        match self
            .call::<_, ReportSnapshotResponse>("CollectorService", "ReportSnapshot", &request)
            .await
        {
            Ok(_) => self.store().delete_pending_snapshot(&row.id),
            Err(f)
                if matches!(
                    f.reason,
                    ErrorReason::LeaseExpired
                        | ErrorReason::InvalidArgument
                        | ErrorReason::NotFound
                        | ErrorReason::PayloadTooLarge
                ) =>
            {
                tracing::info!(event = "snapshot_discarded", connection_id = %row.id, reason = f.reason.as_str_name());
                self.store().delete_pending_snapshot(&row.id)?;
                if f.reason == ErrorReason::LeaseExpired {
                    row.lease_held = false;
                }
                Ok(())
            }
            Err(f) => Err(f),
        }
    }
    /// Poll one connection whose lease this device holds and report the snapshot.
    #[expect(
        clippy::too_many_lines,
        reason = "Lease renewal, polling, scheduling and reporting form one cycle."
    )]
    pub async fn poll_connection(&self, id: &str, now: i64) -> Result<Option<CollectionStatus>> {
        // A reconnect or disconnect through IPC can replace or delete the row while this poll
        // awaits the API or the provider; the poll then writes nothing back.
        let Some((mut row, generation)) = self.connection_at(id)? else {
            return Ok(None);
        };
        let provider = Provider::try_from(row.provider).unwrap_or(Provider::Unspecified);
        if !row.lease_held || row.next_attempt > now {
            return Ok(None);
        }
        // Official tokens stop after authentication expiry until the user reconnects. The device
        // is not ready for the connection meanwhile; a lease it still holds (for example after a
        // failed release) is released so another registered device with a valid token can take
        // over.
        if row.auth_stopped && !providers::uses_local_cli(provider) {
            self.release_lease(&mut row).await;
            self.save_connection(&row, generation)?;
            return Ok(None);
        }
        let renewed: Result<RenewLeaseResponse> = self
            .call(
                "CollectorService",
                "RenewLease",
                &RenewLeaseRequest {
                    connection_id: row.id.clone(),
                    epoch: row.lease_epoch,
                },
            )
            .await;
        match renewed {
            Ok(_) => {}
            Err(f) if f.reason == ErrorReason::LeaseExpired => {
                row.lease_held = false;
                self.store().delete_pending_snapshot(&row.id)?;
                self.save_connection(&row, generation)?;
                return Ok(None);
            }
            Err(f) => return Err(f),
        }
        let secret = self
            .with_key(|key, store| store.secret(key, &format!("connection:{}", row.id)))?
            .and_then(|b| String::from_utf8(b).ok());
        let started = std::time::Instant::now();
        // A poll can outlast the 5-minute lease (sequential requests, each with a 30-second
        // timeout), and the scheduler sends no heartbeat until it completes. Renew the held
        // leases beside the poll; losing the polled lease abandons the poll. A stop request
        // (quit or a stop signal) also abandons it, so the collector releases its leases
        // promptly instead of waiting for the provider. Provider polls only read credentials,
        // and a dropped Grok poll kills its child process.
        let outcome = tokio::select! {
            outcome = providers::poll(&self.ctx, &row, secret.as_deref(), now) => PollEnd::Completed(outcome),
            () = self.keep_leases(&row.id) => PollEnd::LeaseLost,
            () = self.stop_requested() => PollEnd::Stopped,
        };
        let outcome = match outcome {
            PollEnd::Completed(outcome) => outcome,
            PollEnd::LeaseLost => {
                tracing::info!(event = "poll_abandoned", connection_id = %row.id, reason = ErrorReason::LeaseExpired.as_str_name());
                row.lease_held = false;
                self.store().delete_pending_snapshot(&row.id)?;
                self.save_connection(&row, generation)?;
                return Ok(None);
            }
            PollEnd::Stopped => {
                // The lease stays held so that the stopping collector releases it; the
                // connection stays due for the next collector.
                tracing::info!(event = "poll_interrupted", connection_id = %row.id, provider = providers::provider_name(provider));
                return Ok(None);
            }
        };
        // The result belongs to the replaced credential, for example an HTTP 401 of a token the
        // user just replaced; it is neither reported nor stored, and the new registration is
        // polled on its own schedule.
        if self.superseded(&row.id, generation) {
            tracing::info!(event = "poll_abandoned", connection_id = %row.id, provider = providers::provider_name(provider), reason = "connection_registered_again");
            return Ok(None);
        }
        if outcome.skipped {
            row.next_attempt = now + schedule::seconds(schedule::EXTERNAL_INTERVAL);
            self.save_connection(&row, generation)?;
            return Ok(Some(CollectionStatus::AuthExpired));
        }
        let mut snapshot = outcome.snapshot;
        // `now` was taken at the start of the cycle, before the heartbeat that may have acquired
        // this lease, and the server rejects observations earlier than the lease's `acquired_at`.
        // Timestamp the observation when the poll completes, after acquisition and renewal.
        let observed = time::now_timestamp();
        let observed_seconds = observed.seconds;
        snapshot.observed_at = Some(observed);
        let (omitted_resources, omitted_metrics) = fit_snapshot(&mut snapshot);
        if omitted_resources > 0 || omitted_metrics > 0 {
            tracing::warn!(
                event = "snapshot_truncated",
                connection_id = %row.id,
                provider = providers::provider_name(provider),
                omitted_resources,
                omitted_metrics,
                "The snapshot exceeds the AnyDash API limits; the omitted resources keep their previous values."
            );
        }
        let status =
            CollectionStatus::try_from(snapshot.status).unwrap_or(CollectionStatus::Failed);
        let class =
            FailureClass::try_from(snapshot.failure_class).unwrap_or(FailureClass::Unspecified);
        let regular = now + schedule::seconds(schedule::EXTERNAL_INTERVAL);
        // Provider retry waits (`Retry-After`, which is relative to the response, and backoff)
        // start when the poll completes, not at the cycle start: a slow poll or earlier
        // connections of the cycle would otherwise shorten them. `now` is ahead of the clock
        // only when the caller schedules the cycle explicitly.
        let completed = observed_seconds.max(now);
        match status {
            CollectionStatus::Succeeded
            | CollectionStatus::PartiallySucceeded
            | CollectionStatus::NotProvided => {
                row.failures = 0;
                row.next_attempt = regular;
                row.auth_stopped = false;
                row.credential_marker.clear();
                if status != CollectionStatus::NotProvided {
                    row.last_success = now;
                }
            }
            CollectionStatus::RateLimited => {
                row.failures += 1;
                let wait = outcome
                    .retry_after
                    .unwrap_or_else(|| schedule::backoff(row.failures, schedule::jitter()));
                row.next_attempt = completed + schedule::seconds(wait).max(1);
            }
            CollectionStatus::AuthExpired => {
                row.next_attempt = regular;
                if class == FailureClass::AuthExpired && provider != Provider::Grok {
                    row.auth_stopped = true;
                    row.credential_marker.clone_from(&outcome.marker);
                }
            }
            _ => {
                row.failures += 1;
                row.next_attempt = completed
                    + schedule::seconds(schedule::backoff(row.failures, schedule::jitter()));
            }
        }
        row.last_status = status.into();
        row.failure_class = class.into();
        if !matches!(
            status,
            CollectionStatus::Succeeded | CollectionStatus::NotProvided
        ) || class != FailureClass::Unspecified
        {
            let correlation = new_id();
            tracing::warn!(
                event = "provider_poll_failed",
                connection_id = %row.id,
                provider = providers::provider_name(provider),
                failure_class = class.as_str_name(),
                http_status = snapshot.provider_http_status,
                next_attempt = %time::rfc3339(row.next_attempt),
                correlation_id = %correlation
            );
            row.last_error_correlation = correlation;
        } else {
            tracing::info!(
                event = "provider_poll",
                connection_id = %row.id,
                provider = providers::provider_name(provider),
                result = status.as_str_name(),
                duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
            );
        }
        snapshot.next_attempt_at = Some(time::timestamp(row.next_attempt));
        if matches!(
            status,
            CollectionStatus::Succeeded | CollectionStatus::PartiallySucceeded
        ) {
            row.last_active_observed = observed_seconds;
        }
        let result = self.report(&mut row, snapshot).await;
        // This device stays not ready while its CLI belongs to another account or while its
        // official token is stopped after authentication expiry; release the lease so a ready
        // device of the connection can take over.
        let official_stopped = row.auth_stopped && !providers::uses_local_cli(provider);
        if (outcome.account_mismatch || official_stopped) && row.lease_held {
            self.release_lease(&mut row).await;
        }
        self.save_connection(&row, generation)?;
        result.map(|()| Some(status))
    }
    /// Whether a quit or a stop signal was requested.
    #[must_use]
    pub fn stopping(&self) -> bool {
        *self.stopped.borrow()
    }
    /// Complete when a quit or a stop signal is requested.
    async fn stop_requested(&self) {
        let mut stopped = self.stopped.subscribe();
        // The sender lives in `self`, so the wait ends only on a stop request.
        let _ = stopped.wait_for(|stopped| *stopped).await;
    }
    /// Renew every held lease at the heartbeat interval until the lease of `polling` is lost.
    /// Runs beside a provider poll, which blocks the scheduler's heartbeats. Other lost leases
    /// return their connections to candidate selection; other failures are retried at the next
    /// interval while the lease remains valid.
    async fn keep_leases(&self, polling: &str) {
        let period = lock(&self.state)
            .heartbeat_interval
            .unwrap_or(schedule::HEARTBEAT_INTERVAL);
        let mut renewal = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        renewal.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            renewal.tick().await;
            let Ok(rows) = self.connections_at() else {
                continue;
            };
            for (mut row, generation) in rows.into_iter().filter(|(r, _)| r.lease_held) {
                let request = RenewLeaseRequest {
                    connection_id: row.id.clone(),
                    epoch: row.lease_epoch,
                };
                match self
                    .call::<_, RenewLeaseResponse>("CollectorService", "RenewLease", &request)
                    .await
                {
                    // The poll in progress serves a refresh of the polled connection.
                    Ok(r) if r.refresh_requested && row.id != polling => {
                        Self::manual_due(&mut row, time::now());
                        let _ = self.save_connection(&row, generation);
                    }
                    Ok(_) => {}
                    Err(f) if f.reason == ErrorReason::LeaseExpired => {
                        tracing::info!(event = "lease_lost", connection_id = %row.id, epoch = row.lease_epoch);
                        if row.id == polling {
                            return;
                        }
                        row.lease_held = false;
                        let _ = self.store().delete_pending_snapshot(&row.id);
                        let _ = self.save_connection(&row, generation);
                    }
                    Err(f) => {
                        tracing::warn!(event = "lease_renewal_failed", connection_id = %row.id, reason = f.reason.as_str_name(), correlation_id = %f.correlation_id);
                    }
                }
            }
        }
    }
    /// Report a passive Codex observation from local sessions when it is newer than the latest
    /// active observation and the lease is held.
    pub async fn report_passive(&self) -> Result<()> {
        let Some((at, value)) = lock(&self.state).passive.take() else {
            return Ok(());
        };
        let rows = self.connections_at()?;
        // Local sessions belong to the current Codex login; report them only under a connection
        // of that account.
        let login = providers::codex::read(&self.ctx).ok();
        for (mut row, generation) in rows {
            if row.provider == Provider::Codex as i32
                && row.lease_held
                && at > row.last_active_observed
                && login
                    .as_ref()
                    .is_some_and(|l| providers::codex::account_matches(l, &row.account_fingerprint))
            {
                let mut snapshot = providers::codex::map_passive(&value, at);
                snapshot.next_attempt_at = time::optional(row.next_attempt);
                self.report(&mut row, snapshot).await?;
                self.save_connection(&row, generation)?;
            }
        }
        Ok(())
    }
    /// Retry pending snapshots that were not delivered.
    async fn flush_pending(&self) -> Result<()> {
        let rows = self.connections_at()?;
        for (mut row, generation) in rows {
            let Some((epoch, bytes)) = self.store().pending_snapshot(&row.id)? else {
                continue;
            };
            if !row.lease_held || epoch != row.lease_epoch {
                self.store().delete_pending_snapshot(&row.id)?;
                continue;
            }
            if let Ok(snapshot) = Snapshot::decode(bytes.as_slice()) {
                self.report(&mut row, snapshot).await?;
                self.save_connection(&row, generation)?;
            }
        }
        Ok(())
    }
    /// Release every held lease (logout, quit and service uninstall).
    pub async fn release_all(&self) {
        let Ok(rows) = self.connections_at() else {
            return;
        };
        for (mut row, generation) in rows.into_iter().filter(|(r, _)| r.lease_held) {
            self.release_lease(&mut row).await;
            let _ = self.save_connection(&row, generation);
        }
    }
    /// Release one held lease. A failed release leaves the lease to expire on the server.
    async fn release_lease(&self, row: &mut ConnectionRow) {
        let request = ReleaseLeaseRequest {
            connection_id: row.id.clone(),
            epoch: row.lease_epoch,
        };
        let _: Result<ReleaseLeaseResponse> = self
            .call("CollectorService", "ReleaseLease", &request)
            .await;
        row.lease_held = false;
        tracing::info!(event = "lease_released", connection_id = %row.id);
    }
    /// One scheduler pass: local sync and upload every minute, heartbeat every minute and
    /// polls for due connections. API failures back off from 1 to 30 minutes.
    pub async fn cycle(&self, now: i64) {
        if !self.logged_in().unwrap_or(false) {
            return;
        }
        let sync_due = {
            let mut s = lock(&self.state);
            let due = s.sync_requested || now >= s.next_sync;
            if due {
                s.sync_requested = false;
                s.next_sync = now + schedule::seconds(schedule::LOCAL_SYNC_INTERVAL);
            }
            due
        };
        if sync_due && let Err(f) = self.local_sync(now) {
            self.record_error(&f);
        }
        if self.update_required() {
            return;
        }
        let api_ready = lock(&self.state).next_api <= now;
        if !api_ready {
            return;
        }
        let result = async {
            if lock(&self.state).next_heartbeat <= now {
                self.heartbeat(now).await?;
                let mut s = lock(&self.state);
                let interval = s.heartbeat_interval.unwrap_or(schedule::HEARTBEAT_INTERVAL);
                s.next_heartbeat = now + schedule::seconds(interval);
            }
            let upload_due = {
                let s = lock(&self.state);
                (sync_due || s.next_upload <= now) && s.upload_retry_at <= now
            };
            if upload_due {
                match self.upload(now).await {
                    Ok(_) => {
                        let mut s = lock(&self.state);
                        s.upload_failures = 0;
                        s.upload_retry_at = 0;
                        s.next_upload = now + schedule::seconds(schedule::LOCAL_SYNC_INTERVAL);
                    }
                    Err(f) if f.reason == ErrorReason::RateLimited => {
                        let retry_at = now + schedule::seconds(api::wait_for(&f, 1));
                        {
                            let mut s = lock(&self.state);
                            s.upload_retry_at = retry_at;
                            s.next_upload = retry_at;
                        }
                        tracing::info!(event = "upload_rate_limited", correlation_id = %f.correlation_id, next_attempt = %time::rfc3339(retry_at));
                    }
                    Err(f) => return Err(f),
                }
            }
            self.flush_pending().await?;
            self.report_passive().await?;
            let rows = self.store().connections()?;
            for row in rows {
                // A stop request ends the cycle so that the collector releases its leases.
                if self.stopping() {
                    break;
                }
                if row.lease_held && row.next_attempt <= now {
                    self.poll_connection(&row.id, now).await?;
                }
            }
            Ok::<(), Failure>(())
        }
        .await;
        let mut s = lock(&self.state);
        match result {
            Ok(()) => s.api_failures = 0,
            Err(f) => {
                if api::transient(&f) || f.reason == ErrorReason::RateLimited {
                    s.api_failures += 1;
                    s.next_api = now + schedule::seconds(api::wait_for(&f, s.api_failures));
                }
                tracing::info!(event = "cycle_failed", reason = f.reason.as_str_name(), correlation_id = %f.correlation_id, next_attempt = %time::rfc3339(s.next_api));
            }
        }
    }
    #[must_use]
    pub fn quitting(&self) -> bool {
        lock(&self.state).quit
    }
}
#[cfg(test)]
mod tests {
    use super::{CREDENTIALS, Core, Options, Owner};
    use crate::{
        keys::MemoryStore, pb::DeviceCredentials, providers::Endpoints, settings::Settings,
    };
    use prost::Message as _;
    use std::{sync::Arc, time::Duration};
    #[test]
    fn snapshots_are_fitted_to_the_report_limits() {
        use super::{MAX_SNAPSHOT_METRICS, MAX_SNAPSHOT_RESOURCES, MAX_UPLOAD_BYTES, fit_snapshot};
        use crate::pb::{CollectionStatus, MetricObservation, ResourceObservation, Snapshot};
        let resource = |id: usize, metrics: usize| ResourceObservation {
            resource_id: format!("r{id}"),
            metrics: vec![MetricObservation::default(); metrics],
            ..Default::default()
        };
        let succeeded = |resources: Vec<ResourceObservation>| Snapshot {
            status: CollectionStatus::Succeeded.into(),
            metrics: vec![MetricObservation::default(); 3],
            resources,
            ..Default::default()
        };
        // Within the limits nothing changes.
        let mut snapshot = succeeded((0..10).map(|i| resource(i, 2)).collect());
        let before = snapshot.clone();
        assert_eq!(fit_snapshot(&mut snapshot), (0, 0));
        assert_eq!(snapshot, before);
        // Too many resources: the first 1,000 stay and the snapshot becomes partial.
        let mut snapshot = succeeded((0..1200).map(|i| resource(i, 1)).collect());
        assert_eq!(fit_snapshot(&mut snapshot), (200, 200));
        assert_eq!(snapshot.resources.len(), MAX_SNAPSHOT_RESOURCES);
        assert_eq!(snapshot.status, CollectionStatus::PartiallySucceeded as i32);
        // Too many metrics, including resource metrics.
        let mut snapshot = succeeded((0..600).map(|i| resource(i, 10)).collect());
        let (resources, metrics) = fit_snapshot(&mut snapshot);
        let kept: usize = snapshot.metrics.len()
            + snapshot
                .resources
                .iter()
                .map(|r| r.metrics.len())
                .sum::<usize>();
        assert!(kept <= MAX_SNAPSHOT_METRICS && resources > 0 && metrics == resources * 10);
        assert_eq!(kept + metrics, 3 + 6000);
        // The request body stays within the byte limit.
        let mut snapshot = succeeded(
            (0..900)
                .map(|i| {
                    let mut r = resource(i, 1);
                    r.display_name = "x".repeat(2000);
                    r
                })
                .collect(),
        );
        let (resources, _) = fit_snapshot(&mut snapshot);
        assert!(resources > 0);
        assert!(prost::Message::encoded_len(&snapshot) <= MAX_UPLOAD_BYTES);
        // A failure snapshot keeps its status.
        let mut snapshot = Snapshot {
            status: CollectionStatus::Failed.into(),
            metrics: vec![MetricObservation::default(); MAX_SNAPSHOT_METRICS + 1],
            ..Default::default()
        };
        assert_eq!(fit_snapshot(&mut snapshot), (0, 1));
        assert_eq!(snapshot.status, CollectionStatus::Failed as i32);
    }
    #[test]
    fn key_rotation_waits_for_an_encrypted_write() {
        let home = tempfile::tempdir().unwrap();
        let anydash_home = home.path().join("anydash");
        let api = "http://127.0.0.1:9";
        let settings = Settings::with_vars(
            home.path().to_path_buf(),
            &[
                ("ANYDASH_HOME", anydash_home.to_str().unwrap()),
                ("ANYDASH_KEY_SOURCE", "file"),
                ("ANYDASH_API_URL", api),
            ],
        );
        let core = Core::open(Options {
            settings,
            endpoints: Endpoints::mock(api, "grok-not-installed-for-tests"),
            os: Arc::new(MemoryStore::default()),
            owner: Owner::Direct,
        })
        .unwrap();
        let creds = DeviceCredentials {
            access_token: "ada_1".into(),
            ..DeviceCredentials::default()
        };
        // A rotation that starts while a write holds the key must wait for the write, then
        // re-encrypt it; otherwise the write would store ciphertext under the replaced key.
        let rotator = core
            .with_key(|key, store| {
                let rotator = {
                    let core = core.clone();
                    std::thread::spawn(move || core.rotate_key(None, None))
                };
                std::thread::sleep(Duration::from_millis(200));
                assert!(!rotator.is_finished(), "the rotation waits for the write");
                store.put_secret(key, CREDENTIALS, &creds.encode_to_vec())?;
                Ok(rotator)
            })
            .unwrap();
        assert_eq!(
            rotator.join().unwrap().unwrap(),
            1,
            "the write is re-encrypted"
        );
        assert_eq!(core.credentials().unwrap().unwrap().access_token, "ada_1");
    }
}
