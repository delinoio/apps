// SPDX-License-Identifier: FSL-1.1-ALv2
mod support;
use buffa::Message;
use serde_json::json;
use support::{Fixture, key};

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn tenant_membership_authorization_and_mutation_replay() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let service = f.service().await;
    let me = f.rpc("owner", "GetCurrentUser", json!({})).await;
    let again = f.rpc("owner", "GetCurrentUser", json!({})).await;
    assert_eq!(me["userId"], again["userId"]);
    assert_eq!(me["tenants"], again["tenants"]);
    assert_eq!(me["tenants"].as_array().unwrap().len(), 1);
    let request = json!({"requestKey":key(),"name":"Organization"});
    let org = f.rpc("owner", "CreateTenant", request.clone()).await;
    assert_eq!(
        org["tenant"],
        f.rpc("owner", "CreateTenant", request.clone()).await["tenant"]
    );
    let mut conflict = request;
    conflict["name"] = json!("Changed");
    f.error("owner", "CreateTenant", conflict, "already_exists")
        .await;
    let tenant = org["tenant"]["id"].as_str().unwrap();
    f.error(
        "stranger",
        "GetTenant",
        json!({"tenantId":tenant}),
        "permission_denied",
    )
    .await;
    f.error("service", "GetCurrentUser", json!({}), "permission_denied")
        .await;
    f.error(
        "owner",
        "RemoveMember",
        json!({"requestKey":key(),"tenantId":tenant,"userId":me["userId"]}),
        "failed_precondition",
    )
    .await;
    f.error("owner","CreateInvitation",json!({"requestKey":key(),"tenantId":tenant,"email":"member@example.test","roles":["ROLE_MEMBER"],"returnUrl":"https://evil.example/return"}),"invalid_argument").await;
    f.rpc("owner","CreateInvitation",json!({"requestKey":key(),"tenantId":tenant,"email":"member@example.test","roles":["ROLE_MEMBER"],"returnUrl":f.config.return_urls[0]})).await;
    let text: String = sqlx::query_scalar(
        "SELECT payload->>'text' FROM jobs WHERE kind='email' ORDER BY created_at LIMIT 1",
    )
    .fetch_one(&f.db)
    .await
    .unwrap();
    let url = reqwest::Url::parse(
        text.strip_prefix("Accept your organization invitation: ")
            .unwrap(),
    )
    .unwrap();
    let token = url
        .query_pairs()
        .find(|(k, _)| k == "invitation_token")
        .unwrap()
        .1
        .into_owned();
    f.error(
        "stranger",
        "AcceptInvitation",
        json!({"requestKey":key(),"token":token}),
        "permission_denied",
    )
    .await;
    f.rpc(
        "member",
        "AcceptInvitation",
        json!({"requestKey":key(),"token":token}),
    )
    .await;
    f.error(
        "member",
        "AcceptInvitation",
        json!({"requestKey":key(),"token":token}),
        "permission_denied",
    )
    .await;
    let member = f.rpc("member", "GetCurrentUser", json!({})).await;
    f.rpc("owner","SetGroup",json!({"requestKey":key(),"tenantId":tenant,"group":{"name":"users","userIds":[member["userId"]],"permissions":[{"serviceId":service,"capability":"execute"}]}})).await;
    let allowed = f
        .rpc(
            "service",
            "Authorize",
            json!({"tenantId":tenant,"userId":member["userId"],"capability":"execute","paid":false}),
        )
        .await;
    assert_eq!(allowed["allowed"], true);
    let denied = f
        .rpc(
            "service",
            "Authorize",
            json!({"tenantId":tenant,"userId":member["userId"],"capability":"unknown"}),
        )
        .await;
    assert_ne!(denied["allowed"], true);
    f.rpc("owner","SetMemberRoles",json!({"requestKey":key(),"tenantId":tenant,"userId":member["userId"],"roles":["ROLE_ADMIN"]})).await;
    f.error("member","SetMemberRoles",json!({"requestKey":key(),"tenantId":tenant,"userId":member["userId"],"roles":["ROLE_BILLING"]}),"permission_denied").await;
    let transfer = f
        .rpc(
            "owner",
            "TransferOwnership",
            json!({"requestKey":key(),"tenantId":tenant,"recipientId":member["userId"]}),
        )
        .await;
    f.error(
        "stranger",
        "AcceptOwnership",
        json!({"requestKey":key(),"tenantId":tenant,"transferId":transfer["transferId"]}),
        "permission_denied",
    )
    .await;
    f.rpc(
        "member",
        "AcceptOwnership",
        json!({"requestKey":key(),"tenantId":tenant,"transferId":transfer["transferId"]}),
    )
    .await;
    let org = f
        .rpc("member", "GetTenant", json!({"tenantId":tenant}))
        .await;
    assert_eq!(org["tenant"]["ownerId"], member["userId"]);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn tenant_lifecycle_state_precedence_and_service_boundaries() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    f.service().await;
    let mut ids = Vec::new();
    let cases = [
        ("false", false, false, "TENANT_STATE_ACTIVE"),
        ("true", false, false, "TENANT_STATE_BLOCKED"),
        ("false", true, false, "TENANT_STATE_DELETING"),
        ("false", false, true, "TENANT_STATE_DELETED"),
        ("true", true, false, "TENANT_STATE_DELETING"),
        ("true", true, true, "TENANT_STATE_DELETED"),
    ];
    for (blocked, deleting, deleted, _) in cases {
        let org = f
            .rpc(
                "owner",
                "CreateTenant",
                json!({"requestKey":key(),"name":"State fixture"}),
            )
            .await;
        let id = org["tenant"]["id"].as_str().unwrap().to_owned();
        sqlx::query("UPDATE tenants SET operator_block=$2, deleting_at=CASE WHEN $3 THEN now() END, deleted_at=CASE WHEN $4 THEN now() END, renewal_failed=true WHERE id=$1")
            .bind(uuid::Uuid::parse_str(&id).unwrap()).bind(blocked=="true").bind(deleting).bind(deleted).execute(&f.db).await.unwrap();
        ids.push(id);
    }
    ids.push(key());
    let result = f
        .rpc("service", "GetTenantStates", json!({"tenantIds":ids}))
        .await;
    for (i, (_, _, _, state)) in cases.iter().enumerate() {
        assert_eq!(result["states"][i]["tenantId"], ids[i]);
        assert_eq!(result["states"][i]["state"], *state);
    }
    assert_eq!(result["states"][6]["state"], "TENANT_STATE_UNKNOWN");
    let lifecycle = f
        .rpc("service", "CheckTenantLifecycle", json!({"tenantIds":ids}))
        .await;
    let deleted = lifecycle["finalizedTenantIds"].as_array().unwrap();
    assert_eq!(deleted.len(), 2);
    assert!(deleted.contains(&json!(ids[3])));
    assert!(deleted.contains(&json!(ids[5])));
    for actor in ["owner", "ghost", "noscope"] {
        f.error(
            actor,
            "GetTenantStates",
            json!({"tenantIds":ids}),
            "permission_denied",
        )
        .await;
    }
    for input in [
        json!([]),
        json!(["not-a-uuid"]),
        json!(["00000000-0000-0000-0000-000000000000"]),
        json!(vec![key(); 101]),
    ] {
        f.error(
            "service",
            "GetTenantStates",
            json!({"tenantIds":input}),
            "invalid_argument",
        )
        .await;
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn tenant_member_roles_report_current_fixed_roles_only() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    f.service().await;
    let owner = f.rpc("owner", "GetCurrentUser", json!({})).await;
    let stranger = f.rpc("stranger", "GetCurrentUser", json!({})).await;
    let org = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Roles fixture"}),
        )
        .await;
    let tenant = org["tenant"]["id"].as_str().unwrap().to_owned();
    let join = |tenant: String| {
        let f = &f;
        async move {
            f.rpc("owner","CreateInvitation",json!({"requestKey":key(),"tenantId":tenant,"email":"member@example.test","roles":["ROLE_MEMBER"],"returnUrl":f.config.return_urls[0]})).await;
            let text: String = sqlx::query_scalar(
                "SELECT payload->>'text' FROM jobs WHERE kind='email' ORDER BY created_at DESC,id DESC LIMIT 1",
            )
            .fetch_one(&f.db)
            .await
            .unwrap();
            let url = reqwest::Url::parse(
                text.strip_prefix("Accept your organization invitation: ")
                    .unwrap(),
            )
            .unwrap();
            let token = url
                .query_pairs()
                .find(|(k, _)| k == "invitation_token")
                .unwrap()
                .1
                .into_owned();
            f.rpc(
                "member",
                "AcceptInvitation",
                json!({"requestKey":key(),"token":token}),
            )
            .await;
        }
    };
    join(tenant.clone()).await;
    let member = f.rpc("member", "GetCurrentUser", json!({})).await;
    let unknown = key();
    let ids = json!([
        member["userId"],
        owner["userId"],
        stranger["userId"],
        unknown,
        member["userId"]
    ]);
    let read = |ids: serde_json::Value, tenant: String| {
        let f = &f;
        async move {
            f.rpc(
                "service",
                "GetTenantMemberRoles",
                json!({"tenantId":tenant,"userIds":ids}),
            )
            .await
        }
    };
    let result = read(ids.clone(), tenant.clone()).await;
    let members = result["members"].as_array().unwrap();
    assert_eq!(members.len(), 5);
    for (entry, id) in members.iter().zip(ids.as_array().unwrap()) {
        assert_eq!(&entry["userId"], id);
    }
    assert_eq!(members[0]["member"], true);
    assert_eq!(members[0]["roles"], json!(["ROLE_MEMBER"]));
    assert_eq!(members[1]["member"], true);
    assert_eq!(members[1]["roles"], json!(["ROLE_OWNER"]));
    // Current members carry the start of their current membership; non-members carry none.
    let since = |entry: &serde_json::Value| {
        chrono::DateTime::parse_from_rfc3339(entry["memberSince"].as_str().unwrap()).unwrap()
    };
    let joined = since(&members[0]);
    assert!(since(&members[1]) <= joined, "the owner joined first");
    for entry in &members[2..4] {
        assert_ne!(entry["member"], true);
        assert!(entry.get("roles").is_none_or(|r| r == &json!([])));
        assert!(entry.get("memberSince").is_none_or(|v| v.is_null()));
    }
    assert_eq!(members[4], members[0]);
    // Billing-only members are reported with their role; consumers decide what it allows.
    // Role changes keep the membership start.
    f.rpc("owner","SetMemberRoles",json!({"requestKey":key(),"tenantId":tenant,"userId":member["userId"],"roles":["ROLE_BILLING","ROLE_ADMIN"]})).await;
    let result = read(json!([member["userId"]]), tenant.clone()).await;
    assert_eq!(
        result["members"][0]["roles"],
        json!(["ROLE_ADMIN", "ROLE_BILLING"])
    );
    assert_eq!(since(&result["members"][0]), joined);
    // Leaving and rejoining starts a new membership with a new start.
    f.rpc(
        "member",
        "RemoveMember",
        json!({"requestKey":key(),"tenantId":tenant,"userId":member["userId"]}),
    )
    .await;
    let result = read(json!([member["userId"]]), tenant.clone()).await;
    assert_ne!(result["members"][0]["member"], true);
    assert!(
        result["members"][0]
            .get("memberSince")
            .is_none_or(|v| v.is_null())
    );
    join(tenant.clone()).await;
    let result = read(json!([member["userId"]]), tenant.clone()).await;
    assert_eq!(result["members"][0]["member"], true);
    assert_eq!(result["members"][0]["roles"], json!(["ROLE_MEMBER"]));
    assert!(since(&result["members"][0]) > joined);
    f.rpc("owner","SetMemberRoles",json!({"requestKey":key(),"tenantId":tenant,"userId":member["userId"],"roles":["ROLE_BILLING","ROLE_ADMIN"]})).await;
    let audit_before: i64 = sqlx::query_scalar("SELECT count(*) FROM audit")
        .fetch_one(&f.db)
        .await
        .unwrap();
    // A withdrawing user is no longer a member for consumers.
    sqlx::query("UPDATE users SET withdrawing_at=now() WHERE id=$1")
        .bind(uuid::Uuid::parse_str(member["userId"].as_str().unwrap()).unwrap())
        .execute(&f.db)
        .await
        .unwrap();
    let result = read(json!([member["userId"], owner["userId"]]), tenant.clone()).await;
    assert_ne!(result["members"][0]["member"], true);
    assert_eq!(result["members"][1]["member"], true);
    // A deleted tenant has no members; an unknown tenant has none either.
    sqlx::query("UPDATE tenants SET deleted_at=now() WHERE id=$1")
        .bind(uuid::Uuid::parse_str(&tenant).unwrap())
        .execute(&f.db)
        .await
        .unwrap();
    let result = read(json!([owner["userId"]]), tenant.clone()).await;
    assert_ne!(result["members"][0]["member"], true);
    let result = read(json!([owner["userId"]]), key()).await;
    assert_ne!(result["members"][0]["member"], true);
    // The read never writes audit rows.
    let audit_after: i64 = sqlx::query_scalar("SELECT count(*) FROM audit")
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(audit_before, audit_after);
    for actor in ["owner", "ghost", "noscope"] {
        f.error(
            actor,
            "GetTenantMemberRoles",
            json!({"tenantId":tenant,"userIds":[owner["userId"]]}),
            "permission_denied",
        )
        .await;
    }
    for input in [
        json!({"tenantId":tenant,"userIds":[]}),
        json!({"tenantId":tenant,"userIds":["not-a-uuid"]}),
        json!({"tenantId":tenant,"userIds":["00000000-0000-0000-0000-000000000000"]}),
        json!({"tenantId":tenant,"userIds":vec![key(); 101]}),
        json!({"tenantId":"not-a-uuid","userIds":[owner["userId"]]}),
        json!({"userIds":[owner["userId"]]}),
    ] {
        f.error("service", "GetTenantMemberRoles", input, "invalid_argument")
            .await;
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn protobuf_wire_and_authentication_rejection() {
    use delino_protocol::delcore::v1::{GetCurrentUserRequest, GetCurrentUserResponse};
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let endpoint = format!("{}/delcore.v1.DelcoreService/GetCurrentUser", f.base);
    let response = f
        .http
        .post(&endpoint)
        .bearer_auth(f.token("owner"))
        .header("content-type", "application/proto")
        .header("connect-protocol-version", "1")
        .body(GetCurrentUserRequest::default().encode_to_vec())
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let bytes = response.bytes().await.unwrap();
    let me = GetCurrentUserResponse::decode_from_slice(bytes.as_ref()).unwrap();
    assert!(!me.user_id.is_empty());
    assert_eq!(me.tenants.len(), 1);
    for authorization in ["", "Bearer invalid", "Basic invalid"] {
        let response = f
            .http
            .post(&endpoint)
            .header("authorization", authorization)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn request_expiry_is_fixed_from_creation_and_enforced_before_maintenance() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let request_key = key();
    let mut request = json!({"requestKey":request_key,"name":"Original"});
    let first = f.rpc("owner", "CreateTenant", request.clone()).await;
    let original_created: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT created_at FROM requests WHERE request_key=$1")
            .bind(&request_key)
            .fetch_one(&f.db)
            .await
            .unwrap();
    let replay = f.rpc("owner", "CreateTenant", request.clone()).await;
    assert_eq!(first["tenant"]["id"], replay["tenant"]["id"]);
    let replay_created: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT created_at FROM requests WHERE request_key=$1")
            .bind(&request_key)
            .fetch_one(&f.db)
            .await
            .unwrap();
    assert_eq!(original_created, replay_created);
    request["name"] = json!("Changed");
    f.error("owner", "CreateTenant", request.clone(), "already_exists")
        .await;
    sqlx::query("UPDATE requests SET created_at=now()-interval '30 days' WHERE request_key=$1")
        .bind(&request_key)
        .execute(&f.db)
        .await
        .unwrap();
    let fresh = f.rpc("owner", "CreateTenant", request).await;
    assert_ne!(first["tenant"]["id"], fresh["tenant"]["id"]);
    assert_eq!(fresh["tenant"]["name"], "Changed");
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn retention_removes_requests_and_audits_at_the_thirty_day_boundary() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let oldest = key();
    let recent = key();
    f.rpc(
        "owner",
        "CreateTenant",
        json!({"requestKey":oldest,"name":"Old"}),
    )
    .await;
    f.rpc(
        "owner",
        "CreateTenant",
        json!({"requestKey":recent,"name":"Recent"}),
    )
    .await;
    let now = chrono::Utc::now();
    let cutoff = now - chrono::Duration::days(30);
    sqlx::query("UPDATE requests SET created_at=$2 WHERE request_key=$1")
        .bind(&oldest)
        .bind(cutoff)
        .execute(&f.db)
        .await
        .unwrap();
    sqlx::query("UPDATE requests SET created_at=$2 WHERE request_key=$1")
        .bind(&recent)
        .bind(cutoff + chrono::Duration::microseconds(1))
        .execute(&f.db)
        .await
        .unwrap();
    sqlx::query("UPDATE audit SET created_at=$1")
        .bind(cutoff)
        .execute(&f.db)
        .await
        .unwrap();
    f.worker.maintain(now).await.unwrap();
    let (expired, retained, audits): (bool, bool, i64) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM requests WHERE request_key=$1),EXISTS(SELECT 1 FROM requests WHERE request_key=$2),(SELECT count(*) FROM audit)")
        .bind(&oldest).bind(&recent).fetch_one(&f.db).await.unwrap();
    assert!(!expired);
    assert!(retained);
    assert_eq!(audits, 0);
    f.worker
        .maintain(now + chrono::Duration::microseconds(1))
        .await
        .unwrap();
    let retained: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM requests WHERE request_key=$1)")
            .bind(&recent)
            .fetch_one(&f.db)
            .await
            .unwrap();
    assert!(!retained);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn non_rfc_variant_uuid_v7_inputs_are_rejected_without_committing() {
    const COUNTS: &str = "SELECT (SELECT count(*) FROM tenants),(SELECT count(*) FROM requests),(SELECT count(*) FROM audit),(SELECT count(*) FROM catalog_entities),(SELECT count(*) FROM plans),(SELECT count(*) FROM services)";
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let before: (i64, i64, i64, i64, i64, i64) =
        sqlx::query_as(COUNTS).fetch_one(&f.db).await.unwrap();
    for variant in ['0', '7', 'c', 'e', 'f'] {
        let bad = format!("01900000-0000-7000-{variant}000-000000000001");
        let rejected = f
            .error(
                "owner",
                "CreateTenant",
                json!({"requestKey":bad,"name":"Variant"}),
                "invalid_argument",
            )
            .await;
        assert_eq!(rejected["message"], "request_key must be a UUID v7");
        f.error(
            "owner",
            "PutPlan",
            json!({"requestKey":key(),"reason":"variant fixture","plan":{"id":bad,"key":"variant-plan","name":"Variant Plan","rank":10}}),
            "invalid_argument",
        )
        .await;
        f.error(
            "owner",
            "PutService",
            json!({"requestKey":key(),"reason":"variant fixture","service":{"id":bad,"key":"variant","name":"variant","clientId":"variant","capabilities":["execute"],"scopes":["authorize"]}}),
            "invalid_argument",
        )
        .await;
        let after: (i64, i64, i64, i64, i64, i64) =
            sqlx::query_as(COUNTS).fetch_one(&f.db).await.unwrap();
        assert_eq!(before, after, "{bad}");
    }
    // RFC 9562 variants remain accepted for the same inputs.
    for variant in ['8', '9', 'a', 'b'] {
        let good = format!("01900000-0000-7000-{variant}000-000000000001");
        let tenant = f
            .rpc(
                "owner",
                "CreateTenant",
                json!({"requestKey":good,"name":format!("Variant {variant}")}),
            )
            .await;
        assert!(tenant["tenant"]["id"].is_string());
    }
}
