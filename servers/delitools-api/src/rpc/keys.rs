// SPDX-License-Identifier: FSL-1.1-ALv2
//! API key lifecycle: `CreateKey`, `ListKeys` and `RevokeKey`.
//!
//! Key management requires a Logto user token. Personal keys need current tenant membership of the
//! acting user; service keys need the Owner or Admin role. Both checks go to the current tenant
//! authority on every call. Keys that do not exist, belong to another tenant or to another user
//! are reported as `FORBIDDEN` so the API does not reveal which key IDs exist.
use super::{canonical_tenant, now_checked};
use crate::{
    authority::ManagementScope,
    error::{Fault, Result},
    keys::{self, Capabilities},
    platform::Platform,
    store::{
        self,
        audit::{self, EventCategory, Severity},
        keys::{Listing, NewKey},
    },
};
use chrono::{DateTime, Utc};
use delino_protocol::delitools::v1::*;
use uuid::Uuid;

const DEFAULT_PAGE: u32 = 50;
const MAX_PAGE: u32 = 100;

fn scope_for(kind: KeyKind) -> Option<ManagementScope> {
    match kind {
        KeyKind::KEY_KIND_PERSONAL => Some(ManagementScope::PersonalKeys),
        KeyKind::KEY_KIND_SERVICE => Some(ManagementScope::ServiceKeys),
        KeyKind::KEY_KIND_UNSPECIFIED => None,
    }
}

fn kind_label(kind: KeyKind) -> &'static str {
    match kind {
        KeyKind::KEY_KIND_PERSONAL => "personal",
        KeyKind::KEY_KIND_SERVICE => "service",
        KeyKind::KEY_KIND_UNSPECIFIED => "unspecified",
    }
}

fn expiry(request: &CreateKeyRequest, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>> {
    let Some(timestamp) = request.expires_at.as_option() else {
        return Ok(None);
    };
    let nanos = u32::try_from(timestamp.nanos)
        .map_err(|_| Fault::invalid_input("expiresAt is not a valid timestamp"))?;
    let at = DateTime::from_timestamp(timestamp.seconds, nanos)
        .ok_or_else(|| Fault::invalid_input("expiresAt is not a valid timestamp"))?;
    if at <= now {
        return Err(Fault::invalid_input("expiresAt must be in the future"));
    }
    Ok(Some(at))
}

impl Platform {
    pub async fn create_key(
        &self,
        request_id: &str,
        authorization: &str,
        request: CreateKeyRequest,
    ) -> Result<CreateKeyResponse> {
        let user = self.authenticate_user(authorization).await?;
        let tenant = canonical_tenant(&request.tenant_id)?;
        let kind = request
            .kind
            .as_known()
            .filter(|k| *k != KeyKind::KEY_KIND_UNSPECIFIED)
            .ok_or_else(|| Fault::invalid_input("kind must be personal or service"))?;
        let capabilities = Capabilities::from_wire(&request.capabilities).ok_or_else(|| {
            Fault::invalid_input("capabilities must list translator and/or extractor once each")
        })?;
        // Reject a malformed or already past expiry before the authority round trip.
        expiry(&request, now_checked())?;
        self.authorize_management(
            tenant,
            user.user_id,
            scope_for(kind).expect("kind validated above"),
        )
        .await?;
        // The authority request can be slow. Take the creation time and judge the expiry after it
        // completes, so a key is never created already expired and `created_at` is the real
        // creation instant.
        let now = now_checked();
        let expires_at = expiry(&request, now)?;
        let generated = keys::generate();
        let row = store::keys::insert(
            &self.store.pool,
            NewKey {
                id: generated.id,
                tenant_id: tenant,
                user_id: user.user_id,
                kind,
                capabilities,
                secret_hash: &generated.hash,
                expires_at,
            },
            now,
        )
        .await?;
        tracing::info!(
            request_id,
            key_id = %row.id,
            kind = kind_label(kind),
            "api key created"
        );
        audit::record_event(
            &self.store.pool,
            EventCategory::KeyCreated,
            Severity::Info,
            request_id.parse().ok(),
            request_id.parse().unwrap_or_default(),
            serde_json::json!({"keyId": row.id, "kind": kind_label(kind)}),
        )
        .await;
        Ok(CreateKeyResponse {
            request_id: request_id.to_owned(),
            key: Some(row.to_proto()).into(),
            // The only time the secret leaves the service.
            secret: generated.token,
            ..Default::default()
        })
    }

    pub async fn list_keys(
        &self,
        request_id: &str,
        authorization: &str,
        request: ListKeysRequest,
    ) -> Result<ListKeysResponse> {
        let user = self.authenticate_user(authorization).await?;
        let tenant = canonical_tenant(&request.tenant_id)?;
        let size = match request.page_size {
            0 => DEFAULT_PAGE,
            n if n <= MAX_PAGE => n,
            _ => return Err(Fault::invalid_input("pageSize must be 1 to 100")),
        };
        let after = if request.after_id.is_empty() {
            None
        } else if server_common::canonical_id(&request.after_id) {
            Some(Uuid::parse_str(&request.after_id).expect("canonical"))
        } else {
            return Err(Fault::invalid_input("afterId is not a valid cursor"));
        };
        let kind = request
            .kind
            .as_known()
            .ok_or_else(|| Fault::invalid_input("kind is not recognized"))?;
        let listing = match kind {
            KeyKind::KEY_KIND_PERSONAL => {
                self.authorize_management(tenant, user.user_id, ManagementScope::PersonalKeys)
                    .await?;
                Listing::PersonalOf(user.user_id)
            }
            KeyKind::KEY_KIND_SERVICE => {
                self.authorize_management(tenant, user.user_id, ManagementScope::ServiceKeys)
                    .await?;
                Listing::Service
            }
            // Without a filter, list what the caller may manage: own personal keys always, and
            // tenant service keys only when the caller is Owner or Admin.
            KeyKind::KEY_KIND_UNSPECIFIED => {
                self.authorize_management(tenant, user.user_id, ManagementScope::PersonalKeys)
                    .await?;
                match self
                    .authorize_management(tenant, user.user_id, ManagementScope::ServiceKeys)
                    .await
                {
                    Ok(()) => Listing::Both(user.user_id),
                    Err(fault) if fault.reason == ErrorReason::ERROR_REASON_FORBIDDEN => {
                        Listing::PersonalOf(user.user_id)
                    }
                    Err(fault) => return Err(fault),
                }
            }
        };
        let mut rows = store::keys::list(
            &self.store.pool,
            tenant,
            listing,
            after,
            i64::from(size) + 1,
        )
        .await?;
        let more = rows.len() > size as usize;
        rows.truncate(size as usize);
        let next_after_id = if more {
            rows.last().map(|k| k.id.to_string()).unwrap_or_default()
        } else {
            String::new()
        };
        Ok(ListKeysResponse {
            request_id: request_id.to_owned(),
            keys: rows.iter().map(|k| k.to_proto()).collect(),
            next_after_id,
            ..Default::default()
        })
    }

    pub async fn revoke_key(
        &self,
        request_id: &str,
        authorization: &str,
        request: RevokeKeyRequest,
    ) -> Result<RevokeKeyResponse> {
        let user = self.authenticate_user(authorization).await?;
        let tenant = canonical_tenant(&request.tenant_id)?;
        if !server_common::canonical_id(&request.key_id) {
            return Err(Fault::invalid_input("keyId must be a UUID v7"));
        }
        let key_id = Uuid::parse_str(&request.key_id).expect("canonical");
        // A key outside this tenant is indistinguishable from a missing key.
        let key = store::keys::get_in_tenant(&self.store.pool, tenant, key_id)
            .await?
            .ok_or_else(Fault::forbidden)?;
        match key.kind {
            KeyKind::KEY_KIND_PERSONAL => {
                // Users manage only their own personal keys, and only with current membership.
                if key.user_id != user.user_id {
                    return Err(Fault::forbidden());
                }
                self.authorize_management(tenant, user.user_id, ManagementScope::PersonalKeys)
                    .await?;
            }
            _ => {
                self.authorize_management(tenant, user.user_id, ManagementScope::ServiceKeys)
                    .await?;
            }
        }
        // Revocation blocks new requests and retries immediately. A call already running may
        // finish, and its usage remains billable.
        store::keys::revoke(&self.store.pool, key.id, now_checked()).await?;
        tracing::info!(request_id, key_id = %key.id, "api key revoked");
        audit::record_event(
            &self.store.pool,
            EventCategory::KeyRevoked,
            Severity::Info,
            request_id.parse().ok(),
            request_id.parse().unwrap_or_default(),
            serde_json::json!({"keyId": key.id, "kind": kind_label(key.kind)}),
        )
        .await;
        Ok(RevokeKeyResponse {
            request_id: request_id.to_owned(),
            ..Default::default()
        })
    }
}
