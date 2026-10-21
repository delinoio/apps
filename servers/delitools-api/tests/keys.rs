// SPDX-License-Identifier: FSL-1.1-ALv2
//! API key lifecycle through the real Connect router with bearer authentication, a mock Logto and
//! a mock tenant authority; plus execution-time credential rules.
mod support;
use delino_protocol::delitools::v1::Capability;
use delitools_api::{
    authority::TenantRole,
    keys,
    platform::{CredentialKind, Platform},
    store,
};
use serde_json::{Value, json};
use support::{Fixture, id, reason};
use uuid::Uuid;

fn create(tenant: Uuid, kind: &str, caps: &[&str]) -> Value {
    json!({"tenantId": tenant.to_string(), "kind": kind, "capabilities": caps})
}
/// Connect JSON omits empty repeated fields.
fn count(body: &Value) -> usize {
    body["keys"].as_array().map_or(0, Vec::len)
}
const PERSONAL: &str = "KEY_KIND_PERSONAL";
const SERVICE: &str = "KEY_KIND_SERVICE";
const TRANSLATOR: &str = "CAPABILITY_TRANSLATOR";
const EXTRACTOR: &str = "CAPABILITY_EXTRACTOR";

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn key_lifecycle_one_time_secret_and_personal_versus_service_rules() {
    let f = Fixture::setup().await;
    let tenant = id();
    let (owner, owner_id) = f.user("owner", Some((tenant, TenantRole::Owner)));
    let (admin, _) = f.user("admin", Some((tenant, TenantRole::Admin)));
    let (member, member_id) = f.user("member", Some((tenant, TenantRole::Member)));
    let (member2, _) = f.user("member2", Some((tenant, TenantRole::Member)));
    let (billing, _) = f.user("billing", Some((tenant, TenantRole::Billing)));
    let (outsider, _) = f.user("outsider", None);

    // A member creates a personal key; the secret is returned once and only its hash is stored.
    let created = f
        .ok(
            &member,
            "CreateKey",
            create(tenant, PERSONAL, &[TRANSLATOR]),
        )
        .await;
    let secret = created.body["secret"].as_str().unwrap().to_owned();
    assert_eq!(secret.len(), keys::TOKEN_LEN);
    assert!(secret.starts_with("dtk_"));
    let personal = created.body["key"].clone();
    assert_eq!(personal["userId"], member_id.to_string());
    assert_eq!(personal["tenantId"], tenant.to_string());
    assert_eq!(personal["kind"], PERSONAL);
    assert_eq!(personal["capabilities"], json!([TRANSLATOR]));
    assert!(personal.get("revoked").is_none_or(|v| v == false));
    let (key_id, hash) = keys::parse(&secret).unwrap();
    assert_eq!(personal["id"], key_id.to_string());
    let stored: Vec<u8> = sqlx::query_scalar("SELECT secret_hash FROM api_keys WHERE id=$1")
        .bind(key_id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(stored, hash.to_vec());
    assert!(
        !stored
            .windows(8)
            .any(|w| secret.as_bytes().windows(8).any(|s| s == w))
    );
    let dump: String = sqlx::query_scalar("SELECT api_keys::text FROM api_keys WHERE id=$1")
        .bind(key_id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(!dump.contains(secret.rsplit('_').next().unwrap()));

    // Listing never discloses the secret again.
    let listed = f
        .ok(
            &member,
            "ListKeys",
            json!({"tenantId": tenant.to_string(), "kind": PERSONAL}),
        )
        .await;
    assert_eq!(count(&listed.body), 1);
    assert!(
        !listed
            .body
            .to_string()
            .contains(secret.rsplit('_').next().unwrap())
    );
    assert!(listed.body["keys"][0].get("secret").is_none());

    // Service keys need Owner or Admin and belong to the tenant; the issuer is attribution only.
    let expiry = (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339();
    let mut body = create(tenant, SERVICE, &[TRANSLATOR, EXTRACTOR]);
    body["expiresAt"] = json!(expiry);
    let service = f.ok(&owner, "CreateKey", body).await;
    let service_key = service.body["key"].clone();
    assert_eq!(service_key["userId"], owner_id.to_string());
    assert_eq!(service_key["kind"], SERVICE);
    assert!(service_key["expiresAt"].is_string());
    for denied in [&member, &billing, &outsider] {
        f.fails(
            Some(denied),
            "CreateKey",
            create(tenant, SERVICE, &[TRANSLATOR]),
            "permission_denied",
            reason::FORBIDDEN,
        )
        .await;
    }
    f.ok(&admin, "CreateKey", create(tenant, SERVICE, &[EXTRACTOR]))
        .await;
    f.fails(
        Some(&outsider),
        "CreateKey",
        create(tenant, PERSONAL, &[TRANSLATOR]),
        "permission_denied",
        reason::FORBIDDEN,
    )
    .await;

    // Listing follows what the caller may manage.
    let service_listing = |token: String, kind: &'static str| {
        let f = &f;
        async move {
            f.call(
                Some(&token),
                "ListKeys",
                json!({"tenantId": tenant.to_string(), "kind": kind}),
            )
            .await
        }
    };
    assert_eq!(
        service_listing(member.clone(), SERVICE).await.code(),
        "permission_denied"
    );
    assert_eq!(
        count(&service_listing(owner.clone(), SERVICE).await.body),
        2
    );
    let unfiltered_member = f
        .ok(&member, "ListKeys", json!({"tenantId": tenant.to_string()}))
        .await;
    assert_eq!(count(&unfiltered_member.body), 1);
    let unfiltered_owner = f
        .ok(&owner, "ListKeys", json!({"tenantId": tenant.to_string()}))
        .await;
    assert_eq!(
        count(&unfiltered_owner.body),
        2,
        "owner sees service keys and no other user's personal keys"
    );
    let other = f
        .ok(
            &member2,
            "ListKeys",
            json!({"tenantId": tenant.to_string(), "kind": PERSONAL}),
        )
        .await;
    assert_eq!(count(&other.body), 0);

    // Revocation: personal keys only by their own user; service keys by Owner/Admin.
    let revoke = |key: &Value| json!({"tenantId": tenant.to_string(), "keyId": key["id"]});
    for denied in [&member2, &owner, &admin, &outsider] {
        f.fails(
            Some(denied),
            "RevokeKey",
            revoke(&personal),
            "permission_denied",
            reason::FORBIDDEN,
        )
        .await;
    }
    f.fails(
        Some(&member),
        "RevokeKey",
        revoke(&service_key),
        "permission_denied",
        reason::FORBIDDEN,
    )
    .await;
    f.ok(&member, "RevokeKey", revoke(&personal)).await;
    let first_revoked: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT revoked_at FROM api_keys WHERE id=$1")
            .bind(key_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    f.ok(&member, "RevokeKey", revoke(&personal)).await;
    let again: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT revoked_at FROM api_keys WHERE id=$1")
            .bind(key_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(
        first_revoked, again,
        "revoking twice keeps the first revocation time"
    );
    let after = f
        .ok(
            &member,
            "ListKeys",
            json!({"tenantId": tenant.to_string(), "kind": PERSONAL}),
        )
        .await;
    assert_eq!(after.body["keys"][0]["revoked"], true);
    f.ok(&admin, "RevokeKey", revoke(&service_key)).await;

    // Personal-key management follows current membership.
    f.authority.remove_member(tenant, member_id);
    f.fails(
        Some(&member),
        "CreateKey",
        create(tenant, PERSONAL, &[TRANSLATOR]),
        "permission_denied",
        reason::FORBIDDEN,
    )
    .await;
    f.fails(
        Some(&member),
        "RevokeKey",
        revoke(&personal),
        "permission_denied",
        reason::FORBIDDEN,
    )
    .await;
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn tenants_are_isolated_and_inputs_validated() {
    let f = Fixture::setup().await;
    let (a, b) = (id(), id());
    let (owner_a, _) = f.user("owner-a", Some((a, TenantRole::Owner)));
    let (owner_b, _) = f.user("owner-b", Some((b, TenantRole::Owner)));
    let key = f
        .ok(&owner_a, "CreateKey", create(a, SERVICE, &[TRANSLATOR]))
        .await;
    let key_id = key.body["key"]["id"].clone();

    // Another tenant's owner can neither list, revoke nor see the key.
    f.fails(
        Some(&owner_b),
        "ListKeys",
        json!({"tenantId": a.to_string(), "kind": SERVICE}),
        "permission_denied",
        reason::FORBIDDEN,
    )
    .await;
    f.fails(
        Some(&owner_b),
        "RevokeKey",
        json!({"tenantId": a.to_string(), "keyId": key_id}),
        "permission_denied",
        reason::FORBIDDEN,
    )
    .await;
    // The key ID is not found inside the caller's own tenant either, and nothing changes.
    f.fails(
        Some(&owner_b),
        "RevokeKey",
        json!({"tenantId": b.to_string(), "keyId": key_id}),
        "permission_denied",
        reason::FORBIDDEN,
    )
    .await;
    f.fails(
        Some(&owner_a),
        "RevokeKey",
        json!({"tenantId": a.to_string(), "keyId": id().to_string()}),
        "permission_denied",
        reason::FORBIDDEN,
    )
    .await;
    let revoked: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT revoked_at FROM api_keys")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(revoked.is_none());
    let b_listing = f
        .ok(
            &owner_b,
            "ListKeys",
            json!({"tenantId": b.to_string(), "kind": SERVICE}),
        )
        .await;
    assert_eq!(count(&b_listing.body), 0);

    // Input validation.
    let past = (chrono::Utc::now() - chrono::Duration::seconds(5)).to_rfc3339();
    let mut expired = create(a, SERVICE, &[TRANSLATOR]);
    expired["expiresAt"] = json!(past);
    let mut duplicate = create(a, SERVICE, &[TRANSLATOR, TRANSLATOR]);
    duplicate["expiresAt"] = Value::Null;
    for body in [
        create(a, "KEY_KIND_UNSPECIFIED", &[TRANSLATOR]),
        create(a, SERVICE, &[]),
        create(a, SERVICE, &["CAPABILITY_UNSPECIFIED"]),
        duplicate,
        expired,
        json!({"tenantId": "not-a-uuid", "kind": SERVICE, "capabilities": [TRANSLATOR]}),
        json!({"tenantId": "", "kind": SERVICE, "capabilities": [TRANSLATOR]}),
    ] {
        f.fails(
            Some(&owner_a),
            "CreateKey",
            body.clone(),
            "invalid_argument",
            reason::INVALID_INPUT,
        )
        .await;
    }
    for body in [
        json!({"tenantId": a.to_string(), "pageSize": 101}),
        json!({"tenantId": a.to_string(), "afterId": "not-a-cursor"}),
    ] {
        f.fails(
            Some(&owner_a),
            "ListKeys",
            body,
            "invalid_argument",
            reason::INVALID_INPUT,
        )
        .await;
    }
    f.fails(
        Some(&owner_a),
        "RevokeKey",
        json!({"tenantId": a.to_string(), "keyId": "x"}),
        "invalid_argument",
        reason::INVALID_INPUT,
    )
    .await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM api_keys")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        1
    );

    // Pagination is stable and bounded.
    for _ in 0..3 {
        f.ok(&owner_a, "CreateKey", create(a, SERVICE, &[EXTRACTOR]))
            .await;
    }
    let first = f
        .ok(
            &owner_a,
            "ListKeys",
            json!({"tenantId": a.to_string(), "kind": SERVICE, "pageSize": 3}),
        )
        .await;
    assert_eq!(count(&first.body), 3);
    let cursor = first.body["nextAfterId"].as_str().unwrap().to_owned();
    let second = f
        .ok(
            &owner_a,
            "ListKeys",
            json!({"tenantId": a.to_string(), "kind": SERVICE, "pageSize": 3, "afterId": cursor}),
        )
        .await;
    assert_eq!(count(&second.body), 1);
    assert!(second.body.get("nextAfterId").is_none_or(|v| v == ""));
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn management_requires_a_verified_user_token() {
    let f = Fixture::setup().await;
    let tenant = id();
    let (owner, _) = f.user("owner", Some((tenant, TenantRole::Owner)));
    let body = create(tenant, SERVICE, &[TRANSLATOR]);
    // Missing, malformed and forged credentials.
    f.fails(
        None,
        "CreateKey",
        body.clone(),
        "unauthenticated",
        reason::UNAUTHENTICATED,
    )
    .await;
    for bad in ["garbage", "a.b.c", "dtk_short"] {
        f.fails(
            Some(bad),
            "CreateKey",
            body.clone(),
            "unauthenticated",
            reason::UNAUTHENTICATED,
        )
        .await;
    }
    let wrong_audience = {
        let mut claims = json!({"iss": f.issuer, "aud": "https://other", "sub": "owner",
            "exp": chrono::Utc::now().timestamp() + 600});
        claims["roles"] = json!([]);
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some("fixture".into());
        jsonwebtoken::encode(
            &header,
            &claims,
            &jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!("support/oidc-test-key.pem"))
                .unwrap(),
        )
        .unwrap()
    };
    f.fails(
        Some(&wrong_audience),
        "CreateKey",
        body.clone(),
        "unauthenticated",
        reason::UNAUTHENTICATED,
    )
    .await;
    // A valid machine token never acts as a user.
    let machine = f.token("svc-client", &[], true);
    f.authority.add_user(&machine, id());
    f.fails(
        Some(&machine),
        "CreateKey",
        body.clone(),
        "permission_denied",
        reason::FORBIDDEN,
    )
    .await;
    // A verified token for a user the platform does not know.
    let unknown = f.token("unknown-user", &[], false);
    f.fails(
        Some(&unknown),
        "CreateKey",
        body.clone(),
        "permission_denied",
        reason::FORBIDDEN,
    )
    .await;
    // An API key cannot manage keys.
    let created = f.ok(&owner, "CreateKey", body.clone()).await;
    let secret = created.body["secret"].as_str().unwrap().to_owned();
    f.fails(
        Some(&secret),
        "CreateKey",
        body.clone(),
        "permission_denied",
        reason::FORBIDDEN,
    )
    .await;
    f.fails(
        Some(&secret),
        "ListKeys",
        json!({"tenantId": tenant.to_string()}),
        "permission_denied",
        reason::FORBIDDEN,
    )
    .await;
    // The tenant authority being down fails closed with a retryable code.
    f.authority.set_unavailable(true);
    f.fails(
        Some(&owner),
        "CreateKey",
        body.clone(),
        "unavailable",
        reason::INTERNAL,
    )
    .await;
    f.authority.set_unavailable(false);
    f.ok(&owner, "CreateKey", body).await;
    f.finish().await;
}

fn expect_fault(
    result: delitools_api::error::Result<delitools_api::platform::ExecutionPrincipal>,
    code: connectrpc::ErrorCode,
    reason: i32,
) {
    let fault = result.expect_err("expected a rejection");
    assert_eq!(fault.code, code, "{fault}");
    assert_eq!(fault.reason as i32, reason, "{fault}");
}

async fn authenticate(
    platform: &Platform,
    bearer: &str,
    tenant: Uuid,
    capability: Capability,
) -> delitools_api::error::Result<delitools_api::platform::ExecutionPrincipal> {
    platform
        .authenticate_execution(&format!("Bearer {bearer}"), tenant, capability)
        .await
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn execution_credentials_follow_key_kind_capability_and_current_access() {
    use connectrpc::ErrorCode;
    let f = Fixture::setup().await;
    let (tenant, other_tenant) = (id(), id());
    let (owner, owner_id) = f.user("owner", Some((tenant, TenantRole::Owner)));
    let (member, member_id) = f.user("member", Some((tenant, TenantRole::Member)));
    let (outsider, _) = f.user("outsider", None);
    f.authority.add_tenant(other_tenant);
    let translator = Capability::CAPABILITY_TRANSLATOR;
    let extractor = Capability::CAPABILITY_EXTRACTOR;

    let service = f
        .ok(&owner, "CreateKey", create(tenant, SERVICE, &[TRANSLATOR]))
        .await;
    let service_secret = service.body["secret"].as_str().unwrap().to_owned();
    let personal = f
        .ok(
            &member,
            "CreateKey",
            create(tenant, PERSONAL, &[TRANSLATOR, EXTRACTOR]),
        )
        .await;
    let personal_secret = personal.body["secret"].as_str().unwrap().to_owned();

    // Service key: tenant-level, restricted to its capabilities and its own tenant.
    let principal = authenticate(&f.platform, &service_secret, tenant, translator)
        .await
        .unwrap();
    assert_eq!(principal.credential, CredentialKind::ServiceKey);
    assert_eq!(principal.user_id, None);
    assert!(principal.api_key_id.is_some());
    expect_fault(
        authenticate(&f.platform, &service_secret, tenant, extractor).await,
        ErrorCode::PermissionDenied,
        reason::FORBIDDEN,
    );
    expect_fault(
        authenticate(&f.platform, &service_secret, other_tenant, translator).await,
        ErrorCode::PermissionDenied,
        reason::FORBIDDEN,
    );
    // The service key outlives the issuing member's access.
    f.authority.remove_member(tenant, owner_id);
    authenticate(&f.platform, &service_secret, tenant, translator)
        .await
        .unwrap();

    // Personal key: key capabilities intersect the user's current permissions.
    let personal_principal = authenticate(&f.platform, &personal_secret, tenant, extractor)
        .await
        .unwrap();
    assert_eq!(personal_principal.credential, CredentialKind::PersonalKey);
    assert_eq!(personal_principal.user_id, Some(member_id));
    f.authority
        .deny_capability(tenant, Capability::CAPABILITY_EXTRACTOR);
    expect_fault(
        authenticate(&f.platform, &personal_secret, tenant, extractor).await,
        ErrorCode::PermissionDenied,
        reason::FORBIDDEN,
    );
    authenticate(&f.platform, &personal_secret, tenant, translator)
        .await
        .unwrap();
    f.platform
        .reauthorize(&personal_principal)
        .await
        .expect_err("capability now denied");
    // Removing the user's access removes the personal key's permissions at once.
    f.authority.remove_member(tenant, member_id);
    expect_fault(
        authenticate(&f.platform, &personal_secret, tenant, translator).await,
        ErrorCode::PermissionDenied,
        reason::FORBIDDEN,
    );
    f.authority.set_role(tenant, member_id, TenantRole::Member);

    // Logto user tokens act as the user.
    let principal = authenticate(&f.platform, &member, tenant, translator)
        .await
        .unwrap();
    assert_eq!(principal.credential, CredentialKind::UserToken);
    assert_eq!(principal.user_id, Some(member_id));
    expect_fault(
        authenticate(&f.platform, &outsider, tenant, translator).await,
        ErrorCode::PermissionDenied,
        reason::FORBIDDEN,
    );

    // Billing blocks apply to every credential type independently.
    f.authority.set_billing_blocked(tenant, true);
    for bearer in [&service_secret, &personal_secret, &member] {
        expect_fault(
            authenticate(&f.platform, bearer, tenant, translator).await,
            ErrorCode::FailedPrecondition,
            reason::BILLING_BLOCKED,
        );
    }
    f.authority.set_billing_blocked(tenant, false);

    // Fail closed when the authority cannot answer.
    f.authority.set_unavailable(true);
    for bearer in [&service_secret, &personal_secret, &member] {
        expect_fault(
            authenticate(&f.platform, bearer, tenant, translator).await,
            ErrorCode::Unavailable,
            reason::INTERNAL,
        );
    }
    f.authority.set_unavailable(false);

    // Wrong secret, unknown key and malformed key are all plain authentication failures.
    let mut tampered = service_secret.clone();
    tampered.pop();
    tampered.push(if service_secret.ends_with('A') {
        'B'
    } else {
        'A'
    });
    let unknown = keys::generate().token;
    for bad in [&tampered, &unknown, &"dtk_x".to_owned()] {
        expect_fault(
            authenticate(&f.platform, bad, tenant, translator).await,
            ErrorCode::Unauthenticated,
            reason::UNAUTHENTICATED,
        );
    }

    // Expired keys stop authenticating exactly at their expiry instant.
    let expiring = keys::generate();
    let created = store::now() - chrono::Duration::hours(2);
    store::keys::insert(
        &f.pool,
        store::keys::NewKey {
            id: expiring.id,
            tenant_id: tenant,
            user_id: member_id,
            kind: delino_protocol::delitools::v1::KeyKind::KEY_KIND_SERVICE,
            capabilities: keys::Capabilities::ALL,
            secret_hash: &expiring.hash,
            expires_at: Some(created + chrono::Duration::hours(1)),
        },
        created,
    )
    .await
    .unwrap();
    expect_fault(
        authenticate(&f.platform, &expiring.token, tenant, translator).await,
        ErrorCode::Unauthenticated,
        reason::UNAUTHENTICATED,
    );

    // Revocation: new requests and additional attempts are rejected; a principal obtained before
    // revocation is not interrupted by the revocation itself.
    let principal = authenticate(&f.platform, &service_secret, tenant, translator)
        .await
        .unwrap();
    f.authority.set_role(tenant, owner_id, TenantRole::Owner);
    f.ok(
        &owner,
        "RevokeKey",
        json!({"tenantId": tenant.to_string(), "keyId": principal.api_key_id.unwrap().to_string()}),
    )
    .await;
    expect_fault(
        authenticate(&f.platform, &service_secret, tenant, translator).await,
        ErrorCode::Unauthenticated,
        reason::UNAUTHENTICATED,
    );
    let retry = f
        .platform
        .reauthorize(&principal)
        .await
        .expect_err("revoked");
    assert_eq!(retry.code, ErrorCode::Unauthenticated);
    f.finish().await;
}
