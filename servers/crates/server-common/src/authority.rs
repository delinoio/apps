// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{Fault, Result, canonical_id, string};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TenantState {
    Unknown,
    Active,
    Blocked,
    Deleting,
    Deleted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    View,
    Edit,
    Manage,
}
pub struct Authority {
    pub http: reqwest::Client,
    pub url: String,
    token_url: String,
    resource: String,
    client_id: String,
    secret: String,
    token: Mutex<Option<(String, Instant)>>,
}
impl Authority {
    pub fn from_env(timeout: Duration) -> anyhow::Result<Self> {
        use crate::config::{required, safe_url};
        let url = required("DELCORE_URL")?;
        let token_url = required("LOGTO_TOKEN_URL")?;
        anyhow::ensure!(
            safe_url(&url) && safe_url(&token_url),
            "Delcore and token endpoints must be https or loopback http URLs"
        );
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(timeout)
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            url,
            token_url,
            resource: required("DELCORE_RESOURCE")?,
            client_id: required("CLIENT_ID")?,
            secret: required("CLIENT_SECRET")?,
            token: Mutex::new(None),
        })
    }
    pub fn new(
        http: reqwest::Client,
        url: String,
        token_url: String,
        resource: String,
        client_id: String,
        secret: String,
    ) -> Self {
        Self {
            http,
            url,
            token_url,
            resource,
            client_id,
            secret,
            token: Mutex::new(None),
        }
    }
    pub async fn service_token(&self) -> Result<String> {
        let mut cache = self.token.lock().await;
        if let Some((token, expiry)) = &*cache
            && Instant::now() < *expiry
        {
            return Ok(token.clone());
        }
        let response = self
            .http
            .post(&self.token_url)
            .basic_auth(&self.client_id, Some(&self.secret))
            .form(&[
                ("grant_type", "client_credentials"),
                ("resource", self.resource.as_str()),
                ("scope", "authorize"),
            ])
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Fault::unavailable());
        }
        let body = bounded_json(response, 64 << 10).await?;
        let token = string(&body, "access_token");
        if token.is_empty() {
            return Err(Fault::unavailable());
        }
        let expiry = body["expires_in"].as_u64().unwrap_or(0);
        if expiry > 60
            && let Some(at) = Instant::now().checked_add(Duration::from_secs(expiry - 30))
        {
            *cache = Some((token.into(), at));
        }
        Ok(token.into())
    }
    pub async fn call(&self, method: &str, authorization: &str, body: Value) -> Result<Value> {
        let response = self
            .http
            .post(format!(
                "{}/delcore.v1.DelcoreService/{method}",
                self.url.trim_end_matches('/')
            ))
            .header("Authorization", authorization)
            .header("Connect-Protocol-Version", "1")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let value = bounded_json(response, 8 << 20).await?;
        if !status.is_success() {
            return Err(match string(&value, "code") {
                "unauthenticated" => Fault::unauthenticated(),
                "permission_denied" => Fault::denied(),
                _ => Fault::unavailable(),
            });
        }
        Ok(value)
    }
    pub async fn identify(&self, authorization: &str) -> Result<String> {
        let value = self
            .call("GetCurrentUser", authorization, json!({"page":{"size":1}}))
            .await?;
        let user = string(&value, "userId");
        if !canonical_id(user) {
            return Err(Fault::unavailable());
        }
        Ok(user.into())
    }
    /// Requires Delcore to confirm the caller is a current tenant Owner or Admin.
    ///
    /// Delcore stays the only role authority: a successful `ListMembers` call is the proof,
    /// so the reply must be a well-formed `ListMembersResponse` ProtoJSON object. Default
    /// values may be omitted (`{}` is valid), but a reply of any other shape is malformed and
    /// fails closed as `unavailable`. The response body is never logged.
    pub async fn manager(&self, authorization: &str, tenant: &str) -> Result<()> {
        let value = self
            .call(
                "ListMembers",
                authorization,
                json!({"tenantId":tenant,"page":{"size":1}}),
            )
            .await?;
        // The generated decoder accepts a top-level array as an empty message, so the
        // object check must come first.
        if !value.is_object()
            || serde_json::from_value::<delino_protocol::delcore::v1::ListMembersResponse>(value)
                .is_err()
        {
            tracing::warn!(
                reason = "malformed_list_members_response",
                "Delcore returned a malformed membership response"
            );
            return Err(Fault::unavailable());
        }
        Ok(())
    }
    pub async fn states(&self, ids: &[String]) -> Result<Vec<TenantState>> {
        let authorization = format!("Bearer {}", self.service_token().await?);
        let value = self
            .call("GetTenantStates", &authorization, json!({"tenantIds":ids}))
            .await
            .map_err(|_| Fault::unavailable())?;
        let states = value["states"].as_array().ok_or_else(Fault::unavailable)?;
        if states.len() != ids.len() {
            return Err(Fault::unavailable());
        }
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for (entry, id) in states.iter().zip(ids) {
            if string(entry, "tenantId") != id || !seen.insert(id) {
                return Err(Fault::unavailable());
            }
            result.push(match string(entry, "state") {
                "TENANT_STATE_ACTIVE" => TenantState::Active,
                "TENANT_STATE_BLOCKED" => TenantState::Blocked,
                "TENANT_STATE_DELETING" => TenantState::Deleting,
                "TENANT_STATE_DELETED" => TenantState::Deleted,
                "TENANT_STATE_UNKNOWN" => TenantState::Unknown,
                _ => return Err(Fault::unavailable()),
            });
        }
        Ok(result)
    }
    pub async fn allow(&self, authorization: &str, space: &str, role: Role) -> Result<()> {
        let user = self.identify(authorization).await?;
        let token = format!("Bearer {}", self.service_token().await?);
        let roles: &[&str] = match role {
            Role::View => &["view", "edit", "manage"],
            Role::Edit => &["edit", "manage"],
            Role::Manage => &["manage"],
        };
        for capability in roles {
            let v = self
                .call(
                    "Authorize",
                    &token,
                    json!({"tenantId":space,"userId":user,"capability":capability,"paid":false}),
                )
                .await
                .map_err(|_| Fault::unavailable())?;
            if v["allowed"] == true {
                return Ok(());
            }
        }
        Err(Fault::denied())
    }
    pub async fn finalized(&self, ids: &[String]) -> Result<Vec<String>> {
        let token = format!("Bearer {}", self.service_token().await?);
        let v = self
            .call("CheckTenantLifecycle", &token, json!({"tenantIds":ids}))
            .await?;
        let result: Vec<String> =
            serde_json::from_value(v.get("finalizedTenantIds").cloned().unwrap_or(json!([])))?;
        if result.iter().any(|id| !ids.contains(id)) {
            return Err(Fault::unavailable());
        }
        Ok(result)
    }
}
pub async fn bounded_json(mut response: reqwest::Response, limit: usize) -> Result<Value> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > limit {
            return Err(Fault::unavailable());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| Fault::unavailable())
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, http::HeaderMap, http::StatusCode, routing::post};
    use connectrpc::ErrorCode;
    use std::sync::{Arc, Mutex};

    type Reply = (StatusCode, String);

    /// Starts a loopback Delcore mock whose `ListMembers` reply is controlled by the test and
    /// records the forwarded authorization header and request body.
    async fn mock(reply: Arc<Mutex<Reply>>, seen: Arc<Mutex<Vec<(String, Value)>>>) -> Authority {
        let app = Router::new().route(
            "/delcore.v1.DelcoreService/ListMembers",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let reply = reply.clone();
                let seen = seen.clone();
                async move {
                    let authorization = headers
                        .get("authorization")
                        .and_then(|h| h.to_str().ok())
                        .unwrap_or_default()
                        .to_owned();
                    seen.lock().unwrap().push((authorization, body));
                    let (status, body) = reply.lock().unwrap().clone();
                    (status, [("content-type", "application/json")], body)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Authority::new(
            reqwest::Client::new(),
            url,
            "http://127.0.0.1:1/token".into(),
            "resource".into(),
            "client".into(),
            "secret".into(),
        )
    }

    #[tokio::test]
    async fn manager_requires_a_listmembers_response_object() {
        let reply = Arc::new(Mutex::new((StatusCode::OK, String::new())));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let authority = mock(reply.clone(), seen.clone()).await;
        let tenant = "01900000-0000-7000-8000-000000000002";
        let valid = [
            "{}",
            r#"{"members":[]}"#,
            r#"{"members":null}"#,
            r#"{"metadata":null,"nextAfterId":null}"#,
            r#"{"members":[{"userId":"01900000-0000-7000-8000-000000000001","roles":["ROLE_OWNER"]}],"nextAfterId":"next"}"#,
        ];
        for body in valid {
            *reply.lock().unwrap() = (StatusCode::OK, body.into());
            authority
                .manager("Bearer caller", tenant)
                .await
                .unwrap_or_else(|e| panic!("{body} must be accepted: {e:?}"));
        }
        let malformed = [
            "[]",
            "42",
            "true",
            "null",
            r#""text""#,
            r#"{"members":42}"#,
            r#"{"members":[null]}"#,
            r#"{"members":[42]}"#,
            r#"{"nextAfterId":42}"#,
            "{",
            "",
        ];
        for body in malformed {
            *reply.lock().unwrap() = (StatusCode::OK, body.into());
            let fault = authority
                .manager("Bearer caller", tenant)
                .await
                .expect_err(body);
            assert_eq!(fault.code, ErrorCode::Unavailable, "{body}");
            assert_eq!(fault.reason, 9, "{body}");
        }
        // The caller token and page size are forwarded unchanged on every call.
        for (authorization, body) in seen.lock().unwrap().iter() {
            assert_eq!(authorization, "Bearer caller");
            assert_eq!(body, &json!({"tenantId":tenant,"page":{"size":1}}));
        }
    }

    #[tokio::test]
    async fn manager_keeps_non_success_status_mapping() {
        let reply = Arc::new(Mutex::new((StatusCode::OK, String::new())));
        let authority = mock(reply.clone(), Arc::default()).await;
        let cases = [
            (
                StatusCode::UNAUTHORIZED,
                r#"{"code":"unauthenticated"}"#,
                ErrorCode::Unauthenticated,
            ),
            (
                StatusCode::FORBIDDEN,
                r#"{"code":"permission_denied"}"#,
                ErrorCode::PermissionDenied,
            ),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                r#"{"code":"unavailable"}"#,
                ErrorCode::Unavailable,
            ),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "[]",
                ErrorCode::Unavailable,
            ),
        ];
        for (status, body, code) in cases {
            *reply.lock().unwrap() = (status, body.into());
            let fault = authority
                .manager("Bearer caller", "01900000-0000-7000-8000-000000000002")
                .await
                .expect_err(body);
            assert_eq!(fault.code, code, "{status} {body}");
        }
    }
}
