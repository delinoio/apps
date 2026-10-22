// SPDX-License-Identifier: FSL-1.1-ALv2
pub mod api_keys;
pub mod billing;
pub mod catalog;
pub mod events;
pub mod features;
pub mod metering_queries;
pub mod tenant_states;
pub mod tenants;
pub mod usage;
use crate::{Fault, Result, auth::Identity, config::Config};
use chrono::{DateTime, Utc};
use serde_json::Value;
pub use server_common::{canonical_id as canonical, id, string};
use sqlx::PgConnection;
pub fn uuid(value: &str) -> Result<uuid::Uuid> {
    if !canonical(value) && value != "00000000-0000-0000-0000-000000000000" {
        return Err(invalid("invalid canonical UUID v7"));
    }
    uuid::Uuid::parse_str(value).map_err(|_| invalid("invalid UUID"))
}
pub fn invalid(message: impl Into<String>) -> Fault {
    Fault::invalid(message)
}
pub fn forbidden() -> Fault {
    Fault::denied()
}
pub fn precondition(message: impl Into<String>) -> Fault {
    Fault::precondition(message)
}
pub fn conflict() -> Fault {
    Fault::conflict()
}
pub fn has(roles: &[i32], want: &[i32]) -> bool {
    roles.iter().any(|r| want.contains(r))
}
pub fn nonempty(value: &str, max: usize) -> Result<()> {
    if value.is_empty() || value.len() > max {
        Err(invalid(format!("value must contain 1 to {max} bytes")))
    } else {
        Ok(())
    }
}
pub fn page(value: &Value) -> Result<(i64, String)> {
    let size = server_common::integer(value, "size");
    let size = if size == 0 { 50 } else { size };
    if !(1..=100).contains(&size) {
        return Err(invalid("page size must be 1 to 100"));
    }
    let cursor = string(value, "afterId");
    if !cursor.is_empty() && !canonical(cursor) {
        return Err(invalid("invalid cursor"));
    }
    Ok((
        size,
        if cursor.is_empty() {
            "00000000-0000-0000-0000-000000000000".into()
        } else {
            cursor.into()
        },
    ))
}
pub struct Call<'a> {
    pub tx: &'a mut PgConnection,
    pub identity: Identity,
    pub user: String,
    pub service: String,
    pub actor: String,
    pub correlation: String,
    pub now: DateTime<Utc>,
    pub config: &'a Config,
}
impl Call<'_> {
    pub fn operator(&self, reason: &str) -> Result<()> {
        if !self.identity.operator || self.user.is_empty() {
            return Err(forbidden());
        }
        if !(3..=1000).contains(&reason.len()) {
            return Err(invalid("operator reason must contain 3 to 1000 characters"));
        }
        Ok(())
    }
    pub async fn audit(&mut self, tenant: &str, action: &str, reason: &str) -> Result<()> {
        sqlx::query("INSERT INTO audit VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(uuid(&id())?)
            .bind(&self.actor)
            .bind(if tenant.is_empty() {
                None
            } else {
                Some(uuid(tenant)?)
            })
            .bind(action)
            .bind(reason)
            .bind(uuid(&self.correlation)?)
            .bind(self.now)
            .execute(&mut *self.tx)
            .await?;
        Ok(())
    }
    pub async fn roles(&mut self, tenant: &str) -> Result<Vec<i32>> {
        if self.user.is_empty() || !canonical(tenant) {
            return Err(forbidden());
        }
        sqlx::query_scalar("SELECT m.roles FROM members m JOIN tenants t ON t.id=m.tenant_id WHERE m.tenant_id=$1 AND m.user_id=$2 AND t.deleted_at IS NULL").bind(uuid(tenant)?).bind(uuid(&self.user)?).fetch_optional(&mut *self.tx).await?.ok_or_else(forbidden)
    }
    pub async fn require(&mut self, tenant: &str, want: &[i32]) -> Result<()> {
        let roles = self.roles(tenant).await?;
        if !has(&roles, &[1]) && !has(&roles, want) {
            return Err(forbidden());
        }
        Ok(())
    }
    pub async fn member(&mut self, tenant: &str) -> Result<()> {
        self.roles(tenant).await.map(|_| ())
    }
    pub async fn job(
        &mut self,
        tenant: &str,
        kind: &str,
        key: &str,
        payload: &Value,
    ) -> Result<String> {
        let job = id();
        sqlx::query("INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,available_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$6,$6)").bind(uuid(&job)?).bind(if tenant.is_empty(){None}else{Some(uuid(tenant)?)}).bind(kind).bind(key).bind(payload).bind(self.now).execute(&mut *self.tx).await?;
        Ok(job)
    }
    pub async fn service_scope(&mut self, scope: &str) -> Result<()> {
        if self.service.is_empty() {
            return Err(forbidden());
        }
        let allowed: bool = sqlx::query_scalar("SELECT $2=ANY(scopes) FROM services WHERE id=$1")
            .bind(uuid(&self.service)?)
            .bind(scope)
            .fetch_one(&mut *self.tx)
            .await?;
        if allowed && self.identity.scopes.iter().any(|s| s == scope) {
            Ok(())
        } else {
            Err(forbidden())
        }
    }
    pub async fn authorize_replay(&mut self, op: crate::Op, q: &Value) -> Result<()> {
        use crate::Op::*;
        if q.get("reason").is_some() {
            return self.operator(string(q, "reason"));
        }
        if matches!(op, ReportUsage | CorrectUsage | IngestEvent | CorrectEvent) {
            return self.service_scope("usage:write").await;
        }
        let tenant = string(q, "tenantId");
        if q.get("tenantId").is_none() {
            return if self.user.is_empty() {
                Err(forbidden())
            } else {
                Ok(())
            };
        }
        match op {
            CreateApiKey | RevokeApiKey => self.authorize_api_key_replay(op, q).await,
            SetMemberRoles | RemoveMember | CreateInvitation | RevokeInvitation | SetGroup
            | DeleteGroup => self.require(tenant, &[2]).await,
            RequestTopUp | SetAutoTopUp | ChangeSubscription | CancelSubscription => {
                self.require(tenant, &[3]).await
            }
            DeleteTenant | TransferOwnership => self.require(tenant, &[]).await,
            _ => self.member(tenant).await,
        }
    }
}

impl Call<'_> {
    pub async fn dispatch(&mut self, op: crate::Op, q: &Value) -> Result<Value> {
        use crate::Op::*;
        match op {
            GetCurrentUser | CreateTenant | GetTenant | ListMembers | SetMemberRoles
            | RemoveMember | CreateInvitation | AcceptInvitation | RevokeInvitation
            | TransferOwnership | AcceptOwnership => self.tenants(op, q).await,
            GetTenantStates => self.tenant_states(q).await,
            Authorize | ReportUsage | CorrectUsage | ListUsage | ListLedger
            | CheckTenantLifecycle => self.usage(op, q).await,
            CreateApiKey | ListApiKeys | RevokeApiKey | AuthorizeApiKey => {
                self.api_keys(op, q).await
            }
            IngestEvent | CorrectEvent | GetEvent => self.events(op, q).await,
            ListMeterAggregates | GetMeteringStatus => self.metering_queries(op, q).await,
            SetGroup | ListGroups | DeleteGroup | GetCatalog | PutService | PutPlan
            | CreatePrice | PutMeter | PutFeature | PutAddOn | ListBillingHistory => {
                self.catalog(op, q).await
            }
            RequestTopUp | SetAutoTopUp | ChangeSubscription | CancelSubscription
            | DeleteTenant | Withdraw | SetTenantBlocked | ListJobs | RetryJob | ListAudit => {
                self.billing(op, q).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn pagination_has_bounded_sizes_and_canonical_cursors() {
        assert_eq!(page(&Value::Null).unwrap().0, 50);
        assert!(page(&json!({"size":101})).is_err());
        assert!(page(&json!({"size":-1})).is_err());
        assert!(page(&json!({"afterId":"00000000-0000-0000-0000-000000000000"})).is_err());
        assert!(page(&json!({"afterId":"019b0000-0000-7000-8000-000000000001"})).is_ok());
    }
}
