// SPDX-License-Identifier: FSL-1.1-ALv2
//! Delcore identity, tenant state and role reads. AnyDash never reads Delcore tables and never
//! caches authorization state across requests.
use crate::{ApiError, Result, config::Config};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use server_common::authority::{Authority, TenantState};
use std::collections::{HashMap, HashSet};

/// Fixed Delcore roles of one user in one tenant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Roles(u8);
impl Roles {
    pub const OWNER: Self = Self(1);
    pub const ADMIN: Self = Self(2);
    pub const BILLING: Self = Self(4);
    pub const MEMBER: Self = Self(8);
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    pub fn has(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
    /// Owner or Admin.
    pub fn manager(self) -> bool {
        self.has(Self::OWNER) || self.has(Self::ADMIN)
    }
    /// Owner, Admin or Member; Billing-only users and non-members have no AnyDash access.
    pub fn viewer(self) -> bool {
        self.manager() || self.has(Self::MEMBER)
    }
}

/// One user's current membership in one tenant: the fixed roles and the start of the current
/// membership. Non-members have no roles and no start. A departure followed by a rejoin yields a
/// new start, so comparing it with a stored value detects an absence between two reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Membership {
    pub roles: Roles,
    pub since: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct Tenant {
    pub id: String,
    pub name: String,
    pub personal: bool,
    pub owner_id: String,
}

#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub user_id: String,
    pub tenants: Vec<Tenant>,
}

pub struct Delcore {
    authority: Authority,
}

impl Delcore {
    pub fn new(http: reqwest::Client, config: &Config) -> Self {
        Self {
            authority: Authority::new(
                http,
                config.delcore_url.clone(),
                config.logto_token_url.clone(),
                config.delcore_resource.clone(),
                config.client_id.clone(),
                config.client_secret.clone(),
            ),
        }
    }

    /// Reads the caller and every tenant page with the user's Delcore token. The token is used for
    /// this call only and never stored or logged.
    pub async fn current_user(&self, token: &str) -> Result<CurrentUser> {
        let authorization = format!("Bearer {token}");
        let mut after = String::new();
        let mut user_id = String::new();
        let mut tenants = Vec::new();
        let mut seen = HashSet::new();
        for _ in 0..100 {
            let mut page = json!({"size":100});
            if !after.is_empty() {
                page["afterId"] = json!(after);
            }
            let value = self
                .authority
                .call("GetCurrentUser", &authorization, json!({"page":page}))
                .await
                .map_err(|f| match f.code {
                    connectrpc::ErrorCode::Unauthenticated => ApiError::unauthenticated(),
                    _ => ApiError::dependency(),
                })?;
            let user = value["userId"].as_str().unwrap_or_default();
            if !server_common::canonical_id(user) || (!user_id.is_empty() && user_id != user) {
                return Err(ApiError::dependency());
            }
            user_id = user.into();
            for t in value["tenants"].as_array().into_iter().flatten() {
                let id = t["id"].as_str().unwrap_or_default();
                if !server_common::canonical_id(id) {
                    return Err(ApiError::dependency());
                }
                if seen.insert(id.to_owned()) {
                    tenants.push(Tenant {
                        id: id.into(),
                        name: t["name"].as_str().unwrap_or_default().into(),
                        personal: t["personal"].as_bool().unwrap_or(false),
                        owner_id: t["ownerId"].as_str().unwrap_or_default().into(),
                    });
                }
            }
            after = value["nextAfterId"].as_str().unwrap_or_default().into();
            if after.is_empty() {
                return Ok(CurrentUser { user_id, tenants });
            }
        }
        Err(ApiError::dependency())
    }

    /// Tenant states for any number of IDs, in batches of 100.
    pub async fn states(&self, ids: &[String]) -> Result<Vec<TenantState>> {
        let mut out = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(100) {
            let states = self.authority.states(chunk).await.map_err(|_| {
                tracing::warn!(
                    category = "delcore",
                    operation = "GetTenantStates",
                    "Delcore call failed"
                );
                ApiError::dependency()
            })?;
            out.extend(states);
        }
        Ok(out)
    }

    /// Requires the tenant to be `ACTIVE`; every other state, including `UNKNOWN`, is inactive.
    pub async fn require_active(&self, tenant: &str) -> Result<()> {
        match self.states(&[tenant.to_owned()]).await?.as_slice() {
            [TenantState::Active] => Ok(()),
            _ => Err(ApiError::space_inactive()),
        }
    }

    /// Memberships of the given users in one tenant, in batches of 100. Non-members, withdrawing
    /// users and users of deleted tenants have no roles and no membership start.
    pub async fn roles(
        &self,
        tenant: &str,
        users: &[String],
    ) -> Result<HashMap<String, Membership>> {
        let mut unique: Vec<String> = Vec::new();
        for u in users {
            if !unique.contains(u) {
                unique.push(u.clone());
            }
        }
        let mut out = HashMap::new();
        for chunk in unique.chunks(100) {
            let token = self
                .authority
                .service_token()
                .await
                .map_err(|_| ApiError::dependency())?;
            let value = self
                .authority
                .call(
                    "GetTenantMemberRoles",
                    &format!("Bearer {token}"),
                    json!({"tenantId":tenant,"userIds":chunk}),
                )
                .await
                .map_err(|_| {
                    tracing::warn!(
                        category = "delcore",
                        operation = "GetTenantMemberRoles",
                        "Delcore call failed"
                    );
                    ApiError::dependency()
                })?;
            let members = value["members"]
                .as_array()
                .ok_or_else(ApiError::dependency)?;
            if members.len() != chunk.len() {
                return Err(ApiError::dependency());
            }
            for (entry, user) in members.iter().zip(chunk) {
                if entry["userId"].as_str() != Some(user.as_str()) {
                    return Err(ApiError::dependency());
                }
                out.insert(user.clone(), parse_membership(entry)?);
            }
        }
        Ok(out)
    }
}

/// A member entry must carry its membership start: without it a departure and rejoin could not
/// be detected, so a missing or malformed value denies access as a Delcore failure.
fn parse_membership(entry: &Value) -> Result<Membership> {
    if entry["member"].as_bool() != Some(true) {
        return Ok(Membership::default());
    }
    let since = entry["memberSince"]
        .as_str()
        .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
        .map(|v| v.with_timezone(&Utc))
        .ok_or_else(|| {
            tracing::warn!(
                category = "delcore",
                operation = "GetTenantMemberRoles",
                "member entry without a valid membership start"
            );
            ApiError::dependency()
        })?;
    Ok(Membership {
        roles: parse_roles(entry),
        since: Some(since),
    })
}

fn parse_roles(entry: &Value) -> Roles {
    let mut roles = Roles::default();
    for role in entry["roles"].as_array().into_iter().flatten() {
        let r = match (role.as_str(), role.as_i64()) {
            (Some("ROLE_OWNER"), _) | (_, Some(1)) => Roles::OWNER,
            (Some("ROLE_ADMIN"), _) | (_, Some(2)) => Roles::ADMIN,
            (Some("ROLE_BILLING"), _) | (_, Some(3)) => Roles::BILLING,
            (Some("ROLE_MEMBER"), _) | (_, Some(4)) => Roles::MEMBER,
            _ => continue,
        };
        roles = roles.union(r);
    }
    roles
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn role_matrix_helpers_follow_the_team_rules() {
        let since = "2026-10-01T08:30:00.123456Z";
        let e = |roles: Value| {
            parse_membership(&json!({"member":true,"roles":roles,"memberSince":since}))
                .unwrap()
                .roles
        };
        let owner = e(json!(["ROLE_OWNER"]));
        let admin = e(json!(["ROLE_ADMIN"]));
        let member = e(json!(["ROLE_MEMBER"]));
        let billing = e(json!(["ROLE_BILLING"]));
        let gone = parse_membership(&json!({"member":false,"roles":["ROLE_OWNER"]}))
            .unwrap()
            .roles;
        let union = e(json!(["ROLE_BILLING", "ROLE_MEMBER"]));
        assert!(owner.manager() && owner.viewer());
        assert!(admin.manager() && admin.viewer());
        assert!(!member.manager() && member.viewer());
        assert!(!billing.manager() && !billing.viewer());
        assert!(!gone.viewer());
        assert!(union.viewer() && !union.manager());
    }

    #[test]
    fn membership_start_is_exact_and_required_for_members() {
        let m = parse_membership(
            &json!({"member":true,"roles":["ROLE_MEMBER"],"memberSince":"2026-10-01T08:30:00.123456Z"}),
        )
        .unwrap();
        assert_eq!(
            m.since.unwrap().timestamp_micros(),
            DateTime::parse_from_rfc3339("2026-10-01T08:30:00.123456Z")
                .unwrap()
                .timestamp_micros()
        );
        // Non-members have no start, even if one is sent.
        let gone = parse_membership(&json!({"member":false,"memberSince":"2026-10-01T08:30:00Z"}))
            .unwrap();
        assert_eq!(gone, Membership::default());
        // A member without a valid start fails closed.
        for entry in [
            json!({"member":true,"roles":["ROLE_MEMBER"]}),
            json!({"member":true,"roles":["ROLE_MEMBER"],"memberSince":"yesterday"}),
        ] {
            assert!(parse_membership(&entry).is_err());
        }
    }
}
