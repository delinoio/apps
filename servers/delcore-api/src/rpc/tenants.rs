// SPDX-License-Identifier: FSL-1.1-ALv2
use super::*;
use crate::Op;
use chrono::{Datelike, Duration, Months, TimeZone, Utc};
use rand::RngCore;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;

pub(crate) fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

impl Call<'_> {
    pub async fn tenant(&mut self, tenant: &str) -> Result<Value> {
        let row = sqlx::query("SELECT t.id::text,t.name,t.personal,t.owner_id::text,t.plan_id::text, (t.operator_block OR t.renewal_failed OR (t.period_end<=$2 AND (NOT COALESCE((SELECT (cv.definition->>'free')::boolean FROM tenant_catalog_selections cs JOIN catalog_versions cv ON cv.id=cs.version_id WHERE cs.tenant_id=t.id AND cs.product_id=t.plan_id AND cs.ends_at IS NULL),p.free) OR t.external_subscription_id<>'')) OR EXISTS(SELECT 1 FROM disputes d WHERE d.tenant_id=t.id AND active)) AS blocked, t.deleting_at IS NOT NULL AS deleting,COALESCE((SELECT sum(amount)::text FROM ledger WHERE tenant_id=t.id AND kind IN ('credit','usage','refund','correction')),'0') AS balance, COALESCE((SELECT version_id::text FROM tenant_catalog_selections WHERE tenant_id=t.id AND product_id=t.plan_id AND ends_at IS NULL LIMIT 1),'') AS version FROM tenants t JOIN plans p ON p.id=t.plan_id WHERE t.id=$1 AND t.deleted_at IS NULL")
            .bind(uuid(tenant)?).bind(self.now).fetch_one(&mut *self.tx).await?;
        Ok(
            json!({"id":row.try_get::<String,_>(0)?,"name":row.try_get::<String,_>(1)?,"personal":row.try_get::<bool,_>(2)?,"ownerId":row.try_get::<String,_>(3)?,"planId":row.try_get::<String,_>(4)?,"blocked":row.try_get::<bool,_>(5)?,"deleting":row.try_get::<bool,_>(6)?,"creditBalance":row.try_get::<String,_>(7)?,"planVersionId":row.try_get::<String,_>(8)?}),
        )
    }

    pub async fn create_tenant(&mut self, name: &str, personal: bool) -> Result<Value> {
        if self.user.is_empty() {
            return Err(forbidden());
        }
        nonempty(name, 200)?;
        let plan: String =
            sqlx::query_scalar("SELECT id::text FROM plans WHERE is_default AND free")
                .fetch_one(&mut *self.tx)
                .await
                .map_err(|_| precondition("default free plan is not configured"))?;
        let tenant = id();
        let start = Utc
            .with_ymd_and_hms(self.now.year(), self.now.month(), 1, 0, 0, 0)
            .single()
            .expect("valid UTC month");
        let end = start
            .checked_add_months(Months::new(1))
            .expect("valid next month");
        sqlx::query("INSERT INTO tenants(id,name,personal,owner_id,plan_id,period_start,period_end) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(uuid(&tenant)?).bind(name).bind(personal).bind(uuid(&self.user)?).bind(uuid(&plan)?).bind(start).bind(end).execute(&mut *self.tx).await?;
        let version = crate::postgres::active_catalog_version(self.tx, "plan", &plan).await?;
        crate::postgres::replace_tenant_selections(self.tx, &tenant, &[version], self.now).await?;
        sqlx::query("INSERT INTO members VALUES($1,$2,ARRAY[1])")
            .bind(uuid(&tenant)?)
            .bind(uuid(&self.user)?)
            .execute(&mut *self.tx)
            .await?;
        for price in crate::postgres::active_product_prices(self.tx, "plan", &plan).await? {
            sqlx::query(
                "INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1,$2,$3,$4)",
            )
            .bind(uuid(&id())?)
            .bind(uuid(&tenant)?)
            .bind(uuid(&price)?)
            .bind(self.now)
            .execute(&mut *self.tx)
            .await?;
        }
        crate::postgres::grant_allowances(
            self.tx,
            &tenant,
            start,
            end,
            self.now,
            &format!("free:{tenant}:{}", start.format("%Y-%m")),
            None,
        )
        .await?;
        self.audit(&tenant, "tenant.create", "").await?;
        self.tenant(&tenant).await
    }

    async fn grant(&mut self, tenant: &str, roles: &[i32]) -> Result<()> {
        let existing = self.roles(tenant).await?;
        if !existing.iter().any(|r| [1, 2].contains(r)) {
            return Err(forbidden());
        }
        if roles.is_empty() || roles.len() > 3 {
            return Err(invalid("one to three non-owner roles required"));
        }
        let mut seen = std::collections::HashSet::new();
        for role in roles {
            if !(2..=4).contains(role) || !seen.insert(*role) {
                return Err(invalid("invalid or repeated role"));
            }
            if *role != 4 && !existing.contains(&1) {
                return Err(forbidden());
            }
        }
        Ok(())
    }

    pub async fn tenants(&mut self, op: Op, q: &Value) -> Result<Value> {
        let tenant = string(q, "tenantId");
        match op {
            Op::GetCurrentUser => {
                if self.user.is_empty() {
                    return Err(forbidden());
                }
                let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tenants WHERE personal AND owner_id=$1 AND deleted_at IS NULL)").bind(uuid(&self.user)?).fetch_one(&mut *self.tx).await?;
                if !exists {
                    self.create_tenant("Personal", true).await?;
                }
                let (n, after) = page(&q["page"])?;
                let n = n as usize;
                let mut ids:Vec<String>=sqlx::query_scalar("SELECT tenant_id::text FROM members WHERE user_id=$1 AND tenant_id>$2 ORDER BY tenant_id LIMIT $3").bind(uuid(&self.user)?).bind(uuid(&after)?).bind((n+1) as i64).fetch_all(&mut *self.tx).await?;
                let next = if ids.len() > n {
                    ids.truncate(n);
                    ids[n - 1].clone()
                } else {
                    String::new()
                };
                let mut tenants = Vec::new();
                for tenant in ids {
                    tenants.push(self.tenant(&tenant).await?);
                }
                Ok(json!({"userId":self.user,"tenants":tenants,"nextAfterId":next}))
            }
            Op::CreateTenant => {
                Ok(json!({"tenant":self.create_tenant(string(q,"name"),false).await?}))
            }
            Op::GetTenant => {
                self.member(tenant).await?;
                Ok(json!({"tenant":self.tenant(tenant).await?}))
            }
            Op::ListMembers => {
                self.require(tenant, &[2]).await?;
                let (n, after) = page(&q["page"])?;
                let n = n as usize;
                let rows=sqlx::query("SELECT user_id::text,roles FROM members WHERE tenant_id=$1 AND user_id>$2 ORDER BY user_id LIMIT $3").bind(uuid(tenant)?).bind(uuid(&after)?).bind((n+1) as i64).fetch_all(&mut *self.tx).await?;
                let mut members = Vec::new();
                for row in rows {
                    members.push(json!({"userId":row.try_get::<String,_>(0)?,"roles":row.try_get::<Vec<i32>,_>(1)?}));
                }
                let next = if members.len() > n {
                    members.truncate(n);
                    members[n - 1]["userId"].clone()
                } else {
                    json!("")
                };
                Ok(json!({"members":members,"nextAfterId":next}))
            }
            Op::SetMemberRoles => {
                let roles = request_roles(q)?;
                self.grant(tenant, &roles).await?;
                if !canonical(string(q, "userId")) {
                    return Err(invalid("invalid user ID"));
                }
                let user = uuid(string(q, "userId"))?;
                let current: Vec<i32> = sqlx::query_scalar(
                    "SELECT roles FROM members WHERE tenant_id=$1 AND user_id=$2",
                )
                .bind(uuid(tenant)?)
                .bind(user)
                .fetch_one(&mut *self.tx)
                .await
                .map_err(|_| forbidden())?;
                let mine = self.roles(tenant).await?;
                if current.contains(&1)
                    || (!mine.contains(&1) && current.iter().any(|r| [2, 3].contains(r)))
                {
                    return Err(forbidden());
                }
                sqlx::query("UPDATE members SET roles=$3 WHERE tenant_id=$1 AND user_id=$2")
                    .bind(uuid(tenant)?)
                    .bind(user)
                    .bind(roles)
                    .execute(&mut *self.tx)
                    .await?;
                self.audit(tenant, "member.roles", "").await?;
                Ok(json!({}))
            }
            Op::RemoveMember => {
                let mine = self.roles(tenant).await?;
                let user = string(q, "userId");
                if !canonical(user) {
                    return Err(invalid("invalid user ID"));
                }
                let roles: Vec<i32> = sqlx::query_scalar(
                    "SELECT roles FROM members WHERE tenant_id=$1 AND user_id=$2",
                )
                .bind(uuid(tenant)?)
                .bind(uuid(user)?)
                .fetch_one(&mut *self.tx)
                .await
                .map_err(|_| forbidden())?;
                if roles.contains(&1) {
                    return Err(precondition("transfer ownership before leaving"));
                }
                if user != self.user
                    && !mine.contains(&1)
                    && (!mine.contains(&2) || roles.iter().any(|r| [2, 3].contains(r)))
                {
                    return Err(forbidden());
                }
                sqlx::query("DELETE FROM members WHERE tenant_id=$1 AND user_id=$2")
                    .bind(uuid(tenant)?)
                    .bind(uuid(user)?)
                    .execute(&mut *self.tx)
                    .await?;
                self.audit(tenant, "member.remove", "").await?;
                Ok(json!({}))
            }
            Op::CreateInvitation => {
                let roles = request_roles(q)?;
                self.grant(tenant, &roles).await?;
                let t = self.tenant(tenant).await?;
                if t["personal"] == true || t["deleting"] == true {
                    return Err(precondition("tenant cannot accept invitations"));
                }
                let email = string(q, "email");
                let return_url = string(q, "returnUrl");
                if !valid_email(email) || !self.config.allows_return(return_url) {
                    return Err(invalid("invalid recipient or return URL"));
                }
                let mut bytes = [0u8; 32];
                rand::rng().fill_bytes(&mut bytes);
                let token = hex::encode(bytes);
                let invitation = id();
                sqlx::query("INSERT INTO invitations(id,tenant_id,email,token_hash,roles,expires_at) VALUES($1,$2,$3,$4,$5,$6)").bind(uuid(&invitation)?).bind(uuid(tenant)?).bind(email.to_lowercase()).bind(token_hash(&token)).bind(roles).bind(self.now+Duration::seconds(self.config.invitation_seconds)).execute(&mut *self.tx).await?;
                let mut url = reqwest::Url::parse(return_url)
                    .map_err(|_| invalid("invalid recipient or return URL"))?;
                let mut pairs: Vec<(String, String)> = url
                    .query_pairs()
                    .filter(|(key, _)| key != "invitation_token")
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                pairs.push(("invitation_token".into(), token));
                url.query_pairs_mut().clear().extend_pairs(pairs);
                self.job(tenant,"email",&format!("invitation:{invitation}"),&json!({"to":email,"subject":"Organization invitation","text":format!("Accept your organization invitation: {url}")})).await?;
                self.audit(tenant, "invitation.create", "").await?;
                Ok(json!({"invitationId":invitation}))
            }
            Op::AcceptInvitation => {
                let token = string(q, "token");
                if self.user.is_empty() || token.len() != 64 {
                    return Err(forbidden());
                }
                let row=sqlx::query("SELECT i.id::text,i.tenant_id::text,i.roles FROM invitations i JOIN tenants t ON t.id=i.tenant_id WHERE token_hash=$1 AND email=$2 AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at>$3 AND t.deleting_at IS NULL AND NOT t.personal").bind(token_hash(token)).bind(&self.identity.email).bind(self.now).fetch_one(&mut *self.tx).await.map_err(|_|forbidden())?;
                let invitation: String = row.try_get(0)?;
                let tenant: String = row.try_get(1)?;
                let roles: Vec<i32> = row.try_get(2)?;
                sqlx::query("INSERT INTO members VALUES($1,$2,$3) ON CONFLICT DO NOTHING")
                    .bind(uuid(&tenant)?)
                    .bind(uuid(&self.user)?)
                    .bind(roles)
                    .execute(&mut *self.tx)
                    .await?;
                sqlx::query("UPDATE invitations SET accepted_at=$2 WHERE id=$1")
                    .bind(uuid(&invitation)?)
                    .bind(self.now)
                    .execute(&mut *self.tx)
                    .await?;
                self.audit(&tenant, "invitation.accept", "").await?;
                Ok(json!({"tenantId":tenant}))
            }
            Op::RevokeInvitation => {
                self.require(tenant, &[2]).await?;
                if !canonical(string(q, "invitationId")) {
                    return Err(invalid("invalid invitation ID"));
                }
                sqlx::query("UPDATE invitations SET revoked_at=$3 WHERE id=$1 AND tenant_id=$2 AND accepted_at IS NULL").bind(uuid(string(q,"invitationId"))?).bind(uuid(tenant)?).bind(self.now).execute(&mut *self.tx).await?;
                self.audit(tenant, "invitation.revoke", "").await?;
                Ok(json!({}))
            }
            Op::TransferOwnership => {
                self.require(tenant, &[]).await?;
                let t = self.tenant(tenant).await?;
                let recipient = string(q, "recipientId");
                if t["personal"] == true
                    || t["deleting"] == true
                    || recipient == self.user
                    || !canonical(recipient)
                {
                    return Err(precondition("invalid ownership recipient"));
                }
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM members WHERE tenant_id=$1 AND user_id=$2)",
                )
                .bind(uuid(tenant)?)
                .bind(uuid(recipient)?)
                .fetch_one(&mut *self.tx)
                .await?;
                if !exists {
                    return Err(forbidden());
                }
                let transfer = id();
                sqlx::query("INSERT INTO ownership_transfers(id,tenant_id,sender_id,recipient_id,expires_at) VALUES($1,$2,$3,$4,$5)").bind(uuid(&transfer)?).bind(uuid(tenant)?).bind(uuid(&self.user)?).bind(uuid(recipient)?).bind(self.now+Duration::seconds(self.config.invitation_seconds)).execute(&mut *self.tx).await?;
                self.audit(tenant, "ownership.request", "").await?;
                Ok(json!({"transferId":transfer}))
            }
            Op::AcceptOwnership => {
                self.member(tenant).await?;
                if !canonical(string(q, "transferId")) {
                    return Err(invalid("invalid transfer ID"));
                }
                let transfer = uuid(string(q, "transferId"))?;
                let sender:String=sqlx::query_scalar("SELECT x.sender_id::text FROM ownership_transfers x JOIN tenants t ON t.id=x.tenant_id WHERE x.id=$1 AND x.tenant_id=$2 AND x.recipient_id=$3 AND x.accepted_at IS NULL AND x.expires_at>$4 AND t.owner_id=x.sender_id AND t.deleting_at IS NULL").bind(transfer).bind(uuid(tenant)?).bind(uuid(&self.user)?).bind(self.now).fetch_one(&mut *self.tx).await.map_err(|_|precondition("ownership transfer is no longer valid"))?;
                sqlx::query("UPDATE members SET roles=array_remove(roles,1) || ARRAY[4] WHERE tenant_id=$1 AND user_id=$2").bind(uuid(tenant)?).bind(uuid(&sender)?).execute(&mut *self.tx).await?;
                sqlx::query("UPDATE members SET roles=ARRAY[1] WHERE tenant_id=$1 AND user_id=$2")
                    .bind(uuid(tenant)?)
                    .bind(uuid(&self.user)?)
                    .execute(&mut *self.tx)
                    .await?;
                sqlx::query("UPDATE tenants SET owner_id=$2 WHERE id=$1")
                    .bind(uuid(tenant)?)
                    .bind(uuid(&self.user)?)
                    .execute(&mut *self.tx)
                    .await?;
                sqlx::query("UPDATE ownership_transfers SET accepted_at=$2 WHERE id=$1")
                    .bind(transfer)
                    .bind(self.now)
                    .execute(&mut *self.tx)
                    .await?;
                self.audit(tenant, "ownership.accept", "").await?;
                Ok(json!({}))
            }
            _ => Err(invalid("unsupported tenant operation")),
        }
    }
}

fn request_roles(q: &Value) -> Result<Vec<i32>> {
    q["roles"]
        .as_array()
        .map(|roles| {
            roles
                .iter()
                .map(|role| match role.as_str() {
                    Some("ROLE_OWNER") => Ok(1),
                    Some("ROLE_ADMIN") => Ok(2),
                    Some("ROLE_BILLING") => Ok(3),
                    Some("ROLE_MEMBER") => Ok(4),
                    _ => role
                        .as_i64()
                        .and_then(|v| i32::try_from(v).ok())
                        .ok_or_else(|| invalid("invalid or repeated role")),
                })
                .collect()
        })
        .unwrap_or_else(|| Ok(Vec::new()))
}

fn valid_email(email: &str) -> bool {
    // Go's original mailbox comparison rejects display names, comments, and
    // quoted local parts because parsing removes their extra syntax.
    !email.contains(['"', '(', ')', '<', '>'])
        && email_address::EmailAddress::parse_with_options(
            email,
            email_address::Options::default().without_display_text(),
        )
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invitations_require_a_bare_valid_mailbox() {
        for email in [
            "alice@example.com",
            "alice+invite@example.com",
            "alice@localhost",
        ] {
            assert!(valid_email(email), "{email}");
        }
        for email in [
            "",
            "a..b@example.com",
            ".alice@example.com",
            "Alice <alice@example.com>",
            "alice@example.com, bob@example.com",
            "alice@example.com\r\nBcc: x@y.com",
            "alice (comment)@example.com",
            "\"alice\"@example.com",
        ] {
            assert!(!valid_email(email), "{email}");
        }
    }

    #[test]
    fn invitation_tokens_use_sha256_hex() {
        assert_eq!(
            token_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn role_inputs_accept_protobuf_names_and_numbers() {
        assert_eq!(
            request_roles(&json!({"roles":["ROLE_ADMIN",3,"ROLE_MEMBER"]})).unwrap(),
            vec![2, 3, 4]
        );
        assert!(request_roles(&json!({"roles":["ROLE_INVALID"]})).is_err());
    }
}
