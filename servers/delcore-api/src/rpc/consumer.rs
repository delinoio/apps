// SPDX-License-Identifier: FSL-1.1-ALv2

//! Tenant-key authorization modes for the DeliTools consumer. DeliTools checks a
//! tenant-owned service key without any member, a personal key through the named
//! user's current access, and key management through current tenant roles. The
//! modes belong to the service registered as "delitools" and grant no platform
//! authority; blocking, billing and lifecycle checks stay in force.
use super::{Call, canonical, forbidden, invalid, string, uuid};
use crate::{Result, delitools};
use serde_json::Value;

/// Authorization mode of an `Authorize` request (`ConsumerAuthorization`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The user-attributed check: the user's roles or group permissions grant
    /// the capability. Personal keys use it with the key owner's user ID.
    User,
    /// A tenant-owned service key; no user is involved.
    TenantServiceKey,
    /// Personal-key management by a current tenant member.
    PersonalKeyManagement,
    /// Service-key management by a current tenant Owner or Admin.
    ServiceKeyManagement,
}

impl Mode {
    /// Decode the protobuf JSON value: the enum name or its number.
    pub fn parse(value: &Value) -> Result<Self> {
        let number = match value {
            Value::Null => 0,
            Value::Number(n) => n.as_i64().unwrap_or(-1),
            Value::String(s) => match s.as_str() {
                "" | "CONSUMER_AUTHORIZATION_UNSPECIFIED" => 0,
                "CONSUMER_AUTHORIZATION_TENANT_SERVICE_KEY" => 1,
                "CONSUMER_AUTHORIZATION_PERSONAL_KEY_MANAGEMENT" => 2,
                "CONSUMER_AUTHORIZATION_SERVICE_KEY_MANAGEMENT" => 3,
                _ => -1,
            },
            _ => -1,
        };
        match number {
            0 => Ok(Self::User),
            1 => Ok(Self::TenantServiceKey),
            2 => Ok(Self::PersonalKeyManagement),
            3 => Ok(Self::ServiceKeyManagement),
            _ => Err(invalid("unknown consumer authorization mode")),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::TenantServiceKey => "tenant_service_key",
            Self::PersonalKeyManagement => "personal_key_management",
            Self::ServiceKeyManagement => "service_key_management",
        }
    }
}

/// Owner or Admin role numbers in `members.roles`.
const OWNER_ROLE: i32 = 1;
const ADMIN_ROLE: i32 = 2;

impl Call<'_> {
    /// Validate the request shape of a consumer mode and restrict it to DeliTools.
    pub async fn consumer_mode(&mut self, q: &Value) -> Result<Mode> {
        let mode = Mode::parse(&q["consumerAuthorization"])?;
        if mode == Mode::User {
            return Ok(mode);
        }
        if !delitools::is_delitools(self.tx, &self.service).await? {
            tracing::warn!(category="consumer_authorization",correlation_id=%self.correlation,service_id=%self.service,mode=mode.as_str(),"consumer authorization mode rejected for an unregistered consumer");
            return Err(forbidden());
        }
        let user = string(q, "userId");
        let paid = q["paid"].as_bool().unwrap_or(false);
        match mode {
            Mode::TenantServiceKey => {
                if !user.is_empty() {
                    return Err(invalid(
                        "a tenant service key authorization carries no user ID",
                    ));
                }
                if string(q, "capability").is_empty() {
                    return Err(invalid("a capability is required"));
                }
            }
            _ => {
                if !canonical(user) {
                    return Err(invalid("invalid user ID"));
                }
                if paid || !string(q, "capability").is_empty() {
                    return Err(invalid(
                        "key management is neither paid nor capability execution",
                    ));
                }
            }
        }
        Ok(mode)
    }

    /// Whether the principal of the mode currently has access to the tenant.
    /// Every mode requires the tenant not to be finally deleted.
    pub async fn consumer_allowed(&mut self, mode: Mode, q: &Value) -> Result<bool> {
        let tenant = uuid(string(q, "tenantId"))?;
        Ok(match mode {
            Mode::User => unreachable!("the user check is evaluated by the caller"),
            Mode::TenantServiceKey => {
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tenants t JOIN services s ON s.id=$2 WHERE t.id=$1 AND t.deleted_at IS NULL AND $3=ANY(s.capabilities))")
                    .bind(tenant).bind(uuid(&self.service)?).bind(string(q, "capability")).fetch_one(&mut *self.tx).await?
            }
            Mode::PersonalKeyManagement | Mode::ServiceKeyManagement => {
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM members m JOIN users u ON u.id=m.user_id JOIN tenants t ON t.id=m.tenant_id WHERE m.tenant_id=$1 AND m.user_id=$2 AND u.withdrawing_at IS NULL AND t.deleted_at IS NULL AND ($3 OR m.roles && ARRAY[$4,$5]::integer[]))")
                    .bind(tenant).bind(uuid(string(q, "userId"))?)
                    .bind(mode == Mode::PersonalKeyManagement)
                    .bind(OWNER_ROLE).bind(ADMIN_ROLE).fetch_one(&mut *self.tx).await?
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn modes_decode_names_and_numbers() {
        assert_eq!(Mode::parse(&Value::Null).unwrap(), Mode::User);
        assert_eq!(Mode::parse(&json!(0)).unwrap(), Mode::User);
        assert_eq!(
            Mode::parse(&json!("CONSUMER_AUTHORIZATION_TENANT_SERVICE_KEY")).unwrap(),
            Mode::TenantServiceKey
        );
        assert_eq!(Mode::parse(&json!(2)).unwrap(), Mode::PersonalKeyManagement);
        assert_eq!(
            Mode::parse(&json!("CONSUMER_AUTHORIZATION_SERVICE_KEY_MANAGEMENT")).unwrap(),
            Mode::ServiceKeyManagement
        );
        for bad in [json!(4), json!(-1), json!("OTHER"), json!(true), json!(1.5)] {
            assert!(Mode::parse(&bad).is_err(), "{bad}");
        }
    }
}
