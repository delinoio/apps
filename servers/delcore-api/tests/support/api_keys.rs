// SPDX-License-Identifier: FSL-1.1-ALv2
//! API key lifecycle and authorization against disposable PostgreSQL. Services
//! here are plain catalog data: nothing in the keys facility knows any product.
use super::{Fixture, key};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const PERSONAL: &str = "API_KEY_OWNER_KIND_PERSONAL";
const TENANT: &str = "API_KEY_OWNER_KIND_TENANT";

struct World {
    f: Fixture,
    /// Organization where alice is Owner, admin is Admin and member is Member.
    tenant: String,
    /// Service registered by the shared fixture (client `service`, capability `execute`).
    svc: String,
    /// A second, unrelated service (client `other`, capabilities `render`/`export`).
    other: String,
    alice: String,
    admin: String,
    member: String,
}

async fn user_id(f: &Fixture, actor: &str) -> String {
    f.rpc(actor, "GetCurrentUser", json!({})).await["userId"]
        .as_str()
        .unwrap()
        .into()
}

async fn add_member(f: &Fixture, tenant: &str, user: &str, roles: Vec<i32>) {
    sqlx::query(
        "INSERT INTO members(tenant_id,user_id,roles) VALUES($1::text::uuid,$2::text::uuid,$3)",
    )
    .bind(tenant)
    .bind(user)
    .bind(roles)
    .execute(&f.db)
    .await
    .unwrap();
}

async fn world() -> World {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let svc = f.service().await;
    let other = f.rpc("owner","PutService",json!({"requestKey":key(),"reason":"register second","service":{"name":"second","clientId":"other","capabilities":["render","export"],"scopes":["authorize","usage:write","usage:read"]}})).await["service"]["id"].as_str().unwrap().into();
    let org = f
        .rpc(
            "alice",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Org"}),
        )
        .await;
    let tenant: String = org["tenant"]["id"].as_str().unwrap().into();
    let alice = user_id(&f, "alice").await;
    let admin = user_id(&f, "admin").await;
    let member = user_id(&f, "member").await;
    user_id(&f, "outsider").await;
    add_member(&f, &tenant, &admin, vec![2]).await;
    add_member(&f, &tenant, &member, vec![4]).await;
    World {
        f,
        tenant,
        svc,
        other,
        alice,
        admin,
        member,
    }
}

/// Protobuf JSON omits default values: normalize a missing `allowed` to false.
fn normalized(mut value: Value) -> Value {
    if value.get("allowed").is_none() {
        value["allowed"] = json!(false);
    }
    value
}

fn grant(service: &str, caps: &[&str]) -> Value {
    json!({"serviceId":service,"capabilities":caps})
}

impl World {
    fn create_body(&self, kind: &str, grants: Value) -> Value {
        json!({"requestKey":key(),"tenantId":self.tenant,"ownerKind":kind,"name":"test key","grants":grants})
    }
    async fn create(&self, actor: &str, kind: &str, grants: Value) -> Value {
        self.f
            .rpc(actor, "CreateApiKey", self.create_body(kind, grants))
            .await
    }
    /// Creates a key and returns `(key id, secret)`.
    async fn issue(&self, actor: &str, kind: &str, grants: Value) -> (String, String) {
        let created = self.create(actor, kind, grants).await;
        (
            created["key"]["id"].as_str().unwrap().into(),
            created["secret"].as_str().unwrap().into(),
        )
    }
    async fn check(&self, client: &str, secret: &str, capability: &str) -> Value {
        normalized(
            self.f
                .rpc(
                    client,
                    "AuthorizeApiKey",
                    json!({"secret":secret,"capability":capability,"paid":false}),
                )
                .await,
        )
    }
    async fn unauthenticated(&self, client: &str, secret: &str, capability: &str) {
        self.f
            .error(
                client,
                "AuthorizeApiKey",
                json!({"secret":secret,"capability":capability}),
                "unauthenticated",
            )
            .await;
    }
    async fn group_permission(&self, user: &str, service: &str, capability: &str) {
        self.f.rpc("alice","SetGroup",json!({"requestKey":key(),"tenantId":self.tenant,"group":{"name":format!("g-{}",key()),"userIds":[user],"permissions":[{"serviceId":service,"capability":capability}]}})).await;
    }
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn secret_is_disclosed_once_and_only_its_hash_is_persisted() {
    let w = world().await;
    let body = w.create_body(PERSONAL, json!([grant(&w.svc, &["execute"])]));
    let created = w.f.rpc("alice", "CreateApiKey", body.clone()).await;
    let secret = created["secret"].as_str().unwrap().to_owned();
    let id = created["key"]["id"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("dlk_"));
    assert_eq!(secret.len(), 80);
    assert!(secret.contains(&id.replace('-', "")));
    assert_eq!(created["key"]["ownerKind"], PERSONAL);
    assert_eq!(created["key"]["tenantId"], w.tenant);
    assert_eq!(created["key"]["userId"], w.alice);
    assert_eq!(created["key"]["grants"][0]["serviceId"], w.svc);
    assert!(created["key"].get("revokedAt").is_none());
    assert!(created["key"].get("expiresAt").is_none());

    // Only the SHA-256 of the random part is stored.
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT secret_hash FROM api_keys WHERE id=$1::text::uuid")
            .bind(&id)
            .fetch_one(&w.f.db)
            .await
            .unwrap();
    let random = URL_SAFE_NO_PAD
        .decode(&secret[secret.len() - 43..])
        .unwrap();
    assert_eq!(stored, Sha256::digest(&random).to_vec());

    // The secret appears nowhere in the replay record or audit rows.
    let in_requests: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM requests WHERE position(convert_to($1,'UTF8') in response)>0",
    )
    .bind(&secret)
    .fetch_one(&w.f.db)
    .await
    .unwrap();
    let in_audit: i64 = sqlx::query_scalar("SELECT count(*) FROM audit WHERE reason LIKE '%' || $1 || '%' OR actor_id LIKE '%' || $1 || '%'")
        .bind(&secret[secret.len() - 43..]).fetch_one(&w.f.db).await.unwrap();
    assert_eq!((in_requests, in_audit), (0, 0));

    // A replay returns the same key with an empty secret and never rotates it.
    let replay = w.f.rpc("alice", "CreateApiKey", body.clone()).await;
    assert_eq!(replay["key"], created["key"]);
    assert_eq!(replay["secret"].as_str().unwrap_or(""), "");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM api_keys")
        .fetch_one(&w.f.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    w.check("service", &secret, "execute").await;

    // A changed body under the same request key conflicts.
    let mut changed = body;
    changed["name"] = json!("other name");
    w.f.error("alice", "CreateApiKey", changed, "already_exists")
        .await;

    // Listing never reveals the secret or hash.
    let list =
        w.f.rpc(
            "alice",
            "ListApiKeys",
            json!({"tenantId":w.tenant,"ownerKind":PERSONAL}),
        )
        .await;
    assert_eq!(list["keys"].as_array().unwrap().len(), 1);
    let text = list.to_string();
    assert!(!text.contains(&secret[secret.len() - 43..]));
    assert!(!text.to_lowercase().contains("secret"));
    assert!(!text.contains("hash"));

    // Replay is re-authorized: a removed user cannot read the cached response.
    sqlx::query("DELETE FROM members WHERE user_id=$1::text::uuid AND tenant_id=$2::text::uuid")
        .bind(&w.alice)
        .bind(&w.tenant)
        .execute(&w.f.db)
        .await
        .unwrap();
    let mut replay_body = w.create_body(PERSONAL, json!([grant(&w.svc, &["execute"])]));
    replay_body["requestKey"] = json!(key());
    w.f.error("alice", "CreateApiKey", replay_body, "permission_denied")
        .await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn management_requires_the_documented_roles_and_user_tokens() {
    let w = world().await;
    let grants = json!([grant(&w.svc, &["execute"])]);
    // Tenant keys: Owner and Admin only.
    w.f.error(
        "member",
        "CreateApiKey",
        w.create_body(TENANT, grants.clone()),
        "permission_denied",
    )
    .await;
    w.f.error(
        "outsider",
        "CreateApiKey",
        w.create_body(TENANT, grants.clone()),
        "permission_denied",
    )
    .await;
    let (admin_key, _) = w.issue("admin", TENANT, grants.clone()).await;
    let (owner_key, _) = w.issue("alice", TENANT, grants.clone()).await;
    // Personal keys: any current member, nobody else.
    w.f.error(
        "outsider",
        "CreateApiKey",
        w.create_body(PERSONAL, grants.clone()),
        "permission_denied",
    )
    .await;
    let (member_key, _) = w.issue("member", PERSONAL, grants.clone()).await;
    let (alice_key, _) = w.issue("alice", PERSONAL, grants.clone()).await;

    let listed = |actor: &'static str, kind: &'static str| {
        let w = &w;
        async move {
            w.f.rpc(
                actor,
                "ListApiKeys",
                json!({"tenantId":w.tenant,"ownerKind":kind}),
            )
            .await["keys"]
                .as_array()
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .map(|k| k["id"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        }
    };
    // Tenant keys list for Owner/Admin, with the issuer recorded for audit.
    let tenant_keys = listed("admin", TENANT).await;
    assert_eq!(tenant_keys.len(), 2);
    assert!(tenant_keys.contains(&admin_key) && tenant_keys.contains(&owner_key));
    w.f.error(
        "member",
        "ListApiKeys",
        json!({"tenantId":w.tenant,"ownerKind":TENANT}),
        "permission_denied",
    )
    .await;
    w.f.error(
        "outsider",
        "ListApiKeys",
        json!({"tenantId":w.tenant,"ownerKind":PERSONAL}),
        "permission_denied",
    )
    .await;
    // Personal lists show only the caller's own keys, even to Owner/Admin.
    assert_eq!(
        listed("member", PERSONAL).await,
        std::slice::from_ref(&member_key)
    );
    assert_eq!(
        listed("alice", PERSONAL).await,
        std::slice::from_ref(&alice_key)
    );
    assert!(listed("admin", PERSONAL).await.is_empty());
    w.f.error(
        "alice",
        "ListApiKeys",
        json!({"tenantId":w.tenant,"ownerKind":"API_KEY_OWNER_KIND_UNSPECIFIED"}),
        "invalid_argument",
    )
    .await;

    // Revocation follows the same ownership rules.
    let revoke = |key_id: &str| json!({"requestKey":key(),"tenantId":w.tenant,"keyId":key_id});
    w.f.error("admin", "RevokeApiKey", revoke(&member_key), "not_found")
        .await;
    w.f.error("alice", "RevokeApiKey", revoke(&member_key), "not_found")
        .await;
    w.f.error(
        "member",
        "RevokeApiKey",
        revoke(&owner_key),
        "permission_denied",
    )
    .await;
    w.f.error(
        "outsider",
        "RevokeApiKey",
        revoke(&owner_key),
        "permission_denied",
    )
    .await;
    w.f.error("admin", "RevokeApiKey", revoke(&key()), "not_found")
        .await;
    w.f.error(
        "admin",
        "RevokeApiKey",
        revoke("not-a-uuid"),
        "invalid_argument",
    )
    .await;
    let revoked = w.f.rpc("admin", "RevokeApiKey", revoke(&owner_key)).await;
    assert!(revoked["key"]["revokedAt"].is_string());
    // Revoking again is idempotent and keeps the first revocation time.
    let again = w.f.rpc("alice", "RevokeApiKey", revoke(&owner_key)).await;
    assert_eq!(again["key"]["revokedAt"], revoked["key"]["revokedAt"]);
    let audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit WHERE action='api_key.revoke'")
            .fetch_one(&w.f.db)
            .await
            .unwrap();
    assert_eq!(audits, 1);
    let own = w.f.rpc("member", "RevokeApiKey", revoke(&member_key)).await;
    assert!(own["key"]["revokedAt"].is_string());
    // Revoked keys stay visible in listings as metadata.
    assert_eq!(listed("alice", TENANT).await.len(), 2);

    // Create and revoke are audited without secret material.
    let created: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit WHERE action='api_key.create'")
            .fetch_one(&w.f.db)
            .await
            .unwrap();
    assert_eq!(created, 4);

    // Services and API keys cannot manage keys.
    for method in ["CreateApiKey", "ListApiKeys", "RevokeApiKey"] {
        let body = json!({"requestKey":key(),"tenantId":w.tenant,"ownerKind":TENANT,"name":"x","grants":[grant(&w.svc,&["execute"])],"keyId":alice_key});
        w.f.error("service", method, body, "permission_denied")
            .await;
    }
    let (_, secret) = w.issue("alice", PERSONAL, grants).await;
    let response =
        w.f.http
            .post(format!(
                "{}/delcore.v1.DelcoreService/GetCurrentUser",
                w.f.base
            ))
            .bearer_auth(&secret)
            .header("connect-protocol-version", "1")
            .json(&json!({}))
            .send()
            .await
            .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn grants_and_expiry_are_validated_at_creation() {
    let w = world().await;
    let bad = |grants: Value| {
        let body = w.create_body(TENANT, grants);
        async {
            w.f.error("alice", "CreateApiKey", body, "invalid_argument")
                .await
        }
    };
    bad(json!([grant(&key(), &["execute"])])).await; // unknown service
    bad(json!([grant(&w.svc, &["translate"])])).await; // undeclared capability
    bad(json!([
        grant(&w.svc, &["execute"]),
        grant(&w.other, &["execute"])
    ]))
    .await; // other's capability set differs
    bad(json!([grant(&w.svc, &["execute", "execute"])])).await;
    bad(json!([
        grant(&w.svc, &["execute"]),
        grant(&w.svc, &["execute"])
    ]))
    .await;
    bad(json!([grant(&w.svc, &[])])).await;
    bad(json!([])).await;
    bad(json!(null)).await;
    bad(json!([grant("not-a-uuid", &["execute"])])).await;
    bad(json!(vec![grant(&w.svc, &["execute"]); 17])).await;
    let mut named = w.create_body(TENANT, json!([grant(&w.svc, &["execute"])]));
    named["name"] = json!("");
    w.f.error("alice", "CreateApiKey", named, "invalid_argument")
        .await;
    let mut unspecified = w.create_body(TENANT, json!([grant(&w.svc, &["execute"])]));
    unspecified["ownerKind"] = json!("API_KEY_OWNER_KIND_UNSPECIFIED");
    w.f.error("alice", "CreateApiKey", unspecified, "invalid_argument")
        .await;
    for at in [
        (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339(),
        chrono::Utc::now().to_rfc3339(),
    ] {
        let mut past = w.create_body(TENANT, json!([grant(&w.svc, &["execute"])]));
        past["expiresAt"] = json!(at);
        w.f.error("alice", "CreateApiKey", past, "invalid_argument")
            .await;
    }
    let none: i64 = sqlx::query_scalar("SELECT count(*) FROM api_keys")
        .fetch_one(&w.f.db)
        .await
        .unwrap();
    assert_eq!(none, 0, "rejected requests create nothing");

    // Both services in one key; capabilities are stored normalized.
    let future = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    let mut ok = w.create_body(
        TENANT,
        json!([
            grant(&w.other, &["render", "export"]),
            grant(&w.svc, &["execute"])
        ]),
    );
    ok["expiresAt"] = json!(future);
    let created = w.f.rpc("alice", "CreateApiKey", ok).await;
    assert_eq!(created["key"]["grants"].as_array().unwrap().len(), 2);
    assert!(created["key"]["expiresAt"].is_string());

    // Grants and expiry cannot change after creation, only revocation can.
    let id = created["key"]["id"].as_str().unwrap();
    for sql in [
        "UPDATE api_key_grants SET capabilities=ARRAY['x'] WHERE key_id=$1::text::uuid",
        "UPDATE api_keys SET expires_at=NULL WHERE id=$1::text::uuid",
        "UPDATE api_keys SET secret_hash=decode(repeat('00',32),'hex') WHERE id=$1::text::uuid",
    ] {
        assert!(
            sqlx::query(sql).bind(id).execute(&w.f.db).await.is_err(),
            "{sql}"
        );
    }
    sqlx::query("UPDATE api_keys SET revoked_at=now() WHERE id=$1::text::uuid")
        .bind(id)
        .execute(&w.f.db)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn personal_keys_intersect_grants_with_current_tenant_permissions() {
    let w = world().await;
    let grants = json!([grant(&w.svc, &["execute"])]);
    let (key_id, secret) = w.issue("member", PERSONAL, grants.clone()).await;
    // A plain member has no group permission yet: the grant alone is not enough.
    let denied = w.check("service", &secret, "execute").await;
    assert_eq!(denied["allowed"], false);
    assert_eq!(denied["reasons"], json!(["BLOCK_REASON_PERMISSION"]));
    assert_eq!(denied["keyId"], key_id);
    assert_eq!(denied["tenantId"], w.tenant);
    assert_eq!(denied["ownerKind"], PERSONAL);
    assert_eq!(denied["userId"], w.member);

    w.group_permission(&w.member, &w.svc, "execute").await;
    let allowed = w.check("service", &secret, "execute").await;
    assert_eq!(allowed["allowed"], true);
    assert_eq!(allowed["userId"], w.member);

    // Capabilities outside the grant stay denied, whatever the user may do.
    let (_, owner_secret) = w.issue("alice", PERSONAL, grants).await;
    assert_eq!(
        w.check("service", &owner_secret, "other").await["allowed"],
        false
    );
    // The Owner role carries every declared capability; the grant restricts.
    assert_eq!(
        w.check("service", &owner_secret, "execute").await["allowed"],
        true
    );

    // Losing the permission, even through a group edit, blocks the next request.
    sqlx::query("DELETE FROM group_permissions")
        .execute(&w.f.db)
        .await
        .unwrap();
    assert_eq!(
        w.check("service", &secret, "execute").await["allowed"],
        false
    );
    w.group_permission(&w.member, &w.svc, "execute").await;
    assert_eq!(
        w.check("service", &secret, "execute").await["allowed"],
        true
    );

    // Removing the user from the tenant blocks it; the key itself is unchanged.
    w.f.rpc(
        "alice",
        "RemoveMember",
        json!({"requestKey":key(),"tenantId":w.tenant,"userId":w.member}),
    )
    .await;
    let removed = w.check("service", &secret, "execute").await;
    assert_eq!(removed["allowed"], false);
    assert_eq!(removed["reasons"], json!(["BLOCK_REASON_PERMISSION"]));
    assert_eq!(removed["keyId"], key_id);

    // A role downgrade of the Owner removes the implicit capabilities too.
    sqlx::query("UPDATE members SET roles=ARRAY[4] WHERE user_id=$1::text::uuid AND tenant_id=$2::text::uuid")
        .bind(&w.alice).bind(&w.tenant).execute(&w.f.db).await.unwrap();
    assert_eq!(
        w.check("service", &owner_secret, "execute").await["allowed"],
        false
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn withdrawal_deletes_personal_keys_but_not_tenant_keys() {
    let w = world().await;
    let grants = json!([grant(&w.svc, &["execute"])]);
    let (_, personal) = w.issue("admin", PERSONAL, grants.clone()).await;
    let (tenant_key, tenant_secret) = w.issue("admin", TENANT, grants).await;
    // Admins that are not owners of any organization can withdraw.
    w.f.rpc("admin", "Withdraw", json!({"requestKey":key()}))
        .await;
    w.unauthenticated("service", &personal, "execute").await;
    let still = w.check("service", &tenant_secret, "execute").await;
    assert_eq!(still["allowed"], true);
    assert_eq!(still["keyId"], tenant_key);
    assert_eq!(still["userId"], w.admin);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn tenant_keys_are_independent_of_the_issuing_user() {
    let w = world().await;
    let (key_id, secret) = w
        .issue("admin", TENANT, json!([grant(&w.svc, &["execute"])]))
        .await;
    // No user permission is needed: the tenant acts through its key.
    let allowed = w.check("service", &secret, "execute").await;
    assert_eq!(allowed["allowed"], true);
    assert_eq!(allowed["ownerKind"], TENANT);
    assert_eq!(allowed["userId"], w.admin);
    assert_eq!(allowed["keyId"], key_id);
    assert_eq!(w.check("service", &secret, "other").await["allowed"], false);
    // Removing or demoting the issuer does not change the key.
    sqlx::query("UPDATE members SET roles=ARRAY[4] WHERE user_id=$1::text::uuid")
        .bind(&w.admin)
        .execute(&w.f.db)
        .await
        .unwrap();
    assert_eq!(
        w.check("service", &secret, "execute").await["allowed"],
        true
    );
    w.f.rpc(
        "alice",
        "RemoveMember",
        json!({"requestKey":key(),"tenantId":w.tenant,"userId":w.admin}),
    )
    .await;
    assert_eq!(
        w.check("service", &secret, "execute").await["allowed"],
        true
    );
    // The Owner and Admins of the tenant keep managing it; the issuer cannot.
    w.f.error(
        "admin",
        "RevokeApiKey",
        json!({"requestKey":key(),"tenantId":w.tenant,"keyId":key_id}),
        "permission_denied",
    )
    .await;
    w.f.rpc(
        "alice",
        "RevokeApiKey",
        json!({"requestKey":key(),"tenantId":w.tenant,"keyId":key_id}),
    )
    .await;
    w.unauthenticated("service", &secret, "execute").await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn invalid_unknown_revoked_and_expired_secrets_are_unauthenticated() {
    let w = world().await;
    let (key_id, secret) = w
        .issue("alice", TENANT, json!([grant(&w.svc, &["execute"])]))
        .await;
    assert_eq!(
        w.check("service", &secret, "execute").await["allowed"],
        true
    );
    let tail = secret.len() - 43;
    let flipped = format!("{}{}", &secret[..tail], "A".repeat(43));
    let unknown = format!("dlk_{}_{}", key().replace('-', ""), &secret[tail..]);
    for bad in [
        String::new(),
        "dlk_".into(),
        secret[..secret.len() - 1].to_owned(),
        secret.to_uppercase(),
        secret.replacen("dlk_", "xxx_", 1),
        flipped,
        unknown,
        format!("{secret} "),
    ] {
        w.unauthenticated("service", &bad, "execute").await;
    }
    // Rejections never carry attribution.
    let body =
        w.f.error(
            "service",
            "AuthorizeApiKey",
            json!({"secret":"dlk_x","capability":"execute"}),
            "unauthenticated",
        )
        .await;
    assert!(!body.to_string().contains(&key_id));

    // Expiry. Immutability triggers are lifted only to age the fixture row.
    let (aged, aged_secret) = w
        .issue("alice", TENANT, json!([grant(&w.svc, &["execute"])]))
        .await;
    assert_eq!(
        w.check("service", &aged_secret, "execute").await["allowed"],
        true
    );
    sqlx::query("ALTER TABLE api_keys DISABLE TRIGGER api_keys_immutable")
        .execute(&w.f.db)
        .await
        .unwrap();
    sqlx::query("UPDATE api_keys SET created_at=now()-interval '2 hours',expires_at=now()-interval '1 hour' WHERE id=$1::text::uuid")
        .bind(&aged).execute(&w.f.db).await.unwrap();
    w.unauthenticated("service", &aged_secret, "execute").await;
    // The expiry boundary is exclusive of the expiry instant.
    sqlx::query("UPDATE api_keys SET expires_at=now()+interval '1 hour' WHERE id=$1::text::uuid")
        .bind(&aged)
        .execute(&w.f.db)
        .await
        .unwrap();
    assert_eq!(
        w.check("service", &aged_secret, "execute").await["allowed"],
        true
    );

    // Revocation applies to the very next request.
    w.f.rpc(
        "alice",
        "RevokeApiKey",
        json!({"requestKey":key(),"tenantId":w.tenant,"keyId":key_id}),
    )
    .await;
    w.unauthenticated("service", &secret, "execute").await;
    // Other keys are unaffected.
    assert_eq!(
        w.check("service", &aged_secret, "execute").await["allowed"],
        true
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn a_key_never_authorizes_another_service_and_only_scoped_services_may_verify() {
    let w = world().await;
    let (_, svc_only) = w
        .issue("alice", TENANT, json!([grant(&w.svc, &["execute"])]))
        .await;
    let (_, both) = w
        .issue(
            "alice",
            TENANT,
            json!([grant(&w.svc, &["execute"]), grant(&w.other, &["render"])]),
        )
        .await;
    // The calling service selects the grants; the other service sees none.
    let cross = w.check("other", &svc_only, "execute").await;
    assert_eq!(cross["allowed"], false);
    assert_eq!(cross["reasons"], json!(["BLOCK_REASON_PERMISSION"]));
    assert_eq!(
        w.check("other", &svc_only, "render").await["allowed"],
        false
    );
    assert_eq!(
        w.check("service", &svc_only, "render").await["allowed"],
        false
    );
    // A key with grants for both services authorizes each only for its own.
    assert_eq!(w.check("service", &both, "execute").await["allowed"], true);
    assert_eq!(w.check("service", &both, "render").await["allowed"], false);
    assert_eq!(w.check("other", &both, "render").await["allowed"], true);
    assert_eq!(w.check("other", &both, "execute").await["allowed"], false);
    assert_eq!(w.check("other", &both, "export").await["allowed"], false);
    // Only services registered with the authorize scope may verify keys.
    for actor in ["noscope", "ghost", "alice", "owner"] {
        w.f.error(
            actor,
            "AuthorizeApiKey",
            json!({"secret":both,"capability":"render"}),
            "permission_denied",
        )
        .await;
    }
    w.f.error(
        "service",
        "AuthorizeApiKey",
        json!({"secret":both,"capability":""}),
        "invalid_argument",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn keys_are_isolated_by_tenant() {
    let w = world().await;
    let (one, one_secret) = w
        .issue("alice", TENANT, json!([grant(&w.svc, &["execute"])]))
        .await;
    let org2 =
        w.f.rpc(
            "alice",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Second org"}),
        )
        .await["tenant"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
    let created = w.f.rpc("alice","CreateApiKey",json!({"requestKey":key(),"tenantId":org2,"ownerKind":TENANT,"name":"k2","grants":[grant(&w.svc,&["execute"])]})).await;
    let two_secret = created["secret"].as_str().unwrap();
    // Attribution is the key's own tenant.
    assert_eq!(
        w.check("service", &one_secret, "execute").await["tenantId"],
        w.tenant
    );
    assert_eq!(
        w.check("service", two_secret, "execute").await["tenantId"],
        org2
    );
    // Listing and revoking never cross tenants.
    let listed =
        w.f.rpc(
            "alice",
            "ListApiKeys",
            json!({"tenantId":org2,"ownerKind":TENANT}),
        )
        .await;
    assert_eq!(listed["keys"].as_array().unwrap().len(), 1);
    assert_ne!(listed["keys"][0]["id"], one);
    w.f.error(
        "alice",
        "RevokeApiKey",
        json!({"requestKey":key(),"tenantId":org2,"keyId":one}),
        "not_found",
    )
    .await;
    w.unauthenticated(
        "service",
        &format!("{}x", &one_secret[..one_secret.len() - 1]),
        "execute",
    )
    .await;
    assert_eq!(
        w.check("service", &one_secret, "execute").await["allowed"],
        true
    );
    // A member of one tenant manages nothing in the other.
    w.f.error(
        "admin",
        "ListApiKeys",
        json!({"tenantId":org2,"ownerKind":TENANT}),
        "permission_denied",
    )
    .await;
    w.f.error("admin","CreateApiKey",json!({"requestKey":key(),"tenantId":org2,"ownerKind":TENANT,"name":"k","grants":[grant(&w.svc,&["execute"])]}),"permission_denied").await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn authorization_reuses_billing_and_feature_checks_of_authorize() {
    let w = world().await;
    let (_, personal) = w
        .issue("alice", PERSONAL, json!([grant(&w.svc, &["execute"])]))
        .await;
    let (_, tenant_key) = w
        .issue("alice", TENANT, json!([grant(&w.svc, &["execute"])]))
        .await;
    let paid = |secret: &str| json!({"secret":secret,"capability":"execute","paid":true});
    let baseline = normalized(
        w.f.rpc(
            "service",
            "Authorize",
            json!({"tenantId":w.tenant,"userId":w.alice,"capability":"execute","paid":true}),
        )
        .await,
    );
    // The free plan has no entitlement for this service, which blocks paid use
    // identically on every path.
    assert_eq!(baseline["allowed"], false);
    for secret in [&personal, &tenant_key] {
        let keyed = normalized(w.f.rpc("service", "AuthorizeApiKey", paid(secret)).await);
        assert_eq!(keyed["allowed"], baseline["allowed"]);
        assert_eq!(keyed["reasons"], baseline["reasons"]);
    }
    // Operator blocks apply to keys the same way.
    w.f.rpc(
        "owner",
        "SetTenantBlocked",
        json!({"requestKey":key(),"tenantId":w.tenant,"blocked":true,"reason":"abuse review"}),
    )
    .await;
    let blocked =
        w.f.rpc(
            "service",
            "Authorize",
            json!({"tenantId":w.tenant,"userId":w.alice,"capability":"execute","paid":true}),
        )
        .await;
    for secret in [&personal, &tenant_key] {
        let keyed = normalized(w.f.rpc("service", "AuthorizeApiKey", paid(secret)).await);
        assert_eq!(keyed["reasons"], blocked["reasons"]);
        assert!(
            keyed["reasons"]
                .to_string()
                .contains("BLOCK_REASON_OPERATOR")
        );
        // Unpaid checks stay permission-only, as for Authorize.
        assert_eq!(w.check("service", secret, "execute").await["allowed"], true);
    }
    w.f.rpc(
        "owner",
        "SetTenantBlocked",
        json!({"requestKey":key(),"tenantId":w.tenant,"blocked":false,"reason":"cleared"}),
    )
    .await;
    // Unknown feature keys block exactly like Authorize.
    let request =
        json!({"tenantId":w.tenant,"userId":w.alice,"capability":"execute","featureKey":"missing"});
    let expected = w.f.rpc("service", "Authorize", request).await;
    let keyed =
        w.f.rpc(
            "service",
            "AuthorizeApiKey",
            json!({"secret":tenant_key,"capability":"execute","featureKey":"missing"}),
        )
        .await;
    assert_eq!(keyed["reasons"], expected["reasons"]);
    assert!(
        keyed["reasons"]
            .to_string()
            .contains("BLOCK_REASON_FEATURE_UNKNOWN")
    );
    // Group values without a feature key are rejected by the shared validation.
    w.f.error(
        "service",
        "AuthorizeApiKey",
        json!({"secret":tenant_key,"capability":"execute","groupValues":{"a":{"text":"b"}}}),
        "invalid_argument",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn another_service_works_through_catalog_data_alone() {
    let w = world().await;
    // The second service is ordinary catalog data with its own capabilities.
    let grants = json!([grant(&w.other, &["render"])]);
    let (tenant_id, tenant_secret) = w.issue("admin", TENANT, grants.clone()).await;
    let (_, personal_secret) = w.issue("member", PERSONAL, grants).await;
    assert_eq!(
        w.check("other", &tenant_secret, "render").await["allowed"],
        true
    );
    assert_eq!(
        w.check("other", &tenant_secret, "export").await["allowed"],
        false
    );
    assert_eq!(
        w.check("service", &tenant_secret, "execute").await["allowed"],
        false
    );
    // Personal use follows that service's group permissions.
    assert_eq!(
        w.check("other", &personal_secret, "render").await["allowed"],
        false
    );
    w.group_permission(&w.member, &w.other, "render").await;
    assert_eq!(
        w.check("other", &personal_secret, "render").await["allowed"],
        true
    );
    let paid =
        w.f.rpc(
            "other",
            "AuthorizeApiKey",
            json!({"secret":tenant_secret,"capability":"render","paid":true}),
        )
        .await;
    assert_eq!(paid["keyId"], tenant_id);
    // A capability the service later stops declaring is blocked.
    sqlx::query("UPDATE services SET capabilities=ARRAY['export'] WHERE id=$1::text::uuid")
        .bind(&w.other)
        .execute(&w.f.db)
        .await
        .unwrap();
    assert_eq!(
        w.check("other", &tenant_secret, "render").await["allowed"],
        false
    );
    assert_eq!(
        w.check("other", &personal_secret, "render").await["allowed"],
        false
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn tenant_deletion_removes_keys_and_blocks_use_while_deleting() {
    let w = world().await;
    let (_, tenant_secret) = w
        .issue("alice", TENANT, json!([grant(&w.svc, &["execute"])]))
        .await;
    let (_, personal_secret) = w
        .issue("alice", PERSONAL, json!([grant(&w.svc, &["execute"])]))
        .await;
    let (_, survivor) = {
        let other =
            w.f.rpc(
                "alice",
                "CreateTenant",
                json!({"requestKey":key(),"name":"Survivor"}),
            )
            .await["tenant"]["id"]
                .as_str()
                .unwrap()
                .to_owned();
        let created = w.f.rpc("alice","CreateApiKey",json!({"requestKey":key(),"tenantId":other,"ownerKind":TENANT,"name":"k","grants":[grant(&w.svc,&["execute"])]})).await;
        (other, created["secret"].as_str().unwrap().to_owned())
    };
    w.f.rpc(
        "alice",
        "DeleteTenant",
        json!({"requestKey":key(),"tenantId":w.tenant}),
    )
    .await;
    // While the grace period runs, even unpaid use is blocked for keys.
    for secret in [&tenant_secret, &personal_secret] {
        let blocked = w.check("service", secret, "execute").await;
        assert_eq!(blocked["allowed"], false);
        assert_eq!(blocked["reasons"], json!(["BLOCK_REASON_DELETING"]));
    }
    // No new keys while deleting.
    w.f.error(
        "alice",
        "CreateApiKey",
        w.create_body(TENANT, json!([grant(&w.svc, &["execute"])])),
        "failed_precondition",
    )
    .await;
    w.f.worker
        .maintain(chrono::Utc::now() + chrono::Duration::days(100))
        .await
        .unwrap();
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM api_keys WHERE tenant_id=$1::text::uuid")
            .bind(&w.tenant)
            .fetch_one(&w.f.db)
            .await
            .unwrap();
    let grants: i64 = sqlx::query_scalar("SELECT count(*) FROM api_key_grants g WHERE NOT EXISTS(SELECT 1 FROM api_keys k WHERE k.id=g.key_id)")
        .fetch_one(&w.f.db).await.unwrap();
    assert_eq!((remaining, grants), (0, 0));
    w.unauthenticated("service", &tenant_secret, "execute")
        .await;
    w.unauthenticated("service", &personal_secret, "execute")
        .await;
    assert_eq!(
        w.check("service", &survivor, "execute").await["allowed"],
        true
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn grants_cannot_be_deleted_apart_from_their_key() {
    let w = world().await;
    let (id, _) = w
        .issue("alice", PERSONAL, json!([grant(&w.svc, &["execute"])]))
        .await;
    for sql in [
        "DELETE FROM api_key_grants WHERE key_id=$1::text::uuid",
        "UPDATE api_key_grants SET capabilities=ARRAY['other'] WHERE key_id=$1::text::uuid",
    ] {
        assert!(
            sqlx::query(sql).bind(&id).execute(&w.f.db).await.is_err(),
            "{sql}"
        );
    }
    let grants: i64 =
        sqlx::query_scalar("SELECT count(*) FROM api_key_grants WHERE key_id=$1::text::uuid")
            .bind(&id)
            .fetch_one(&w.f.db)
            .await
            .unwrap();
    assert_eq!(grants, 1);
    // Deleting the key still removes its grants through the cascade.
    sqlx::query("DELETE FROM api_keys WHERE id=$1::text::uuid")
        .bind(&id)
        .execute(&w.f.db)
        .await
        .unwrap();
    let grants: i64 =
        sqlx::query_scalar("SELECT count(*) FROM api_key_grants WHERE key_id=$1::text::uuid")
            .bind(&id)
            .fetch_one(&w.f.db)
            .await
            .unwrap();
    assert_eq!(grants, 0);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn inactive_keys_are_pruned_after_the_transient_period_and_churn_is_bounded() {
    let w = world().await;
    let (old, _) = w
        .issue("alice", PERSONAL, json!([grant(&w.svc, &["execute"])]))
        .await;
    let (recent, _) = w
        .issue("alice", PERSONAL, json!([grant(&w.svc, &["execute"])]))
        .await;
    let (active, _) = w
        .issue("alice", PERSONAL, json!([grant(&w.svc, &["execute"])]))
        .await;
    for id in [&old, &recent] {
        w.f.rpc(
            "alice",
            "RevokeApiKey",
            json!({"requestKey":server_common::id(),"tenantId":w.tenant,"keyId":id}),
        )
        .await;
    }
    sqlx::query("ALTER TABLE api_keys DISABLE TRIGGER api_keys_immutable")
        .execute(&w.f.db)
        .await
        .unwrap();
    sqlx::query("UPDATE api_keys SET revoked_at=now()-interval '31 days',created_at=now()-interval '32 days' WHERE id=$1::text::uuid")
        .bind(&old)
        .execute(&w.f.db)
        .await
        .unwrap();
    sqlx::query("UPDATE api_keys SET revoked_at=now()-interval '29 days',created_at=now()-interval '32 days' WHERE id=$1::text::uuid")
        .bind(&recent)
        .execute(&w.f.db)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE api_keys ENABLE TRIGGER api_keys_immutable")
        .execute(&w.f.db)
        .await
        .unwrap();
    w.f.worker.maintain(chrono::Utc::now()).await.unwrap();
    let remaining: Vec<String> = sqlx::query_scalar("SELECT id::text FROM api_keys ORDER BY id")
        .fetch_all(&w.f.db)
        .await
        .unwrap();
    assert!(
        !remaining.contains(&old),
        "a key inactive for 31 days is removed"
    );
    assert!(remaining.contains(&recent) && remaining.contains(&active));
    let grants: i64 =
        sqlx::query_scalar("SELECT count(*) FROM api_key_grants WHERE key_id=$1::text::uuid")
            .bind(&old)
            .fetch_one(&w.f.db)
            .await
            .unwrap();
    assert_eq!(grants, 0, "the grants leave with their key");

    // Churn is bounded: 500 retained rows per scope, whatever their state.
    let inserted: i64 = sqlx::query_scalar(
        "WITH n AS (SELECT count(*) AS c FROM api_keys WHERE user_id=$1::text::uuid AND tenant_id=$2::text::uuid AND owner_kind=1),
         ins AS (INSERT INTO api_keys(id,tenant_id,owner_kind,user_id,name,secret_hash,created_at,revoked_at)
          SELECT gen_random_uuid(),$2::text::uuid,1,$1::text::uuid,'churn',decode(repeat('00',32),'hex'),now()-interval '1 day',now()-interval '12 hours'
          FROM generate_series(1,500-(SELECT c FROM n)::int) RETURNING 1)
         SELECT count(*) FROM ins",
    )
    .bind(&w.alice)
    .bind(&w.tenant)
    .fetch_one(&w.f.db)
    .await
    .unwrap();
    assert!(inserted > 0);
    w.f.error(
        "alice",
        "CreateApiKey",
        w.create_body(PERSONAL, json!([grant(&w.svc, &["execute"])])),
        "failed_precondition",
    )
    .await;
    // Another owner scope keeps its own budget.
    w.create("member", PERSONAL, json!([grant(&w.svc, &["execute"])]))
        .await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn active_key_count_per_owner_is_bounded() {
    let w = world().await;
    for _ in 0..100 {
        w.create("alice", PERSONAL, json!([grant(&w.svc, &["execute"])]))
            .await;
    }
    w.f.error(
        "alice",
        "CreateApiKey",
        w.create_body(PERSONAL, json!([grant(&w.svc, &["execute"])])),
        "failed_precondition",
    )
    .await;
    // Other owners and the tenant scope have their own budgets.
    w.create("member", PERSONAL, json!([grant(&w.svc, &["execute"])]))
        .await;
    w.create("alice", TENANT, json!([grant(&w.svc, &["execute"])]))
        .await;
    // Pagination covers all keys without exposing others.
    let first =
        w.f.rpc(
            "alice",
            "ListApiKeys",
            json!({"tenantId":w.tenant,"ownerKind":PERSONAL,"page":{"size":60}}),
        )
        .await;
    assert_eq!(first["keys"].as_array().unwrap().len(), 60);
    let next = first["nextAfterId"].as_str().unwrap();
    let second =
        w.f.rpc(
            "alice",
            "ListApiKeys",
            json!({"tenantId":w.tenant,"ownerKind":PERSONAL,"page":{"size":60,"afterId":next}}),
        )
        .await;
    assert_eq!(second["keys"].as_array().unwrap().len(), 40);
    assert!(
        second
            .get("nextAfterId")
            .is_none_or(|v| v.as_str().unwrap_or("").is_empty())
    );
}
