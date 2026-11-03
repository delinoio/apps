// SPDX-License-Identifier: FSL-1.1-ALv2
//! Enrollment, device credentials, versioned usage uploads, retention and LiteLLM pricing on a
//! disposable PostgreSQL database with mocked Logto, Delcore and pricing sources.
mod support;
use anydash_api::worker::Job;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use support::{Fixture, at, id};

fn bucket_start(back: Duration) -> chrono::DateTime<Utc> {
    anydash_api::time::floor_bucket(Utc::now() - back)
}

fn bucket(
    tool: &str,
    model: &str,
    start: chrono::DateTime<Utc>,
    version: u64,
    sessions: u64,
    tokens: Value,
) -> Value {
    json!({"tool":tool,"model":model,"bucketStart":at(start),"version":version.to_string(),"sessionCount":sessions.to_string(),"tokens":tokens})
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn enrollment_codes_and_device_credentials() {
    let f = Fixture::setup().await;
    let a = f.person("alice");
    let user = f.user(&a);
    // Every user-token method needs a connected account first.
    f.fail(
        &user,
        "AccountService/GetAccount",
        json!({}),
        "ERROR_REASON_ACCOUNT_NOT_CONNECTED",
    )
    .await;
    let account = f.connect(&a).await;
    assert_eq!(account["account"]["userId"], a.user_id);
    assert_eq!(account["account"]["personalSpaceId"], a.personal);
    assert_eq!(
        account["account"]["onboardingState"],
        "ONBOARDING_STATE_DEVICE_REQUIRED"
    );
    // ConnectAccount is idempotent and rejects a Delcore token of another subject.
    f.connect(&a).await;
    let b = f.person("bob");
    f.fail(
        &user,
        "AccountService/ConnectAccount",
        json!({"delcoreAccessToken": f.token(&b, support::DELCORE_RESOURCE)}),
        "ERROR_REASON_UNAUTHENTICATED",
    )
    .await;
    // Personal onboarding: the personal dashboard needs a connected collector device.
    f.fail(
        &user,
        "DashboardService/ListDashboards",
        json!({"spaceId":a.personal}),
        "ERROR_REASON_ONBOARDING_REQUIRED",
    )
    .await;

    let start = |name: &str| {
        let f = &f;
        let name = name.to_owned();
        async move {
            f.ok(
                &f.anonymous(),
                "DeviceService/StartEnrollment",
                json!({"deviceName":name,"platform":"DEVICE_PLATFORM_LINUX","architecture":"CPU_ARCHITECTURE_X64","collectorKind":"COLLECTOR_KIND_CLI","collectorVersion":"1.0.0","osVersion":"24.04"}),
            )
            .await
        }
    };
    // Five failed attempts invalidate the code.
    let e = start("laptop").await;
    assert!(
        e["verificationUrl"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/connect/{}", e["enrollmentId"].as_str().unwrap()))
    );
    assert!(
        !e["verificationUrl"]
            .as_str()
            .unwrap()
            .contains(e["userCode"].as_str().unwrap())
    );
    assert_eq!(e["pollInterval"], "5s");
    let summary = f
        .ok(
            &user,
            "DeviceService/GetEnrollment",
            json!({"enrollmentId":e["enrollmentId"]}),
        )
        .await;
    assert_eq!(summary["enrollment"]["state"], "ENROLLMENT_STATE_PENDING");
    assert_eq!(summary["enrollment"]["remainingAttempts"], 5);
    let pending = f
        .ok(
            &f.anonymous(),
            "DeviceService/PollEnrollment",
            json!({"enrollmentId":e["enrollmentId"],"deviceCode":e["deviceCode"]}),
        )
        .await;
    assert_eq!(pending["state"], "ENROLLMENT_STATE_PENDING");
    let fast = f
        .fail(
            &f.anonymous(),
            "DeviceService/PollEnrollment",
            json!({"enrollmentId":e["enrollmentId"],"deviceCode":e["deviceCode"]}),
            "ERROR_REASON_RATE_LIMITED",
        )
        .await;
    assert_eq!(
        fast.detail.rate_limit_scope,
        delino_protocol::anydash::v1::RateLimitScope::Enrollment
    );
    assert!(fast.retry_after.is_some());
    f.fail(
        &f.anonymous(),
        "DeviceService/PollEnrollment",
        json!({"enrollmentId":e["enrollmentId"],"deviceCode":"x".repeat(43)}),
        "ERROR_REASON_UNAUTHENTICATED",
    )
    .await;
    for _ in 0..5 {
        let mismatch = f
            .fail(
                &user,
                "DeviceService/ApproveEnrollment",
                json!({"enrollmentId":e["enrollmentId"],"userCode":"BBBB-BBBB"}),
                "ERROR_REASON_ENROLLMENT_CODE_MISMATCH",
            )
            .await;
        assert_eq!(mismatch.detail.field, "user_code");
    }
    f.fail(
        &user,
        "DeviceService/ApproveEnrollment",
        json!({"enrollmentId":e["enrollmentId"],"userCode":e["userCode"]}),
        "ERROR_REASON_ENROLLMENT_INVALIDATED",
    )
    .await;
    // Codes expire 10 minutes after the start.
    let e = start("expired").await;
    f.exec(&format!(
        "UPDATE enrollments SET expires_at=now()-interval '1 second' WHERE id='{}'",
        e["enrollmentId"].as_str().unwrap()
    ))
    .await;
    f.fail(
        &user,
        "DeviceService/ApproveEnrollment",
        json!({"enrollmentId":e["enrollmentId"],"userCode":e["userCode"]}),
        "ERROR_REASON_ENROLLMENT_EXPIRED",
    )
    .await;
    // An approved enrollment that no poll consumed before the code expiry delivers no credentials.
    let e = start("late").await;
    let late_device = f
        .ok(
            &user,
            "DeviceService/ApproveEnrollment",
            json!({"enrollmentId":e["enrollmentId"],"userCode":e["userCode"]}),
        )
        .await["device"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.exec(&format!(
        "UPDATE enrollments SET expires_at=now()-interval '1 second' WHERE id='{}'",
        e["enrollmentId"].as_str().unwrap()
    ))
    .await;
    f.fail(
        &f.anonymous(),
        "DeviceService/PollEnrollment",
        json!({"enrollmentId":e["enrollmentId"],"deviceCode":e["deviceCode"]}),
        "ERROR_REASON_ENROLLMENT_EXPIRED",
    )
    .await;
    f.exec("UPDATE enrollments SET last_polled_at=NULL").await;
    f.fail(
        &f.anonymous(),
        "DeviceService/PollEnrollment",
        json!({"enrollmentId":e["enrollmentId"],"deviceCode":e["deviceCode"]}),
        "ERROR_REASON_ENROLLMENT_EXPIRED",
    )
    .await;
    assert_eq!(
        f.ok(
            &user,
            "DeviceService/GetEnrollment",
            json!({"enrollmentId":e["enrollmentId"]}),
        )
        .await["enrollment"]["state"],
        "ENROLLMENT_STATE_EXPIRED"
    );
    assert_eq!(
        f.count(&format!(
            "SELECT count(*) FROM device_credentials WHERE device_id='{late_device}'"
        ))
        .await,
        0
    );
    // The device the approval created stays logged out until the user removes it.
    f.ok(
        &user,
        "DeviceService/DeleteDevice",
        json!({"deviceId":late_device}),
    )
    .await;
    // Rejection invalidates polling.
    let e = start("unexpected").await;
    f.ok(
        &user,
        "DeviceService/RejectEnrollment",
        json!({"enrollmentId":e["enrollmentId"]}),
    )
    .await;
    f.fail(
        &f.anonymous(),
        "DeviceService/PollEnrollment",
        json!({"enrollmentId":e["enrollmentId"],"deviceCode":e["deviceCode"]}),
        "ERROR_REASON_ENROLLMENT_INVALIDATED",
    )
    .await;
    // A code works once; credentials are delivered by the first poll after approval only.
    let e = start("desk").await;
    // Lower-case and spaced codes are accepted.
    let code = e["userCode"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .replace('-', " ");
    let approved = f
        .ok(
            &user,
            "DeviceService/ApproveEnrollment",
            json!({"enrollmentId":e["enrollmentId"],"userCode":code}),
        )
        .await;
    let device = approved["device"]["id"].as_str().unwrap().to_owned();
    f.fail(
        &user,
        "DeviceService/ApproveEnrollment",
        json!({"enrollmentId":e["enrollmentId"],"userCode":e["userCode"]}),
        "ERROR_REASON_ENROLLMENT_ALREADY_USED",
    )
    .await;
    let polled = f
        .ok(
            &f.anonymous(),
            "DeviceService/PollEnrollment",
            json!({"enrollmentId":e["enrollmentId"],"deviceCode":e["deviceCode"]}),
        )
        .await;
    assert_eq!(polled["state"], "ENROLLMENT_STATE_CONSUMED");
    let access = polled["credentials"]["accessToken"]
        .as_str()
        .unwrap()
        .to_owned();
    let refresh = polled["credentials"]["refreshToken"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(access.starts_with("ada_") && access.len() == 47);
    assert!(refresh.starts_with("adr_") && refresh.len() == 47);
    f.exec("UPDATE enrollments SET last_polled_at=NULL").await;
    f.fail(
        &f.anonymous(),
        "DeviceService/PollEnrollment",
        json!({"enrollmentId":e["enrollmentId"],"deviceCode":e["deviceCode"]}),
        "ERROR_REASON_ENROLLMENT_ALREADY_USED",
    )
    .await;
    // Only digests are stored.
    assert_eq!(f.count(&format!("SELECT count(*) FROM device_credentials WHERE access_token_sha256='{access}' OR refresh_token_sha256='{refresh}'")).await, 0);
    assert_eq!(
        f.count("SELECT count(*) FROM enrollments WHERE length(user_code_sha256)<>64")
            .await,
        0
    );
    let account = f.ok(&user, "AccountService/GetAccount", json!({})).await;
    assert_eq!(
        account["account"]["onboardingState"],
        "ONBOARDING_STATE_READY"
    );
    f.ok(
        &user,
        "DashboardService/ListDashboards",
        json!({"spaceId":a.personal}),
    )
    .await;
    f.ok(
        &f.device(&access),
        "CollectorService/Heartbeat",
        json!({"collectorVersion":"1.0.0","osVersion":"24.04"}),
    )
    .await;
    // A device token is the wrong credential for user methods and vice versa.
    f.fail(
        &f.device(&access),
        "AccountService/GetAccount",
        json!({}),
        "ERROR_REASON_UNAUTHENTICATED",
    )
    .await;
    f.fail(
        &user,
        "CollectorService/Heartbeat",
        json!({}),
        "ERROR_REASON_UNAUTHENTICATED",
    )
    .await;

    // Invalid refresh tokens never count toward the device limit: they are authenticated first
    // and counted per client address, so knowing a device ID cannot exhaust its quota.
    let device_windows = format!(
        "SELECT COALESCE(sum(count),0)::int8 FROM rate_limit_windows WHERE scope=2 AND subject='{device}'"
    );
    let charged = f.count(&device_windows).await;
    let forged = format!("adr_{}", "A".repeat(43));
    for _ in 0..5 {
        f.fail(
            &f.anonymous(),
            "DeviceService/RefreshDeviceToken",
            json!({"deviceId":device,"refreshToken":forged}),
            "ERROR_REASON_UNAUTHENTICATED",
        )
        .await;
    }
    f.fail(
        &f.anonymous(),
        "DeviceService/RevokeDeviceCredentials",
        json!({"deviceId":device,"refreshToken":forged}),
        "ERROR_REASON_UNAUTHENTICATED",
    )
    .await;
    assert_eq!(f.count(&device_windows).await, charged);
    assert_eq!(
        f.count("SELECT COALESCE(sum(count),0)::int8 FROM rate_limit_windows WHERE scope=2 AND subject='invalid-refresh:127.0.0.1'")
            .await,
        6
    );
    // Over the invalid-token limit, refresh-token requests from that address are rejected before
    // the token is checked. Pre-fill the current and next windows to avoid a minute boundary.
    f.exec(
        "INSERT INTO rate_limit_windows(scope,subject,window_start,count) SELECT 2,'invalid-refresh:127.0.0.1',w,60 FROM \
         (VALUES (date_bin('60 seconds', clock_timestamp(), TIMESTAMPTZ '2000-01-01')),(date_bin('60 seconds', clock_timestamp(), TIMESTAMPTZ '2000-01-01')+interval '60 seconds')) v(w) \
         ON CONFLICT (scope,subject,window_start) DO UPDATE SET count=60",
    )
    .await;
    for token in [&forged, &refresh] {
        let e = f
            .fail(
                &f.anonymous(),
                "DeviceService/RefreshDeviceToken",
                json!({"deviceId":device,"refreshToken":token}),
                "ERROR_REASON_RATE_LIMITED",
            )
            .await;
        assert_eq!(
            e.detail.rate_limit_scope,
            delino_protocol::anydash::v1::RateLimitScope::DeviceToken
        );
    }
    f.exec("DELETE FROM rate_limit_windows WHERE subject='invalid-refresh:127.0.0.1'")
        .await;
    assert_eq!(f.count(&device_windows).await, charged);

    // Refresh rotates both tokens; the old access token stops working.
    let rotated = f
        .ok(
            &f.anonymous(),
            "DeviceService/RefreshDeviceToken",
            json!({"deviceId":device,"refreshToken":refresh}),
        )
        .await;
    let access2 = rotated["credentials"]["accessToken"]
        .as_str()
        .unwrap()
        .to_owned();
    let refresh2 = rotated["credentials"]["refreshToken"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(refresh2, refresh);
    f.fail(
        &f.device(&access),
        "CollectorService/Heartbeat",
        json!({}),
        "ERROR_REASON_UNAUTHENTICATED",
    )
    .await;
    f.ok(&f.device(&access2), "CollectorService/Heartbeat", json!({}))
        .await;
    // Reuse of a rotated refresh token revokes the device credentials.
    f.fail(
        &f.anonymous(),
        "DeviceService/RefreshDeviceToken",
        json!({"deviceId":device,"refreshToken":refresh}),
        "ERROR_REASON_REFRESH_TOKEN_REUSED",
    )
    .await;
    f.fail(
        &f.device(&access2),
        "CollectorService/Heartbeat",
        json!({}),
        "ERROR_REASON_DEVICE_CREDENTIALS_INACTIVE",
    )
    .await;
    f.fail(
        &f.anonymous(),
        "DeviceService/RefreshDeviceToken",
        json!({"deviceId":device,"refreshToken":refresh2}),
        "ERROR_REASON_DEVICE_CREDENTIALS_INACTIVE",
    )
    .await;
    let devices = f.ok(&user, "DeviceService/ListDevices", json!({})).await;
    assert_eq!(
        devices["devices"][0]["credentialState"],
        "DEVICE_CREDENTIAL_STATE_REUSE_REVOKED"
    );

    // Signing in again reattaches the same device.
    let e = f
        .ok(
            &f.anonymous(),
            "DeviceService/StartEnrollment",
            json!({"deviceName":"desk","platform":"DEVICE_PLATFORM_LINUX","architecture":"CPU_ARCHITECTURE_X64","collectorKind":"COLLECTOR_KIND_CLI","collectorVersion":"1.0.0","osVersion":"24.04","previousDeviceId":device}),
        )
        .await;
    let summary = f
        .ok(
            &user,
            "DeviceService/GetEnrollment",
            json!({"enrollmentId":e["enrollmentId"]}),
        )
        .await;
    assert_eq!(summary["enrollment"]["reattach"], true);
    // Another user approving the same code gets a new device, never the previous one.
    f.connect(&b).await;
    let other = f
        .ok(
            &f.user(&b),
            "DeviceService/GetEnrollment",
            json!({"enrollmentId":e["enrollmentId"]}),
        )
        .await;
    assert_ne!(other["enrollment"]["reattach"], true);
    let approved = f
        .ok(
            &user,
            "DeviceService/ApproveEnrollment",
            json!({"enrollmentId":e["enrollmentId"],"userCode":e["userCode"]}),
        )
        .await;
    assert_eq!(approved["device"]["id"], device);
    let polled = f
        .ok(
            &f.anonymous(),
            "DeviceService/PollEnrollment",
            json!({"enrollmentId":e["enrollmentId"],"deviceCode":e["deviceCode"]}),
        )
        .await;
    let access3 = polled["credentials"]["accessToken"]
        .as_str()
        .unwrap()
        .to_owned();
    let refresh3 = polled["credentials"]["refreshToken"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        polled["device"]["credentialState"],
        "DEVICE_CREDENTIAL_STATE_ACTIVE"
    );
    // Upload data, then log out: the registration and data remain.
    let up = f
        .ok(
            &f.device(&access3),
            "CollectorService/UploadUsage",
            json!({"buckets":[bucket("LOCAL_TOOL_CODEX","gpt-5",bucket_start(Duration::hours(1)),1,1,json!({"inputTokens":"1","outputTokens":"1","cacheReadTokens":"0"}))]}),
        )
        .await;
    assert_eq!(up["results"][0]["outcome"], "BUCKET_OUTCOME_APPLIED");
    f.ok(
        &f.anonymous(),
        "DeviceService/RevokeDeviceCredentials",
        json!({"deviceId":device,"refreshToken":refresh3}),
    )
    .await;
    f.fail(
        &f.device(&access3),
        "CollectorService/Heartbeat",
        json!({}),
        "ERROR_REASON_UNAUTHENTICATED",
    )
    .await;
    let devices = f.ok(&user, "DeviceService/ListDevices", json!({})).await;
    assert_eq!(devices["devices"].as_array().unwrap().len(), 1);
    assert_eq!(
        devices["devices"][0]["credentialState"],
        "DEVICE_CREDENTIAL_STATE_LOGGED_OUT"
    );
    assert_eq!(f.count("SELECT count(*) FROM usage_buckets").await, 1);
    let (again, access4, refresh4) = f.enroll(&a, "desk", &device).await;
    assert_eq!(again, device);
    // A refresh token expires 90 days after the latest accepted upload.
    f.exec("UPDATE device_credentials SET refresh_expires_at=now()-interval '1 second'")
        .await;
    f.fail(
        &f.anonymous(),
        "DeviceService/RefreshDeviceToken",
        json!({"deviceId":device,"refreshToken":refresh4}),
        "ERROR_REASON_DEVICE_CREDENTIALS_INACTIVE",
    )
    .await;
    f.fail(
        &f.device(&access4),
        "CollectorService/Heartbeat",
        json!({}),
        "ERROR_REASON_DEVICE_CREDENTIALS_INACTIVE",
    )
    .await;
    // The daily job marks inactive devices expired.
    let (second, _, _) = f.enroll(&a, "second", "").await;
    f.exec(&format!("UPDATE device_credentials SET refresh_expires_at=now()-interval '1 second' WHERE device_id='{second}'")).await;
    f.job(Job::CredentialExpiry).await;
    let devices = f.ok(&user, "DeviceService/ListDevices", json!({})).await;
    assert!(
        devices["devices"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["credentialState"] == "DEVICE_CREDENTIAL_STATE_EXPIRED")
    );
    // A user can revoke the credentials of their own device.
    let (third, access5, _) = f.enroll(&a, "third", "").await;
    f.ok(
        &user,
        "DeviceService/RevokeDeviceCredentials",
        json!({"deviceId":third}),
    )
    .await;
    f.fail(
        &f.device(&access5),
        "CollectorService/Heartbeat",
        json!({}),
        "ERROR_REASON_DEVICE_CREDENTIALS_INACTIVE",
    )
    .await;
    f.fail(
        &f.user(&b),
        "DeviceService/RevokeDeviceCredentials",
        json!({"deviceId":third}),
        "ERROR_REASON_NOT_FOUND",
    )
    .await;
    // Activity covers enrollment, reattachment, logout, revocation and invalidation.
    let activity = f.ok(&user, "AccountService/ListActivity", json!({})).await;
    let kinds: Vec<&str> = activity["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    for kind in [
        "ACTIVITY_KIND_DEVICE_ENROLLED",
        "ACTIVITY_KIND_DEVICE_REATTACHED",
        "ACTIVITY_KIND_DEVICE_LOGGED_OUT",
        "ACTIVITY_KIND_DEVICE_CREDENTIALS_REVOKED",
        "ACTIVITY_KIND_ENROLLMENT_INVALIDATED",
    ] {
        assert!(kinds.contains(&kind), "{kind} in {kinds:?}");
    }
    let page = f
        .ok(
            &user,
            "AccountService/ListActivity",
            json!({"page":{"size":2}}),
        )
        .await;
    assert_eq!(page["events"].as_array().unwrap().len(), 2);
    let next = page["nextCursor"].as_str().unwrap();
    let page2 = f
        .ok(
            &user,
            "AccountService/ListActivity",
            json!({"page":{"size":2,"cursor":next}}),
        )
        .await;
    assert_ne!(page2["events"][0]["id"], page["events"][0]["id"]);
    // Cursors are bound to the caller and filters.
    f.fail(
        &f.user(&b),
        "AccountService/ListActivity",
        json!({"page":{"size":2,"cursor":next}}),
        "ERROR_REASON_INVALID_ARGUMENT",
    )
    .await;
    // No token or code is ever stored in plain text in audit rows.
    assert_eq!(
        f.count(&format!(
            "SELECT count(*) FROM audit_events WHERE correlation_id IN ('{access}','{refresh}')"
        ))
        .await,
        0
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn versioned_uploads_retention_and_estimated_cost() {
    let f = Fixture::setup().await;
    let a = f.person("alice");
    f.connect(&a).await;
    let (device, access, _) = f.enroll(&a, "laptop", "").await;
    let d = f.device(&access);
    // No price version yet: costs cannot be calculated.
    let early = bucket_start(Duration::hours(30));
    let r = f
        .ok(&d, "CollectorService/UploadUsage", json!({"buckets":[bucket("LOCAL_TOOL_CODEX","gpt-5",early,1,1,json!({"inputTokens":"10","outputTokens":"1","cacheReadTokens":"0"}))]}))
        .await;
    assert_eq!(r["results"][0]["outcome"], "BUCKET_OUTCOME_APPLIED");
    assert_eq!(f.count("SELECT count(*) FROM usage_buckets WHERE NOT cost_computable AND price_version_id IS NULL").await, 1);
    f.job(Job::PriceSync).await;
    assert_eq!(f.count("SELECT count(*) FROM price_versions").await, 1);
    let v1: uuid::Uuid = sqlx::query_scalar("SELECT id FROM price_versions")
        .fetch_one(&f.db)
        .await
        .unwrap();

    let b1 = bucket_start(Duration::hours(2));
    let tokens = json!({"inputTokens":"2000","outputTokens":"100","cacheReadTokens":"500"});
    let up = |buckets: Vec<Value>| {
        let f = &f;
        let d = &d;
        async move {
            f.ok(
                d,
                "CollectorService/UploadUsage",
                json!({"buckets":buckets}),
            )
            .await
        }
    };
    let r = up(vec![bucket(
        "LOCAL_TOOL_CODEX",
        "gpt-5",
        b1,
        10,
        2,
        tokens.clone(),
    )])
    .await;
    assert_eq!(r["results"][0]["outcome"], "BUCKET_OUTCOME_APPLIED");
    assert_eq!(r["results"][0]["storedVersion"], "10");
    assert!(r["retentionStart"].is_string());
    // The same version is ignored even with different content; resends never add usage twice.
    let r = up(vec![bucket(
        "LOCAL_TOOL_CODEX",
        "gpt-5",
        b1,
        10,
        9,
        json!({"inputTokens":"9"}),
    )])
    .await;
    assert_eq!(
        r["results"][0]["outcome"],
        "BUCKET_OUTCOME_SAME_VERSION_IGNORED"
    );
    let r = up(vec![bucket(
        "LOCAL_TOOL_CODEX",
        "gpt-5",
        b1,
        5,
        9,
        json!({"inputTokens":"9"}),
    )])
    .await;
    assert_eq!(
        r["results"][0]["outcome"],
        "BUCKET_OUTCOME_OLDER_VERSION_IGNORED"
    );
    assert_eq!(r["results"][0]["storedVersion"], "10");
    let (sessions, cost): (i64, String) = sqlx::query_as(
        "SELECT session_count,estimated_cost_usd::text FROM usage_buckets WHERE bucket_start=$1",
    )
    .bind(b1)
    .fetch_one(&f.db)
    .await
    .unwrap();
    assert_eq!(sessions, 2);
    // 2000 × 0.00000125 + 100 × 0.00001 + 500 × 0.000000125
    assert_eq!(cost, "0.003562500000");
    // A newer version replaces the bucket.
    up(vec![bucket(
        "LOCAL_TOOL_CODEX",
        "gpt-5",
        b1,
        20,
        3,
        json!({"inputTokens":"4000","outputTokens":"100","cacheReadTokens":"500"}),
    )])
    .await;
    let sessions: i64 =
        sqlx::query_scalar("SELECT session_count FROM usage_buckets WHERE bucket_start=$1")
            .bind(b1)
            .fetch_one(&f.db)
            .await
            .unwrap();
    assert_eq!(sessions, 3);
    // Corrections within 90 days are accepted; older buckets and future buckets are not.
    let r = up(vec![
        bucket(
            "LOCAL_TOOL_CODEX",
            "gpt-5",
            bucket_start(Duration::days(89)),
            1,
            1,
            tokens.clone(),
        ),
        bucket(
            "LOCAL_TOOL_CODEX",
            "gpt-5",
            bucket_start(Duration::days(91)),
            1,
            1,
            tokens.clone(),
        ),
        bucket(
            "LOCAL_TOOL_CODEX",
            "gpt-5",
            anydash_api::time::floor_bucket(Utc::now() + Duration::hours(2)),
            1,
            1,
            tokens.clone(),
        ),
    ])
    .await;
    let outcomes: Vec<&str> = r["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(
        outcomes,
        [
            "BUCKET_OUTCOME_APPLIED",
            "BUCKET_OUTCOME_OUTSIDE_RETENTION",
            "BUCKET_OUTCOME_INVALID"
        ]
    );
    // Whole-request validation.
    let misaligned = bucket(
        "LOCAL_TOOL_CODEX",
        "gpt-5",
        b1 + Duration::minutes(1),
        1,
        1,
        tokens.clone(),
    );
    let e = f
        .fail(
            &d,
            "CollectorService/UploadUsage",
            json!({"buckets":[misaligned]}),
            "ERROR_REASON_INVALID_ARGUMENT",
        )
        .await;
    assert_eq!(e.detail.field, "buckets[0].bucket_start");
    let dup = bucket("LOCAL_TOOL_CODEX", "gpt-5", b1, 30, 1, tokens.clone());
    f.fail(
        &d,
        "CollectorService/UploadUsage",
        json!({"buckets":[dup.clone(),dup]}),
        "ERROR_REASON_INVALID_ARGUMENT",
    )
    .await;
    f.fail(
        &d,
        "CollectorService/UploadUsage",
        json!({"buckets":[bucket("LOCAL_TOOL_CODEX","gpt-5",b1,0,1,tokens.clone())]}),
        "ERROR_REASON_INVALID_ARGUMENT",
    )
    .await;
    let many: Vec<Value> = (0..2001)
        .map(|i| bucket("LOCAL_TOOL_CODEX", &format!("m{i}"), b1, 1, 0, json!({})))
        .collect();
    f.fail(
        &d,
        "CollectorService/UploadUsage",
        json!({"buckets":many}),
        "ERROR_REASON_PAYLOAD_TOO_LARGE",
    )
    .await;

    // Claude Code needs the cache-write count; unknown models have no price.
    up(vec![
        bucket("LOCAL_TOOL_CLAUDE_CODE", "claude-opus-4", b1, 1, 1, json!({"inputTokens":"10","outputTokens":"10","cacheReadTokens":"10"})),
        bucket("LOCAL_TOOL_CODEX", "unknown-model", b1, 1, 1, tokens.clone()),
        bucket("LOCAL_TOOL_CLAUDE_CODE", "claude-opus-4", b1 - Duration::minutes(15), 1, 4, json!({"inputTokens":"100","outputTokens":"10","cacheReadTokens":"0","cacheWriteTokens":"100"})),
    ])
    .await;
    assert_eq!(f.count("SELECT count(*) FROM usage_buckets WHERE tool=2 AND model='claude-opus-4' AND cost_computable").await, 1);
    assert_eq!(
        f.count(
            "SELECT count(*) FROM usage_buckets WHERE model='unknown-model' AND NOT cost_computable"
        )
        .await,
        1
    );
    // The anthropic/ prefixed LiteLLM key matched: 100 × 0.000015 + 10 × 0.000075 + 100 × 0.00001875.
    let claude: String = sqlx::query_scalar(
        "SELECT estimated_cost_usd::text FROM usage_buckets WHERE tool=2 AND cost_computable",
    )
    .fetch_one(&f.db)
    .await
    .unwrap();
    assert_eq!(claude, "0.004125000000");

    // A price change creates a new version; past buckets keep their pinned version and value.
    f.mock.lock().unwrap().prices.1 = support::price_document(0.000002);
    f.job(Job::PriceSync).await;
    assert_eq!(f.count("SELECT count(*) FROM price_versions").await, 2);
    let pinned: (uuid::Uuid, String) = sqlx::query_as("SELECT price_version_id,estimated_cost_usd::text FROM usage_buckets WHERE bucket_start=$1 AND model='gpt-5'")
        .bind(b1)
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(pinned.0, v1);
    // A correction is recomputed with the same pinned version.
    up(vec![bucket(
        "LOCAL_TOOL_CODEX",
        "gpt-5",
        b1,
        40,
        3,
        tokens.clone(),
    )])
    .await;
    let corrected: (uuid::Uuid, String) = sqlx::query_as("SELECT price_version_id,estimated_cost_usd::text FROM usage_buckets WHERE bucket_start=$1 AND model='gpt-5'")
        .bind(b1)
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(corrected, (v1, "0.003562500000".into()));
    // New usage after the new version takes effect uses it. The current bucket started before
    // this sync, so move both versions back in time.
    sqlx::query("UPDATE price_versions SET effective_from=effective_from-CASE WHEN id=$1 THEN interval '2 days' ELSE interval '30 minutes' END")
        .bind(v1)
        .execute(&f.db)
        .await
        .unwrap();
    let fresh = anydash_api::time::floor_bucket(Utc::now());
    up(vec![bucket(
        "LOCAL_TOOL_CODEX",
        "gpt-5",
        fresh,
        1,
        1,
        tokens.clone(),
    )])
    .await;
    let fresh_cost: String = sqlx::query_scalar("SELECT estimated_cost_usd::text FROM usage_buckets WHERE bucket_start=$1 AND model='gpt-5'")
        .bind(fresh)
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(fresh_cost, "0.005062500000");
    // An unchanged document, a failed fetch and an invalid document keep the current version.
    f.job(Job::PriceSync).await;
    f.mock.lock().unwrap().prices = (500, String::new());
    f.job(Job::PriceSync).await;
    f.mock.lock().unwrap().prices = (200, "{\"gpt-5\":{\"input_cost_per_token\":-1}}".into());
    f.job(Job::PriceSync).await;
    f.mock.lock().unwrap().prices = (200, "not json".into());
    f.job(Job::PriceSync).await;
    assert_eq!(f.count("SELECT count(*) FROM price_versions").await, 2);
    let outcomes: Vec<i16> =
        sqlx::query_scalar("SELECT outcome FROM price_sync_runs ORDER BY started_at")
            .fetch_all(&f.db)
            .await
            .unwrap();
    assert_eq!(outcomes, [1, 1, 2, 3, 4, 4]);
    let failures: Vec<String> = sqlx::query_scalar(
        "SELECT failure_category FROM price_sync_runs WHERE outcome IN (3,4) ORDER BY started_at",
    )
    .fetch_all(&f.db)
    .await
    .unwrap();
    assert_eq!(failures, ["status", "too_few_models", "invalid_json"]);
    assert!(
        f.server
            .metrics
            .encode()
            .contains("anydash_price_sync_validation_failures_total 2")
    );

    // Widget data: totals, sessions in their start bucket, partial cost and notices.
    let user = f.user(&a);
    let dash = f
        .ok(
            &user,
            "DashboardService/CreateDashboard",
            json!({"requestId":id(),"spaceId":a.personal,"name":"Usage"}),
        )
        .await;
    let dash_id = dash["dashboard"]["id"].as_str().unwrap().to_owned();
    let added = f
        .ok(
            &user,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash_id,"expectedVersion":"1","requestId":id(),"type":"WIDGET_TYPE_LOCAL_USAGE_SUMMARY",
                "layout":{"x":0,"y":0,"width":4,"height":2,"presentation":"WIDGET_PRESENTATION_NUMBER"},
                "query":{"period":"QUERY_PERIOD_LAST_24_HOURS"},"source":{"allDevices":true}}),
        )
        .await;
    assert_eq!(
        added["dashboard"]["widgets"][0]["sharingState"],
        "SHARING_STATE_PRIVATE"
    );
    let data = f
        .ok(
            &user,
            "WidgetDataService/QueryWidgetData",
            json!({"dashboardId":dash_id,"widgetId":added["widgetId"],"timeZone":"Asia/Kathmandu"}),
        )
        .await;
    let totals = &data["data"]["usageSummary"]["totals"];
    // Buckets in the last 24 hours: gpt-5 (3 sessions), claude ×2 (1 + 4), unknown (1), fresh gpt-5 (1).
    assert_eq!(totals["sessionCount"], "10");
    assert_eq!(totals["estimatedCost"]["state"], "METRIC_STATE_PARTIAL");
    assert_eq!(data["data"]["timeZone"], "Asia/Kathmandu");
    assert_eq!(data["data"]["summary"]["primaryLabel"], "Tokens");
    let notices = data["data"]["notices"].as_array().unwrap();
    assert!(notices.contains(&json!("DATA_NOTICE_ESTIMATED_COST")));
    // The range starts after the device's earliest stored bucket.
    assert!(!notices.contains(&json!("DATA_NOTICE_BACKFILL_LIMITED")));
    f.fail(
        &user,
        "WidgetDataService/QueryWidgetData",
        json!({"dashboardId":dash_id,"widgetId":added["widgetId"],"timeZone":"Mars/Base"}),
        "ERROR_REASON_INVALID_ARGUMENT",
    )
    .await;
    // Hourly series in a +5:45 zone.
    let series = f
        .ok(
            &user,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash_id,"expectedVersion":"2","requestId":id(),"type":"WIDGET_TYPE_LOCAL_USAGE_SERIES",
                "layout":{"x":4,"y":0,"width":8,"height":3,"presentation":"WIDGET_PRESENTATION_LINE_CHART"},
                "query":{"period":"QUERY_PERIOD_LAST_24_HOURS","granularity":"TIME_GRANULARITY_HOUR"},"source":{"deviceIds":[device]}}),
        )
        .await;
    let data = f
        .ok(&user, "WidgetDataService/QueryWidgetData", json!({"dashboardId":dash_id,"widgetId":series["widgetId"],"timeZone":"Asia/Kathmandu"}))
        .await;
    let points = data["data"]["usageSeries"]["points"].as_array().unwrap();
    assert!(points.len() >= 24);
    assert!(points.iter().all(|p| p["utcOffsetMinutes"] == 345));
    let sessions: u64 = points
        .iter()
        .map(|p| {
            p["totals"]["sessionCount"]
                .as_str()
                .unwrap_or("0")
                .parse::<u64>()
                .unwrap()
        })
        .sum();
    assert_eq!(sessions, 10);
    // Breakdown by tool.
    let breakdown = f
        .ok(
            &user,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash_id,"expectedVersion":"3","requestId":id(),"type":"WIDGET_TYPE_LOCAL_USAGE_BREAKDOWN",
                "layout":{"x":0,"y":3,"width":6,"height":3,"presentation":"WIDGET_PRESENTATION_TABLE"},
                "query":{"period":"QUERY_PERIOD_LAST_90_DAYS","breakdown":"USAGE_BREAKDOWN_DIMENSION_TOOL"},"source":{"allDevices":true}}),
        )
        .await;
    f.ok(
        &d,
        "CollectorService/ReportParserStatus",
        json!({"statuses":[{"tool":"LOCAL_TOOL_CODEX","state":"PARSER_STATE_PARTIAL","skippedRecordCount":"3","parserVersion":"1"}]}),
    )
    .await;
    let data = f
        .ok(
            &user,
            "WidgetDataService/QueryWidgetData",
            json!({"dashboardId":dash_id,"widgetId":breakdown["widgetId"]}),
        )
        .await;
    let rows = data["data"]["usageBreakdown"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["tool"], "LOCAL_TOOL_CODEX");
    assert_eq!(rows[0]["parserState"], "PARSER_STATE_PARTIAL");
    assert_eq!(rows[0]["skippedRecordCount"], "3");
    let notices = data["data"]["notices"].as_array().unwrap();
    assert!(notices.contains(&json!("DATA_NOTICE_PARTIAL_AGGREGATION")));
    // A 90-day range starts before the earliest bucket of a device enrolled today.
    assert!(notices.contains(&json!("DATA_NOTICE_BACKFILL_LIMITED")));
    let whole = f
        .ok(
            &user,
            "WidgetDataService/QueryDashboardData",
            json!({"dashboardId":dash_id}),
        )
        .await;
    assert_eq!(whole["dashboardVersion"], "4");
    assert_eq!(whole["widgets"].as_array().unwrap().len(), 3);

    // The retention job removes buckets older than 90 days.
    sqlx::query("INSERT INTO usage_buckets(device_id,tool,model,bucket_start,user_id,version,session_count,cost_computable,updated_at) SELECT id,1,'old',$1,user_id,1,1,false,now() FROM devices")
        .bind(bucket_start(Duration::days(91)))
        .execute(&f.db)
        .await
        .unwrap();
    f.job(Job::Retention).await;
    assert_eq!(
        f.count("SELECT count(*) FROM usage_buckets WHERE model='old'")
            .await,
        0
    );
    assert_eq!(
        f.count("SELECT count(*) FROM usage_buckets WHERE bucket_start<now()-interval '88 days'")
            .await,
        1
    );
    f.cleanup().await;
}

/// Refresh and refresh-token logout charge the device limit on their own transaction
/// connection while they hold the device row lock, so they never wait for a second pooled
/// connection. They therefore complete on an API instance with a single-connection pool, and
/// rejected requests still count.
#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn refresh_token_requests_need_one_pooled_connection() {
    let f = Fixture::setup().await;
    let a = f.person("alice");
    f.connect(&a).await;
    let (device, _, refresh) = f.enroll(&a, "laptop", "").await;
    let (other, _, other_refresh) = f.enroll(&a, "desk", "").await;
    let narrow = f.with_pool_size(1).await;
    let refresh_call = |token: &str| json!({"deviceId":device,"refreshToken":token});
    let device_windows = format!(
        "SELECT COALESCE(sum(count),0)::int8 FROM rate_limit_windows WHERE scope=2 AND subject='{device}'"
    );

    let rotated = narrow
        .ok(
            &narrow.anonymous(),
            "DeviceService/RefreshDeviceToken",
            refresh_call(&refresh),
        )
        .await;
    let refresh2 = rotated["credentials"]["refreshToken"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(f.count(&device_windows).await, 1);

    // Over the device limit, the authenticated refresh is rejected and its charge persists.
    // Pre-fill the current and next windows to avoid a minute boundary.
    f.exec(&format!(
        "INSERT INTO rate_limit_windows(scope,subject,window_start,count) SELECT 2,'{device}',w,60 FROM \
         (VALUES (date_bin('60 seconds', clock_timestamp(), TIMESTAMPTZ '2000-01-01')),(date_bin('60 seconds', clock_timestamp(), TIMESTAMPTZ '2000-01-01')+interval '60 seconds')) v(w) \
         ON CONFLICT (scope,subject,window_start) DO UPDATE SET count=60"
    ))
    .await;
    narrow
        .fail(
            &narrow.anonymous(),
            "DeviceService/RefreshDeviceToken",
            refresh_call(&refresh2),
            "ERROR_REASON_RATE_LIMITED",
        )
        .await;
    assert_eq!(f.count(&device_windows).await, 121);
    f.exec(&format!(
        "DELETE FROM rate_limit_windows WHERE subject='{device}'"
    ))
    .await;

    // Reuse detection and the inactive rejection that follows also count.
    narrow
        .fail(
            &narrow.anonymous(),
            "DeviceService/RefreshDeviceToken",
            refresh_call(&refresh),
            "ERROR_REASON_REFRESH_TOKEN_REUSED",
        )
        .await;
    narrow
        .fail(
            &narrow.anonymous(),
            "DeviceService/RefreshDeviceToken",
            refresh_call(&refresh2),
            "ERROR_REASON_DEVICE_CREDENTIALS_INACTIVE",
        )
        .await;
    assert_eq!(f.count(&device_windows).await, 2);

    // Collector logout with the refresh token.
    narrow
        .ok(
            &narrow.anonymous(),
            "DeviceService/RevokeDeviceCredentials",
            json!({"deviceId":other,"refreshToken":other_refresh}),
        )
        .await;
    assert_eq!(
        f.count(&format!(
            "SELECT COALESCE(sum(count),0)::int8 FROM rate_limit_windows WHERE scope=2 AND subject='{other}'"
        ))
        .await,
        1
    );
    narrow.db.close().await;
    f.cleanup().await;
}

/// Polling maps only a missing enrollment to UNAUTHENTICATED. The collector treats that reason as
/// a permanent invalidation, so a database failure must stay a retryable server error.
#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn poll_enrollment_database_failure_is_not_unauthenticated() {
    let f = Fixture::setup().await;
    let a = f.person("alice");
    f.connect(&a).await;
    let start = f
        .ok(
            &f.anonymous(),
            "DeviceService/StartEnrollment",
            json!({"deviceName":"laptop","platform":"DEVICE_PLATFORM_LINUX","architecture":"CPU_ARCHITECTURE_X64","collectorKind":"COLLECTOR_KIND_CLI","collectorVersion":"1.0.0","osVersion":"24.04"}),
        )
        .await;
    let enrollment = start["enrollmentId"].as_str().unwrap().to_owned();
    let poll = json!({"enrollmentId":enrollment,"deviceCode":start["deviceCode"]});

    // A lock held by another transaction makes the row read fail after the lock timeout.
    let impatient = f.with_lock_timeout(200).await;
    let lock = f
        .lock_row(&format!(
            "SELECT id FROM enrollments WHERE id='{enrollment}' FOR UPDATE"
        ))
        .await;
    let failed = impatient
        .fail(
            &impatient.anonymous(),
            "DeviceService/PollEnrollment",
            poll.clone(),
            "ERROR_REASON_INTERNAL",
        )
        .await;
    assert_eq!(failed.status, 500);
    lock.rollback().await.unwrap();

    // Once the lock is released the same request succeeds: nothing was invalidated.
    let pending = impatient
        .ok(&impatient.anonymous(), "DeviceService/PollEnrollment", poll)
        .await;
    assert_eq!(pending["state"], "ENROLLMENT_STATE_PENDING");
    let state = f
        .count(&format!(
            "SELECT count(*) FROM enrollments WHERE id='{enrollment}' AND state=1"
        ))
        .await;
    assert_eq!(state, 1, "the enrollment is still pending");

    // A missing enrollment and a wrong device code are still authentication failures.
    impatient
        .fail(
            &impatient.anonymous(),
            "DeviceService/PollEnrollment",
            json!({"enrollmentId":id(),"deviceCode":start["deviceCode"]}),
            "ERROR_REASON_UNAUTHENTICATED",
        )
        .await;
    impatient
        .fail(
            &impatient.anonymous(),
            "DeviceService/PollEnrollment",
            json!({"enrollmentId":enrollment,"deviceCode":"x".repeat(43)}),
            "ERROR_REASON_UNAUTHENTICATED",
        )
        .await;
    impatient.db.close().await;
    f.cleanup().await;
}

/// Timestamps outside years 1 to 9999 are INVALID_ARGUMENT naming the field, never a database
/// error or a silently dropped value. ProtoJSON cannot express them, so the requests use the
/// binary protobuf encoding.
#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn out_of_range_timestamps_are_invalid_arguments() {
    use buffa_types::google::protobuf::Timestamp;
    use delino_protocol::anydash::v1::{
        __buffa::oneof::metric_value::Value as MetricOneof, MetricObservation, MetricState,
        MetricValue, ReportParserStatusRequest, ReportSnapshotRequest, UploadUsageRequest,
    };
    const INVALID: &str = "ERROR_REASON_INVALID_ARGUMENT";
    let out_of_range = || {
        Some(Timestamp {
            seconds: -999_999_999_900,
            ..Default::default()
        })
        .into()
    };
    let f = Fixture::setup().await;
    let a = f.person("alice");
    f.connect(&a).await;
    let (_, access, _) = f.enroll(&a, "laptop", "").await;
    let d = f.device(&access);

    // UploadUsage: the bucket start is part of the whole-request validation.
    let mut upload: UploadUsageRequest = serde_json::from_value(json!({"buckets":[bucket(
        "LOCAL_TOOL_CODEX",
        "gpt-5",
        bucket_start(Duration::hours(2)),
        1,
        1,
        json!({"inputTokens":"10"}),
    )]}))
    .unwrap();
    upload.buckets[0].bucket_start = out_of_range();
    let e = f
        .fail_proto(&d, "CollectorService/UploadUsage", &upload, INVALID)
        .await;
    assert_eq!(e.status, 400);
    assert_eq!(e.detail.field, "buckets[0].bucket_start");
    // Year 9999 is representable but not aligned or current; the far future stays invalid.
    upload.buckets[0].bucket_start = Some(Timestamp {
        seconds: 253_402_300_800,
        ..Default::default()
    })
    .into();
    let e = f
        .fail_proto(&d, "CollectorService/UploadUsage", &upload, INVALID)
        .await;
    assert_eq!(e.detail.field, "buckets[0].bucket_start");
    // A 100-day-old aligned bucket is still a valid bucket outside retention.
    let r = f
        .ok(
            &d,
            "CollectorService/UploadUsage",
            json!({"buckets":[bucket(
                "LOCAL_TOOL_CODEX",
                "gpt-5",
                bucket_start(Duration::days(100)),
                1,
                1,
                json!({"inputTokens":"10"}),
            )]}),
        )
        .await;
    assert_eq!(
        r["results"][0]["outcome"],
        "BUCKET_OUTCOME_OUTSIDE_RETENTION"
    );

    // ReportParserStatus: observed_at is optional, but a present value must be valid.
    let mut status: ReportParserStatusRequest = serde_json::from_value(json!({"statuses":[
        {"tool":"LOCAL_TOOL_CODEX","state":"PARSER_STATE_OK","parserVersion":"1"},
        {"tool":"LOCAL_TOOL_CLAUDE_CODE","state":"PARSER_STATE_OK","parserVersion":"1"},
    ]}))
    .unwrap();
    status.statuses[1].observed_at = out_of_range();
    let e = f
        .fail_proto(&d, "CollectorService/ReportParserStatus", &status, INVALID)
        .await;
    assert_eq!(e.detail.field, "statuses[1].observed_at");
    assert_eq!(
        f.count("SELECT count(*) FROM device_parser_status").await,
        0,
        "an invalid request stores nothing"
    );
    status.statuses[1].observed_at = Some(Timestamp {
        seconds: bucket_start(Duration::hours(1)).timestamp(),
        ..Default::default()
    })
    .into();
    f.ok(
        &d,
        "CollectorService/ReportParserStatus",
        json!({"statuses":[
            {"tool":"LOCAL_TOOL_CODEX","state":"PARSER_STATE_OK","parserVersion":"1"},
            {"tool":"LOCAL_TOOL_CLAUDE_CODE","state":"PARSER_STATE_OK","parserVersion":"1","observedAt":at(Utc::now())},
        ]}),
    )
    .await;
    assert_eq!(
        f.count("SELECT count(*) FROM device_parser_status WHERE observed_at IS NULL")
            .await,
        1,
        "an absent observed_at is still stored as absent"
    );

    // ReportSnapshot: metric periods and time values are validated before any lease check.
    let snapshot = |metric: Value| -> ReportSnapshotRequest {
        serde_json::from_value(json!({"connectionId":id(),"leaseEpoch":"1","snapshot":{
            "observedAt":at(Utc::now()),"status":"COLLECTION_STATUS_SUCCEEDED","metrics":[metric],
            "source":"OBSERVATION_SOURCE_ACTIVE_POLL"}}))
        .unwrap()
    };
    let cumulative = || {
        json!({"kind":"METRIC_KIND_SUBSCRIPTION_USED_PERCENT","dimension":"week","temporality":"METRIC_TEMPORALITY_CUMULATIVE",
            "value":{"state":"METRIC_STATE_VALUE","percent":1.0},
            "periodStart":at(Utc::now() - Duration::hours(2)),"periodEnd":at(Utc::now() - Duration::hours(1))})
    };
    let report = |request: ReportSnapshotRequest, edit: &dyn Fn(&mut MetricObservation)| {
        let mut request = request;
        let mut snap = request.snapshot.as_option().cloned().unwrap();
        edit(&mut snap.metrics[0]);
        request.snapshot = Some(snap).into();
        let f = &f;
        let d = &d;
        async move {
            f.fail_proto(d, "CollectorService/ReportSnapshot", &request, INVALID)
                .await
        }
    };
    let e = report(snapshot(cumulative()), &|m| m.period_start = out_of_range()).await;
    assert_eq!(e.detail.field, "snapshot.metrics[0].period_start");
    let e = report(snapshot(cumulative()), &|m| m.period_end = out_of_range()).await;
    assert_eq!(e.detail.field, "snapshot.metrics[0].period_end");
    // A non-cumulative metric with an invalid period used to drop the period silently.
    let instantaneous = json!({"kind":"METRIC_KIND_SUBSCRIPTION_USED_PERCENT","dimension":"week",
        "temporality":"METRIC_TEMPORALITY_INSTANTANEOUS","value":{"state":"METRIC_STATE_VALUE","percent":1.0}});
    let e = report(snapshot(instantaneous.clone()), &|m| {
        m.period_end = out_of_range()
    })
    .await;
    assert_eq!(e.detail.field, "snapshot.metrics[0].period_end");
    // A time-valued metric outside the range is invalid too.
    let e = report(snapshot(instantaneous), &|m| {
        m.value = Some(MetricValue {
            state: MetricState::Value.into(),
            value: Some(MetricOneof::Time(Box::new(Timestamp {
                seconds: -999_999_999_900,
                ..Default::default()
            }))),
            ..Default::default()
        })
        .into()
    })
    .await;
    assert_eq!(e.detail.field, "snapshot.metrics[0].value");
    f.cleanup().await;
}

/// Reports parser statuses as a device.
async fn report_parsers(f: &Fixture, caller: &support::Caller, statuses: Value) {
    f.ok(
        caller,
        "CollectorService/ReportParserStatus",
        json!({"statuses":statuses}),
    )
    .await;
}

/// Returns (parser state, skipped count) of the breakdown row whose `field` equals `key`. The
/// row is read 25 times through QueryWidgetData and once through QueryDashboardData; all reads
/// must agree.
async fn breakdown_row(
    f: &Fixture,
    user: &support::Caller,
    dash: &str,
    widget: &str,
    field: &str,
    key: &str,
) -> (Value, Value) {
    let mut seen = Vec::new();
    for _ in 0..25 {
        let data = f
            .ok(
                user,
                "WidgetDataService/QueryWidgetData",
                json!({"dashboardId":dash,"widgetId":widget}),
            )
            .await;
        let rows = data["data"]["usageBreakdown"]["rows"].as_array().unwrap();
        let r = rows.iter().find(|r| r[field] == key).unwrap();
        seen.push((r["parserState"].clone(), r["skippedRecordCount"].clone()));
    }
    let whole = f
        .ok(
            user,
            "WidgetDataService/QueryDashboardData",
            json!({"dashboardId":dash}),
        )
        .await;
    let data = whole["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["widgetId"] == widget)
        .unwrap();
    let rows = data["usageBreakdown"]["rows"].as_array().unwrap();
    let r = rows.iter().find(|r| r[field] == key).unwrap();
    seen.push((r["parserState"].clone(), r["skippedRecordCount"].clone()));
    assert!(
        seen.windows(2).all(|w| w[0] == w[1]),
        "unstable row {key}: {seen:?}"
    );
    seen.remove(0)
}

/// A breakdown row covering several parser statuses reports the same state on every query:
/// PARTIAL > OK > NO_SOURCES > DISABLED, with the skipped record counts summed.
#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn breakdown_parser_state_follows_a_fixed_precedence() {
    let f = Fixture::setup().await;
    let a = f.person("alice");
    f.connect(&a).await;
    let user = f.user(&a);
    let (laptop, laptop_access, _) = f.enroll(&a, "laptop", "").await;
    let (desk, desk_access, _) = f.enroll(&a, "desk", "").await;
    let (laptop_caller, desk_caller) = (f.device(&laptop_access), f.device(&desk_access));
    let start = bucket_start(Duration::hours(2));
    let tokens = json!({"inputTokens":"10","outputTokens":"1","cacheReadTokens":"0"});
    for (caller, tool) in [
        (&laptop_caller, "LOCAL_TOOL_CODEX"),
        (&laptop_caller, "LOCAL_TOOL_CLAUDE_CODE"),
        (&desk_caller, "LOCAL_TOOL_CODEX"),
        (&desk_caller, "LOCAL_TOOL_CLAUDE_CODE"),
    ] {
        f.ok(
            caller,
            "CollectorService/UploadUsage",
            json!({"buckets":[bucket(tool, "gpt-5", start, 1, 1, tokens.clone())]}),
        )
        .await;
    }
    let status = |tool: &str, state: &str, skipped: u32| json!({"tool":tool,"state":state,"skippedRecordCount":skipped.to_string(),"parserVersion":"1"});

    let dash = f
        .ok(
            &user,
            "DashboardService/CreateDashboard",
            json!({"requestId":id(),"spaceId":a.personal,"name":"Parsers"}),
        )
        .await["dashboard"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut widgets = Vec::new();
    for (version, dimension, y) in [
        ("1", "USAGE_BREAKDOWN_DIMENSION_DEVICE", 0),
        ("2", "USAGE_BREAKDOWN_DIMENSION_TOOL", 3),
    ] {
        let added = f
            .ok(
                &user,
                "DashboardService/AddWidget",
                json!({"dashboardId":dash,"expectedVersion":version,"requestId":id(),"type":"WIDGET_TYPE_LOCAL_USAGE_BREAKDOWN",
                    "layout":{"x":0,"y":y,"width":6,"height":3,"presentation":"WIDGET_PRESENTATION_TABLE"},
                    "query":{"period":"QUERY_PERIOD_LAST_24_HOURS","breakdown":dimension},"source":{"allDevices":true}}),
            )
            .await;
        widgets.push(added["widgetId"].as_str().unwrap().to_owned());
    }
    let (by_device, by_tool) = (&widgets[0], &widgets[1]);

    // Laptop: Codex OK, Claude Code NO_SOURCES. Desk: Codex DISABLED, Claude Code NO_SOURCES.
    report_parsers(
        &f,
        &laptop_caller,
        json!([
            status("LOCAL_TOOL_CODEX", "PARSER_STATE_OK", 0),
            status("LOCAL_TOOL_CLAUDE_CODE", "PARSER_STATE_NO_SOURCES", 0)
        ]),
    )
    .await;
    report_parsers(
        &f,
        &desk_caller,
        json!([
            status("LOCAL_TOOL_CODEX", "PARSER_STATE_DISABLED", 0),
            status("LOCAL_TOOL_CLAUDE_CODE", "PARSER_STATE_NO_SOURCES", 0)
        ]),
    )
    .await;
    // DEVICE rows combine the tools of one device; TOOL rows combine the devices of one tool.
    macro_rules! row {
        ($widget:expr, $field:expr, $key:expr) => {
            breakdown_row(&f, &user, &dash, $widget, $field, $key).await
        };
    }
    assert_eq!(row!(by_device, "deviceId", &laptop).0, "PARSER_STATE_OK");
    assert_eq!(
        row!(by_device, "deviceId", &desk).0,
        "PARSER_STATE_NO_SOURCES"
    );
    assert_eq!(
        row!(by_tool, "tool", "LOCAL_TOOL_CODEX").0,
        "PARSER_STATE_OK"
    );
    assert_eq!(
        row!(by_tool, "tool", "LOCAL_TOOL_CLAUDE_CODE").0,
        "PARSER_STATE_NO_SOURCES"
    );

    // NO_SOURCES outranks DISABLED.
    report_parsers(
        &f,
        &laptop_caller,
        json!([status("LOCAL_TOOL_CODEX", "PARSER_STATE_DISABLED", 0)]),
    )
    .await;
    assert_eq!(
        row!(by_device, "deviceId", &laptop).0,
        "PARSER_STATE_NO_SOURCES"
    );
    assert_eq!(
        row!(by_tool, "tool", "LOCAL_TOOL_CODEX").0,
        "PARSER_STATE_DISABLED"
    );

    // PARTIAL outranks everything and the skipped counts add up.
    report_parsers(
        &f,
        &laptop_caller,
        json!([status("LOCAL_TOOL_CODEX", "PARSER_STATE_PARTIAL", 3)]),
    )
    .await;
    report_parsers(
        &f,
        &desk_caller,
        json!([status("LOCAL_TOOL_CODEX", "PARSER_STATE_OK", 4)]),
    )
    .await;
    assert_eq!(
        row!(by_device, "deviceId", &laptop),
        (json!("PARSER_STATE_PARTIAL"), json!("3"))
    );
    assert_eq!(
        row!(by_tool, "tool", "LOCAL_TOOL_CODEX"),
        (json!("PARSER_STATE_PARTIAL"), json!("7"))
    );
    assert_eq!(row!(by_device, "deviceId", &desk).0, "PARSER_STATE_OK");
    f.cleanup().await;
}
