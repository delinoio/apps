// SPDX-License-Identifier: Apache-2.0
//! Collector behavior against a mocked AnyDash API: enrollment, device tokens, leases and
//! epochs, snapshots, uploads, offline buffering, update-required handling and the shared
//! process with its local IPC service.
mod support;
use anydash::{
    collector,
    core::{Core, Owner},
    ipc,
    pb::{
        self, AcquireLeaseRequest, AcquireLeaseResponse, AddConnectionCredentialRequest,
        BucketOutcome, BucketResult, CollectionStatus, CollectorRunState, Connection,
        ConnectionAssignment, CreateConnectionRequest, CreateConnectionResponse, Device,
        DeviceCredentials, EnrollmentState, ErrorDetail, ErrorReason, GetCollectorStatusRequest,
        GetCollectorStatusResponse, HeartbeatRequest, HeartbeatResponse, Lease, LocalNotice,
        PollEnrollmentResponse, Provider, QuitRequest, QuitResponse, RefreshDeviceTokenRequest,
        RefreshDeviceTokenResponse, ReleaseLeaseResponse, RenewLeaseRequest, RenewLeaseResponse,
        ReportParserStatusResponse, ReportSnapshotRequest, ReportSnapshotResponse,
        RevokeDeviceCredentialsRequest, RevokeDeviceCredentialsResponse, StartEnrollmentRequest,
        StartEnrollmentResponse, UnregisterConnectionDeviceResponse, UploadUsageRequest,
        UploadUsageResponse,
    },
    store::Tokens,
    time,
};
use prost::Message as _;
use serde_json::json;
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use support::{Mock, Recorded, Reply};
const DEVICE: &str = "01890000-0000-7000-8000-00000000d001";
#[derive(Default)]
struct Api {
    approved: bool,
    epoch: u64,
    holder: String,
    upload: Option<ErrorReason>,
    report: Option<ErrorReason>,
    access_expires: i64,
    /// Expiry of refreshed access tokens; 0 means one hour.
    refreshed_expires: i64,
    refreshes: u32,
    /// Heartbeat interval in seconds; 0 means 60.
    heartbeat_seconds: i64,
    /// When set, a lease expires this long after its acquisition, last heartbeat or renewal,
    /// and `RenewLease` and `ReportSnapshot` then return `LEASE_EXPIRED`.
    lease_ttl: Option<Duration>,
    lease_extended: Option<std::time::Instant>,
}
impl Api {
    fn lease_valid(&self) -> bool {
        self.holder == DEVICE
            && self
                .lease_ttl
                .zip(self.lease_extended)
                .is_none_or(|(ttl, at)| at.elapsed() < ttl)
    }
    fn lease_expired() -> Reply {
        Reply::reason(400, "failed_precondition", ErrorReason::LeaseExpired)
    }
}
fn creds(expires: i64, n: u32) -> DeviceCredentials {
    DeviceCredentials {
        access_token: format!("ada_{n}"),
        access_token_expires_at: Some(time::timestamp(expires)),
        refresh_token: format!("adr_{n}"),
        refresh_token_expires_at: None,
    }
}
#[expect(
    clippy::too_many_lines,
    reason = "The mock serves every collector-facing API method."
)]
fn api(state: Arc<Mutex<Api>>) -> impl Fn(&Recorded) -> Reply + Send + Sync + 'static {
    move |r: &Recorded| {
        // Mocked provider endpoints share the server.
        match r.route() {
            "/v2/teams/team_1" => {
                return Reply::json(200, &json!({"name": "Team", "billing": {"plan": "pro"}}));
            }
            "/v10/projects" => return Reply::json(200, &json!({"projects": [], "pagination": {}})),
            "/v1/billing/charges" => return Reply::text(200, ""),
            _ => {}
        }
        assert_eq!(r.header("x-anydash-client-kind"), Some("CLIENT_KIND_CLI"));
        assert_eq!(
            r.header("x-anydash-client-version"),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(r.header("content-type"), Some("application/proto"));
        let mut s = state.lock().unwrap();
        let method = r.route().trim_start_matches("/anydash.v1.");
        match method {
            "DeviceService/StartEnrollment" => Reply::proto(&StartEnrollmentResponse {
                enrollment_id: "e1".into(),
                device_code: "device-code".into(),
                user_code: "BCDF-GHJK".into(),
                verification_url: "https://anydash.delino.io/connect/e1".into(),
                expires_at: Some(time::timestamp(time::now() + 600)),
                poll_interval: Some(prost_types::Duration {
                    seconds: 0,
                    nanos: 50_000_000,
                }),
            }),
            "DeviceService/PollEnrollment" if !s.approved => {
                Reply::proto(&PollEnrollmentResponse {
                    state: EnrollmentState::Pending.into(),
                    ..Default::default()
                })
            }
            "DeviceService/PollEnrollment" => Reply::proto(&PollEnrollmentResponse {
                state: EnrollmentState::Consumed.into(),
                device: Some(Device {
                    id: DEVICE.into(),
                    ..Default::default()
                }),
                credentials: Some(creds(
                    if s.access_expires == 0 {
                        time::now() + 3600
                    } else {
                        s.access_expires
                    },
                    0,
                )),
            }),
            "DeviceService/RefreshDeviceToken" => {
                s.refreshes += 1;
                Reply::proto(&RefreshDeviceTokenResponse {
                    credentials: Some(creds(
                        if s.refreshed_expires == 0 {
                            time::now() + 3600
                        } else {
                            s.refreshed_expires
                        },
                        s.refreshes,
                    )),
                })
            }
            "DeviceService/RevokeDeviceCredentials" => {
                Reply::proto(&RevokeDeviceCredentialsResponse {})
            }
            "CollectorService/Heartbeat" => {
                if s.lease_valid() {
                    s.lease_extended = Some(std::time::Instant::now());
                }
                Reply::proto(&HeartbeatResponse {
                    server_time: Some(time::timestamp(time::now())),
                    version_status: Some(pb::ClientVersionStatus {
                        minimum_version: "0.0.1".into(),
                        latest_version: "9.9.9".into(),
                        update_available: true,
                        update_required: false,
                    }),
                    assignments: vec![ConnectionAssignment {
                        connection_id: "c1".into(),
                        provider: Provider::Vercel.into(),
                        lease: (s.epoch > 0).then(|| Lease {
                            connection_id: "c1".into(),
                            holder_device_id: s.holder.clone(),
                            epoch: s.epoch,
                            ..Default::default()
                        }),
                        acquire_recommended: s.epoch == 0 || s.holder.is_empty(),
                        refresh_requested: false,
                    }],
                    local_sync_requested: false,
                    heartbeat_interval: Some(prost_types::Duration {
                        seconds: if s.heartbeat_seconds == 0 {
                            60
                        } else {
                            s.heartbeat_seconds
                        },
                        nanos: 0,
                    }),
                })
            }
            "CollectorService/AcquireLease" => {
                s.epoch += 1;
                s.holder = DEVICE.into();
                s.lease_extended = Some(std::time::Instant::now());
                Reply::proto(&AcquireLeaseResponse {
                    granted: true,
                    lease: Some(Lease {
                        connection_id: "c1".into(),
                        holder_device_id: DEVICE.into(),
                        epoch: s.epoch,
                        ..Default::default()
                    }),
                })
            }
            "CollectorService/RenewLease" => {
                let req: RenewLeaseRequest = support::decode(r);
                if req.epoch == s.epoch && s.lease_valid() {
                    s.lease_extended = Some(std::time::Instant::now());
                    Reply::proto(&RenewLeaseResponse {
                        lease: None,
                        refresh_requested: false,
                    })
                } else {
                    Reply::connect_error(
                        400,
                        "failed_precondition",
                        ErrorDetail {
                            reason: ErrorReason::LeaseExpired.into(),
                            correlation_id: "c".into(),
                            current_lease_epoch: s.epoch,
                            ..Default::default()
                        },
                    )
                }
            }
            "CollectorService/ReleaseLease" => {
                s.holder.clear();
                Reply::proto(&ReleaseLeaseResponse {})
            }
            "CollectorService/ReportSnapshot" if !s.lease_valid() => Api::lease_expired(),
            "CollectorService/ReportSnapshot" => s.report.map_or_else(
                || {
                    Reply::proto(&ReportSnapshotResponse {
                        outcome: pb::SnapshotOutcome::LatestUpdated.into(),
                    })
                },
                |reason| Reply::reason(400, "failed_precondition", reason),
            ),
            "CollectorService/UploadUsage" => upload(s.upload, r),
            "CollectorService/ReportParserStatus" => Reply::proto(&ReportParserStatusResponse {}),
            "ConnectionService/CreateConnection" => {
                let req: CreateConnectionRequest = support::decode(r);
                Reply::proto(&CreateConnectionResponse {
                    connection: Some(Connection {
                        id: "c1".into(),
                        provider: req.provider,
                        display_name: "Vercel".into(),
                        account_fingerprint: req.account_fingerprint,
                        ..Default::default()
                    }),
                })
            }
            "ConnectionService/UnregisterConnectionDevice" => {
                Reply::proto(&UnregisterConnectionDeviceResponse { connection: None })
            }
            "ConnectionService/RequestConnectionRefresh" => {
                Reply::proto(&pb::RequestConnectionRefreshResponse {
                    next_allowed_at: Some(time::timestamp(time::now() + 60)),
                })
            }
            other => panic!("unexpected request {other}"),
        }
    }
}
fn upload(failure: Option<ErrorReason>, r: &Recorded) -> Reply {
    if let Some(reason) = failure {
        let unavailable = reason == ErrorReason::DependencyUnavailable;
        return Reply::reason(
            if unavailable { 503 } else { 400 },
            if unavailable {
                "unavailable"
            } else {
                "failed_precondition"
            },
            reason,
        );
    }
    let req: UploadUsageRequest = support::decode(r);
    assert!(req.buckets.len() <= 2000 && r.body.len() <= 1 << 20);
    Reply::proto(&UploadUsageResponse {
        results: req
            .buckets
            .iter()
            .map(|b| BucketResult {
                key: None,
                outcome: BucketOutcome::Applied.into(),
                stored_version: b.version,
            })
            .collect(),
        retention_start: None,
    })
}
async fn logged_in(home: &Path, state: &Arc<Mutex<Api>>, owner: Owner) -> (Mock, Arc<Core>) {
    let mock = Mock::start(api(state.clone())).await;
    let core = support::core(home, &mock, owner, &[]);
    let progress = core.start_login("laptop").await.unwrap();
    assert_eq!(
        (progress.user_code.as_str(), progress.state),
        ("BCDF-GHJK", EnrollmentState::Pending as i32)
    );
    tokio::time::sleep(Duration::from_millis(120)).await;
    state.lock().unwrap().approved = true;
    for _ in 0..50 {
        if core.logged_in().unwrap() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(core.logged_in().unwrap());
    assert_eq!(
        core.login_progress().unwrap().state,
        EnrollmentState::Consumed as i32
    );
    (mock, core)
}
#[tokio::test(flavor = "multi_thread")]
async fn enrollment_logout_keeps_registration_and_relogin_reattaches() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api::default()));
    let (mock, core) = logged_in(home.path(), &state, Owner::Direct).await;
    let start: StartEnrollmentRequest = support::decode(
        &mock
            .recorded()
            .into_iter()
            .find(|r| r.route().ends_with("StartEnrollment"))
            .unwrap(),
    );
    assert_eq!(
        (
            start.device_name.as_str(),
            start.previous_device_id.as_str()
        ),
        ("laptop", "")
    );
    assert_eq!(start.collector_kind, pb::CollectorKind::Cli as i32);
    assert_eq!(core.status(false).unwrap().device_id, DEVICE);
    assert_eq!(
        core.start_login("again").await.unwrap_err().reason,
        ErrorReason::AlreadyLoggedIn
    );
    core.logout().await.unwrap();
    let revoke: RevokeDeviceCredentialsRequest = support::decode(
        &mock
            .recorded()
            .into_iter()
            .find(|r| r.route().ends_with("RevokeDeviceCredentials"))
            .unwrap(),
    );
    assert_eq!(
        (revoke.device_id.as_str(), revoke.refresh_token.as_str()),
        (DEVICE, "adr_0")
    );
    assert!(
        mock.recorded()
            .iter()
            .find(|r| r.route().ends_with("RevokeDeviceCredentials"))
            .unwrap()
            .header("authorization")
            .is_none()
    );
    let status = core.status(false).unwrap();
    assert_eq!(
        (status.run_state, status.device_id.as_str()),
        (CollectorRunState::NotLoggedIn as i32, DEVICE)
    );
    state.lock().unwrap().approved = false;
    mock.clear();
    core.start_login("").await.unwrap();
    let again: StartEnrollmentRequest = support::decode(&mock.recorded()[0]);
    assert_eq!(
        again.previous_device_id, DEVICE,
        "re-login reattaches the retained device ID"
    );
    assert!(
        !again.device_name.is_empty(),
        "the default name is the host name"
    );
}
#[tokio::test(flavor = "multi_thread")]
async fn access_token_is_refreshed_before_expiry_and_stored_first() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api {
        access_expires: time::now() + 3600,
        ..Api::default()
    }));
    let (mock, core) = logged_in(home.path(), &state, Owner::Direct).await;
    assert_eq!(core.access_token().await.unwrap(), "ada_0");
    assert_eq!(
        mock.count("/anydash.v1.DeviceService/RefreshDeviceToken"),
        0
    );
    // A token within the refresh margin is rotated.
    let home2 = tempfile::tempdir().unwrap();
    let state2 = Arc::new(Mutex::new(Api {
        access_expires: time::now() + 120,
        ..Api::default()
    }));
    let (mock2, core2) = logged_in(home2.path(), &state2, Owner::Direct).await;
    assert_eq!(core2.access_token().await.unwrap(), "ada_1");
    let refresh: RefreshDeviceTokenRequest = support::decode(
        &mock2
            .recorded()
            .into_iter()
            .find(|r| r.route().ends_with("RefreshDeviceToken"))
            .unwrap(),
    );
    assert_eq!(refresh.refresh_token, "adr_0");
    // The new pair is durable: a new process reads it without another refresh.
    let reopened = support::core(home2.path(), &mock2, Owner::Direct, &[]);
    assert_eq!(reopened.access_token().await.unwrap(), "ada_1");
    assert_eq!(
        mock2.count("/anydash.v1.DeviceService/RefreshDeviceToken"),
        1
    );
    drop((core, mock));
}
#[tokio::test(flavor = "multi_thread")]
async fn logout_revokes_the_refresh_token_current_after_lease_release() {
    let home = tempfile::tempdir().unwrap();
    // Every access token is within the refresh margin, so each API call rotates the pair.
    let soon = time::now() + 120;
    let state = Arc::new(Mutex::new(Api {
        access_expires: soon,
        refreshed_expires: soon,
        ..Api::default()
    }));
    let (mock, core) = logged_in(home.path(), &state, Owner::Foreground).await;
    vercel_connection(&core).await.unwrap();
    core.heartbeat(time::now()).await.unwrap();
    assert!(core.status(true).unwrap().connections[0].lease_held);
    mock.clear();
    core.logout().await.unwrap();
    assert!(
        mock.recorded()
            .iter()
            .any(|r| r.route().ends_with("ReleaseLease"))
    );
    let refreshes = state.lock().unwrap().refreshes;
    let revoke: RevokeDeviceCredentialsRequest = support::decode(
        &mock
            .recorded()
            .into_iter()
            .find(|r| r.route().ends_with("RevokeDeviceCredentials"))
            .unwrap(),
    );
    assert_eq!(
        revoke.refresh_token,
        format!("adr_{refreshes}"),
        "the refresh token rotated by the lease release is revoked"
    );
    assert!(!core.logged_in().unwrap());
}
#[tokio::test(flavor = "multi_thread")]
async fn refresh_token_reuse_requires_a_new_login() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api {
        access_expires: time::now() + 60,
        ..Api::default()
    }));
    let (mock, core) = logged_in(home.path(), &state, Owner::Foreground).await;
    mock.set(|_| Reply::reason(401, "unauthenticated", ErrorReason::RefreshTokenReused));
    assert_eq!(
        core.access_token().await.unwrap_err().reason,
        ErrorReason::RefreshTokenReused
    );
    assert_eq!(
        core.status(true).unwrap().run_state,
        CollectorRunState::CredentialsInactive as i32
    );
    assert!(!core.logged_in().unwrap());
    let error = core.heartbeat(time::now()).await.unwrap_err();
    assert_eq!(error.reason, ErrorReason::NotLoggedIn);
}
fn vercel_connection(
    core: &Arc<Core>,
) -> impl std::future::Future<Output = anydash::error::Result<Connection>> + '_ {
    core.add_connection(AddConnectionCredentialRequest {
        provider: Provider::Vercel.into(),
        connection_id: String::new(),
        display_name: String::new(),
        secret: "vercel-token".into(),
        scope_id: "team_1".into(),
    })
}
#[tokio::test(flavor = "multi_thread")]
#[expect(
    clippy::too_many_lines,
    reason = "One scenario follows a lease through its lifecycle."
)]
async fn lease_acquire_epoch_reports_and_loss() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api::default()));
    let (mock, core) = logged_in(home.path(), &state, Owner::Foreground).await;
    let connection = vercel_connection(&core).await.unwrap();
    let create: CreateConnectionRequest = support::decode(
        &mock
            .recorded()
            .into_iter()
            .find(|r| r.route().ends_with("CreateConnection"))
            .unwrap(),
    );
    assert_eq!(
        create.account_fingerprint,
        anydash::providers::fingerprint(Provider::Vercel, "team_1")
    );
    assert!(
        !String::from_utf8_lossy(
            &mock
                .recorded()
                .iter()
                .find(|r| r.route().ends_with("CreateConnection"))
                .unwrap()
                .body
        )
        .contains("vercel-token"),
        "secrets never leave the device"
    );
    let now = time::now();
    let heartbeat = core.heartbeat(now).await.unwrap();
    assert_eq!(heartbeat.assignments.len(), 1);
    let hb: HeartbeatRequest = support::decode(
        &mock
            .recorded()
            .into_iter()
            .find(|r| r.route().ends_with("Heartbeat"))
            .unwrap(),
    );
    assert_eq!(hb.ready_connection_ids, vec![connection.id.clone()]);
    let acquire: AcquireLeaseRequest = support::decode(
        &mock
            .recorded()
            .into_iter()
            .find(|r| r.route().ends_with("AcquireLease"))
            .unwrap(),
    );
    assert_eq!(acquire.connection_id, "c1");
    assert_eq!(
        core.poll_connection("c1", now).await.unwrap(),
        Some(CollectionStatus::Succeeded)
    );
    let report: ReportSnapshotRequest = support::decode(
        &mock
            .recorded()
            .into_iter()
            .find(|r| r.route().ends_with("ReportSnapshot"))
            .unwrap(),
    );
    assert_eq!(report.lease_epoch, 1);
    let snapshot = report.snapshot.unwrap();
    assert_eq!(snapshot.next_attempt_at.unwrap().seconds, now + 300);
    let status = core.status(true).unwrap();
    assert!(status.connections[0].lease_held && status.connections[0].credential_present);
    assert_eq!(status.connections[0].lease_epoch, 1);
    assert!(
        status
            .notices
            .contains(&(LocalNotice::UpdateAvailable as i32))
    );
    // Another device took over: renewal fails, the lease is dropped and nothing is polled.
    {
        let mut s = state.lock().unwrap();
        s.epoch = 2;
        s.holder = "01890000-0000-7000-8000-00000000d002".into();
    }
    mock.clear();
    assert_eq!(core.poll_connection("c1", now + 300).await.unwrap(), None);
    assert!(!core.status(true).unwrap().connections[0].lease_held);
    assert_eq!(mock.count("/v10/projects"), 0);
    // A late report under a superseded epoch is discarded.
    state.lock().unwrap().holder.clear();
    core.heartbeat(now + 400).await.unwrap();
    assert_eq!(core.status(true).unwrap().connections[0].lease_epoch, 3);
    state.lock().unwrap().report = Some(ErrorReason::LeaseExpired);
    assert_eq!(
        core.poll_connection("c1", now + 400).await.unwrap(),
        Some(CollectionStatus::Succeeded)
    );
    assert!(!core.status(true).unwrap().connections[0].lease_held);
    // Release on quit, logout or uninstall.
    state.lock().unwrap().report = None;
    core.heartbeat(now + 500).await.unwrap();
    core.release_all().await;
    assert!(
        mock.recorded()
            .iter()
            .any(|r| r.route().ends_with("ReleaseLease"))
    );
    // A manual local sync is allowed once per minute.
    core.request_local_sync(now).unwrap();
    let limited = core.request_local_sync(now + 20).unwrap_err();
    assert_eq!(
        (limited.reason, limited.retry_after),
        (ErrorReason::RateLimited, Some(Duration::from_secs(40)))
    );
    // Disconnect unregisters this device and deletes the local credential.
    core.remove_connection("c1").await.unwrap();
    assert_eq!(core.status(true).unwrap().connections.len(), 0);
}
#[tokio::test(flavor = "multi_thread")]
async fn leases_are_renewed_during_a_poll_longer_than_the_lease() {
    let home = tempfile::tempdir().unwrap();
    // Scaled down: a 1-second heartbeat interval and a 2-second lease against a provider that
    // answers after 3.5 seconds.
    let state = Arc::new(Mutex::new(Api {
        heartbeat_seconds: 1,
        lease_ttl: Some(Duration::from_secs(2)),
        ..Api::default()
    }));
    let (mock, core) = logged_in(home.path(), &state, Owner::Foreground).await;
    vercel_connection(&core).await.unwrap();
    let base = api(state.clone());
    mock.set(move |r| {
        let reply = base(r);
        if r.route() == "/v10/projects" {
            reply.delayed(Duration::from_millis(3500))
        } else {
            reply
        }
    });
    let now = time::now();
    core.heartbeat(now).await.unwrap();
    mock.clear();
    assert_eq!(
        core.poll_connection("c1", now).await.unwrap(),
        Some(CollectionStatus::Succeeded)
    );
    assert_eq!(mock.count("/anydash.v1.CollectorService/ReportSnapshot"), 1);
    assert!(
        core.status(true).unwrap().connections[0].lease_held,
        "the snapshot was accepted under a valid lease"
    );
    assert!(
        mock.count("/anydash.v1.CollectorService/RenewLease") >= 3,
        "the lease is renewed before the poll and while it runs"
    );
    // A lease lost during the poll abandons it without a report.
    let s = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut s = s.lock().unwrap();
        s.epoch += 1;
        s.holder = "01890000-0000-7000-8000-00000000d002".into();
    });
    mock.clear();
    assert_eq!(core.poll_connection("c1", now + 300).await.unwrap(), None);
    assert!(!core.status(true).unwrap().connections[0].lease_held);
    assert_eq!(mock.count("/anydash.v1.CollectorService/ReportSnapshot"), 0);
}
#[tokio::test(flavor = "multi_thread")]
async fn active_observation_is_timestamped_after_lease_acquisition() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api::default()));
    let (mock, core) = logged_in(home.path(), &state, Owner::Foreground).await;
    vercel_connection(&core).await.unwrap();
    // The server records `acquired_at` with microsecond precision when it grants the lease.
    let acquired = Arc::new(Mutex::new(None));
    let base = api(state.clone());
    let a = acquired.clone();
    mock.set(move |r| {
        if r.route().ends_with("AcquireLease") {
            *a.lock().unwrap() = Some(chrono::Utc::now().timestamp_micros());
        }
        base(r)
    });
    // The cycle time is taken before the heartbeat that acquires the lease.
    let now = time::now();
    core.heartbeat(now).await.unwrap();
    assert_eq!(
        core.poll_connection("c1", now).await.unwrap(),
        Some(CollectionStatus::Succeeded)
    );
    let report: ReportSnapshotRequest = support::decode(
        &mock
            .recorded()
            .into_iter()
            .find(|r| r.route().ends_with("ReportSnapshot"))
            .unwrap(),
    );
    let observed = report.snapshot.unwrap().observed_at.unwrap();
    assert_eq!(observed.nanos % 1_000, 0, "microsecond precision");
    let observed = observed.seconds * 1_000_000 + i64::from(observed.nanos / 1_000);
    let acquired = acquired.lock().unwrap().expect("the lease was acquired");
    assert!(
        observed >= acquired,
        "observed_at {observed} precedes the lease acquisition {acquired}"
    );
}
#[tokio::test(flavor = "multi_thread")]
async fn codex_connection_is_not_polled_with_another_account() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api::default()));
    let (mock, core) = logged_in(home.path(), &state, Owner::Foreground).await;
    let auth = home.path().join(".codex/auth.json");
    let sign_in = |account: &str| {
        support::write(
            &auth,
            &json!({"tokens": {"access_token": support::jwt(time::now() + 3600), "account_id": account}})
                .to_string(),
        );
    };
    sign_in("acct-a");
    core.add_connection(AddConnectionCredentialRequest {
        provider: Provider::Codex.into(),
        ..Default::default()
    })
    .await
    .unwrap();
    // The Codex CLI is now signed in to another account.
    sign_in("acct-b");
    let base = api(state.clone());
    mock.set(move |r| {
        if r.route() == "/backend-api/wham/usage" {
            Reply::json(200, &json!({"plan_type": "plus"}))
        } else {
            base(r)
        }
    });
    let now = time::now();
    core.heartbeat(now).await.unwrap();
    let hb: HeartbeatRequest = support::decode(
        &mock
            .recorded()
            .into_iter()
            .find(|r| r.route().ends_with("Heartbeat"))
            .unwrap(),
    );
    assert!(
        hb.ready_connection_ids.is_empty(),
        "the device is not ready for another account's connection"
    );
    assert_eq!(
        core.poll_connection("c1", now).await.unwrap(),
        Some(CollectionStatus::AuthExpired)
    );
    assert_eq!(mock.count("/backend-api/wham/usage"), 0);
    let report: ReportSnapshotRequest = support::decode(
        &mock
            .recorded()
            .into_iter()
            .find(|r| r.route().ends_with("ReportSnapshot"))
            .unwrap(),
    );
    assert_eq!(
        report.snapshot.unwrap().failure_class,
        pb::FailureClass::CredentialMissing as i32
    );
    // The lease goes to a ready device of the connection's account.
    assert_eq!(mock.count("/anydash.v1.CollectorService/ReleaseLease"), 1);
    assert!(!core.status(true).unwrap().connections[0].lease_held);
    // Signing in to the connection's account again makes the device ready.
    sign_in("acct-a");
    mock.clear();
    core.heartbeat(now + 60).await.unwrap();
    let hb: HeartbeatRequest = support::decode(
        &mock
            .recorded()
            .into_iter()
            .find(|r| r.route().ends_with("Heartbeat"))
            .unwrap(),
    );
    assert_eq!(hb.ready_connection_ids, vec!["c1".to_owned()]);
}
#[tokio::test(flavor = "multi_thread")]
async fn provider_backoff_and_official_auth_expiry_stop() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api::default()));
    let (mock, core) = logged_in(home.path(), &state, Owner::Foreground).await;
    vercel_connection(&core).await.unwrap();
    let now = time::now();
    core.heartbeat(now).await.unwrap();
    let base = api(state.clone());
    let failing = Arc::new(Mutex::new(503u16));
    let f = failing.clone();
    mock.set(move |r| {
        if r.route() == "/v10/projects" {
            Reply::text(*f.lock().unwrap(), "")
        } else {
            base(r)
        }
    });
    assert_eq!(
        core.poll_connection("c1", now).await.unwrap(),
        Some(CollectionStatus::Failed)
    );
    let next = core.status(true).unwrap().connections[0]
        .next_attempt_at
        .unwrap()
        .seconds;
    assert!(
        (now + 60..=now + 72).contains(&next),
        "first failure waits 1 minute plus up to 20% jitter"
    );
    assert_ne!(
        core.status(true).unwrap().connections[0]
            .last_error_correlation_id
            .len(),
        0
    );
    assert_eq!(
        core.poll_connection("c1", next).await.unwrap(),
        Some(CollectionStatus::Failed)
    );
    let second = core.status(true).unwrap().connections[0]
        .next_attempt_at
        .unwrap()
        .seconds;
    assert!(
        (next + 120..=next + 144).contains(&second),
        "second failure waits 2 minutes plus jitter"
    );
    // Not due yet: nothing is polled.
    assert_eq!(core.poll_connection("c1", second - 1).await.unwrap(), None);
    *failing.lock().unwrap() = 401;
    assert_eq!(
        core.poll_connection("c1", second).await.unwrap(),
        Some(CollectionStatus::AuthExpired)
    );
    let count = mock.count("/v10/projects");
    // Official tokens stay stopped until the user reconnects.
    assert_eq!(
        core.poll_connection("c1", second + 3600).await.unwrap(),
        None
    );
    assert_eq!(mock.count("/v10/projects"), count);
}
#[tokio::test(flavor = "multi_thread")]
async fn official_auth_expiry_releases_the_lease_until_reconnection() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api::default()));
    let (mock, core) = logged_in(home.path(), &state, Owner::Foreground).await;
    vercel_connection(&core).await.unwrap();
    let now = time::now();
    core.heartbeat(now).await.unwrap();
    // The stored token is rejected, and the first lease release fails.
    let base = api(state.clone());
    let release_fails = Arc::new(Mutex::new(true));
    let fails = release_fails.clone();
    mock.set(move |r| match r.route() {
        "/v10/projects" if r.header("authorization") == Some("Bearer vercel-token") => {
            Reply::text(401, "")
        }
        "/anydash.v1.CollectorService/ReleaseLease"
            if std::mem::take(&mut *fails.lock().unwrap()) =>
        {
            Reply::reason(503, "unavailable", ErrorReason::DependencyUnavailable)
        }
        "/anydash.v1.ConnectionService/RegisterConnectionDevice" => {
            Reply::proto(&pb::RegisterConnectionDeviceResponse {
                connection: Some(Connection {
                    id: "c1".into(),
                    provider: Provider::Vercel.into(),
                    display_name: "Vercel".into(),
                    ..Default::default()
                }),
            })
        }
        _ => base(r),
    });
    mock.clear();
    assert_eq!(
        core.poll_connection("c1", now).await.unwrap(),
        Some(CollectionStatus::AuthExpired)
    );
    assert_eq!(mock.count("/anydash.v1.CollectorService/ReleaseLease"), 1);
    assert!(!core.status(true).unwrap().connections[0].lease_held);
    // The stopped token is not ready; the lease the failed release left on the server is
    // released on the next due cycle without calling the provider.
    let ready = |mock: &Mock| -> Vec<String> {
        let hb: HeartbeatRequest = support::decode(
            &mock
                .recorded()
                .into_iter()
                .rfind(|r| r.route().ends_with("Heartbeat"))
                .unwrap(),
        );
        hb.ready_connection_ids
    };
    core.heartbeat(now + 60).await.unwrap();
    assert_eq!(
        ready(&mock),
        Vec::<String>::new(),
        "a stopped token is not ready"
    );
    assert!(core.status(true).unwrap().connections[0].lease_held);
    assert_eq!(core.poll_connection("c1", now + 300).await.unwrap(), None);
    assert_eq!(mock.count("/anydash.v1.CollectorService/ReleaseLease"), 2);
    assert_eq!(mock.count("/v10/projects"), 1);
    assert!(state.lock().unwrap().holder.is_empty(), "the lease is free");
    assert!(!core.status(true).unwrap().connections[0].lease_held);
    // Another registered device with a valid token takes over.
    {
        let mut s = state.lock().unwrap();
        s.epoch += 1;
        s.holder = "01890000-0000-7000-8000-00000000d002".into();
    }
    core.heartbeat(now + 360).await.unwrap();
    assert_eq!(ready(&mock), Vec::<String>::new());
    assert!(!core.status(true).unwrap().connections[0].lease_held);
    // Reconnecting with a new token makes the device ready again.
    core.add_connection(AddConnectionCredentialRequest {
        provider: Provider::Vercel.into(),
        connection_id: "c1".into(),
        display_name: String::new(),
        secret: "vercel-token-2".into(),
        scope_id: "team_1".into(),
    })
    .await
    .unwrap();
    core.heartbeat(now + 420).await.unwrap();
    assert_eq!(ready(&mock), vec!["c1".to_owned()]);
}
#[tokio::test(flavor = "multi_thread")]
async fn a_poll_of_a_replaced_token_does_not_overwrite_the_reconnection() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api::default()));
    let (mock, core) = logged_in(home.path(), &state, Owner::Foreground).await;
    vercel_connection(&core).await.unwrap();
    let now = time::now();
    core.heartbeat(now).await.unwrap();
    // The old token is rejected, slowly enough for the user to reconnect meanwhile.
    let base = api(state.clone());
    mock.set(move |r| match r.route() {
        "/v10/projects" if r.header("authorization") == Some("Bearer vercel-token") => {
            Reply::text(401, "").delayed(Duration::from_millis(800))
        }
        "/anydash.v1.ConnectionService/RegisterConnectionDevice" => {
            Reply::proto(&pb::RegisterConnectionDeviceResponse {
                connection: Some(Connection {
                    id: "c1".into(),
                    provider: Provider::Vercel.into(),
                    display_name: "Vercel".into(),
                    ..Default::default()
                }),
            })
        }
        _ => base(r),
    });
    mock.clear();
    let poll = {
        let core = core.clone();
        tokio::spawn(async move { core.poll_connection("c1", now).await })
    };
    for _ in 0..100 {
        if mock.count("/v10/projects") > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(mock.count("/v10/projects"), 1, "the old token's poll runs");
    core.add_connection(AddConnectionCredentialRequest {
        provider: Provider::Vercel.into(),
        connection_id: "c1".into(),
        display_name: String::new(),
        secret: "vercel-token-2".into(),
        scope_id: "team_1".into(),
    })
    .await
    .unwrap();
    assert_eq!(
        poll.await.unwrap().unwrap(),
        None,
        "the obsolete result is abandoned"
    );
    assert_eq!(mock.count("/anydash.v1.CollectorService/ReportSnapshot"), 0);
    // The replacement token is polled; the old token's HTTP 401 did not stop the connection.
    core.heartbeat(now + 1).await.unwrap();
    assert_eq!(
        core.poll_connection("c1", now + 1).await.unwrap(),
        Some(CollectionStatus::Succeeded)
    );
    assert!(mock.recorded().iter().any(|r| r.route() == "/v10/projects"
        && r.header("authorization") == Some("Bearer vercel-token-2")));
}
#[tokio::test(flavor = "multi_thread")]
async fn registration_for_another_provider_is_rejected() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api::default()));
    let (mock, core) = logged_in(home.path(), &state, Owner::Foreground).await;
    vercel_connection(&core).await.unwrap();
    let base = api(state.clone());
    mock.set(move |r| match r.route() {
        "/v1/organizations" => Reply::json(200, &json!([])),
        // The server returns the connection with its own provider.
        "/anydash.v1.ConnectionService/RegisterConnectionDevice" => {
            let req: pb::RegisterConnectionDeviceRequest = support::decode(r);
            Reply::proto(&pb::RegisterConnectionDeviceResponse {
                connection: Some(Connection {
                    id: req.connection_id,
                    provider: Provider::Vercel.into(),
                    display_name: "Vercel".into(),
                    ..Default::default()
                }),
            })
        }
        _ => base(r),
    });
    let supabase = |id: &str| {
        core.add_connection(AddConnectionCredentialRequest {
            provider: Provider::Supabase.into(),
            connection_id: id.into(),
            display_name: String::new(),
            secret: "supabase-token".into(),
            scope_id: String::new(),
        })
    };
    // A connection registered on this device is checked before anything else happens.
    mock.clear();
    let error = supabase("c1").await.unwrap_err();
    assert_eq!(
        (error.reason, error.detail.field.as_str()),
        (ErrorReason::InvalidArgument, "provider")
    );
    assert_eq!(
        mock.count("/anydash.v1.ConnectionService/RegisterConnectionDevice"),
        0
    );
    let status = core.status(true).unwrap();
    assert_eq!(status.connections.len(), 1);
    assert_eq!(status.connections[0].provider, Provider::Vercel as i32);
    // An unknown connection is checked against the server's provider; the registration is undone
    // and nothing is stored locally.
    mock.clear();
    let error = supabase("c2").await.unwrap_err();
    assert_eq!(
        (error.reason, error.detail.field.as_str()),
        (ErrorReason::InvalidArgument, "provider")
    );
    assert_eq!(
        mock.count("/anydash.v1.ConnectionService/UnregisterConnectionDevice"),
        1
    );
    let status = core.status(true).unwrap();
    assert_eq!(status.connections.len(), 1);
    assert_eq!(status.connections[0].connection_id, "c1");
}
#[tokio::test(flavor = "multi_thread")]
async fn snapshots_rejected_as_too_large_are_not_retried() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api::default()));
    let (mock, core) = logged_in(home.path(), &state, Owner::Foreground).await;
    vercel_connection(&core).await.unwrap();
    let now = time::now();
    core.heartbeat(now).await.unwrap();
    state.lock().unwrap().report = Some(ErrorReason::PayloadTooLarge);
    assert_eq!(
        core.poll_connection("c1", now).await.unwrap(),
        Some(CollectionStatus::Succeeded)
    );
    let pending: i64 = rusqlite::Connection::open(home.path().join("anydash/data/collector.db"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM pending_snapshots", [], |r| r.get(0))
        .unwrap();
    assert_eq!(pending, 0, "the rejected payload is discarded");
    assert!(core.status(true).unwrap().connections[0].lease_held);
    // The next cycle does not resend it before polling again.
    state.lock().unwrap().report = None;
    mock.clear();
    core.cycle(now + 60).await;
    assert_eq!(mock.count("/anydash.v1.CollectorService/ReportSnapshot"), 0);
}
#[tokio::test(flavor = "multi_thread")]
async fn provider_retry_waits_start_when_the_response_arrives() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api::default()));
    let (mock, core) = logged_in(home.path(), &state, Owner::Foreground).await;
    vercel_connection(&core).await.unwrap();
    let now = time::now();
    core.heartbeat(now).await.unwrap();
    // The provider answers HTTP 429 with `Retry-After: 60` only after 2.5 seconds.
    let base = api(state.clone());
    let failing = Arc::new(Mutex::new(429u16));
    let f = failing.clone();
    mock.set(move |r| {
        if r.route() == "/v10/projects" {
            Reply::text(*f.lock().unwrap(), "")
                .with_header("Retry-After", "60")
                .delayed(Duration::from_millis(2500))
        } else {
            base(r)
        }
    });
    let next_attempt = || {
        core.status(true).unwrap().connections[0]
            .next_attempt_at
            .unwrap()
            .seconds
    };
    assert_eq!(
        core.poll_connection("c1", now).await.unwrap(),
        Some(CollectionStatus::RateLimited)
    );
    let next = next_attempt();
    assert!(
        next >= now + 62,
        "Retry-After counts from the response, not the poll start: {next} < {}",
        now + 62
    );
    // Backoff after a slow transient failure also starts when the response arrives. Make the
    // connection due again at the same cycle time.
    *failing.lock().unwrap() = 503;
    rusqlite::Connection::open(home.path().join("anydash/data/collector.db"))
        .unwrap()
        .execute("UPDATE connections SET next_attempt = 0", [])
        .unwrap();
    let started = time::now();
    assert_eq!(
        core.poll_connection("c1", now).await.unwrap(),
        Some(CollectionStatus::Failed)
    );
    let backoff = next_attempt();
    assert!(
        backoff >= started + 2 + 120,
        "the second failure waits 2 minutes after the response: {backoff}"
    );
}
#[tokio::test(flavor = "multi_thread")]
async fn offline_buffering_update_required_and_chunked_upload() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api {
        upload: Some(ErrorReason::DependencyUnavailable),
        ..Api::default()
    }));
    let (mock, core) = logged_in(home.path(), &state, Owner::Foreground).await;
    let now = time::now();
    let claude = home.path().join(".claude/projects/p/s.jsonl");
    let line = |ts: i64, id: &str| {
        json!({"type": "assistant", "timestamp": time::rfc3339(ts), "sessionId": "s", "message": {"id": id, "model": "m", "usage": {"input_tokens": 1, "output_tokens": 1}}}).to_string()
    };
    // 2,100 distinct 15-minute buckets within the last 90 days.
    let lines: String = (0..2100)
        .map(|i| line(now - 3600 - i * 900, &format!("m{i}")) + "\n")
        .collect();
    support::write(&claude, &lines);
    core.local_sync(now).unwrap();
    let pending = core.status(true).unwrap().pending_bucket_count;
    assert!(pending >= 2100);
    // Offline: the upload fails transiently and the buckets stay buffered.
    let error = core.upload(now).await.unwrap_err();
    assert!(anydash::api::transient(&error));
    assert_eq!(core.status(true).unwrap().pending_bucket_count, pending);
    // UPDATE_REQUIRED stops uploads; aggregation continues locally.
    state.lock().unwrap().upload = Some(ErrorReason::UpdateRequired);
    assert_eq!(
        core.upload(now).await.unwrap_err().reason,
        ErrorReason::UpdateRequired
    );
    support::write(&claude, &(lines + &line(now - 60, "late") + "\n"));
    core.local_sync(now).unwrap();
    let status = core.status(true).unwrap();
    assert_eq!(status.run_state, CollectorRunState::UpdateRequired as i32);
    assert!(
        status
            .notices
            .contains(&(LocalNotice::UpdateRequired as i32))
    );
    assert!(status.pending_bucket_count > pending);
    mock.clear();
    assert_eq!(
        core.upload(now).await.unwrap_err().reason,
        ErrorReason::UpdateRequired
    );
    assert!(
        mock.recorded().is_empty(),
        "no API calls until the executable is updated"
    );
    // After an update (a new version clears the flag), the buffer uploads in chunks.
    let updated = support::core(home.path(), &mock, Owner::Foreground, &[]);
    let store_flag_reset = updated.status(true).unwrap().run_state;
    assert_eq!(
        store_flag_reset,
        CollectorRunState::UpdateRequired as i32,
        "the same version keeps the flag"
    );
    drop(updated);
    let db = rusqlite::Connection::open(home.path().join("anydash/data/collector.db")).unwrap();
    db.execute(
        "UPDATE meta SET value = '0.0.0-old' WHERE key = 'update_required'",
        [],
    )
    .unwrap();
    drop(db);
    state.lock().unwrap().upload = None;
    let updated = support::core(home.path(), &mock, Owner::Foreground, &[]);
    let uploaded = updated.upload(now).await.unwrap();
    assert!(uploaded > 2000);
    assert!(mock.count("/anydash.v1.CollectorService/UploadUsage") >= 2);
    assert_eq!(updated.status(true).unwrap().pending_bucket_count, 0);
    let _ = Tokens::default();
}
#[tokio::test(flavor = "multi_thread")]
async fn local_sync_requests_do_not_bypass_upload_rate_limits() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api::default()));
    let (mock, core) = logged_in(home.path(), &state, Owner::Foreground).await;
    let now = time::now();
    let claude = home.path().join(".claude/projects/p/s.jsonl");
    let line = |ts: i64, id: &str| {
        json!({"type": "assistant", "timestamp": time::rfc3339(ts), "sessionId": "s", "message": {"id": id, "model": "m", "usage": {"input_tokens": 1, "output_tokens": 1}}}).to_string() + "\n"
    };
    support::write(&claude, &line(now - 600, "m1"));
    let base = api(state.clone());
    mock.set(move |r| {
        if r.route().ends_with("UploadUsage") {
            Reply::reason(429, "resource_exhausted", ErrorReason::RateLimited)
                .with_header("retry-after", "600")
        } else {
            base(r)
        }
    });
    let uploads = || mock.count("/anydash.v1.CollectorService/UploadUsage");
    core.cycle(now).await;
    assert_eq!(uploads(), 1);
    // A manual local sync aggregates new usage but waits for the upload deadline.
    support::write(&claude, &(line(now - 600, "m1") + &line(now - 300, "m2")));
    core.request_local_sync(now + 10).unwrap();
    core.cycle(now + 10).await;
    // Scheduled syncs also wait.
    core.cycle(now + 70).await;
    assert_eq!(uploads(), 1, "no upload before Retry-After elapses");
    assert!(core.status(true).unwrap().pending_bucket_count >= 1);
    let base = api(state.clone());
    mock.set(base);
    core.cycle(now + 600).await;
    assert_eq!(uploads(), 2);
    assert_eq!(core.status(true).unwrap().pending_bucket_count, 0);
}
#[tokio::test(flavor = "multi_thread")]
async fn quit_interrupts_a_running_provider_poll_and_releases_leases() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api::default()));
    let (mock, core) = logged_in(home.path(), &state, Owner::Foreground).await;
    vercel_connection(&core).await.unwrap();
    drop(core);
    // The provider answers only after a minute.
    let base = api(state.clone());
    mock.set(move |r| {
        let reply = base(r);
        if r.route() == "/v10/projects" {
            reply.delayed(Duration::from_secs(60))
        } else {
            reply
        }
    });
    let running = collector::start(anydash::core::Options {
        settings: support::settings(home.path(), &[("ANYDASH_API_URL", &mock.base)]),
        endpoints: anydash::providers::Endpoints::mock(&mock.base, "grok-not-installed-for-tests"),
        os: Arc::new(anydash::keys::MemoryStore::default()),
        owner: Owner::Foreground,
    })
    .unwrap();
    let core = running.core.clone();
    let task = tokio::spawn(running.run());
    for _ in 0..250 {
        if mock.count("/v10/projects") > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        mock.count("/v10/projects"),
        1,
        "the provider poll is running"
    );
    core.quit().unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("quit does not wait for the provider poll")
        .unwrap()
        .unwrap();
    assert_eq!(mock.count("/anydash.v1.CollectorService/ReleaseLease"), 1);
    assert_eq!(mock.count("/anydash.v1.CollectorService/ReportSnapshot"), 0);
}
struct NoRunner;
impl anydash::service::Runner for NoRunner {
    fn run(&mut self, _: &str, _: &[String]) -> anydash::error::Result<()> {
        Ok(())
    }
}
/// Run the CLI with the API at `api_url`.
async fn run_cli(home: &Path, api_url: &str, args: &[&str]) -> (i32, serde_json::Value) {
    let mut env = anydash::cli::Env {
        settings: support::settings(home, &[("ANYDASH_API_URL", api_url)]),
        endpoints: anydash::providers::Endpoints::mock(api_url, "grok-not-installed-for-tests"),
        os: Arc::new(anydash::keys::MemoryStore::default()),
        exe: home.join("bin/anydash"),
        runner: Box::new(NoRunner),
        platform: anydash::service::Platform::current(),
        open_browser: false,
    };
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let (mut out, mut err) = (vec![], vec![]);
    let mut input = std::io::Cursor::new(vec![]);
    let mut io = anydash::cli::Io {
        out: &mut out,
        err: &mut err,
        input: &mut input,
        tty: false,
    };
    let code = anydash::cli::run(&args, &mut env, &mut io).await;
    (code, serde_json::from_slice(&out).unwrap_or_default())
}
#[tokio::test(flavor = "multi_thread")]
#[expect(
    clippy::too_many_lines,
    reason = "One scenario covers the IPC and direct paths with their failures."
)]
async fn connection_refresh_goes_through_the_running_collector() {
    const CONNECTION: &str = "01890000-0000-7000-8000-0000000000c1";
    const CORRELATION: &str = "01890000-0000-7000-8000-0000000000aa";
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(Api::default()));
    let (mock, enrolled) = logged_in(home.path(), &state, Owner::Foreground).await;
    drop(enrolled);
    let limited = Arc::new(Mutex::new(false));
    let base = api(state.clone());
    let l = limited.clone();
    mock.set(move |r| {
        if r.route().ends_with("RequestConnectionRefresh") && *l.lock().unwrap() {
            Reply::connect_error(
                429,
                "resource_exhausted",
                ErrorDetail {
                    reason: ErrorReason::RateLimited.into(),
                    correlation_id: CORRELATION.into(),
                    retry_after: Some(prost_types::Duration {
                        seconds: 42,
                        nanos: 0,
                    }),
                    rate_limit_scope: pb::RateLimitScope::ManualRefresh.into(),
                    ..Default::default()
                },
            )
        } else {
            base(r)
        }
    });
    let refreshes = || mock.count("/anydash.v1.ConnectionService/RequestConnectionRefresh");
    let running = collector::start(anydash::core::Options {
        settings: support::settings(home.path(), &[("ANYDASH_API_URL", &mock.base)]),
        endpoints: anydash::providers::Endpoints::mock(&mock.base, "grok-not-installed-for-tests"),
        os: Arc::new(anydash::keys::MemoryStore::default()),
        owner: Owner::Foreground,
    })
    .unwrap();
    let task = tokio::spawn(running.run());
    // The CLI's own API URL is unreachable: only the collector can reach the API, so a
    // success shows that the CLI never used the device credentials itself.
    let unreachable = "http://127.0.0.1:9";
    let (code, v) = run_cli(home.path(), unreachable, &["--json", "refresh", CONNECTION]).await;
    assert_eq!(code, 0, "{v}");
    assert!(v["result"]["nextAllowedAt"].as_str().is_some());
    assert_eq!(refreshes(), 1);
    let request: pb::RequestConnectionRefreshRequest = support::decode(
        &mock
            .recorded()
            .into_iter()
            .find(|r| r.route().ends_with("RequestConnectionRefresh"))
            .unwrap(),
    );
    assert_eq!(request.connection_id, CONNECTION);
    // API errors pass through with their retry wait and correlation ID.
    *limited.lock().unwrap() = true;
    let (code, v) = run_cli(home.path(), unreachable, &["--json", "refresh", CONNECTION]).await;
    assert_eq!(code, 1);
    assert_eq!(v["error"]["reason"], "ERROR_REASON_RATE_LIMITED");
    assert_eq!(v["error"]["retryAfterSeconds"], 42);
    assert_eq!(v["error"]["correlationId"], CORRELATION);
    let client = ipc::Client::connect(&support::settings(home.path(), &[]).dirs())
        .await
        .unwrap();
    let f = client
        .call::<_, pb::RequestLocalConnectionRefreshResponse>(
            "RequestLocalConnectionRefresh",
            &pb::RequestLocalConnectionRefreshRequest {
                connection_id: CONNECTION.into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        (f.detail.rate_limit_scope, f.correlation_id.as_str()),
        (pb::RateLimitScope::ManualRefresh as i32, CORRELATION)
    );
    // A malformed ID is rejected before any API call.
    for id in [
        "c1",
        "01890000-0000-7000-8000-0000000000C1",
        "01890000-0000-4000-8000-0000000000c1",
    ] {
        let f = client
            .call::<_, pb::RequestLocalConnectionRefreshResponse>(
                "RequestLocalConnectionRefresh",
                &pb::RequestLocalConnectionRefreshRequest {
                    connection_id: id.into(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(
            (f.reason, f.detail.field.as_str()),
            (ErrorReason::InvalidArgument, "connection_id"),
            "{id}"
        );
    }
    assert_eq!(refreshes(), 3);
    let _: QuitResponse = client.call("Quit", &QuitRequest {}).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // Without a running collector the command runs the same handler directly.
    *limited.lock().unwrap() = false;
    let (code, v) = run_cli(home.path(), &mock.base, &["--json", "refresh", CONNECTION]).await;
    assert_eq!(code, 0, "{v}");
    assert_eq!(refreshes(), 4);
    let (code, v) = run_cli(home.path(), &mock.base, &["--json", "refresh", "c1"]).await;
    assert_eq!(
        (code, v["error"]["reason"].as_str()),
        (1, Some("ERROR_REASON_INVALID_ARGUMENT"))
    );
    assert_eq!(refreshes(), 4);
    // An older collector without the method answers `unimplemented`; the CLI asks for an
    // update and never calls the API with the device credentials itself.
    #[cfg(unix)]
    {
        let dirs = support::settings(home.path(), &[]).dirs();
        let old = tokio::net::UnixListener::bind(ipc::socket_path(&dirs)).unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = old.accept().await else {
                    return;
                };
                let service = hyper::service::service_fn(|_| async {
                    Ok::<_, std::convert::Infallible>(
                        hyper::Response::builder()
                            .status(501)
                            .header("Content-Type", "application/json")
                            .body(http_body_util::Full::new(bytes::Bytes::from_static(
                                br#"{"code":"unimplemented","message":"Unknown method."}"#,
                            )))
                            .unwrap(),
                    )
                });
                tokio::spawn(
                    hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), service),
                );
            }
        });
        let (code, v) = run_cli(home.path(), &mock.base, &["--json", "refresh", CONNECTION]).await;
        assert_eq!(code, 1, "{v}");
        assert!(
            v["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("update the older")),
            "{v}"
        );
        assert_eq!(refreshes(), 4);
    }
}
/// The first call uses the connection that found the collector. A second open can fail
/// although the collector runs (on Windows, `ERROR_PIPE_BUSY` before the collector creates its
/// next pipe instance); here the socket is removed after the connection is made.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn the_first_ipc_call_uses_the_connection_that_found_the_collector() {
    let home = tempfile::tempdir().unwrap();
    let mock = Mock::start(api(Arc::new(Mutex::new(Api::default())))).await;
    let core = support::core(home.path(), &mock, Owner::Foreground, &[]);
    let listener = ipc::bind(&core.dirs).unwrap();
    let server = tokio::spawn(ipc::serve(core.clone(), listener));
    let client = ipc::Client::connect(&core.dirs).await.unwrap();
    std::fs::remove_file(ipc::socket_path(&core.dirs)).unwrap();
    let response: GetCollectorStatusResponse = client
        .call("GetCollectorStatus", &GetCollectorStatusRequest {})
        .await
        .unwrap();
    assert!(response.status.is_some());
    // Later calls open their own connection.
    let f = client
        .call::<_, GetCollectorStatusResponse>("GetCollectorStatus", &GetCollectorStatusRequest {})
        .await
        .unwrap_err();
    assert_eq!(f.reason, ErrorReason::DependencyUnavailable);
    server.abort();
}
#[tokio::test(flavor = "multi_thread")]
async fn one_shared_process_with_ipc_quit_and_service_ownership() {
    let home = tempfile::tempdir().unwrap();
    let mock = Mock::start(|_| Reply::text(500, "")).await;
    let settings = support::settings(home.path(), &[("ANYDASH_API_URL", &mock.base)]);
    let options = |owner| anydash::core::Options {
        settings: settings.clone(),
        endpoints: anydash::providers::Endpoints::mock(&mock.base, "grok"),
        os: Arc::new(anydash::keys::MemoryStore::default()),
        owner,
    };
    let running = collector::start(options(Owner::Desktop)).unwrap();
    let task = tokio::spawn(running.run());
    let dirs = settings.dirs();
    let client = ipc::Client::connect(&dirs)
        .await
        .expect("the collector answers on local IPC");
    // A second collector for the same OS user is refused; the app and the CLI share this one.
    assert!(collector::try_lock(&dirs).unwrap().is_none());
    assert!(collector::start(options(Owner::Foreground)).is_err());
    let status: GetCollectorStatusResponse = client
        .call("GetCollectorStatus", &GetCollectorStatusRequest {})
        .await
        .unwrap();
    assert_eq!(
        status.status.unwrap().owner_kind,
        pb::CollectorKind::DesktopApp as i32
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(ipc::socket_path(&dirs))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    // Closing the client (the app window) does not stop collection.
    drop(client);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!task.is_finished());
    let client = ipc::Client::connect(&dirs).await.unwrap();
    // Unknown methods return Connect unimplemented.
    let unknown = client
        .call::<_, QuitResponse>("Teleport", &QuitRequest {})
        .await
        .unwrap_err();
    assert!(unknown.message.contains("update the older"));
    // The CLI uses the same process for commands.
    let paths: pb::ListSourcePathsResponse = client
        .call("ListSourcePaths", &pb::ListSourcePathsRequest {})
        .await
        .unwrap();
    assert_ne!(paths.paths.len(), 0);
    // An explicit quit stops a desktop collector.
    let _: QuitResponse = client.call("Quit", &QuitRequest {}).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(collector::try_lock(&dirs).unwrap().is_some());
    // A service collector refuses Quit.
    let running = collector::start(options(Owner::Service)).unwrap();
    let core = running.core.clone();
    let task = tokio::spawn(running.run());
    let client = ipc::Client::connect(&dirs).await.unwrap();
    let refused = client
        .call::<_, QuitResponse>("Quit", &QuitRequest {})
        .await
        .unwrap_err();
    assert_eq!(refused.reason, ErrorReason::ServiceManaged);
    let _ = core.stopped.send(true);
    core.wake.notify_one();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let _ = ReportSnapshotResponse::default().encode_to_vec();
}
