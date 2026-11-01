// SPDX-License-Identifier: FSL-1.1-ALv2
//! Tenant isolation, the team role matrix and data-owner sharing.
mod support;
use serde_json::{Value, json};
use support::{Fixture, id};

fn layout(presentation: &str) -> Value {
    json!({"x":0,"y":0,"width":4,"height":2,"presentation":presentation})
}

async fn version(f: &Fixture, caller: &support::Caller, dashboard: &str) -> String {
    f.ok(
        caller,
        "DashboardService/GetDashboard",
        json!({"dashboardId":dashboard}),
    )
    .await["dashboard"]["version"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn team_roles_isolation_and_data_owner_sharing() {
    let f = Fixture::setup().await;
    let owner = f.person("owner");
    let admin = f.person("admin");
    let member = f.person("member");
    let billing = f.person("billing");
    let outsider = f.person("outsider");
    let team = f.team(
        "Acme",
        &[
            (&owner, &["ROLE_OWNER"]),
            (&admin, &["ROLE_ADMIN"]),
            (&member, &["ROLE_MEMBER"]),
            (&billing, &["ROLE_BILLING"]),
        ],
    );
    for p in [&owner, &admin, &member, &billing, &outsider] {
        f.connect(p).await;
    }
    let (o, m, ad, bl, x) = (
        f.user(&owner),
        f.user(&member),
        f.user(&admin),
        f.user(&billing),
        f.user(&outsider),
    );
    let (_, owner_token, _) = f.enroll(&owner, "owner-laptop", "").await;
    let (member_device, member_token, _) = f.enroll(&member, "member-laptop", "").await;
    let od = f.device(&owner_token);
    let codex = f
        .ok(
            &od,
            "ConnectionService/CreateConnection",
            json!({"requestId":id(),"provider":"PROVIDER_CODEX"}),
        )
        .await["connection"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Spaces: Members see the team with view and edit; Billing-only users do not see it.
    let spaces = |p: &support::Person| {
        let f = &f;
        let token = f.token(p, support::DELCORE_RESOURCE);
        let caller = f.user(p);
        async move {
            f.ok(
                &caller,
                "DashboardService/ListSpaces",
                json!({"delcoreAccessToken":token}),
            )
            .await
        }
    };
    let s = spaces(&member).await;
    let list = s["spaces"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["kind"], "SPACE_KIND_PERSONAL");
    assert_eq!(list[1]["id"], team);
    assert_eq!(
        list[1]["permissions"],
        json!([
            "SPACE_PERMISSION_VIEW_DASHBOARDS",
            "SPACE_PERMISSION_EDIT_DASHBOARDS"
        ])
    );
    assert_eq!(
        spaces(&billing).await["spaces"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        spaces(&admin).await["spaces"][1]["permissions"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    // The admin has no collector device: the team space works, the personal space needs onboarding.
    assert_eq!(
        spaces(&admin).await["spaces"][0]["onboardingState"],
        "ONBOARDING_STATE_DEVICE_REQUIRED"
    );
    f.fail(
        &ad,
        "DashboardService/ListDashboards",
        json!({"spaceId":admin.personal}),
        "ERROR_REASON_ONBOARDING_REQUIRED",
    )
    .await;
    f.ok(
        &ad,
        "DashboardService/ListDashboards",
        json!({"spaceId":team}),
    )
    .await;
    // A Delcore token of another subject is rejected.
    f.fail(
        &m,
        "DashboardService/ListSpaces",
        json!({"delcoreAccessToken": f.token(&owner, support::DELCORE_RESOURCE)}),
        "ERROR_REASON_UNAUTHENTICATED",
    )
    .await;

    // Create: Owner and Admin only.
    for (caller, reason) in [
        (&m, "ERROR_REASON_PERMISSION_DENIED"),
        (&bl, "ERROR_REASON_PERMISSION_DENIED"),
        (&x, "ERROR_REASON_PERMISSION_DENIED"),
    ] {
        f.fail(
            caller,
            "DashboardService/CreateDashboard",
            json!({"requestId":id(),"spaceId":team,"name":"Nope"}),
            reason,
        )
        .await;
    }
    let request = id();
    let dash = f
        .ok(
            &ad,
            "DashboardService/CreateDashboard",
            json!({"requestId":request,"spaceId":team,"name":"Team board"}),
        )
        .await["dashboard"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let replay = f
        .ok(
            &ad,
            "DashboardService/CreateDashboard",
            json!({"requestId":request,"spaceId":team,"name":"Team board"}),
        )
        .await;
    assert_eq!(replay["dashboard"]["id"], dash);
    f.fail(
        &ad,
        "DashboardService/CreateDashboard",
        json!({"requestId":request,"spaceId":team,"name":"Other"}),
        "ERROR_REASON_REQUEST_ID_REUSED",
    )
    .await;
    // View: Owner, Admin and Member; Billing-only users and non-members never see it.
    for caller in [&o, &ad, &m] {
        f.ok(
            caller,
            "DashboardService/GetDashboard",
            json!({"dashboardId":dash}),
        )
        .await;
    }
    for caller in [&bl, &x] {
        f.fail(
            caller,
            "DashboardService/GetDashboard",
            json!({"dashboardId":dash}),
            "ERROR_REASON_NOT_FOUND",
        )
        .await;
        f.fail(
            caller,
            "DashboardService/ListDashboards",
            json!({"spaceId":team}),
            "ERROR_REASON_PERMISSION_DENIED",
        )
        .await;
    }
    // Members rename; versions are checked.
    let renamed = f
        .ok(
            &m,
            "DashboardService/RenameDashboard",
            json!({"dashboardId":dash,"expectedVersion":"1","name":"Renamed"}),
        )
        .await;
    assert_eq!(renamed["dashboard"]["version"], "2");
    let conflict = f
        .fail(
            &o,
            "DashboardService/RenameDashboard",
            json!({"dashboardId":dash,"expectedVersion":"1","name":"Stale"}),
            "ERROR_REASON_VERSION_CONFLICT",
        )
        .await;
    assert_eq!(conflict.detail.current_version, 2);
    assert_eq!(conflict.code, "aborted");

    // The owner shares subscription limits; the member shares local usage of one device.
    let owner_widget = f
        .ok(
            &o,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash,"expectedVersion":"2","requestId":id(),"type":"WIDGET_TYPE_SUBSCRIPTION_LIMITS","layout":layout("WIDGET_PRESENTATION_GAUGE"),"query":{},"source":{"connectionIds":[codex]}}),
        )
        .await;
    let owner_widget_id = owner_widget["widgetId"].as_str().unwrap().to_owned();
    assert_eq!(
        owner_widget["dashboard"]["widgets"][0]["sharingState"],
        "SHARING_STATE_SHARED"
    );
    // A member cannot share another user's connection.
    f.fail(
        &m,
        "DashboardService/AddWidget",
        json!({"dashboardId":dash,"expectedVersion":"3","requestId":id(),"type":"WIDGET_TYPE_SUBSCRIPTION_LIMITS","layout":layout("WIDGET_PRESENTATION_GAUGE"),"query":{},"source":{"connectionIds":[codex]}}),
        "ERROR_REASON_NOT_FOUND",
    )
    .await;
    let member_widget = f
        .ok(
            &m,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash,"expectedVersion":"3","requestId":id(),"type":"WIDGET_TYPE_LOCAL_USAGE_SERIES","layout":layout("WIDGET_PRESENTATION_BAR_CHART"),
                "query":{"period":"QUERY_PERIOD_LAST_7_DAYS","granularity":"TIME_GRANULARITY_DAY"},"source":{"deviceIds":[member_device]}}),
        )
        .await["widgetId"]
        .as_str()
        .unwrap()
        .to_owned();
    // Only the data owner sees the source.
    let seen = f
        .ok(
            &m,
            "DashboardService/GetDashboard",
            json!({"dashboardId":dash}),
        )
        .await;
    let widgets = seen["dashboard"]["widgets"].as_array().unwrap();
    let theirs = widgets.iter().find(|w| w["id"] == owner_widget_id).unwrap();
    assert!(theirs.get("source").is_none());
    assert_ne!(theirs["callerIsDataOwner"], true);
    let mine = widgets.iter().find(|w| w["id"] == member_widget).unwrap();
    assert_eq!(mine["callerIsDataOwner"], true);
    assert_eq!(mine["source"]["deviceIds"], json!([member_device]));
    // Members co-edit layout of any widget, but never another owner's query or sharing.
    let v = version(&f, &m, &dash).await;
    f.ok(
        &m,
        "DashboardService/UpdateWidgetLayout",
        json!({"dashboardId":dash,"expectedVersion":v,"updates":[{"widgetId":owner_widget_id,"layout":{"x":4,"y":1,"width":4,"height":3,"title":"Codex","presentation":"WIDGET_PRESENTATION_LIST"}}]}),
    )
    .await;
    let v = version(&f, &m, &dash).await;
    f.fail(
        &m,
        "DashboardService/UpdateWidgetQuery",
        json!({"dashboardId":dash,"expectedVersion":v,"widgetId":owner_widget_id,"query":{"dimensions":["weekly"]}}),
        "ERROR_REASON_DATA_OWNER_REQUIRED",
    )
    .await;
    // A team role never bypasses the data-owner rule.
    f.fail(
        &ad,
        "DashboardService/SetWidgetSharingScope",
        json!({"dashboardId":dash,"expectedVersion":v,"widgetId":owner_widget_id,"source":{"connectionIds":[codex]}}),
        "ERROR_REASON_DATA_OWNER_REQUIRED",
    )
    .await;
    f.fail(
        &ad,
        "DashboardService/RevokeWidgetSharing",
        json!({"dashboardId":dash,"widgetId":owner_widget_id}),
        "ERROR_REASON_DATA_OWNER_REQUIRED",
    )
    .await;
    // Members cannot remove another user's widget; Admins can.
    f.fail(
        &m,
        "DashboardService/RemoveWidget",
        json!({"dashboardId":dash,"expectedVersion":v,"widgetId":owner_widget_id}),
        "ERROR_REASON_PERMISSION_DENIED",
    )
    .await;
    // Members cannot delete dashboards.
    f.fail(
        &m,
        "DashboardService/DeleteDashboard",
        json!({"dashboardId":dash}),
        "ERROR_REASON_PERMISSION_DENIED",
    )
    .await;
    // The owner updates their own query.
    let v = version(&f, &o, &dash).await;
    f.ok(
        &o,
        "DashboardService/UpdateWidgetQuery",
        json!({"dashboardId":dash,"expectedVersion":v,"widgetId":owner_widget_id,"query":{"dimensions":["five_hour"]}}),
    )
    .await;

    // Shared data is readable by members; revocation ends access immediately.
    let query = |caller: &support::Caller, widget: &str| {
        let f = &f;
        let dash = dash.clone();
        let widget = widget.to_owned();
        let caller = support::Caller {
            authorization: caller.authorization.clone(),
            kind: caller.kind,
            version: caller.version,
        };
        async move {
            f.ok(
                &caller,
                "WidgetDataService/QueryWidgetData",
                json!({"dashboardId":dash,"widgetId":widget}),
            )
            .await
        }
    };
    let data = query(&m, &owner_widget_id).await;
    assert_eq!(data["data"]["sharingState"], "SHARING_STATE_SHARED");
    assert!(data["data"]["metricPanel"].is_object());
    let revoked = f
        .ok(
            &o,
            "DashboardService/RevokeWidgetSharing",
            json!({"dashboardId":dash,"widgetId":owner_widget_id}),
        )
        .await;
    assert!(revoked["dashboard"]["version"].is_string());
    let data = query(&m, &owner_widget_id).await;
    assert_eq!(data["data"]["sharingState"], "SHARING_STATE_ENDED");
    assert_eq!(
        data["data"]["sharingEndReason"],
        "SHARING_END_REASON_OWNER_REVOKED"
    );
    assert!(data["data"].get("metricPanel").is_none() && data["data"].get("summary").is_none());
    // Sharing again restores access.
    let v = version(&f, &o, &dash).await;
    f.ok(
        &o,
        "DashboardService/SetWidgetSharingScope",
        json!({"dashboardId":dash,"expectedVersion":v,"widgetId":owner_widget_id,"source":{"connectionIds":[codex]}}),
    )
    .await;
    assert_eq!(
        query(&m, &owner_widget_id).await["data"]["sharingState"],
        "SHARING_STATE_SHARED"
    );
    // Leaving the team ends sharing immediately; rejoining does not restore it.
    f.set_roles(&team, &owner, &["ROLE_BILLING"]);
    let data = query(&m, &owner_widget_id).await;
    assert_eq!(data["data"]["sharingState"], "SHARING_STATE_ENDED");
    assert_eq!(
        data["data"]["sharingEndReason"],
        "SHARING_END_REASON_OWNER_LEFT_SPACE"
    );
    f.set_roles(&team, &owner, &["ROLE_OWNER"]);
    assert_eq!(
        query(&m, &owner_widget_id).await["data"]["sharingState"],
        "SHARING_STATE_ENDED"
    );
    // Role reads are batched: one call per tenant for every data owner of the dashboard.
    f.mock.lock().unwrap().calls.clear();
    f.ok(
        &ad,
        "WidgetDataService/QueryDashboardData",
        json!({"dashboardId":dash}),
    )
    .await;
    let roles_calls = f
        .mock
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|c| *c == "GetTenantMemberRoles")
        .count();
    assert_eq!(
        roles_calls, 2,
        "one for the caller and one batch for the data owners"
    );
    // Deleting the source device ends the member's widget.
    f.ok(
        &f.user(&member),
        "DeviceService/DeleteDevice",
        json!({"deviceId":member_device}),
    )
    .await;
    let data = query(&o, &member_widget).await;
    assert_eq!(
        data["data"]["sharingEndReason"],
        "SHARING_END_REASON_SOURCE_DELETED"
    );
    f.fail(
        &f.device(&member_token),
        "CollectorService/Heartbeat",
        json!({}),
        "ERROR_REASON_DEVICE_CREDENTIALS_INACTIVE",
    )
    .await;
    // Admins remove other users' widgets and delete dashboards.
    let v = version(&f, &ad, &dash).await;
    f.ok(
        &ad,
        "DashboardService/RemoveWidget",
        json!({"dashboardId":dash,"expectedVersion":v,"widgetId":member_widget}),
    )
    .await;

    // Tenant isolation.
    f.fail(
        &x,
        "ConnectionService/GetConnection",
        json!({"connectionId":codex}),
        "ERROR_REASON_NOT_FOUND",
    )
    .await;
    f.fail(
        &x,
        "ConnectionService/DeleteConnection",
        json!({"connectionId":codex}),
        "ERROR_REASON_NOT_FOUND",
    )
    .await;
    assert_eq!(
        f.ok(&x, "DeviceService/ListDevices", json!({})).await["devices"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        0
    );
    f.fail(
        &x,
        "DashboardService/ListDashboards",
        json!({"spaceId":owner.personal}),
        "ERROR_REASON_PERMISSION_DENIED",
    )
    .await;
    let personal = f
        .ok(
            &o,
            "DashboardService/CreateDashboard",
            json!({"requestId":id(),"spaceId":owner.personal,"name":"Mine"}),
        )
        .await["dashboard"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for caller in [&x, &ad, &m] {
        f.fail(
            caller,
            "DashboardService/GetDashboard",
            json!({"dashboardId":personal}),
            "ERROR_REASON_NOT_FOUND",
        )
        .await;
        f.fail(
            caller,
            "WidgetDataService/QueryDashboardData",
            json!({"dashboardId":personal}),
            "ERROR_REASON_NOT_FOUND",
        )
        .await;
    }
    f.fail(
        &x,
        "DeviceService/SetPreferredDevice",
        json!({"deviceId":member_device}),
        "ERROR_REASON_NOT_FOUND",
    )
    .await;

    // Activity: Owners and Admins see team events; Members do not.
    let events = f
        .ok(&ad, "AccountService/ListActivity", json!({"spaceId":team}))
        .await;
    let kinds: Vec<&str> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    for kind in [
        "ACTIVITY_KIND_DASHBOARD_CREATED",
        "ACTIVITY_KIND_WIDGET_SHARED",
        "ACTIVITY_KIND_WIDGET_SHARING_ENDED",
        "ACTIVITY_KIND_PERMISSION_DENIED",
    ] {
        assert!(kinds.contains(&kind), "{kind} in {kinds:?}");
    }
    f.fail(
        &m,
        "AccountService/ListActivity",
        json!({"spaceId":team}),
        "ERROR_REASON_PERMISSION_DENIED",
    )
    .await;
    let own = f.ok(&o, "AccountService/ListActivity", json!({})).await;
    let kinds: Vec<&str> = own["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(
        kinds.contains(&"ACTIVITY_KIND_CONNECTION_CREATED")
            && kinds.contains(&"ACTIVITY_KIND_DEVICE_ENROLLED")
    );
    assert!(!kinds.contains(&"ACTIVITY_KIND_DASHBOARD_CREATED"));
    // Every denial wrote an audit event.
    assert!(
        f.count("SELECT count(*) FROM audit_events WHERE kind=16")
            .await
            >= 8
    );
    f.ok(
        &ad,
        "DashboardService/DeleteDashboard",
        json!({"dashboardId":dash}),
    )
    .await;
    f.fail(
        &m,
        "DashboardService/GetDashboard",
        json!({"dashboardId":dash}),
        "ERROR_REASON_NOT_FOUND",
    )
    .await;
    f.cleanup().await;
}

/// The widget's sharing state and end reason as reported by `QueryWidgetData`.
async fn sharing(
    f: &Fixture,
    caller: &support::Caller,
    dashboard: &str,
    widget: &str,
) -> (String, String) {
    let data = f
        .ok(
            caller,
            "WidgetDataService/QueryWidgetData",
            json!({"dashboardId":dashboard,"widgetId":widget}),
        )
        .await;
    (
        data["data"]["sharingState"].as_str().unwrap().to_owned(),
        data["data"]["sharingEndReason"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    )
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn departure_and_rejoin_end_sharing_without_an_intermediate_query() {
    let f = Fixture::setup().await;
    let owner = f.person("owner");
    let member = f.person("member");
    let team = f.team(
        "Acme",
        &[(&owner, &["ROLE_OWNER"]), (&member, &["ROLE_MEMBER"])],
    );
    for p in [&owner, &member] {
        f.connect(p).await;
    }
    let (o, m) = (f.user(&owner), f.user(&member));
    let (_, member_token, _) = f.enroll(&member, "member-laptop", "").await;
    let codex = f
        .ok(
            &f.device(&member_token),
            "ConnectionService/CreateConnection",
            json!({"requestId":id(),"provider":"PROVIDER_CODEX"}),
        )
        .await["connection"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let dash = f
        .ok(
            &o,
            "DashboardService/CreateDashboard",
            json!({"requestId":id(),"spaceId":team,"name":"Team board"}),
        )
        .await["dashboard"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let v = version(&f, &m, &dash).await;
    let widget = f
        .ok(
            &m,
            "DashboardService/AddWidget",
            json!({"dashboardId":dash,"expectedVersion":v,"requestId":id(),"type":"WIDGET_TYPE_SUBSCRIPTION_LIMITS","layout":layout("WIDGET_PRESENTATION_GAUGE"),"query":{},"source":{"connectionIds":[codex]}}),
        )
        .await["widgetId"]
        .as_str()
        .unwrap()
        .to_owned();
    // The data owner's membership start is stored with the shared widget.
    assert_eq!(
        f.count(&format!(
            "SELECT count(*) FROM widgets WHERE id='{widget}' AND data_owner_member_since IS NOT NULL"
        ))
        .await,
        1
    );
    let state = |caller| sharing(&f, caller, &dash, &widget);
    assert_eq!(state(&o).await.0, "SHARING_STATE_SHARED");
    // A role change keeps the membership and therefore the sharing.
    f.set_roles(&team, &member, &["ROLE_ADMIN"]);
    assert_eq!(state(&o).await.0, "SHARING_STATE_SHARED");
    // The data owner leaves and rejoins with no query in between: the new membership start ends
    // sharing at the next evaluation, and the end is persisted.
    f.set_roles(&team, &member, &[]);
    f.set_roles(&team, &member, &["ROLE_MEMBER"]);
    assert_eq!(
        state(&o).await,
        (
            "SHARING_STATE_ENDED".to_owned(),
            "SHARING_END_REASON_OWNER_LEFT_SPACE".to_owned()
        )
    );
    assert_eq!(
        f.count(&format!(
            "SELECT count(*) FROM widgets WHERE id='{widget}' AND sharing_state=3 AND sharing_end_reason=2"
        ))
        .await,
        1
    );
    assert_eq!(state(&m).await.0, "SHARING_STATE_ENDED");
    // The rejoined owner can share explicitly again; the current membership start is stored.
    let v = version(&f, &m, &dash).await;
    f.ok(
        &m,
        "DashboardService/SetWidgetSharingScope",
        json!({"dashboardId":dash,"expectedVersion":v,"widgetId":widget,"source":{"connectionIds":[codex]}}),
    )
    .await;
    assert_eq!(state(&o).await.0, "SHARING_STATE_SHARED");
    assert_eq!(state(&m).await.0, "SHARING_STATE_SHARED");
    f.cleanup().await;
}
