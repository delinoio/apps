// SPDX-License-Identifier: FSL-1.1-ALv2
//! Client version, Origin and size policy, quotas, rate limits, deletion deadlines, tenant
//! states and user withdrawal.
mod support;
use anydash_api::worker::Job;
use chrono::{Duration, Utc};
use delino_protocol::anydash::v1::{ClientKind, QuotaKind, RateLimitScope};
use serde_json::json;
use support::{Caller, Fixture, at, id};

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn client_policy_quotas_and_rate_limits() {
    let f = Fixture::setup().await;
    let a = f.person("alice");
    // The minimum version check runs before authentication.
    let old = Caller {
        authorization: None,
        kind: "CLIENT_KIND_DESKTOP",
        version: "0.9.9",
    };
    let e = f
        .fail(
            &old,
            "AccountService/GetAccount",
            json!({}),
            "ERROR_REASON_UPDATE_REQUIRED",
        )
        .await;
    assert_eq!(e.detail.minimum_client_version, "1.0.0");
    assert_eq!(e.detail.client_kind, ClientKind::Desktop);
    assert_eq!(e.code, "failed_precondition");
    let pre = Caller {
        authorization: None,
        kind: "CLIENT_KIND_CLI",
        version: "1.0.0-rc.1",
    };
    f.fail(
        &pre,
        "DeviceService/StartEnrollment",
        json!({}),
        "ERROR_REASON_UPDATE_REQUIRED",
    )
    .await;
    let bad = Caller {
        authorization: None,
        kind: "CLIENT_KIND_UNKNOWN",
        version: "1.0.0",
    };
    let e = f
        .fail(
            &bad,
            "AccountService/GetAccount",
            json!({}),
            "ERROR_REASON_INVALID_ARGUMENT",
        )
        .await;
    assert_eq!(e.detail.field, "X-AnyDash-Client-Kind");
    let bad = Caller {
        authorization: None,
        kind: "CLIENT_KIND_WEB",
        version: "one",
    };
    let e = f
        .fail(
            &bad,
            "AccountService/GetAccount",
            json!({}),
            "ERROR_REASON_INVALID_ARGUMENT",
        )
        .await;
    assert_eq!(e.detail.field, "X-AnyDash-Client-Version");
    // Origin policy: only the web origin, never arbitrary origins.
    let response = f
        .http
        .post(format!("{}/anydash.v1.AccountService/GetAccount", f.base))
        .header("origin", "https://evil.example")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let response = f
        .http
        .request(
            reqwest::Method::OPTIONS,
            format!("{}/anydash.v1.AccountService/GetAccount", f.base),
        )
        .header("origin", "https://anydash.example.test")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://anydash.example.test"
    );
    assert!(
        response
            .headers()
            .get("access-control-allow-credentials")
            .is_none()
    );
    // Health and operator-only metrics.
    assert_eq!(
        f.http
            .get(format!("{}/health/ready", f.base))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        f.http
            .get(format!("{}/metrics", f.base))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let metrics = f
        .http
        .get(format!("{}/metrics", f.base))
        .bearer_auth("synthetic-metrics-token-at-least-32-characters")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(metrics.contains("anydash_api_requests_total{method=\"GetAccount\",outcome=\"client_error\",service=\"AccountService\"}"));
    assert!(metrics.contains("anydash_deletion_jobs_pending 0"));

    f.connect(&a).await;
    let user = f.user(&a);
    let (device, token, _) = f.enroll(&a, "one", "").await;
    let d = f.device(&token);
    // Bodies above 1 MiB are rejected with a typed error.
    let big = json!({"buckets":[{"tool":"LOCAL_TOOL_CODEX","model":"x".repeat(1_100_000)}]});
    f.fail(
        &d,
        "CollectorService/UploadUsage",
        big,
        "ERROR_REASON_PAYLOAD_TOO_LARGE",
    )
    .await;

    // Rate limits: 600 per minute per user token and 60 per device token, shared through
    // PostgreSQL. Pre-fill the current and next windows to avoid a minute boundary.
    for (scope, subject, limit) in [(1i16, a.user_id.clone(), 600), (2, device.clone(), 60)] {
        sqlx::query(
            "INSERT INTO rate_limit_windows(scope,subject,window_start,count) SELECT $1,$2,w,$3 FROM \
             (VALUES (date_bin('60 seconds', clock_timestamp(), TIMESTAMPTZ '2000-01-01')),(date_bin('60 seconds', clock_timestamp(), TIMESTAMPTZ '2000-01-01')+interval '60 seconds')) v(w) \
             ON CONFLICT (scope,subject,window_start) DO UPDATE SET count=$3",
        )
        .bind(scope)
        .bind(subject.parse::<uuid::Uuid>().unwrap().to_string())
        .bind(limit)
        .execute(&f.db)
        .await
        .unwrap();
    }
    let e = f
        .fail(
            &user,
            "AccountService/GetAccount",
            json!({}),
            "ERROR_REASON_RATE_LIMITED",
        )
        .await;
    assert_eq!(e.detail.rate_limit_scope, RateLimitScope::UserToken);
    assert!(
        e.retry_after
            .is_some_and(|r| (1..=60).contains(&r.parse::<u64>().unwrap()))
    );
    assert_eq!(e.code, "resource_exhausted");
    let e = f
        .fail(
            &d,
            "CollectorService/Heartbeat",
            json!({}),
            "ERROR_REASON_RATE_LIMITED",
        )
        .await;
    assert_eq!(e.detail.rate_limit_scope, RateLimitScope::DeviceToken);
    f.exec("DELETE FROM rate_limit_windows").await;
    // Unauthenticated enrollment starts: 10 per minute per client address.
    f.exec(
        "INSERT INTO rate_limit_windows(scope,subject,window_start,count) SELECT 4,'start:127.0.0.1',w,10 FROM \
         (VALUES (date_bin('60 seconds', clock_timestamp(), TIMESTAMPTZ '2000-01-01')),(date_bin('60 seconds', clock_timestamp(), TIMESTAMPTZ '2000-01-01')+interval '60 seconds')) v(w)"
    )
    .await;
    let e = f
        .fail(
            &f.anonymous(),
            "DeviceService/StartEnrollment",
            json!({"deviceName":"x","platform":"DEVICE_PLATFORM_LINUX","architecture":"CPU_ARCHITECTURE_X64","collectorKind":"COLLECTOR_KIND_CLI"}),
            "ERROR_REASON_RATE_LIMITED",
        )
        .await;
    assert_eq!(e.detail.rate_limit_scope, RateLimitScope::Enrollment);
    f.exec("DELETE FROM rate_limit_windows").await;

    // Quotas: 20 devices, 50 connections, 50 dashboards per space, 100 widgets per dashboard.
    f.exec(&format!(
        "INSERT INTO devices(id,user_id,name,platform,architecture,collector_kind,collector_version,os_version,enrolled_at,credential_state) \
         SELECT gen_random_uuid(),'{}','extra',1,1,2,'1.0.0','x',now(),2 FROM generate_series(1,19)",
        a.user_id
    ))
    .await;
    let start = f
        .ok(
            &f.anonymous(),
            "DeviceService/StartEnrollment",
            json!({"deviceName":"21st","platform":"DEVICE_PLATFORM_LINUX","architecture":"CPU_ARCHITECTURE_X64","collectorKind":"COLLECTOR_KIND_CLI"}),
        )
        .await;
    let e = f
        .fail(
            &user,
            "DeviceService/ApproveEnrollment",
            json!({"enrollmentId":start["enrollmentId"],"userCode":start["userCode"]}),
            "ERROR_REASON_QUOTA_EXCEEDED",
        )
        .await;
    let quota = e.detail.quota.as_option().unwrap();
    assert_eq!(
        (quota.kind.as_known(), quota.limit, quota.used),
        (Some(QuotaKind::CollectorDevicesPerUser), 20, 20)
    );
    // Reattaching an existing device does not count against the quota.
    let (again, _, _) = f.enroll(&a, "one", &device).await;
    assert_eq!(again, device);
    f.exec(&format!(
        "INSERT INTO connections(id,user_id,provider,display_name,account_label,account_fingerprint,created_at) SELECT gen_random_uuid(),'{}',1,'c','','',now() FROM generate_series(1,50)",
        a.user_id
    ))
    .await;
    let (_, fresh, _) = f.enroll(&a, "one", &device).await;
    let e = f
        .fail(
            &f.device(&fresh),
            "ConnectionService/CreateConnection",
            json!({"requestId":id(),"provider":"PROVIDER_GROK"}),
            "ERROR_REASON_QUOTA_EXCEEDED",
        )
        .await;
    assert_eq!(e.detail.quota.as_option().unwrap().limit, 50);
    let account = f.ok(&user, "AccountService/GetAccount", json!({})).await;
    let quotas = account["account"]["quotas"].as_array().unwrap();
    assert_eq!(quotas[0]["used"], 20);
    assert_eq!(quotas[1]["used"], 50);
    f.exec(&format!(
        "INSERT INTO dashboards(id,tenant_id,space_kind,created_by_user_id,name,version,created_at,updated_at) SELECT gen_random_uuid(),'{}',1,'{}','d',1,now(),now() FROM generate_series(1,49)",
        a.personal, a.user_id
    ))
    .await;
    let last = f
        .ok(
            &user,
            "DashboardService/CreateDashboard",
            json!({"requestId":id(),"spaceId":a.personal,"name":"Fiftieth"}),
        )
        .await;
    let dash = last["dashboard"]["id"].as_str().unwrap().to_owned();
    let e = f
        .fail(
            &user,
            "DashboardService/CreateDashboard",
            json!({"requestId":id(),"spaceId":a.personal,"name":"Too many"}),
            "ERROR_REASON_QUOTA_EXCEEDED",
        )
        .await;
    assert_eq!(
        e.detail.quota.as_option().unwrap().kind.as_known(),
        Some(QuotaKind::DashboardsPerSpace)
    );
    f.exec(&format!(
        "INSERT INTO widgets(id,dashboard_id,data_owner_user_id,type,x,y,width,height,title,presentation,query,all_devices,sharing_state,sort_order,created_at) \
         SELECT gen_random_uuid(),'{dash}','{}',3,0,0,1,1,'',1,'{{}}',true,1,g,now() FROM generate_series(1,100) g",
        a.user_id
    ))
    .await;
    let e = f
        .fail(
            &user,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash,"expectedVersion":"1","requestId":id(),"type":"WIDGET_TYPE_LOCAL_USAGE_SUMMARY",
                "layout":{"x":0,"y":0,"width":2,"height":2,"presentation":"WIDGET_PRESENTATION_NUMBER"},"query":{"period":"QUERY_PERIOD_TODAY"},"source":{"allDevices":true}}),
            "ERROR_REASON_QUOTA_EXCEEDED",
        )
        .await;
    assert_eq!(e.detail.quota.as_option().unwrap().used, 100);
    f.cleanup().await;
}

/// An `AddWidget` retry that overlaps the original request waits for the dashboard lock before
/// it looks up its request ID, so it returns the original result instead of `VERSION_CONFLICT`.
#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn overlapping_add_widget_retry_replays_the_original() {
    let f = Fixture::setup().await;
    let a = f.person("alice");
    f.connect(&a).await;
    f.enroll(&a, "laptop", "").await;
    let user = f.user(&a);
    let dash = f
        .ok(
            &user,
            "DashboardService/CreateDashboard",
            json!({"requestId":id(),"spaceId":a.personal,"name":"Main"}),
        )
        .await["dashboard"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let body = json!({"dashboardId":dash,"expectedVersion":"1","requestId":id(),"type":"WIDGET_TYPE_LOCAL_USAGE_SUMMARY",
        "layout":{"x":0,"y":0,"width":2,"height":2,"presentation":"WIDGET_PRESENTATION_NUMBER"},"query":{"period":"QUERY_PERIOD_TODAY"},"source":{"allDevices":true}});
    // Hold the dashboard row so that the original request and its retry overlap.
    let mut blocker = f.db.begin().await.unwrap();
    sqlx::query(&format!(
        "SELECT 1 FROM dashboards WHERE id='{dash}' FOR UPDATE"
    ))
    .execute(&mut *blocker)
    .await
    .unwrap();
    let release = async {
        for _ in 0..200 {
            let waiting: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE application_name=current_setting('application_name') AND wait_event_type='Lock'",
            )
            .fetch_one(&f.db)
            .await
            .unwrap();
            if waiting == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        blocker.commit().await.unwrap();
    };
    let (first, retry, ()) = tokio::join!(
        f.call(&user, "DashboardService/AddWidget", body.clone()),
        f.call(&user, "DashboardService/AddWidget", body.clone()),
        release,
    );
    let first = first.expect("original AddWidget");
    let retry = retry.expect("overlapping AddWidget retry");
    assert_eq!(first["widgetId"], retry["widgetId"]);
    assert_eq!(
        f.count(&format!(
            "SELECT count(*) FROM widgets WHERE dashboard_id='{dash}'"
        ))
        .await,
        1
    );
    assert_eq!(
        f.count(&format!("SELECT version FROM dashboards WHERE id='{dash}'"))
            .await,
        2
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn deletion_deadlines_and_account_data_deletion() {
    let f = Fixture::setup().await;
    let a = f.person("alice");
    f.connect(&a).await;
    let user = f.user(&a);
    let (d1, t1, _) = f.enroll(&a, "one", "").await;
    let (d2, t2, _) = f.enroll(&a, "two", "").await;
    let c1 = f.device(&t1);
    let conn = f
        .ok(
            &c1,
            "ConnectionService/CreateConnection",
            json!({"requestId":id(),"provider":"PROVIDER_CODEX"}),
        )
        .await["connection"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.ok(
        &f.device(&t2),
        "ConnectionService/RegisterConnectionDevice",
        json!({"connectionId":conn}),
    )
    .await;
    f.ok(
        &c1,
        "CollectorService/Heartbeat",
        json!({"readyConnectionIds":[conn]}),
    )
    .await;
    f.ok(
        &c1,
        "CollectorService/AcquireLease",
        json!({"connectionId":conn}),
    )
    .await;
    f.ok(
        &c1,
        "CollectorService/ReportSnapshot",
        json!({"connectionId":conn,"leaseEpoch":"1","snapshot":{"observedAt":at(Utc::now()+Duration::seconds(1)),"status":"COLLECTION_STATUS_SUCCEEDED",
            "metrics":[{"kind":"METRIC_KIND_SUBSCRIPTION_USED_PERCENT","dimension":"weekly","value":{"state":"METRIC_STATE_VALUE","percent":12}}]}}),
    )
    .await;
    for (token, model) in [(&t1, "a"), (&t2, "b")] {
        f.ok(
            &f.device(token),
            "CollectorService/UploadUsage",
            json!({"buckets":[{"tool":"LOCAL_TOOL_CODEX","model":model,"bucketStart":at(anydash_api::time::floor_bucket(Utc::now())),"version":"1","sessionCount":"1"}]}),
        )
        .await;
    }
    // Deleting a device blocks it immediately; its usage goes, connection history stays.
    let r = f
        .ok(&user, "DeviceService/DeleteDevice", json!({"deviceId":d1}))
        .await;
    let deadline =
        chrono::DateTime::parse_from_rfc3339(r["purgeDeadline"].as_str().unwrap()).unwrap();
    assert!(
        (deadline.with_timezone(&Utc) - Utc::now() - Duration::hours(24))
            .num_seconds()
            .abs()
            < 60
    );
    f.fail(
        &c1,
        "CollectorService/Heartbeat",
        json!({}),
        "ERROR_REASON_DEVICE_CREDENTIALS_INACTIVE",
    )
    .await;
    assert_eq!(
        f.ok(&user, "DeviceService/ListDevices", json!({})).await["devices"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.count("SELECT count(*) FROM connections WHERE lease_device_id IS NOT NULL")
            .await,
        0
    );
    f.job(Job::Deletion).await;
    assert_eq!(
        f.count(&format!("SELECT count(*) FROM devices WHERE id='{d1}'"))
            .await,
        0
    );
    assert_eq!(f.count("SELECT count(*) FROM usage_buckets").await, 1);
    assert_eq!(f.count("SELECT count(*) FROM metric_hourly").await, 1);
    assert_eq!(f.count("SELECT count(*) FROM deletion_jobs WHERE completed_at IS NOT NULL AND completed_at<=deadline").await, 1);
    // Deleting a connection deletes its history and lease; widgets lose the source.
    let dash = f
        .ok(
            &user,
            "DashboardService/CreateDashboard",
            json!({"requestId":id(),"spaceId":a.personal,"name":"Board"}),
        )
        .await["dashboard"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let widget = f
        .ok(
            &user,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash,"expectedVersion":"1","requestId":id(),"type":"WIDGET_TYPE_SUBSCRIPTION_LIMITS",
                "layout":{"x":0,"y":0,"width":3,"height":2,"presentation":"WIDGET_PRESENTATION_NUMBER"},"query":{},"source":{"connectionIds":[conn]}}),
        )
        .await["widgetId"]
        .as_str()
        .unwrap()
        .to_owned();
    f.ok(
        &user,
        "ConnectionService/DeleteConnection",
        json!({"connectionId":conn}),
    )
    .await;
    f.fail(
        &user,
        "ConnectionService/GetConnection",
        json!({"connectionId":conn}),
        "ERROR_REASON_NOT_FOUND",
    )
    .await;
    let data = f
        .ok(
            &user,
            "WidgetDataService/QueryWidgetData",
            json!({"dashboardId":dash,"widgetId":widget}),
        )
        .await;
    assert_eq!(
        data["data"]["sharingEndReason"],
        "SHARING_END_REASON_SOURCE_DELETED"
    );
    f.job(Job::Deletion).await;
    assert_eq!(
        f.count("SELECT count(*) FROM metric_hourly").await
            + f.count("SELECT count(*) FROM metric_latest").await,
        0
    );
    assert_eq!(f.count("SELECT count(*) FROM connections").await, 0);
    // Deleting a dashboard deletes its widgets.
    f.ok(
        &user,
        "DashboardService/DeleteDashboard",
        json!({"dashboardId":dash}),
    )
    .await;
    f.fail(
        &user,
        "DashboardService/GetDashboard",
        json!({"dashboardId":dash}),
        "ERROR_REASON_NOT_FOUND",
    )
    .await;
    f.job(Job::Deletion).await;
    assert_eq!(
        f.count("SELECT count(*) FROM widgets").await
            + f.count("SELECT count(*) FROM dashboards").await,
        0
    );
    // AnyDash data deletion needs confirmation and keeps the account.
    f.ok(
        &user,
        "DashboardService/CreateDashboard",
        json!({"requestId":id(),"spaceId":a.personal,"name":"Board"}),
    )
    .await;
    f.fail(
        &user,
        "AccountService/DeleteAccountData",
        json!({}),
        "ERROR_REASON_INVALID_ARGUMENT",
    )
    .await;
    f.ok(
        &user,
        "AccountService/DeleteAccountData",
        json!({"confirm":true}),
    )
    .await;
    f.fail(
        &f.device(&t2),
        "CollectorService/Heartbeat",
        json!({}),
        "ERROR_REASON_DEVICE_CREDENTIALS_INACTIVE",
    )
    .await;
    let account = f.ok(&user, "AccountService/GetAccount", json!({})).await;
    assert_eq!(
        account["account"]["onboardingState"],
        "ONBOARDING_STATE_DEVICE_REQUIRED"
    );
    f.job(Job::Deletion).await;
    for table in [
        "devices",
        "usage_buckets",
        "dashboards",
        "connections",
        "device_credentials",
    ] {
        assert_eq!(
            f.count(&format!("SELECT count(*) FROM {table}")).await,
            0,
            "{table}"
        );
    }
    assert_eq!(
        f.count("SELECT count(*) FROM deletion_jobs WHERE completed_at IS NULL")
            .await,
        0
    );
    let events = f.ok(&user, "AccountService/ListActivity", json!({})).await;
    let kinds: Vec<&str> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["ACTIVITY_KIND_ACCOUNT_DATA_DELETION_REQUESTED"]);
    // The user can start again.
    let (fresh, _, _) = f.enroll(&a, "again", &d2).await;
    assert_ne!(fresh, d2);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn account_data_deletion_removes_previously_ended_team_widgets() {
    let f = Fixture::setup().await;
    let owner = f.person("owner");
    let member = f.person("member");
    let team = f.team(
        "Acme",
        &[(&owner, &["ROLE_OWNER"]), (&member, &["ROLE_MEMBER"])],
    );
    f.connect(&owner).await;
    f.connect(&member).await;
    let (o, m) = (f.user(&owner), f.user(&member));
    f.enroll(&member, "laptop", "").await;
    let dash = f
        .ok(
            &o,
            "DashboardService/CreateDashboard",
            json!({"requestId":id(),"spaceId":team,"name":"Team"}),
        )
        .await["dashboard"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let add = |version: &'static str| {
        let (f, m, dash) = (&f, &m, dash.clone());
        async move {
            f.ok(
                m,
                "DashboardService/AddWidget",
                json!({"dashboardId":dash,"expectedVersion":version,"requestId":id(),"type":"WIDGET_TYPE_LOCAL_USAGE_SUMMARY",
                    "layout":{"x":0,"y":0,"width":3,"height":2,"presentation":"WIDGET_PRESENTATION_NUMBER"},"query":{"period":"QUERY_PERIOD_TODAY"},"source":{"allDevices":true}}),
            )
            .await["widgetId"]
                .as_str()
                .unwrap()
                .to_owned()
        }
    };
    let revoked = add("1").await;
    let shared = add("2").await;
    // One widget has already ended for another reason before the deletion request.
    f.ok(
        &m,
        "DashboardService/RevokeWidgetSharing",
        json!({"dashboardId":dash,"widgetId":revoked}),
    )
    .await;
    f.ok(
        &m,
        "AccountService/DeleteAccountData",
        json!({"confirm":true}),
    )
    .await;
    // The ended widget keeps its original reason until the deletion job removes it.
    assert_eq!(
        f.count(&format!(
            "SELECT count(*) FROM widgets WHERE id='{revoked}' AND sharing_end_reason=1"
        ))
        .await,
        1
    );
    // A widget added after the request is new data and is not deleted by it.
    f.enroll(&member, "again", "").await;
    let version = f
        .ok(
            &m,
            "DashboardService/GetDashboard",
            json!({"dashboardId":dash}),
        )
        .await["dashboard"]["version"]
        .as_str()
        .unwrap()
        .to_owned();
    let later = f
        .ok(
            &m,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash,"expectedVersion":version,"requestId":id(),"type":"WIDGET_TYPE_LOCAL_USAGE_SUMMARY",
                "layout":{"x":0,"y":0,"width":3,"height":2,"presentation":"WIDGET_PRESENTATION_NUMBER"},"query":{"period":"QUERY_PERIOD_TODAY"},"source":{"allDevices":true}}),
        )
        .await["widgetId"]
        .as_str()
        .unwrap()
        .to_owned();
    f.job(Job::Deletion).await;
    for widget in [&revoked, &shared] {
        assert_eq!(
            f.count(&format!("SELECT count(*) FROM widgets WHERE id='{widget}'"))
                .await,
            0,
            "{widget}"
        );
    }
    assert_eq!(
        f.count(&format!("SELECT count(*) FROM widgets WHERE id='{later}'"))
            .await,
        1
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn tenant_states_and_user_withdrawal() {
    let f = Fixture::setup().await;
    let owner = f.person("owner");
    let member = f.person("member");
    let team = f.team(
        "Acme",
        &[(&owner, &["ROLE_OWNER"]), (&member, &["ROLE_MEMBER"])],
    );
    f.connect(&owner).await;
    f.connect(&member).await;
    let o = f.user(&owner);
    let m = f.user(&member);
    let (_, member_token, _) = f.enroll(&member, "laptop", "").await;
    let md = f.device(&member_token);
    let dash = f
        .ok(
            &o,
            "DashboardService/CreateDashboard",
            json!({"requestId":id(),"spaceId":team,"name":"Team"}),
        )
        .await["dashboard"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let widget = f
        .ok(
            &m,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash,"expectedVersion":"1","requestId":id(),"type":"WIDGET_TYPE_LOCAL_USAGE_SUMMARY",
                "layout":{"x":0,"y":0,"width":3,"height":2,"presentation":"WIDGET_PRESENTATION_NUMBER"},"query":{"period":"QUERY_PERIOD_TODAY"},"source":{"allDevices":true}}),
        )
        .await["widgetId"]
        .as_str()
        .unwrap()
        .to_owned();
    // BLOCKED and DELETING stop all activity in the space: viewing, editing and uploads.
    for state in [
        "TENANT_STATE_BLOCKED",
        "TENANT_STATE_DELETING",
        "TENANT_STATE_UNKNOWN",
    ] {
        f.set_state(&member.personal, state);
        f.fail(
            &m,
            "AccountService/GetAccount",
            json!({}),
            "ERROR_REASON_SPACE_INACTIVE",
        )
        .await;
        f.fail(
            &md,
            "CollectorService/Heartbeat",
            json!({}),
            "ERROR_REASON_SPACE_INACTIVE",
        )
        .await;
        f.set_state(&member.personal, "TENANT_STATE_ACTIVE");
        f.set_state(&team, state);
        f.fail(
            &m,
            "DashboardService/GetDashboard",
            json!({"dashboardId":dash}),
            "ERROR_REASON_SPACE_INACTIVE",
        )
        .await;
        f.fail(
            &o,
            "WidgetDataService/QueryWidgetData",
            json!({"dashboardId":dash,"widgetId":widget}),
            "ERROR_REASON_SPACE_INACTIVE",
        )
        .await;
        f.set_state(&team, "TENANT_STATE_ACTIVE");
    }
    // Only explicit DELETED authorizes cleanup; UNKNOWN and errors never do.
    f.set_state(&team, "TENANT_STATE_UNKNOWN");
    f.job(Job::TenantLifecycle).await;
    assert_eq!(
        f.count("SELECT count(*) FROM dashboards WHERE deleted_at IS NULL")
            .await,
        1
    );
    f.set_state(&team, "TENANT_STATE_ACTIVE");
    // Delcore errors deny access with DEPENDENCY_UNAVAILABLE.
    f.mock.lock().unwrap().delcore_down = true;
    let e = f
        .fail(
            &m,
            "DashboardService/GetDashboard",
            json!({"dashboardId":dash}),
            "ERROR_REASON_DEPENDENCY_UNAVAILABLE",
        )
        .await;
    assert_eq!(e.code, "unavailable");
    f.mock.lock().unwrap().delcore_down = false;

    // User withdrawal: the member's personal tenant is deleted.
    f.set_state(&member.personal, "TENANT_STATE_DELETED");
    f.job(Job::TenantLifecycle).await;
    f.fail(
        &md,
        "CollectorService/Heartbeat",
        json!({}),
        "ERROR_REASON_DEVICE_CREDENTIALS_INACTIVE",
    )
    .await;
    let data = f
        .ok(
            &o,
            "WidgetDataService/QueryWidgetData",
            json!({"dashboardId":dash,"widgetId":widget}),
        )
        .await;
    assert_eq!(data["data"]["sharingState"], "SHARING_STATE_ENDED");
    assert_eq!(
        data["data"]["sharingEndReason"],
        "SHARING_END_REASON_OWNER_DATA_DELETED"
    );
    f.job(Job::Deletion).await;
    assert_eq!(
        f.count(&format!(
            "SELECT count(*) FROM accounts WHERE user_id='{}'",
            member.user_id
        ))
        .await,
        0
    );
    assert_eq!(
        f.count(&format!(
            "SELECT count(*) FROM devices WHERE user_id='{}'",
            member.user_id
        ))
        .await,
        0
    );
    assert_eq!(
        f.count(&format!(
            "SELECT count(*) FROM widgets WHERE data_owner_user_id='{}'",
            member.user_id
        ))
        .await,
        0
    );
    // The withdrawal is enqueued once.
    f.job(Job::TenantLifecycle).await;
    assert_eq!(
        f.count("SELECT count(*) FROM deletion_jobs WHERE kind=5")
            .await,
        1
    );

    // A deleted team tenant deletes its team dashboards.
    f.set_state(&team, "TENANT_STATE_DELETED");
    f.job(Job::TenantLifecycle).await;
    f.fail(
        &o,
        "DashboardService/GetDashboard",
        json!({"dashboardId":dash}),
        "ERROR_REASON_NOT_FOUND",
    )
    .await;
    f.job(Job::Deletion).await;
    assert_eq!(f.count("SELECT count(*) FROM dashboards").await, 0);
    // The owner's personal space is unaffected.
    f.ok(&o, "AccountService/GetAccount", json!({})).await;
    f.cleanup().await;
}

/// Layout validation names the violating layout field in `ErrorDetail.field` for both
/// `AddWidget` and `UpdateWidgetLayout`.
#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn widget_layout_errors_name_the_violating_field() {
    let f = Fixture::setup().await;
    let a = f.person("alice");
    let user = f.user(&a);
    f.connect(&a).await;
    f.enroll(&a, "laptop", "").await;
    let dash = f
        .ok(
            &user,
            "DashboardService/CreateDashboard",
            json!({"requestId":id(),"spaceId":a.personal,"name":"Layouts"}),
        )
        .await["dashboard"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let layout = |x: u32, y: u32, width: u32, height: u32| json!({"x":x,"y":y,"width":width,"height":height,"presentation":"WIDGET_PRESENTATION_NUMBER"});
    let add = |version: &str, layout: serde_json::Value| {
        json!({"dashboardId":dash,"expectedVersion":version,"requestId":id(),"type":"WIDGET_TYPE_LOCAL_USAGE_SUMMARY",
            "layout":layout,"query":{"period":"QUERY_PERIOD_TODAY"},"source":{"allDevices":true}})
    };
    for (bad, field) in [
        (layout(12, 0, 1, 1), "layout.x"),
        (layout(0, 0, 0, 1), "layout.width"),
        (layout(0, 0, 13, 1), "layout.width"),
        (layout(8, 0, 6, 1), "layout.width"),
        (layout(0, 1000, 1, 1), "layout.y"),
        (layout(0, 0, 1, 0), "layout.height"),
        (layout(0, 0, 1, 13), "layout.height"),
    ] {
        let e = f
            .fail(
                &user,
                "DashboardService/AddWidget",
                add("1", bad.clone()),
                "ERROR_REASON_INVALID_ARGUMENT",
            )
            .await;
        assert_eq!(e.detail.field, field, "layout {bad}");
    }

    // The far corner of the grid is valid.
    let edge = f
        .ok(
            &user,
            "DashboardService/AddWidget",
            add("1", layout(11, 999, 1, 12)),
        )
        .await;
    let widget = edge["widgetId"].as_str().unwrap().to_owned();
    let version = edge["dashboard"]["version"].as_str().unwrap().to_owned();
    let second = f
        .ok(
            &user,
            "DashboardService/AddWidget",
            add(&version, layout(0, 0, 2, 2)),
        )
        .await;
    let other = second["widgetId"].as_str().unwrap().to_owned();
    let version = second["dashboard"]["version"].as_str().unwrap().to_owned();

    let update = |updates: serde_json::Value| json!({"dashboardId":dash,"expectedVersion":version,"updates":updates});
    let e = f
        .fail(
            &user,
            "DashboardService/UpdateWidgetLayout",
            update(json!([
                {"widgetId":widget,"layout":layout(0, 0, 12, 12)},
                {"widgetId":other,"layout":layout(0, 0, 1, 13)},
            ])),
            "ERROR_REASON_INVALID_ARGUMENT",
        )
        .await;
    assert_eq!(e.detail.field, "updates[1].layout.height");
    f.ok(
        &user,
        "DashboardService/UpdateWidgetLayout",
        update(json!([{"widgetId":widget,"layout":layout(0, 0, 12, 12)}])),
    )
    .await;
    f.cleanup().await;
}
