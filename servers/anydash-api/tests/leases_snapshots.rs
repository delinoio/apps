// SPDX-License-Identifier: FSL-1.1-ALv2
//! Lease epochs, heartbeat failover, snapshots, hourly history and late arrival.
mod support;
use anydash_api::worker::Job;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use support::{Fixture, at, id};

fn used(dimension: &str, percent: f64) -> Value {
    json!({"kind":"METRIC_KIND_SUBSCRIPTION_USED_PERCENT","dimension":dimension,"value":{"state":"METRIC_STATE_VALUE","percent":percent},"temporality":"METRIC_TEMPORALITY_INSTANTANEOUS"})
}

async fn beat(f: &Fixture, c: &support::Caller, conn: &str, ready: bool) -> Value {
    let body = if ready {
        json!({"readyConnectionIds":[conn]})
    } else {
        json!({})
    };
    f.ok(c, "CollectorService/Heartbeat", body).await
}

fn snapshot(observed: chrono::DateTime<Utc>, status: &str, metrics: Vec<Value>) -> Value {
    json!({"observedAt":at(observed),"status":status,"metrics":metrics,"source":"OBSERVATION_SOURCE_ACTIVE_POLL"})
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn lease_epochs_and_heartbeat_failover() {
    let f = Fixture::setup().await;
    let a = f.person("alice");
    f.connect(&a).await;
    let user = f.user(&a);
    let (d1, t1, _) = f.enroll(&a, "one", "").await;
    let (d2, t2, _) = f.enroll(&a, "two", "").await;
    let (d3, t3, _) = f.enroll(&a, "three", "").await;
    let (c1, c2, c3) = (f.device(&t1), f.device(&t2), f.device(&t3));
    let fingerprint = "a".repeat(64);
    let request = id();
    let created = f
        .ok(&c1, "ConnectionService/CreateConnection", json!({"requestId":request,"provider":"PROVIDER_CODEX","accountLabel":"me@example.test","accountFingerprint":fingerprint}))
        .await;
    let conn = created["connection"]["id"].as_str().unwrap().to_owned();
    assert_eq!(created["connection"]["displayName"], "Codex");
    // An identical replay returns the original connection; different content is rejected.
    let replay = f
        .ok(&c1, "ConnectionService/CreateConnection", json!({"requestId":request,"provider":"PROVIDER_CODEX","accountLabel":"me@example.test","accountFingerprint":fingerprint}))
        .await;
    assert_eq!(replay["connection"]["id"], conn);
    f.fail(
        &c1,
        "ConnectionService/CreateConnection",
        json!({"requestId":request,"provider":"PROVIDER_GROK"}),
        "ERROR_REASON_REQUEST_ID_REUSED",
    )
    .await;
    f.fail(
        &c2,
        "ConnectionService/RegisterConnectionDevice",
        json!({"connectionId":conn,"accountFingerprint":"b".repeat(64)}),
        "ERROR_REASON_CONNECTION_ACCOUNT_MISMATCH",
    )
    .await;
    f.ok(
        &c2,
        "ConnectionService/RegisterConnectionDevice",
        json!({"connectionId":conn,"accountFingerprint":fingerprint}),
    )
    .await;
    f.ok(
        &c3,
        "ConnectionService/RegisterConnectionDevice",
        json!({"connectionId":conn}),
    )
    .await;
    let listed = f
        .ok(&c3, "ConnectionService/ListConnections", json!({}))
        .await;
    assert_eq!(listed["connections"].as_array().unwrap().len(), 1);
    // No registered device has sent a heartbeat yet.
    let state = f
        .ok(
            &user,
            "ConnectionService/GetConnection",
            json!({"connectionId":conn}),
        )
        .await;
    assert_eq!(
        state["connection"]["state"],
        "CONNECTION_STATE_NO_RUNNING_COLLECTOR"
    );

    // d1 and d2 are ready; d2 has the most recent heartbeat; d3 is online but not ready.
    beat(&f, &c1, &conn, true).await;
    beat(&f, &c3, &conn, false).await;
    let hb = beat(&f, &c2, &conn, true).await;
    assert_eq!(hb["assignments"][0]["connectionId"], conn);
    assert_eq!(hb["assignments"][0]["acquireRecommended"], true);
    assert_eq!(hb["heartbeatInterval"], "60s");
    assert_eq!(hb["versionStatus"]["minimumVersion"], "1.0.0");
    assert_eq!(hb["versionStatus"]["updateAvailable"], true);
    let state = f
        .ok(
            &user,
            "ConnectionService/GetConnection",
            json!({"connectionId":conn}),
        )
        .await;
    assert_eq!(
        state["connection"]["state"],
        "CONNECTION_STATE_WAITING_FOR_FIRST_RESULT"
    );
    // Concurrent acquisition grants exactly one lease.
    let mut tasks = Vec::new();
    for i in 0..12 {
        let caller = if i % 2 == 0 { t1.clone() } else { t2.clone() };
        let conn = conn.clone();
        let base = f.base.clone();
        tasks.push(tokio::spawn(async move {
            let r: Value = reqwest::Client::new()
                .post(format!("{base}/anydash.v1.CollectorService/AcquireLease"))
                .header("content-type", "application/json")
                .header("x-anydash-client-kind", "CLIENT_KIND_CLI")
                .header("x-anydash-client-version", "1.0.0")
                .bearer_auth(&caller)
                .body(json!({"connectionId":conn}).to_string())
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            (i % 2, r)
        }));
    }
    for t in tasks {
        let (who, r) = t.await.unwrap();
        assert_eq!(r["granted"] == true, who == 1, "{r}");
        // A non-candidate that runs before the grant sees no lease.
        if who == 1 || !r["lease"].is_null() {
            assert_eq!(r["lease"]["holderDeviceId"], d2);
            assert_eq!(r["lease"]["epoch"], "1");
        }
    }
    assert_eq!(f.count("SELECT lease_epoch FROM connections").await, 1);
    // Only the holder can report, under the current epoch. Request times have second precision.
    let now = Utc::now() + Duration::seconds(1);
    f.fail(
        &c1,
        "CollectorService/ReportSnapshot",
        json!({"connectionId":conn,"leaseEpoch":"1","snapshot":snapshot(now, "COLLECTION_STATUS_SUCCEEDED", vec![used("five_hour", 10.0)])}),
        "ERROR_REASON_LEASE_EXPIRED",
    )
    .await;
    let r = f
        .ok(&c2, "CollectorService/ReportSnapshot", json!({"connectionId":conn,"leaseEpoch":"1","snapshot":snapshot(now, "COLLECTION_STATUS_SUCCEEDED", vec![used("five_hour", 10.0)])}))
        .await;
    assert_eq!(r["outcome"], "SNAPSHOT_OUTCOME_LATEST_UPDATED");
    // A preferred-device change does not preempt a valid lease.
    f.ok(
        &user,
        "DeviceService/SetPreferredDevice",
        json!({"deviceId":d1}),
    )
    .await;
    let r = f
        .ok(
            &c1,
            "CollectorService/AcquireLease",
            json!({"connectionId":conn}),
        )
        .await;
    assert_ne!(r["granted"], true);
    let renewed = f
        .ok(
            &c2,
            "CollectorService/RenewLease",
            json!({"connectionId":conn,"epoch":"1"}),
        )
        .await;
    assert_eq!(renewed["lease"]["epoch"], "1");

    // The holder stops sending heartbeats for 5 minutes: the lease expires and the preferred
    // device takes over at its next heartbeat.
    f.exec(&format!(
        "UPDATE devices SET last_heartbeat_at=now()-interval '6 minutes' WHERE id='{d2}'; UPDATE connections SET lease_expires_at=now()-interval '1 second'"
    ))
    .await;
    f.job(Job::LeaseExpiry).await;
    assert!(
        f.server
            .metrics
            .encode()
            .contains("anydash_lease_failovers_total{provider=\"codex\"} 1")
    );
    let hb = beat(&f, &c1, &conn, true).await;
    assert_eq!(hb["assignments"][0]["acquireRecommended"], true);
    let r = f
        .ok(
            &c1,
            "CollectorService/AcquireLease",
            json!({"connectionId":conn}),
        )
        .await;
    assert_eq!(r["granted"], true);
    assert_eq!(r["lease"]["epoch"], "2");
    // The old holder comes back: its results under the old epoch are rejected.
    beat(&f, &c2, &conn, true).await;
    let e = f
        .fail(
            &c2,
            "CollectorService/ReportSnapshot",
            json!({"connectionId":conn,"leaseEpoch":"1","snapshot":snapshot(Utc::now(), "COLLECTION_STATUS_SUCCEEDED", vec![used("five_hour", 99.0)])}),
            "ERROR_REASON_LEASE_EXPIRED",
        )
        .await;
    assert_eq!(e.detail.current_lease_epoch, 2);
    f.fail(
        &c2,
        "CollectorService/RenewLease",
        json!({"connectionId":conn,"epoch":"1"}),
        "ERROR_REASON_LEASE_EXPIRED",
    )
    .await;
    // Without the preferred device, the most recent heartbeat wins; ties go to the lower ID.
    f.exec(&format!(
        "UPDATE devices SET last_heartbeat_at=now()-interval '6 minutes' WHERE id='{d1}'; UPDATE connections SET lease_expires_at=now()-interval '1 second'"
    ))
    .await;
    beat(&f, &c2, &conn, true).await;
    beat(&f, &c3, &conn, true).await;
    let r = f
        .ok(
            &c2,
            "CollectorService/AcquireLease",
            json!({"connectionId":conn}),
        )
        .await;
    assert_ne!(r["granted"], true);
    let r = f
        .ok(
            &c3,
            "CollectorService/AcquireLease",
            json!({"connectionId":conn}),
        )
        .await;
    assert_eq!(r["granted"], true);
    assert_eq!(r["lease"]["epoch"], "3");
    f.exec(&format!(
        "UPDATE devices SET last_heartbeat_at=now() WHERE id IN ('{d2}','{d3}'); UPDATE connections SET lease_expires_at=now()-interval '1 second'"
    ))
    .await;
    let lower = std::cmp::min(d2.clone(), d3.clone());
    let lower_token = if lower == d2 { &c2 } else { &c3 };
    let higher_token = if lower == d2 { &c3 } else { &c2 };
    assert_ne!(
        f.ok(
            higher_token,
            "CollectorService/AcquireLease",
            json!({"connectionId":conn})
        )
        .await["granted"],
        true
    );
    let r = f
        .ok(
            lower_token,
            "CollectorService/AcquireLease",
            json!({"connectionId":conn}),
        )
        .await;
    assert_eq!(r["lease"]["epoch"], "4");
    let activity = f
        .ok(
            &user,
            "AccountService/ListActivity",
            json!({"kinds":["ACTIVITY_KIND_COLLECTION_DEVICE_CHANGED"]}),
        )
        .await;
    assert!(activity["events"].as_array().unwrap().len() >= 2);
    // Release ends the lease immediately; a stale epoch is a no-op.
    f.ok(
        lower_token,
        "CollectorService/ReleaseLease",
        json!({"connectionId":conn,"epoch":"3"}),
    )
    .await;
    assert_eq!(
        f.count("SELECT count(*) FROM connections WHERE lease_device_id IS NOT NULL")
            .await,
        1
    );
    f.ok(
        lower_token,
        "CollectorService/ReleaseLease",
        json!({"connectionId":conn,"epoch":"4"}),
    )
    .await;
    assert_eq!(
        f.count("SELECT count(*) FROM connections WHERE lease_device_id IS NOT NULL")
            .await,
        0
    );

    // Manual refresh: once per minute, delivered once to the lease holder.
    let r = f
        .ok(
            &user,
            "ConnectionService/RequestConnectionRefresh",
            json!({"connectionId":conn}),
        )
        .await;
    assert!(r["nextAllowedAt"].is_string());
    let e = f
        .fail(
            &c1,
            "ConnectionService/RequestConnectionRefresh",
            json!({"connectionId":conn}),
            "ERROR_REASON_RATE_LIMITED",
        )
        .await;
    assert_eq!(
        e.detail.rate_limit_scope,
        delino_protocol::anydash::v1::RateLimitScope::ManualRefresh
    );
    let r = f
        .ok(
            lower_token,
            "CollectorService/AcquireLease",
            json!({"connectionId":conn}),
        )
        .await;
    assert_eq!(r["granted"], true);
    let hb = f
        .ok(
            lower_token,
            "CollectorService/Heartbeat",
            json!({"readyConnectionIds":[conn]}),
        )
        .await;
    assert_eq!(hb["assignments"][0]["refreshRequested"], true);
    let hb = f
        .ok(
            lower_token,
            "CollectorService/Heartbeat",
            json!({"readyConnectionIds":[conn]}),
        )
        .await;
    assert_ne!(hb["assignments"][0]["refreshRequested"], true);
    f.ok(
        &user,
        "DeviceService/RequestDeviceSync",
        json!({"deviceId":d1}),
    )
    .await;
    f.fail(
        &user,
        "DeviceService/RequestDeviceSync",
        json!({"deviceId":d1}),
        "ERROR_REASON_RATE_LIMITED",
    )
    .await;
    f.exec(&format!(
        "UPDATE devices SET last_heartbeat_at=now() WHERE id='{d1}'"
    ))
    .await;
    let hb = beat(&f, &c1, &conn, true).await;
    assert_eq!(hb["localSyncRequested"], true);
    assert_ne!(beat(&f, &c1, &conn, true).await["localSyncRequested"], true);

    // Unregistering ends the device's lease; a device cannot unregister another device.
    f.fail(
        &c1,
        "ConnectionService/UnregisterConnectionDevice",
        json!({"connectionId":conn,"deviceId":d2}),
        "ERROR_REASON_PERMISSION_DENIED",
    )
    .await;
    let holder = if lower == d2 { &d2 } else { &d3 };
    f.ok(
        &user,
        "ConnectionService/UnregisterConnectionDevice",
        json!({"connectionId":conn,"deviceId":holder}),
    )
    .await;
    assert_eq!(
        f.count("SELECT count(*) FROM connections WHERE lease_device_id IS NOT NULL")
            .await,
        0
    );
    // States: no running collector, then no registered device.
    f.exec("UPDATE devices SET last_heartbeat_at=now()-interval '10 minutes'")
        .await;
    let state = f
        .ok(
            &user,
            "ConnectionService/GetConnection",
            json!({"connectionId":conn}),
        )
        .await;
    assert_eq!(
        state["connection"]["state"],
        "CONNECTION_STATE_NO_RUNNING_COLLECTOR"
    );
    for (caller, device) in [(&c1, &d1), (&c2, &d2), (&c3, &d3)] {
        if device != holder {
            f.ok(
                caller,
                "ConnectionService/UnregisterConnectionDevice",
                json!({"connectionId":conn}),
            )
            .await;
        }
    }
    let state = f
        .ok(
            &user,
            "ConnectionService/GetConnection",
            json!({"connectionId":conn}),
        )
        .await;
    assert_eq!(
        state["connection"]["state"],
        "CONNECTION_STATE_NO_REGISTERED_DEVICE"
    );
    // History stays with the connection.
    assert_eq!(f.count("SELECT count(*) FROM metric_hourly").await, 1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn snapshots_hourly_history_and_late_arrival() {
    let f = Fixture::setup().await;
    let a = f.person("alice");
    f.connect(&a).await;
    let user = f.user(&a);
    let (_, t1, _) = f.enroll(&a, "one", "").await;
    let c1 = f.device(&t1);
    let conn = f
        .ok(
            &c1,
            "ConnectionService/CreateConnection",
            json!({"requestId":id(),"provider":"PROVIDER_CLAUDE_CODE"}),
        )
        .await["connection"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.ok(
        &c1,
        "CollectorService/Heartbeat",
        json!({"readyConnectionIds":[conn]}),
    )
    .await;
    assert_eq!(
        f.ok(
            &c1,
            "CollectorService/AcquireLease",
            json!({"connectionId":conn})
        )
        .await["granted"],
        true
    );
    // The lease was acquired three hours ago so that earlier observations are inside it.
    f.exec("UPDATE connections SET lease_acquired_at=now()-interval '3 hours'")
        .await;
    let base = anydash_api::time::floor_hour(Utc::now()) - Duration::hours(2);
    let report = |minutes: i64, status: &str, metrics: Vec<Value>| {
        let f = &f;
        let c1 = &c1;
        let conn = conn.clone();
        let observed = base + Duration::minutes(minutes);
        let status = status.to_owned();
        async move {
            f.ok(c1, "CollectorService/ReportSnapshot", json!({"connectionId":conn,"leaseEpoch":"1","snapshot":snapshot(observed, &status, metrics)}))
                .await["outcome"]
                .as_str()
                .unwrap()
                .to_owned()
        }
    };
    let hourly = |hour: i64| {
        let f = &f;
        async move {
            let v: Option<f64> = sqlx::query_scalar("SELECT (value->>'percent')::float8 FROM metric_hourly WHERE hour_start=$1 AND dimension='five_hour'")
                .bind(base + Duration::hours(hour))
                .fetch_optional(&f.db)
                .await
                .unwrap();
            v
        }
    };
    let ok = "COLLECTION_STATUS_SUCCEEDED";
    assert_eq!(
        report(10, ok, vec![used("five_hour", 10.0)]).await,
        "SNAPSHOT_OUTCOME_LATEST_UPDATED"
    );
    assert_eq!(
        report(40, ok, vec![used("five_hour", 20.0)]).await,
        "SNAPSHOT_OUTCOME_LATEST_UPDATED"
    );
    assert_eq!(hourly(0).await, Some(20.0));
    assert_eq!(
        report(
            70,
            ok,
            vec![used("five_hour", 30.0), used("seven_day", 5.0)]
        )
        .await,
        "SNAPSHOT_OUTCOME_LATEST_UPDATED"
    );
    // Late results land in the hour of their observation time; the latest value is unchanged.
    assert_eq!(
        report(20, ok, vec![used("five_hour", 15.0)]).await,
        "SNAPSHOT_OUTCOME_HISTORY_ONLY"
    );
    assert_eq!(
        hourly(0).await,
        Some(20.0),
        "an earlier observation in the hour is ignored"
    );
    assert_eq!(
        report(50, ok, vec![used("five_hour", 25.0)]).await,
        "SNAPSHOT_OUTCOME_HISTORY_ONLY"
    );
    assert_eq!(
        hourly(0).await,
        Some(25.0),
        "a later observation in the hour wins"
    );
    assert_eq!(hourly(1).await, Some(30.0));
    assert_eq!(
        report(70, ok, vec![used("five_hour", 31.0)]).await,
        "SNAPSHOT_OUTCOME_DUPLICATE"
    );
    let latest: f64 = sqlx::query_scalar(
        "SELECT (value->>'percent')::float8 FROM metric_latest WHERE dimension='five_hour'",
    )
    .fetch_one(&f.db)
    .await
    .unwrap();
    assert_eq!(latest, 30.0);
    // A failed cycle keeps the last successful value and time.
    assert_eq!(
        report(80, "COLLECTION_STATUS_FAILED", vec![]).await,
        "SNAPSHOT_OUTCOME_LATEST_UPDATED"
    );
    let conn_state = f
        .ok(
            &user,
            "ConnectionService/GetConnection",
            json!({"connectionId":conn}),
        )
        .await;
    assert_eq!(conn_state["connection"]["state"], "CONNECTION_STATE_ERROR");
    assert_eq!(
        conn_state["connection"]["lastSuccessAt"],
        at(base + Duration::minutes(70))
    );
    assert!(
        !conn_state["connection"]["lastErrorCorrelationId"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    // Not-provided metrics are stored apart from failures.
    report(
        90,
        ok,
        vec![json!({"kind":"METRIC_KIND_SUBSCRIPTION_REMAINING_AMOUNT","value":{"state":"METRIC_STATE_NOT_PROVIDED"}})],
    )
    .await;
    // Validation of snapshots.
    let invalid = [
        json!({"kind":"METRIC_KIND_SUBSCRIPTION_REMAINING_PERCENT","dimension":"x","value":{"state":"METRIC_STATE_VALUE","percent":1}}),
        json!({"kind":"METRIC_KIND_SUBSCRIPTION_USED_PERCENT","dimension":"x","value":{"state":"METRIC_STATE_VALUE"}}),
        json!({"kind":"METRIC_KIND_SUBSCRIPTION_USED_PERCENT","dimension":"x","value":{"state":"METRIC_STATE_FAILED","percent":1}}),
        json!({"kind":"METRIC_KIND_DEPLOYMENT_BILLED_COST","dimension":"x","value":{"state":"METRIC_STATE_VALUE","money":{"currencyCode":"usd","amount":"1"}}}),
    ];
    for m in invalid {
        f.fail(
            &c1,
            "CollectorService/ReportSnapshot",
            json!({"connectionId":conn,"leaseEpoch":"1","snapshot":snapshot(base + Duration::minutes(95), ok, vec![m])}),
            "ERROR_REASON_INVALID_ARGUMENT",
        )
        .await;
    }
    for observed in [
        Utc::now() - Duration::hours(4),
        Utc::now() + Duration::minutes(5),
    ] {
        f.fail(
            &c1,
            "CollectorService/ReportSnapshot",
            json!({"connectionId":conn,"leaseEpoch":"1","snapshot":snapshot(observed, ok, vec![])}),
            "ERROR_REASON_INVALID_ARGUMENT",
        )
        .await;
    }

    // Widgets: limits with derived remaining percentage and history by hour.
    let dash = f
        .ok(
            &user,
            "DashboardService/CreateDashboard",
            json!({"requestId":id(),"spaceId":a.personal,"name":"Limits"}),
        )
        .await["dashboard"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let limits = f
        .ok(
            &user,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash,"expectedVersion":"1","requestId":id(),"type":"WIDGET_TYPE_SUBSCRIPTION_LIMITS",
                "layout":{"x":0,"y":0,"width":4,"height":2,"presentation":"WIDGET_PRESENTATION_GAUGE"},"query":{},"source":{"connectionIds":[conn]}}),
        )
        .await["widgetId"]
        .as_str()
        .unwrap()
        .to_owned();
    let data = f
        .ok(
            &user,
            "WidgetDataService/QueryWidgetData",
            json!({"dashboardId":dash,"widgetId":limits}),
        )
        .await;
    let readings = data["data"]["metricPanel"]["readings"].as_array().unwrap();
    let find = |kind: &str, dim: &str| {
        readings
            .iter()
            .find(|r| r["kind"] == kind && r["dimension"].as_str().unwrap_or("") == dim)
            .unwrap_or_else(|| panic!("{kind} {dim} in {readings:?}"))
            .clone()
    };
    let used5 = find("METRIC_KIND_SUBSCRIPTION_USED_PERCENT", "five_hour");
    assert_eq!(used5["current"]["state"], "METRIC_STATE_FAILED");
    assert_eq!(used5["lastSuccess"]["percent"], 30.0);
    let remaining = find("METRIC_KIND_SUBSCRIPTION_REMAINING_PERCENT", "five_hour");
    assert_eq!(remaining["current"]["state"], "METRIC_STATE_FAILED");
    assert_eq!(remaining["lastSuccess"]["percent"], 70.0);
    let amount = find("METRIC_KIND_SUBSCRIPTION_REMAINING_AMOUNT", "");
    assert_eq!(amount["current"]["state"], "METRIC_STATE_NOT_PROVIDED");
    assert_eq!(
        data["data"]["summary"]["primaryValue"]["state"],
        "METRIC_STATE_FAILED"
    );
    let history = f
        .ok(
            &user,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash,"expectedVersion":"2","requestId":id(),"type":"WIDGET_TYPE_SUBSCRIPTION_HISTORY",
                "layout":{"x":4,"y":0,"width":8,"height":3,"presentation":"WIDGET_PRESENTATION_LINE_CHART"},
                "query":{"period":"QUERY_PERIOD_LAST_24_HOURS","granularity":"TIME_GRANULARITY_HOUR","dimensions":["five_hour"]},"source":{"connectionIds":[conn]}}),
        )
        .await["widgetId"]
        .as_str()
        .unwrap()
        .to_owned();
    let data = f
        .ok(
            &user,
            "WidgetDataService/QueryWidgetData",
            json!({"dashboardId":dash,"widgetId":history}),
        )
        .await;
    let percents = |series: &Value| -> Vec<f64> {
        series["points"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["value"]["percent"].as_f64().unwrap())
            .collect()
    };
    // Without a kind filter the history holds the used percentage and the derived remaining
    // percentage, as the latest readings do.
    let series = data["data"]["metricHistory"]["series"].as_array().unwrap();
    assert_eq!(series.len(), 2, "{series:?}");
    assert_eq!(series[0]["kind"], "METRIC_KIND_SUBSCRIPTION_USED_PERCENT");
    assert_eq!(percents(&series[0]), [25.0, 30.0]);
    assert_eq!(
        series[1]["kind"],
        "METRIC_KIND_SUBSCRIPTION_REMAINING_PERCENT"
    );
    assert_eq!(percents(&series[1]), [75.0, 70.0]);
    // A history filtered to the remaining percentage derives it from the stored used percentage.
    let remaining_history = f
        .ok(
            &user,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash,"expectedVersion":"3","requestId":id(),"type":"WIDGET_TYPE_SUBSCRIPTION_HISTORY",
                "layout":{"x":0,"y":20,"width":8,"height":3,"presentation":"WIDGET_PRESENTATION_LINE_CHART"},
                "query":{"period":"QUERY_PERIOD_LAST_24_HOURS","granularity":"TIME_GRANULARITY_HOUR","dimensions":["five_hour"],
                    "metricKinds":["METRIC_KIND_SUBSCRIPTION_REMAINING_PERCENT"]},"source":{"connectionIds":[conn]}}),
        )
        .await["widgetId"]
        .as_str()
        .unwrap()
        .to_owned();
    let data = f
        .ok(
            &user,
            "WidgetDataService/QueryWidgetData",
            json!({"dashboardId":dash,"widgetId":remaining_history}),
        )
        .await;
    let series = data["data"]["metricHistory"]["series"].as_array().unwrap();
    assert_eq!(series.len(), 1, "{series:?}");
    assert_eq!(
        series[0]["kind"],
        "METRIC_KIND_SUBSCRIPTION_REMAINING_PERCENT"
    );
    assert_eq!(percents(&series[0]), [75.0, 70.0]);
    assert_eq!(data["data"]["summary"]["primaryValue"]["percent"], 70.0);

    // Deployment resources: a newer successful snapshot replaces the resource set.
    let vercel = f
        .ok(
            &c1,
            "ConnectionService/CreateConnection",
            json!({"requestId":id(),"provider":"PROVIDER_VERCEL","displayName":"Team"}),
        )
        .await["connection"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.ok(
        &c1,
        "CollectorService/Heartbeat",
        json!({"readyConnectionIds":[conn, vercel]}),
    )
    .await;
    let lease = f
        .ok(
            &c1,
            "CollectorService/AcquireLease",
            json!({"connectionId":vercel}),
        )
        .await;
    let epoch = lease["lease"]["epoch"].clone();
    let resource = |rid: &str| {
        json!({"resourceId":rid,"kind":"RESOURCE_KIND_VERCEL_PROJECT","displayName":rid,"parentId":"team_1","health":"RESOURCE_HEALTH_HEALTHY","providerStatus":"READY","statusState":"METRIC_STATE_VALUE",
            "metrics":[{"kind":"METRIC_KIND_DEPLOYMENT_BILLED_COST","dimension":"total","value":{"state":"METRIC_STATE_VALUE","money":{"currencyCode":"USD","amount":"1.5"}},"temporality":"METRIC_TEMPORALITY_CUMULATIVE","periodStart":at(Utc::now()-Duration::days(3)),"periodEnd":at(Utc::now())}]})
    };
    let start = Utc::now();
    for (observed, rids, outcome) in [
        (
            start + Duration::seconds(1),
            vec!["prj_a", "prj_b"],
            "SNAPSHOT_OUTCOME_LATEST_UPDATED",
        ),
        (
            start + Duration::seconds(3),
            vec!["prj_a"],
            "SNAPSHOT_OUTCOME_LATEST_UPDATED",
        ),
        // A late snapshot that still lists the removed resource updates history only and does
        // not restore that resource's latest metrics.
        (
            start + Duration::seconds(2),
            vec!["prj_a", "prj_b"],
            "SNAPSHOT_OUTCOME_HISTORY_ONLY",
        ),
    ] {
        let reported = f.ok(
            &c1,
            "CollectorService/ReportSnapshot",
            json!({"connectionId":vercel,"leaseEpoch":epoch,"snapshot":{"observedAt":at(observed),"status":ok,"resources":rids.iter().map(|r| resource(r)).collect::<Vec<_>>(),
                "metrics":[{"kind":"METRIC_KIND_DEPLOYMENT_BILLED_COST","dimension":"total","value":{"state":"METRIC_STATE_VALUE","money":{"currencyCode":"USD","amount":"9"}},"temporality":"METRIC_TEMPORALITY_CUMULATIVE","periodStart":at(Utc::now()-Duration::days(3)),"periodEnd":at(Utc::now())}]}}),
        )
        .await;
        assert_eq!(reported["outcome"], outcome);
    }
    assert_eq!(
        f.count(&format!(
            "SELECT count(*) FROM metric_latest WHERE connection_id='{vercel}' AND resource_id='prj_b'"
        ))
        .await,
        0
    );
    let deployments = f
        .ok(
            &user,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash,"expectedVersion":"4","requestId":id(),"type":"WIDGET_TYPE_DEPLOYMENT_RESOURCES",
                "layout":{"x":0,"y":3,"width":6,"height":3,"presentation":"WIDGET_PRESENTATION_LIST"},"query":{},"source":{"connectionIds":[vercel]}}),
        )
        .await["widgetId"]
        .as_str()
        .unwrap()
        .to_owned();
    let data = f
        .ok(
            &user,
            "WidgetDataService/QueryWidgetData",
            json!({"dashboardId":dash,"widgetId":deployments}),
        )
        .await;
    let resources = data["data"]["resourceList"]["resources"]
        .as_array()
        .unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0]["resourceId"], "prj_a");
    assert_eq!(
        resources[0]["metrics"][0]["current"]["money"]["amount"],
        "1.5"
    );
    assert_eq!(data["data"]["summary"]["primaryValue"]["integer"], "1");
    // A subscription widget cannot use a deployment connection.
    f.fail(
        &user,
        "DashboardService/AddWidget",
        json!({"dashboardId":dash,"expectedVersion":"5","requestId":id(),"type":"WIDGET_TYPE_SUBSCRIPTION_LIMITS",
            "layout":{"x":0,"y":0,"width":4,"height":2,"presentation":"WIDGET_PRESENTATION_GAUGE"},"query":{},"source":{"connectionIds":[vercel]}}),
        "ERROR_REASON_INVALID_ARGUMENT",
    )
    .await;
    // A resource allowlist also excludes account-level readings from panels and history.
    let cost = f
        .ok(
            &user,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash,"expectedVersion":"5","requestId":id(),"type":"WIDGET_TYPE_DEPLOYMENT_COST",
                "layout":{"x":0,"y":6,"width":4,"height":2,"presentation":"WIDGET_PRESENTATION_NUMBER"},"query":{"resourceIds":["prj_a"]},"source":{"connectionIds":[vercel]}}),
        )
        .await["widgetId"]
        .as_str()
        .unwrap()
        .to_owned();
    let data = f
        .ok(
            &user,
            "WidgetDataService/QueryWidgetData",
            json!({"dashboardId":dash,"widgetId":cost}),
        )
        .await;
    let readings = data["data"]["metricPanel"]["readings"].as_array().unwrap();
    assert!(!readings.is_empty());
    assert!(
        readings.iter().all(|r| r["resourceId"] == "prj_a"),
        "{readings:?}"
    );
    assert_eq!(
        data["data"]["summary"]["primaryValue"]["money"]["amount"],
        "1.5"
    );
    let cost_history = f
        .ok(
            &user,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash,"expectedVersion":"6","requestId":id(),"type":"WIDGET_TYPE_METRIC_HISTORY",
                "layout":{"x":4,"y":6,"width":8,"height":3,"presentation":"WIDGET_PRESENTATION_LINE_CHART"},
                "query":{"period":"QUERY_PERIOD_LAST_24_HOURS","granularity":"TIME_GRANULARITY_HOUR","resourceIds":["prj_a"]},"source":{"connectionIds":[vercel]}}),
        )
        .await["widgetId"]
        .as_str()
        .unwrap()
        .to_owned();
    let data = f
        .ok(
            &user,
            "WidgetDataService/QueryWidgetData",
            json!({"dashboardId":dash,"widgetId":cost_history}),
        )
        .await;
    let series = data["data"]["metricHistory"]["series"].as_array().unwrap();
    assert!(!series.is_empty());
    assert!(
        series.iter().all(|s| s["resourceId"] == "prj_a"),
        "{series:?}"
    );
    assert!(f.server.metrics.encode().contains(
        "anydash_collection_snapshots_total{provider=\"claude_code\",status=\"failed\"} 1"
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn concurrent_heartbeats_with_crossed_leases_do_not_deadlock() {
    let f = Fixture::setup().await;
    let a = f.person("alice");
    f.connect(&a).await;
    let (d1, t1, _) = f.enroll(&a, "one", "").await;
    let (d2, t2, _) = f.enroll(&a, "two", "").await;
    let (c1, c2) = (f.device(&t1), f.device(&t2));
    let mut connections = Vec::new();
    for provider in ["PROVIDER_CODEX", "PROVIDER_CLAUDE_CODE"] {
        let conn = f
            .ok(
                &c1,
                "ConnectionService/CreateConnection",
                json!({"requestId":id(),"provider":provider}),
            )
            .await["connection"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        f.ok(
            &c2,
            "ConnectionService/RegisterConnectionDevice",
            json!({"connectionId":conn}),
        )
        .await;
        connections.push(conn);
    }
    connections.sort();
    // Both devices are registered for both connections and each holds a different lease.
    for (conn, holder) in connections.iter().zip([&d1, &d2]) {
        f.exec(&format!(
            "UPDATE connections SET lease_epoch=1,lease_device_id='{holder}',lease_acquired_at=now(),\
             lease_expires_at=now()+interval '5 minutes',last_holder_device_id='{holder}' WHERE id='{conn}'"
        ))
        .await;
    }
    let ready = json!({"readyConnectionIds":connections}).to_string();
    for _ in 0..25 {
        let tasks: Vec<_> = [t1.clone(), t2.clone()]
            .into_iter()
            .map(|token| {
                let (base, body) = (f.base.clone(), ready.clone());
                tokio::spawn(async move {
                    reqwest::Client::new()
                        .post(format!("{base}/anydash.v1.CollectorService/Heartbeat"))
                        .header("content-type", "application/json")
                        .header("x-anydash-client-kind", "CLIENT_KIND_CLI")
                        .header("x-anydash-client-version", "1.0.0")
                        .bearer_auth(&token)
                        .body(body)
                        .send()
                        .await
                        .unwrap()
                })
            })
            .collect();
        for task in tasks {
            let response = task.await.unwrap();
            let status = response.status();
            assert!(
                status.is_success(),
                "{status}: {}",
                response.text().await.unwrap()
            );
        }
    }
    // Every heartbeat extended only the device's own lease.
    for (conn, holder) in connections.iter().zip([&d1, &d2]) {
        assert_eq!(
            f.count(&format!(
                "SELECT count(*) FROM connections WHERE id='{conn}' AND lease_device_id='{holder}' AND lease_expires_at>now()"
            ))
            .await,
            1
        );
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn concurrent_heartbeat_and_snapshot_of_one_device_do_not_deadlock() {
    let f = Fixture::setup().await;
    let a = f.person("alice");
    f.connect(&a).await;
    let (_, token, _) = f.enroll(&a, "one", "").await;
    let c = f.device(&token);
    let conn = f
        .ok(
            &c,
            "ConnectionService/CreateConnection",
            json!({"requestId":id(),"provider":"PROVIDER_CODEX"}),
        )
        .await["connection"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    beat(&f, &c, &conn, true).await;
    assert_eq!(
        f.ok(
            &c,
            "CollectorService/AcquireLease",
            json!({"connectionId":conn})
        )
        .await["granted"],
        true
    );
    // The lease was acquired an hour ago so that every observation below is inside it.
    f.exec("UPDATE connections SET lease_acquired_at=now()-interval '1 hour'")
        .await;
    let rounds = 50;
    let start = Utc::now() - Duration::minutes(30);
    for round in 0..rounds {
        // Keep the device under its per-minute request limit; this test needs more requests.
        f.exec("DELETE FROM rate_limit_windows").await;
        let observed = start + Duration::seconds(round);
        let requests = [
            (
                "Heartbeat",
                json!({"readyConnectionIds":[conn]}).to_string(),
            ),
            (
                "ReportSnapshot",
                json!({"connectionId":conn,"leaseEpoch":"1","snapshot":snapshot(observed, "COLLECTION_STATUS_SUCCEEDED", vec![used("five_hour", 10.0)])})
                    .to_string(),
            ),
        ];
        let tasks: Vec<_> = requests
            .into_iter()
            .map(|(method, body)| {
                let (base, token) = (f.base.clone(), token.clone());
                tokio::spawn(async move {
                    reqwest::Client::new()
                        .post(format!("{base}/anydash.v1.CollectorService/{method}"))
                        .header("content-type", "application/json")
                        .header("x-anydash-client-kind", "CLIENT_KIND_CLI")
                        .header("x-anydash-client-version", "1.0.0")
                        .bearer_auth(&token)
                        .body(body)
                        .send()
                        .await
                        .unwrap()
                })
            })
            .collect();
        for task in tasks {
            let response = task.await.unwrap();
            let status = response.status();
            assert!(
                status.is_success(),
                "{status}: {}",
                response.text().await.unwrap()
            );
        }
    }
    assert_eq!(
        f.count(&format!(
            "SELECT count(*) FROM connections WHERE id='{conn}' AND latest_observed_at IS NOT NULL"
        ))
        .await,
        1
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn concurrent_heartbeat_and_registration_changes_of_one_device_do_not_deadlock() {
    let f = Fixture::setup().await;
    let a = f.person("alice");
    f.connect(&a).await;
    let (_, t1, _) = f.enroll(&a, "one", "").await;
    let (_, t2, _) = f.enroll(&a, "two", "").await;
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
    let rounds = 40;
    for round in 0..rounds {
        // Keep the device under its per-minute request limit; this test needs more requests.
        f.exec("DELETE FROM rate_limit_windows").await;
        // Device two alternately registers for and unregisters from the connection while it
        // reports itself ready for it.
        let change = if round % 2 == 0 {
            "ConnectionService/RegisterConnectionDevice"
        } else {
            "ConnectionService/UnregisterConnectionDevice"
        };
        let requests = [
            (
                "CollectorService/Heartbeat",
                json!({"readyConnectionIds":[conn]}).to_string(),
            ),
            (change, json!({"connectionId":conn}).to_string()),
        ];
        let tasks: Vec<_> = requests
            .into_iter()
            .map(|(method, body)| {
                let (base, token) = (f.base.clone(), t2.clone());
                tokio::spawn(async move {
                    reqwest::Client::new()
                        .post(format!("{base}/anydash.v1.{method}"))
                        .header("content-type", "application/json")
                        .header("x-anydash-client-kind", "CLIENT_KIND_CLI")
                        .header("x-anydash-client-version", "1.0.0")
                        .bearer_auth(&token)
                        .body(body)
                        .send()
                        .await
                        .unwrap()
                })
            })
            .collect();
        for task in tasks {
            let response = task.await.unwrap();
            let status = response.status();
            assert!(
                status.is_success(),
                "{status}: {}",
                response.text().await.unwrap()
            );
        }
    }
    // The last change was an unregistration.
    assert_eq!(
        f.count(&format!(
            "SELECT count(*) FROM connection_devices WHERE connection_id='{conn}'"
        ))
        .await,
        1
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn daily_metric_history_uses_the_observation_local_day() {
    use chrono::TimeZone;
    let f = Fixture::setup().await;
    let a = f.person("alice");
    f.connect(&a).await;
    let user = f.user(&a);
    let (_, token, _) = f.enroll(&a, "one", "").await;
    let c = f.device(&token);
    let conn = f
        .ok(
            &c,
            "ConnectionService/CreateConnection",
            json!({"requestId":id(),"provider":"PROVIDER_CLAUDE_CODE"}),
        )
        .await["connection"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    beat(&f, &c, &conn, true).await;
    assert_eq!(
        f.ok(
            &c,
            "CollectorService/AcquireLease",
            json!({"connectionId":conn})
        )
        .await["granted"],
        true
    );
    // The lease was acquired three days ago so that every observation below is inside it.
    f.exec("UPDATE connections SET lease_acquired_at=now()-interval '3 days'")
        .await;
    // Asia/Kolkata is UTC+05:30, so each UTC hour starts on the previous local day for the first
    // 30 minutes of a local day.
    let tz = chrono_tz::Asia::Kolkata;
    let midnight = |days_ago: i64| {
        let date = Utc::now().with_timezone(&tz).date_naive() - Duration::days(days_ago);
        tz.from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
            .single()
            .unwrap()
            .with_timezone(&Utc)
    };
    let (yesterday, today) = (midnight(1), midnight(0));
    for (observed, percent) in [
        // 23:25 the day before yesterday.
        (yesterday - Duration::minutes(35), 10.0),
        // 00:25 yesterday, stored in the UTC hour that starts at 23:30 the day before.
        (yesterday + Duration::minutes(25), 20.0),
        // 23:50 yesterday, stored in the UTC hour that starts at 23:30 yesterday and ends today.
        (today - Duration::minutes(10), 30.0),
    ] {
        assert_eq!(
            f.ok(
                &c,
                "CollectorService/ReportSnapshot",
                json!({"connectionId":conn,"leaseEpoch":"1","snapshot":snapshot(observed, "COLLECTION_STATUS_SUCCEEDED", vec![used("five_hour", percent)])}),
            )
            .await["outcome"],
            "SNAPSHOT_OUTCOME_LATEST_UPDATED"
        );
    }
    let dash = f
        .ok(
            &user,
            "DashboardService/CreateDashboard",
            json!({"requestId":id(),"spaceId":a.personal,"name":"Daily"}),
        )
        .await["dashboard"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut version = 1;
    let mut daily = |period: &'static str| {
        let (f, user, dash, conn) = (&f, &user, dash.clone(), conn.clone());
        version += 1;
        let expected = (version - 1).to_string();
        async move {
            let widget = f
                .ok(
                    user,
                    "DashboardService/AddWidget",
                    json!({"dashboardId":dash,"expectedVersion":expected,"requestId":id(),"type":"WIDGET_TYPE_SUBSCRIPTION_HISTORY",
                        "layout":{"x":0,"y":0,"width":8,"height":3,"presentation":"WIDGET_PRESENTATION_LINE_CHART"},
                        "query":{"period":period,"granularity":"TIME_GRANULARITY_DAY","dimensions":["five_hour"],
                            "metricKinds":["METRIC_KIND_SUBSCRIPTION_USED_PERCENT"]},"source":{"connectionIds":[conn]}}),
                )
                .await["widgetId"]
                .as_str()
                .unwrap()
                .to_owned();
            let data = f
                .ok(
                    user,
                    "WidgetDataService/QueryWidgetData",
                    json!({"dashboardId":dash,"widgetId":widget,"timeZone":"Asia/Kolkata"}),
                )
                .await;
            data["data"]["metricHistory"]["series"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|s| s["points"].as_array().cloned().unwrap_or_default())
                .map(|p| p["value"]["percent"].as_f64().unwrap())
                .collect::<Vec<f64>>()
        }
    };
    // One point per local day of observation: the last one of each day.
    assert_eq!(daily("QUERY_PERIOD_LAST_7_DAYS").await, [10.0, 30.0]);
    // Today has no observation; yesterday's 23:50 observation is outside the range even though
    // its UTC hour overlaps today.
    assert_eq!(daily("QUERY_PERIOD_TODAY").await, Vec::<f64>::new());
    f.cleanup().await;
}
