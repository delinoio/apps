// SPDX-License-Identifier: FSL-1.1-ALv2
pub mod alerts;
pub mod auth;
pub mod decimal;
pub mod github;
pub mod maintenance;
pub mod measurement;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use connectrpc::{RequestContext, Response, ServiceRequest, ServiceResult};
use delino_protocol::gittracker::v1::*;
use measurement::State;
use rand::RngCore;
use serde_json::{Value, json};
use server_common::{
    Fault, Result,
    authority::{Authority, Role},
    now, string, uuid,
};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool, Row};
use std::collections::HashSet;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Upload,
    Complete,
    ConnectRepository,
    ListRepositories,
    GetHistory,
    GetExecution,
    Compare,
    IssueToken,
    RevokeToken,
    Delete,
    Disconnect,
}
impl Op {
    fn role(self) -> Role {
        match self {
            Self::Upload | Self::Complete => Role::Edit,
            Self::IssueToken
            | Self::RevokeToken
            | Self::Delete
            | Self::Disconnect
            | Self::ConnectRepository => Role::Manage,
            _ => Role::View,
        }
    }
}
pub struct Server {
    pub metrics: std::sync::Arc<server_common::telemetry::Metrics>,
    pub db: PgPool,
    pub authority: Authority,
    pub github: github::GitHub,
    pub actions: auth::Actions,
    pub audit_retention: chrono::Duration,
    pub webhook_secret: String,
}
pub fn valid_sha(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_context(c: &Context) -> Result<()> {
    if c.repository_id.is_empty()
        || c.execution_id.is_empty()
        || c.execution_id.len() > 200
        || !valid_sha(&c.commit)
        || c.pull_request < 0
        || c.branch.is_empty()
        || (!c.baseline_commit.is_empty() && !valid_sha(&c.baseline_commit))
    {
        return Err(Fault::invalid(
            "repository, execution, commit and branch are required",
        ));
    }
    Ok(())
}
pub fn repository_row(row: &sqlx::postgres::PgRow) -> Repository {
    Repository {
        id: row.get::<uuid::Uuid, _>("id").to_string(),
        github_repository_id: row.get::<i64, _>("github_id").to_string(),
        space_id: row.get::<uuid::Uuid, _>("space_id").to_string(),
        full_name: row.get("full_name"),
        installation_id: row.get("installation_id"),
        public: row.get("public"),
        connected: row.get("connected"),
        ..Default::default()
    }
}
pub async fn repository(db: &mut PgConnection, id: &str) -> Result<(Repository, State)> {
    let row = sqlx::query("SELECT * FROM git_tracker_repositories WHERE id=$1 FOR UPDATE")
        .bind(uuid(id)?)
        .fetch_one(db)
        .await?;
    let state: Value = row.get("state");
    Ok((repository_row(&row), serde_json::from_value(state)?))
}
pub async fn audit(db: &mut PgConnection, space: &str, action: &str, trace: &str) -> Result<()> {
    sqlx::query("INSERT INTO git_tracker_audit VALUES($1,$2,$3,$4,$5)")
        .bind(uuid::Uuid::now_v7())
        .bind(uuid(space)?)
        .bind(action)
        .bind(uuid(trace)?)
        .bind(now())
        .execute(db)
        .await?;
    Ok(())
}
impl Server {
    pub fn record_failure(&self, fault: &Fault) {
        if fault.source == server_common::error::FaultSource::Persistence {
            self.metrics
                .events
                .with_label_values(&["persistence", "failure"])
                .inc();
        }
    }
    async fn handle<R: serde::de::DeserializeOwned>(
        &self,
        ctx: RequestContext,
        op: Op,
        q: Value,
    ) -> ServiceResult<R> {
        let trace = server_common::id();
        let authorization = ctx
            .headers()
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        match self.execute(authorization, op, q, &trace).await {
            Ok(out) => Ok(Response::new(
                serde_json::from_value(out).map_err(|_| Fault::unavailable().connect())?,
            )
            .with_header("X-Correlation-Id", trace)
            .with_header("Cache-Control", "no-store")),
            Err(f) => {
                self.record_failure(&f);
                tracing::warn!(correlation_id=%trace,category=?f.code,"request failed");
                let mut headers = axum::http::HeaderMap::new();
                headers.insert("X-Correlation-Id", trace.parse().unwrap());
                headers.insert("Cache-Control", "no-store".parse().unwrap());
                Err(f.connect().with_headers(headers))
            }
        }
    }
    async fn execute(&self, authorization: &str, op: Op, q: Value, trace: &str) -> Result<Value> {
        if op == Op::ConnectRepository {
            return self.connect_repository(authorization, &q, trace).await;
        }
        if op == Op::ListRepositories {
            let space = string(&q, "spaceId");
            if !space.is_empty() {
                self.authority
                    .allow(authorization, space, Role::View)
                    .await?
            }
            let rows=sqlx::query("SELECT * FROM git_tracker_repositories WHERE ($1='' AND public AND connected) OR space_id::text=$1 ORDER BY full_name").bind(space).fetch_all(&self.db).await?;
            return Ok(json!({"repositories":rows.iter().map(repository_row).collect::<Vec<_>>()}));
        }
        let context: Option<Context> = q
            .get("context")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?;
        if matches!(op, Op::Upload | Op::Complete) {
            valid_context(
                context
                    .as_ref()
                    .ok_or_else(|| Fault::invalid("context is required"))?,
            )?
        }
        let rid = context
            .as_ref()
            .map(|c| c.repository_id.as_str())
            .unwrap_or_else(|| string(&q, "repositoryId"));
        uuid(rid)?;
        let mut tx = self.db.begin().await?;
        let (repo, mut state) = repository(&mut tx, rid).await?;
        self.authorize(&mut tx, authorization, &repo, op.role(), context.as_ref())
            .await?;
        if op.role() == Role::Edit && !repo.connected {
            return Err(Fault::precondition("repository is disconnected"));
        }
        let interrupted = state.interrupt(now());
        let mut out = json!({});
        match op {
            Op::Upload => {
                let c = context.as_ref().unwrap();
                let d: Dataset =
                    serde_json::from_value(q.get("dataset").cloned().unwrap_or(json!({})))?;
                let rows: Vec<delino_protocol::gittracker::v1::Row> =
                    serde_json::from_value(q.get("rows").cloned().unwrap_or(json!([])))?;
                let (parents, base) = if state.retry(c, &d, &rows) {
                    (vec![], String::new())
                } else {
                    self.github.resolve(&repo, c).await?
                };
                state.upload(c, &d, &rows, now(), parents, base)?;
                let r = &state.records[&c.execution_id];
                out = json!({"execution":r.execution,"comparisons":state.comparisons(r,None)});
            }
            Op::Complete => {
                let c = context.as_ref().unwrap();
                let completed = state.records.get(&c.execution_id).is_some_and(|r| {
                    r.execution.state == ExecutionState::EXECUTION_STATE_COMPLETED
                });
                let parents = if completed {
                    vec![]
                } else {
                    self.github.resolve(&repo, c).await?.0
                };
                state.complete(c, now())?;
                let r = state.records.get_mut(&c.execution_id).unwrap();
                if parents.len() > r.ancestors.len() {
                    r.ancestors = parents
                }
                out = json!({"execution":r.execution});
            }
            Op::GetExecution => {
                let r = state
                    .records
                    .get(string(&q, "executionId"))
                    .ok_or_else(Fault::missing)?;
                out = json!({"execution":r.execution,"comparisons":state.comparisons(r,None)});
            }
            Op::Compare => {
                let r = state
                    .representative(string(&q, "targetCommit"), None, false)
                    .ok_or_else(Fault::missing)?;
                let baseline = state.representative(string(&q, "baselineCommit"), None, false);
                out = json!({"comparisons":state.comparisons(r,Some(baseline))});
            }
            Op::GetHistory => {
                out = self.history(&repo, &state, &q).await?;
            }
            Op::IssueToken => {
                let expiry = DateTime::parse_from_rfc3339(string(&q, "expiresAt"))
                    .map_err(|_| Fault::invalid("future expiration is required"))?
                    .with_timezone(&Utc);
                if expiry <= now() {
                    return Err(Fault::invalid("future expiration is required"));
                }
                let mut bytes = [0; 32];
                rand::rng().fill_bytes(&mut bytes);
                let token = format!("gt_{}", URL_SAFE_NO_PAD.encode(bytes));
                let id = uuid::Uuid::now_v7();
                sqlx::query("INSERT INTO git_tracker_tokens(id,repository_id,digest,expires_at) VALUES($1,$2,$3,$4)").bind(id).bind(uuid(rid)?).bind(Sha256::digest(token.as_bytes()).to_vec()).bind(expiry).execute(&mut *tx).await?;
                audit(&mut tx, &repo.space_id, "token_issued", trace).await?;
                out = json!({"tokenId":id,"token":token});
            }
            Op::RevokeToken => {
                sqlx::query("UPDATE git_tracker_tokens SET revoked=true WHERE id::text=$1 AND repository_id=$2").bind(string(&q,"tokenId")).bind(uuid(rid)?).execute(&mut *tx).await?;
                audit(&mut tx, &repo.space_id, "token_revoked", trace).await?;
            }
            Op::Disconnect => {
                sqlx::query(
                    "UPDATE git_tracker_repositories SET connected=false,public=false WHERE id=$1",
                )
                .bind(uuid(rid)?)
                .execute(&mut *tx)
                .await?;
                audit(&mut tx, &repo.space_id, "repository_disconnected", trace).await?;
            }
            Op::Delete => {
                if q["confirmed"] != true {
                    return Err(Fault::invalid("deletion confirmation is required"));
                }
                match string(&q, "kind") {
                    "DELETE_KIND_REPOSITORY" => {
                        state
                            .deleted
                            .extend(state.records.keys().map(|id| (id.clone(), true)));
                        state.records.clear();
                    }
                    "DELETE_KIND_EXECUTION" => {
                        let id = string(&q, "id");
                        state.deleted.insert(id.into(), true);
                        state.records.remove(id);
                    }
                    "DELETE_KIND_DATASET" => {
                        let id = string(&q, "id");
                        for r in state.records.values_mut() {
                            r.removed.insert(id.into(), true);
                            r.execution.measurements.retain(|m| m.dataset.id != id);
                        }
                    }
                    _ => return Err(Fault::invalid("invalid deletion kind")),
                }
                audit(&mut tx, &repo.space_id, "measurements_deleted", trace).await?;
            }
            Op::ConnectRepository | Op::ListRepositories => unreachable!(),
        }
        sqlx::query("UPDATE git_tracker_repositories SET state=$2 WHERE id=$1")
            .bind(uuid(rid)?)
            .bind(serde_json::to_value(&state)?)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        if interrupted > 0 {
            tracing::info!(count = interrupted, "executions interrupted")
        }
        if op.role() == Role::Edit {
            let c = context.as_ref().unwrap();
            if let Err(fault) = self.publish(rid, &c.execution_id).await {
                self.record_failure(&fault);
                out["warnings"] = json!(["PR comment could not be updated"])
            }
            tracing::info!(
                correlation_id = trace,
                execution_id = c.execution_id,
                "measurement saved"
            );
        }
        Ok(out)
    }
    async fn publish(&self, rid: &str, execution: &str) -> Result<()> {
        // Re-read under the writer lock so delayed requests cannot replace a newer PR comment.
        let mut tx = self.db.begin().await?;
        let (repo, state) = repository(&mut tx, rid).await?;
        if let Some(record) = state.records.get(execution) {
            self.github.comment(&repo, &state, record).await?
        }
        Ok(())
    }
    async fn connect_repository(&self, h: &str, q: &Value, trace: &str) -> Result<Value> {
        let space = string(q, "spaceId");
        let space_id = uuid(space)?;
        self.authority.allow(h, space, Role::Manage).await?;
        self.authority.manager(h, space).await?;
        let actor = self.authority.identify(h).await?;
        let token = string(q, "githubAccessToken");
        let user = self.github.user(token).await.map_err(|_| Fault::denied())?;
        let mut r = self
            .github
            .connect(string(q, "fullName"), token)
            .await
            .map_err(|_| Fault::denied())?;
        r.space_id = space.into();
        let mut tx = self.db.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(&r.github_repository_id)
            .execute(&mut *tx)
            .await?;
        let github_id: i64 = r
            .github_repository_id
            .parse()
            .map_err(|_| Fault::unavailable())?;
        let active:Option<uuid::Uuid>=sqlx::query_scalar("SELECT space_id FROM git_tracker_repositories WHERE github_id=$1 AND connected FOR UPDATE").bind(github_id).fetch_optional(&mut *tx).await?;
        if active.is_some_and(|id| id != space_id) {
            return Err(Fault::exists());
        }
        let id:uuid::Uuid=sqlx::query_scalar("INSERT INTO git_tracker_repositories(id,github_id,space_id,full_name,installation_id,public) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(github_id,space_id) DO UPDATE SET full_name=EXCLUDED.full_name,installation_id=EXCLUDED.installation_id,public=EXCLUDED.public,connected=true RETURNING id") .bind(uuid::Uuid::now_v7()).bind(github_id).bind(space_id).bind(&r.full_name).bind(&r.installation_id).bind(r.public).fetch_one(&mut *tx).await?;
        r.id = id.to_string();
        sqlx::query("INSERT INTO git_tracker_links(space_id,user_id,github_id) VALUES($1,$2,$3) ON CONFLICT(space_id,user_id) DO UPDATE SET github_id=EXCLUDED.github_id").bind(space_id).bind(uuid(&actor)?).bind(user).execute(&mut *tx).await?;
        audit(&mut tx, space, "repository_connected", trace).await?;
        tx.commit().await?;
        Ok(json!({"repository":r}))
    }
    async fn history(&self, repo: &Repository, state: &State, q: &Value) -> Result<Value> {
        fn date(q: &Value, key: &str) -> Result<Option<DateTime<Utc>>> {
            let s = string(q, key);
            if s.is_empty() {
                Ok(None)
            } else {
                Ok(Some(
                    DateTime::parse_from_rfc3339(s)
                        .map_err(|_| Fault::invalid("invalid date range"))?
                        .with_timezone(&Utc),
                ))
            }
        }
        let since = date(q, "since")?;
        let until = date(q, "until")?;
        let cursor = string(q, "cursor");
        let offset = if cursor.is_empty() {
            0
        } else {
            cursor
                .parse::<usize>()
                .map_err(|_| Fault::invalid("invalid cursor"))?
        };
        let branch = string(q, "branch");
        let mut records: Vec<_> = state
            .records
            .values()
            .filter(|r| {
                let at = DateTime::parse_from_rfc3339(&r.execution.registered_at).ok();
                (branch.is_empty() || r.execution.context.branch == branch)
                    && since.is_none_or(|s| at.is_some_and(|t| t >= s))
                    && until.is_none_or(|u| at.is_some_and(|t| t <= u))
            })
            .collect();
        records.sort_by(|a, b| {
            b.execution
                .registration_order
                .cmp(&a.execution.registration_order)
        });
        let mut commits = records
            .iter()
            .find(|r| r.execution.context.pull_request == 0)
            .map(|r| {
                std::iter::once(r.execution.context.commit.clone())
                    .chain(r.ancestors.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if repo.connected && !branch.is_empty() {
            let needed: HashSet<_> = records
                .iter()
                .filter(|r| {
                    r.execution.context.pull_request == 0
                        && r.execution.state == ExecutionState::EXECUTION_STATE_COMPLETED
                })
                .map(|r| r.execution.context.commit.clone())
                .collect();
            commits = self.github.branch(repo, branch, needed).await?;
        }
        let to = string(q, "toCommit");
        let from = string(q, "fromCommit");
        if !to.is_empty()
            && let Some(i) = commits.iter().position(|c| c == to)
        {
            commits = commits[i..].to_vec()
        }
        if !from.is_empty()
            && let Some(i) = commits.iter().position(|c| c == from)
        {
            commits.truncate(i + 1)
        }
        if !to.is_empty() || !from.is_empty() {
            records.retain(|r| commits.contains(&r.execution.context.commit))
        }
        let offset = offset.min(records.len());
        let end = offset.saturating_add(100).min(records.len());
        let next = if offset.saturating_add(100) <= records.len() {
            end.to_string()
        } else {
            String::new()
        };
        let executions: Vec<_> = records[offset..end]
            .iter()
            .map(|r| {
                let mut e = r.execution.clone();
                e.representative = state
                    .representative(&e.context.commit, None, false)
                    .is_some_and(|v| v.execution.registration_order == e.registration_order);
                let dataset = string(q, "datasetId");
                if !dataset.is_empty() {
                    e.measurements.retain(|m| m.dataset.id == dataset)
                }
                e
            })
            .collect();
        Ok(json!({"executions":executions,"nextCursor":next,"commits":commits}))
    }
}
// The generated trait permits borrowed responses; these handlers return owned messages.
#[allow(refining_impl_trait)]
impl GitTrackerService for Server {
    async fn upload(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, UploadRequest>,
    ) -> ServiceResult<UploadResponse> {
        self.handle(
            ctx,
            Op::Upload,
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn complete(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CompleteRequest>,
    ) -> ServiceResult<CompleteResponse> {
        self.handle(
            ctx,
            Op::Complete,
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn connect_repository(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ConnectRepositoryRequest>,
    ) -> ServiceResult<ConnectRepositoryResponse> {
        self.handle(
            ctx,
            Op::ConnectRepository,
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn list_repositories(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListRepositoriesRequest>,
    ) -> ServiceResult<ListRepositoriesResponse> {
        self.handle(
            ctx,
            Op::ListRepositories,
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn get_history(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetHistoryRequest>,
    ) -> ServiceResult<GetHistoryResponse> {
        self.handle(
            ctx,
            Op::GetHistory,
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn get_execution(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetExecutionRequest>,
    ) -> ServiceResult<GetExecutionResponse> {
        self.handle(
            ctx,
            Op::GetExecution,
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn compare(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CompareRequest>,
    ) -> ServiceResult<CompareResponse> {
        self.handle(
            ctx,
            Op::Compare,
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn issue_token(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, IssueTokenRequest>,
    ) -> ServiceResult<IssueTokenResponse> {
        self.handle(
            ctx,
            Op::IssueToken,
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn revoke_token(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, RevokeTokenRequest>,
    ) -> ServiceResult<RevokeTokenResponse> {
        self.handle(
            ctx,
            Op::RevokeToken,
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn delete(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DeleteRequest>,
    ) -> ServiceResult<DeleteResponse> {
        self.handle(
            ctx,
            Op::Delete,
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn disconnect(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DisconnectRequest>,
    ) -> ServiceResult<DisconnectResponse> {
        self.handle(
            ctx,
            Op::Disconnect,
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
}
