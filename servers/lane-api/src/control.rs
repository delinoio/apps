// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{Fault, Principal, Result, Server, string};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
impl Server {
    pub async fn account_dispatch(
        &self,
        db: &mut PgConnection,
        p: &Principal,
        method: &str,
        q: &Value,
    ) -> Result<Value> {
        if method.starts_with("List") {
            return self.auth.dispatch(db, p, method, q).await;
        }
        let request = string(&q["meta"], "requestId");
        if !crate::canonical_id(request) {
            return Err(Fault::invalid("request ID must be UUID v7"));
        }
        let mut normalized = q.clone();
        normalized["meta"] = json!({"requestId":request,"origin":p.origin()});
        let digest = crate::hash(
            &serde_jcs::to_vec(&normalized).map_err(|_| Fault::invalid("invalid request"))?,
        );
        if let Some(row)=sqlx::query("SELECT fingerprint,response,session_id FROM lane_control_receipts WHERE account_id=$1 AND method=$2 AND request_id=$3").bind(&p.account_id).bind(method).bind(request).fetch_optional(&mut *db).await? {
            if row.get::<String,_>("fingerprint")!=digest || (method=="BeginDataDeletion" && row.get::<String,_>("session_id")!=p.session_id) {
                return Err(Fault::new(connectrpc::ErrorCode::AlreadyExists,6,"request ID was used with different content or session"));
            }
            return Ok(row.get("response"));
        }
        let out = if method == "RegisterPush" {
            crate::push::register(db, p, q).await?
        } else {
            self.auth.dispatch(db, p, method, q).await?
        };
        // Account clearing removes receipts and revokes this grant. Do not reintroduce its data.
        if method != "DeleteAccountData" {
            sqlx::query("INSERT INTO lane_control_receipts(account_id,method,request_id,session_id,fingerprint,response) VALUES($1,$2,$3,$4,$5,$6)").bind(&p.account_id).bind(method).bind(request).bind(&p.session_id).bind(digest).bind(&out).execute(db).await?;
        }
        Ok(out)
    }
}
/// A newly approved grant may retrieve only its own old generation's deletion directive.
pub async fn deletion_pull(
    db: &mut PgConnection,
    p: &Principal,
    q: &Value,
) -> Result<Option<Value>> {
    let epoch = crate::auth::epoch(db, p).await?;
    let supplied = string(q, "epoch");
    if epoch == supplied {
        return Ok(None);
    }
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT mode,new_epoch FROM lane_account_deletions WHERE account_id=$1 AND old_epoch=$2",
    )
    .bind(&p.account_id)
    .bind(supplied)
    .fetch_optional(db)
    .await?;
    if let Some((mode, new_epoch)) = row
        && mode == "DATA_DELETION_MODE_DELETE_EVERYWHERE"
    {
        return Ok(Some(
            json!({"changes":[{"sequence":"0","deletion":{"kind":"ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT","id":p.account_id,"deletionVersionId":new_epoch}}],"nextCursor":"","hasMore":false}),
        ));
    }
    Err(Fault::new(
        connectrpc::ErrorCode::FailedPrecondition,
        17,
        "account generation changed; explicit reconnection required",
    ))
}
