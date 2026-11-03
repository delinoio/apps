// SPDX-License-Identifier: FSL-1.1-ALv2
use super::*;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use sqlx::Row;
use std::collections::HashMap;

impl Call<'_> {
    /// Reads the fixed roles of 1 to 100 users in one tenant for a registered consumer service.
    ///
    /// The read never writes state and returns no names or personal data. A current member is
    /// reported with the start of the current membership. A user who is not a current member, is
    /// withdrawing, or belongs to a deleted or unknown tenant is reported as a non-member without
    /// roles or membership start.
    pub async fn tenant_member_roles(&mut self, q: &Value) -> Result<Value> {
        self.service_scope("authorize").await?;
        let tenant = string(q, "tenantId");
        if !canonical(tenant) {
            return Err(invalid("invalid tenant ID"));
        }
        let ids = q["userIds"]
            .as_array()
            .ok_or_else(|| invalid("supply 1 to 100 user IDs"))?;
        if ids.is_empty() || ids.len() > 100 {
            return Err(invalid("supply 1 to 100 user IDs"));
        }
        let parsed = ids
            .iter()
            .map(|v| {
                let value = v.as_str().unwrap_or("");
                if !canonical(value) {
                    return Err(invalid("invalid user ID"));
                }
                uuid(value)
            })
            .collect::<Result<Vec<_>>>()?;
        let rows = sqlx::query(
            "SELECT m.user_id::text,m.roles,m.member_since FROM members m \
             JOIN tenants t ON t.id=m.tenant_id JOIN users u ON u.id=m.user_id \
             WHERE m.tenant_id=$1 AND m.user_id=ANY($2::uuid[]) \
             AND t.deleted_at IS NULL AND u.withdrawing_at IS NULL",
        )
        .bind(uuid(tenant)?)
        .bind(parsed)
        .fetch_all(&mut *self.tx)
        .await?;
        let mut known = HashMap::new();
        for row in rows {
            let user: String = row.try_get(0)?;
            let mut roles: Vec<i32> = row.try_get(1)?;
            roles.sort_unstable();
            roles.dedup();
            let since: DateTime<Utc> = row.try_get(2)?;
            known.insert(user, (roles, since));
        }
        let mut members = 0usize;
        let mut entries = Vec::new();
        for id in ids {
            let id = id.as_str().unwrap_or("");
            match known.get(id) {
                Some((roles, since)) => {
                    members += 1;
                    // Microseconds keep the stored value exact, so consumers can compare it.
                    let since = since.to_rfc3339_opts(SecondsFormat::Micros, true);
                    entries
                        .push(json!({"userId":id,"member":true,"roles":roles,"memberSince":since}));
                }
                None => entries.push(json!({"userId":id,"member":false,"roles":[]})),
            }
        }
        // Only aggregate counts belong in logs; IDs and roles stay in the response.
        tracing::info!(correlation_id=%self.correlation,service_id=%self.service,requested=ids.len(),members,"tenant member roles read");
        Ok(json!({"members":entries}))
    }
}
