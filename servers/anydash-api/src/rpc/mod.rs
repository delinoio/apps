// SPDX-License-Identifier: FSL-1.1-ALv2
//! Connect service implementations and helpers shared by them.
use crate::{ApiError, Result, call::Call};
use base64::Engine;
use buffa::{Enumeration, Message};
use chrono::{DateTime, Utc};
use delino_protocol::anydash::v1::*;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

pub mod account;
pub mod collector;
pub mod connection;
pub mod dashboard;
pub mod device;
pub mod widget_data;

crate::service!(DeviceService, "DeviceService", {
    start_enrollment => "StartEnrollment", StartEnrollmentRequest, StartEnrollmentResponse, device::start_enrollment;
    get_enrollment => "GetEnrollment", GetEnrollmentRequest, GetEnrollmentResponse, device::get_enrollment;
    approve_enrollment => "ApproveEnrollment", ApproveEnrollmentRequest, ApproveEnrollmentResponse, device::approve_enrollment;
    reject_enrollment => "RejectEnrollment", RejectEnrollmentRequest, RejectEnrollmentResponse, device::reject_enrollment;
    poll_enrollment => "PollEnrollment", PollEnrollmentRequest, PollEnrollmentResponse, device::poll_enrollment;
    refresh_device_token => "RefreshDeviceToken", RefreshDeviceTokenRequest, RefreshDeviceTokenResponse, device::refresh_device_token;
    revoke_device_credentials => "RevokeDeviceCredentials", RevokeDeviceCredentialsRequest, RevokeDeviceCredentialsResponse, device::revoke_device_credentials;
    list_devices => "ListDevices", ListDevicesRequest, ListDevicesResponse, device::list_devices;
    delete_device => "DeleteDevice", DeleteDeviceRequest, DeleteDeviceResponse, device::delete_device;
    set_preferred_device => "SetPreferredDevice", SetPreferredDeviceRequest, SetPreferredDeviceResponse, device::set_preferred_device;
    request_device_sync => "RequestDeviceSync", RequestDeviceSyncRequest, RequestDeviceSyncResponse, device::request_device_sync;
});

crate::service!(CollectorService, "CollectorService", {
    heartbeat => "Heartbeat", HeartbeatRequest, HeartbeatResponse, collector::heartbeat;
    upload_usage => "UploadUsage", UploadUsageRequest, UploadUsageResponse, collector::upload_usage;
    report_parser_status => "ReportParserStatus", ReportParserStatusRequest, ReportParserStatusResponse, collector::report_parser_status;
    acquire_lease => "AcquireLease", AcquireLeaseRequest, AcquireLeaseResponse, collector::acquire_lease;
    renew_lease => "RenewLease", RenewLeaseRequest, RenewLeaseResponse, collector::renew_lease;
    release_lease => "ReleaseLease", ReleaseLeaseRequest, ReleaseLeaseResponse, collector::release_lease;
    report_snapshot => "ReportSnapshot", ReportSnapshotRequest, ReportSnapshotResponse, collector::report_snapshot;
});

crate::service!(ConnectionService, "ConnectionService", {
    create_connection => "CreateConnection", CreateConnectionRequest, CreateConnectionResponse, connection::create_connection;
    list_connections => "ListConnections", ListConnectionsRequest, ListConnectionsResponse, connection::list_connections;
    get_connection => "GetConnection", GetConnectionRequest, GetConnectionResponse, connection::get_connection;
    delete_connection => "DeleteConnection", DeleteConnectionRequest, DeleteConnectionResponse, connection::delete_connection;
    register_connection_device => "RegisterConnectionDevice", RegisterConnectionDeviceRequest, RegisterConnectionDeviceResponse, connection::register_connection_device;
    unregister_connection_device => "UnregisterConnectionDevice", UnregisterConnectionDeviceRequest, UnregisterConnectionDeviceResponse, connection::unregister_connection_device;
    request_connection_refresh => "RequestConnectionRefresh", RequestConnectionRefreshRequest, RequestConnectionRefreshResponse, connection::request_connection_refresh;
});

crate::service!(DashboardService, "DashboardService", {
    list_spaces => "ListSpaces", ListSpacesRequest, ListSpacesResponse, dashboard::list_spaces;
    list_dashboards => "ListDashboards", ListDashboardsRequest, ListDashboardsResponse, dashboard::list_dashboards;
    get_dashboard => "GetDashboard", GetDashboardRequest, GetDashboardResponse, dashboard::get_dashboard;
    create_dashboard => "CreateDashboard", CreateDashboardRequest, CreateDashboardResponse, dashboard::create_dashboard;
    rename_dashboard => "RenameDashboard", RenameDashboardRequest, RenameDashboardResponse, dashboard::rename_dashboard;
    delete_dashboard => "DeleteDashboard", DeleteDashboardRequest, DeleteDashboardResponse, dashboard::delete_dashboard;
    add_widget => "AddWidget", AddWidgetRequest, AddWidgetResponse, dashboard::add_widget;
    remove_widget => "RemoveWidget", RemoveWidgetRequest, RemoveWidgetResponse, dashboard::remove_widget;
    update_widget_layout => "UpdateWidgetLayout", UpdateWidgetLayoutRequest, UpdateWidgetLayoutResponse, dashboard::update_widget_layout;
    update_widget_query => "UpdateWidgetQuery", UpdateWidgetQueryRequest, UpdateWidgetQueryResponse, dashboard::update_widget_query;
    set_widget_sharing_scope => "SetWidgetSharingScope", SetWidgetSharingScopeRequest, SetWidgetSharingScopeResponse, dashboard::set_widget_sharing_scope;
    revoke_widget_sharing => "RevokeWidgetSharing", RevokeWidgetSharingRequest, RevokeWidgetSharingResponse, dashboard::revoke_widget_sharing;
});

crate::service!(WidgetDataService, "WidgetDataService", {
    query_widget_data => "QueryWidgetData", QueryWidgetDataRequest, QueryWidgetDataResponse, widget_data::query_widget_data;
    query_dashboard_data => "QueryDashboardData", QueryDashboardDataRequest, QueryDashboardDataResponse, widget_data::query_dashboard_data;
});

crate::service!(AccountService, "AccountService", {
    connect_account => "ConnectAccount", ConnectAccountRequest, ConnectAccountResponse, account::connect_account;
    get_account => "GetAccount", GetAccountRequest, GetAccountResponse, account::get_account;
    update_account_settings => "UpdateAccountSettings", UpdateAccountSettingsRequest, UpdateAccountSettingsResponse, account::update_account_settings;
    list_activity => "ListActivity", ListActivityRequest, ListActivityResponse, account::list_activity;
    delete_account_data => "DeleteAccountData", DeleteAccountDataRequest, DeleteAccountDataResponse, account::delete_account_data;
});

pub const QUOTA_DEVICES: u32 = 20;
pub const QUOTA_CONNECTIONS: u32 = 50;
pub const QUOTA_DASHBOARDS: u32 = 50;
pub const QUOTA_WIDGETS: u32 = 100;

/// `widget_sources.source_kind` values.
pub const SOURCE_CONNECTION: i16 = 1;
pub const SOURCE_DEVICE: i16 = 2;

/// `deletion_jobs.kind` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeletionKind {
    Connection = 1,
    Device = 2,
    Dashboard = 3,
    AccountData = 4,
    Withdrawal = 5,
    Tenant = 6,
}

pub fn e16<E: Enumeration>(e: E) -> i16 {
    e.to_i32() as i16
}
pub fn enum_of<E: Enumeration + Default>(v: i16) -> E {
    E::from_i32(i32::from(v)).unwrap_or_default()
}

/// Opaque page cursor bound to the caller, method and filters.
#[derive(Serialize, Deserialize)]
struct CursorBody {
    c: String,
    t: i64,
    i: String,
}

fn context_digest(context: &str) -> String {
    crate::sha256(context.as_bytes())[..16].to_owned()
}

pub fn encode_cursor(context: &str, at: DateTime<Utc>, id: &str) -> String {
    let body = CursorBody {
        c: context_digest(context),
        t: at.timestamp_micros(),
        i: id.into(),
    };
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&body).unwrap_or_default())
}

/// Decodes a cursor; a cursor from another caller, method or filter set is invalid.
pub fn decode_cursor(context: &str, cursor: &str) -> Result<Option<(DateTime<Utc>, Uuid)>> {
    if cursor.is_empty() {
        return Ok(None);
    }
    let invalid = || ApiError::invalid("page.cursor", "invalid cursor");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| invalid())?;
    let body: CursorBody = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if body.c != context_digest(context) {
        return Err(invalid());
    }
    let at = DateTime::from_timestamp_micros(body.t).ok_or_else(invalid)?;
    let id = Uuid::parse_str(&body.i).map_err(|_| invalid())?;
    Ok(Some((at, id)))
}

/// Fingerprint of decoded request content for `request_id` replay comparison.
pub fn fingerprint<M: Message>(message: &M) -> String {
    crate::sha256(&message.encode_to_vec())
}

/// Looks up a creation `request_id` within its 24-hour window. Returns the original result ID
/// for an identical replay and `REQUEST_ID_REUSED` for different content.
pub async fn replay(
    db: &mut PgConnection,
    caller: &str,
    method: &str,
    request_id: Uuid,
    fingerprint: &str,
    now: DateTime<Utc>,
) -> Result<Option<Uuid>> {
    let row: Option<(String, Uuid)> = sqlx::query_as(
        "SELECT fingerprint_sha256,result_id FROM request_ids WHERE caller_id=$1 AND method=$2 AND request_id=$3 AND created_at>$4",
    )
    .bind(caller)
    .bind(method)
    .bind(request_id)
    .bind(now - chrono::Duration::hours(24))
    .fetch_optional(&mut *db)
    .await?;
    match row {
        Some((f, _)) if f != fingerprint => Err(ApiError::new(
            ErrorReason::RequestIdReused,
            "request ID was used with different content",
        )
        .field("request_id")),
        Some((_, id)) => Ok(Some(id)),
        None => Ok(None),
    }
}

pub async fn remember(
    db: &mut PgConnection,
    caller: &str,
    method: &str,
    request_id: Uuid,
    fingerprint: &str,
    result: Uuid,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO request_ids(caller_id,method,request_id,fingerprint_sha256,result_id,created_at) VALUES($1,$2,$3,$4,$5,$6) \
         ON CONFLICT (caller_id,method,request_id) DO UPDATE SET fingerprint_sha256=EXCLUDED.fingerprint_sha256,result_id=EXCLUDED.result_id,created_at=EXCLUDED.created_at",
    )
    .bind(caller)
    .bind(method)
    .bind(request_id)
    .bind(fingerprint)
    .bind(result)
    .bind(now)
    .execute(db)
    .await?;
    Ok(())
}

/// Enqueues a physical deletion with the 24-hour deadline and returns the deadline.
pub async fn enqueue_deletion(
    db: &mut PgConnection,
    kind: DeletionKind,
    target: Option<Uuid>,
    user: Option<Uuid>,
    tenant: Option<Uuid>,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    let deadline = now + chrono::Duration::hours(crate::time::DELETION_HOURS);
    sqlx::query(
        "INSERT INTO deletion_jobs(id,kind,target_id,user_id,tenant_id,requested_at,deadline) VALUES($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(crate::new_id())
    .bind(kind as i16)
    .bind(target)
    .bind(user)
    .bind(tenant)
    .bind(now)
    .bind(deadline)
    .execute(db)
    .await?;
    Ok(deadline)
}

/// Ends every lease held by a device immediately.
pub async fn end_device_leases(db: &mut PgConnection, device: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE connections SET last_holder_device_id=lease_device_id,lease_device_id=NULL,lease_expires_at=NULL WHERE lease_device_id=$1",
    )
    .bind(device)
    .execute(db)
    .await?;
    Ok(())
}

/// Removes a deleted connection or device from widget sources. Widgets left without a source end
/// with `SOURCE_DELETED`; team widgets record the sharing end.
pub async fn detach_source(
    db: &mut PgConnection,
    call: &Call,
    kind: i16,
    source: Uuid,
) -> Result<()> {
    let widgets: Vec<Uuid> = sqlx::query_scalar(
        "DELETE FROM widget_sources WHERE source_kind=$1 AND source_id=$2 RETURNING widget_id",
    )
    .bind(kind)
    .bind(source)
    .fetch_all(&mut *db)
    .await?;
    if widgets.is_empty() {
        return Ok(());
    }
    let ended: Vec<(Uuid, Uuid, Uuid, i16, Uuid)> = sqlx::query_as(
        "UPDATE widgets w SET sharing_state=$2,sharing_end_reason=$3 FROM dashboards d \
         WHERE w.id=ANY($1) AND d.id=w.dashboard_id AND NOT w.all_devices AND w.sharing_state<>$2 \
         AND NOT EXISTS (SELECT 1 FROM widget_sources s WHERE s.widget_id=w.id) \
         RETURNING w.id,w.dashboard_id,d.tenant_id,d.space_kind,w.data_owner_user_id",
    )
    .bind(&widgets)
    .bind(e16(SharingState::Ended))
    .bind(e16(SharingEndReason::SourceDeleted))
    .fetch_all(&mut *db)
    .await?;
    for (widget, dashboard, tenant, space, owner) in ended {
        if space == e16(SpaceKind::Team) {
            crate::audit::record(
                db,
                call,
                crate::audit::Event {
                    kind: ActivityKind::WidgetSharingEnded,
                    actor_user: Some(owner),
                    subject_user: Some(owner),
                    tenant: Some(tenant),
                    dashboard: Some(dashboard),
                    widget: Some(widget),
                    ..Default::default()
                },
            )
            .await?;
        }
    }
    Ok(())
}

/// Locks the device row. Calls that change a device together with its connections or connection
/// registrations lock the device row first, then the connection rows, then the registration rows,
/// so concurrent calls for one device cannot deadlock.
pub async fn lock_device(db: &mut PgConnection, device: Uuid) -> Result<()> {
    sqlx::query("SELECT 1 FROM devices WHERE id=$1 FOR UPDATE")
        .bind(device)
        .execute(db)
        .await?;
    Ok(())
}

/// Locks the account row for quota checks.
pub async fn lock_account(db: &mut PgConnection, user: Uuid) -> Result<()> {
    sqlx::query("SELECT 1 FROM accounts WHERE user_id=$1 FOR UPDATE")
        .bind(user)
        .fetch_optional(db)
        .await?;
    Ok(())
}

pub fn ts(t: DateTime<Utc>) -> buffa_types::google::protobuf::Timestamp {
    crate::time::timestamp(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursors_are_bound_to_their_context() {
        let now = crate::time::now();
        let id = crate::new_id();
        let c = encode_cursor("user:a|ListDevices", now, &id.to_string());
        assert_eq!(
            decode_cursor("user:a|ListDevices", &c).unwrap(),
            Some((now, id))
        );
        assert!(decode_cursor("user:b|ListDevices", &c).is_err());
        assert!(decode_cursor("user:a|ListConnections", &c).is_err());
        assert!(decode_cursor("user:a|ListDevices", "garbage").is_err());
        assert_eq!(decode_cursor("x", "").unwrap(), None);
    }
}
