// SPDX-License-Identifier: FSL-1.1-ALv2
use super::*;
use serde_json::{Value, json};
use sqlx::Row;
use std::collections::HashMap;

impl Call<'_> {
    pub async fn tenant_states(&mut self, q: &Value) -> Result<Value> {
        self.service_scope("authorize").await?;
        let ids = q["tenantIds"]
            .as_array()
            .ok_or_else(|| invalid("supply 1 to 100 known tenant IDs"))?;
        if ids.is_empty() || ids.len() > 100 {
            return Err(invalid("supply 1 to 100 known tenant IDs"));
        }
        let parsed = ids
            .iter()
            .map(|v| {
                let value = v.as_str().unwrap_or("");
                if !canonical(value) {
                    return Err(invalid("invalid tenant ID"));
                }
                uuid(value)
            })
            .collect::<Result<Vec<_>>>()?;
        let rows=sqlx::query("SELECT id::text,deleted_at IS NOT NULL,deleting_at IS NOT NULL,operator_block FROM tenants WHERE id=ANY($1::uuid[])").bind(parsed).fetch_all(&mut *self.tx).await?;
        let mut known = HashMap::new();
        for row in rows {
            let id: String = row.try_get(0)?;
            let state = if row.try_get::<bool, _>(1)? {
                4
            } else if row.try_get::<bool, _>(2)? {
                3
            } else if row.try_get::<bool, _>(3)? {
                2
            } else {
                1
            };
            known.insert(id, state);
        }
        let mut counts = HashMap::<&str, usize>::new();
        let mut states = Vec::new();
        for id in ids {
            let id = id.as_str().unwrap_or("");
            let state = known.get(id).copied().unwrap_or(5);
            let name = match state {
                1 => "TENANT_STATE_ACTIVE",
                2 => "TENANT_STATE_BLOCKED",
                3 => "TENANT_STATE_DELETING",
                4 => "TENANT_STATE_DELETED",
                _ => "TENANT_STATE_UNKNOWN",
            };
            *counts.entry(name).or_default() += 1;
            states.push(json!({"tenantId":id,"state":state}));
        }
        // Only aggregate counts belong in logs; IDs and individual states stay in the response.
        tracing::info!(correlation_id=%self.correlation,service_id=%self.service,requested=ids.len(),states=?counts,"tenant states read");
        Ok(json!({"states":states}))
    }
}
