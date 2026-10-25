// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{
    Fault, Result, Server,
    auth::{self, Permission},
    hash, id, integer, stamp, string,
};
use buffa::Message;
use connectrpc::{RequestContext, Response, ServiceRequest, ServiceResult};
use delino_protocol::perfine::v1::*;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};

// Upload completion establishes evidence availability, not remote job termination.
fn complete_evidence(experiment: &mut Value) {
    experiment["evidenceCompletion"] = json!("COMPLETION_STATE_COMPLETE");
    if experiment["origin"] != "EXECUTION_ORIGIN_GITHUB_ACTIONS" {
        experiment["executionState"] = json!("EXECUTION_STATE_COMPLETED");
    }
}

struct RejectedUpload {
    id: uuid::Uuid,
    keys: Vec<String>,
}

fn permission(op: &str) -> Permission {
    match op {
        "ConnectRepository"
        | "DisconnectRepository"
        | "SetRawArtifactConsent"
        | "GetRawArtifactConsent"
        | "DeleteExperiment"
        | "GetDeletion" => Permission::Manage,
        "ApproveDraft" | "PublishDraft" | "GetApproval" | "GetPublication" => Permission::Publish,
        "BeginUpload"
        | "CompleteUpload"
        | "GetUpload"
        | "CheckUploadEligibility"
        | "PrepareExperiment"
        | "StartExperiment"
        | "ResumeExperiment"
        | "CancelExperiment"
        | "ListCloudTools" => Permission::Experiment,
        _ => Permission::Read,
    }
}
fn mutation(op: &str) -> bool {
    !op.starts_with("Get") && !op.starts_with("List") && op != "CheckUploadEligibility"
}
fn required<'a>(q: &'a Value, k: &str) -> Result<&'a Value> {
    q.get(k)
        .filter(|v| v.is_object())
        .ok_or_else(|| Fault::invalid("required message missing"))
}
fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && s != "."
        && s != ".."
}
fn sha(s: &str, n: usize) -> Result<()> {
    if s.len() != n
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Fault::invalid("invalid digest"));
    }
    Ok(())
}
async fn lock(db: &mut PgConnection, key: &str) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(key)
        .execute(db)
        .await?;
    Ok(())
}
async fn connection(db: &mut PgConnection, space: &str, rid: &str) -> Result<Value> {
    Ok(sqlx::query_scalar(
        "SELECT data FROM perfine_connections WHERE id=$1 AND space=$2 FOR UPDATE",
    )
    .bind(id(rid)?)
    .bind(id(space)?)
    .fetch_one(db)
    .await?)
}
async fn experiment(db: &mut PgConnection, space: &str, eid: &str) -> Result<Value> {
    tombstone(db, eid).await?;
    Ok(sqlx::query_scalar(
        "SELECT data FROM perfine_experiments WHERE id=$1 AND space=$2 FOR UPDATE",
    )
    .bind(id(eid)?)
    .bind(id(space)?)
    .fetch_one(db)
    .await?)
}
async fn tombstone(db: &mut PgConnection, eid: &str) -> Result<()> {
    if sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM perfine_tombstones WHERE id=$1)")
        .bind(id(eid)?)
        .fetch_one(db)
        .await?
    {
        return Err(Fault::precondition("experiment deleted"));
    }
    Ok(())
}
async fn save_experiment(db: &mut PgConnection, e: &Value) -> Result<()> {
    sqlx::query("UPDATE perfine_experiments SET data=$2 WHERE id=$1")
        .bind(id(string(e, "id"))?)
        .bind(e)
        .execute(db)
        .await?;
    Ok(())
}
fn inactive(c: &Value) -> Result<()> {
    if c["state"] != "CONNECTION_STATE_CONNECTED" {
        Err(Fault::precondition("connection inactive"))
    } else {
        Ok(())
    }
}
impl Server {
    pub async fn handle<R: serde::de::DeserializeOwned>(
        &self,
        ctx: RequestContext,
        op: &str,
        q: Value,
    ) -> ServiceResult<R> {
        let trace = if id(string(&q, "requestId")).is_err() {
            uuid::Uuid::now_v7().to_string()
        } else {
            string(&q, "requestId").into()
        };
        let h = ctx
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let gh = ctx
            .headers()
            .get("x-github-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let out = self.execute(h, gh, op, q, &trace).await;
        match out {
            Ok(mut out) => {
                self.metrics
                    .completed
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                out["requestId"] = json!(trace);
                Ok(Response::new(
                    serde_json::from_value(out).map_err(|_| Fault::unavailable().connect())?,
                )
                .with_header("x-request-id", trace)
                .with_header("cache-control", "no-store"))
            }
            Err(f) => {
                self.metrics
                    .failed
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tracing::warn!(request_id=%trace,procedure=op,code=?f.code,"Perfine request failed");
                let reason = crate::error::reason(&f);
                let detail = ErrorDetail {
                    request_id: trace.clone(),
                    reason: reason.into(),
                    message: f.message.clone(),
                    ..Default::default()
                };
                let mut headers = axum::http::HeaderMap::new();
                headers.insert("x-request-id", trace.parse().unwrap());
                headers.insert("cache-control", "no-store".parse().unwrap());
                Err(f
                    .connect()
                    .with_detail(connectrpc::ErrorDetail::from_message(
                        "perfine.v1.ErrorDetail",
                        &detail,
                    ))
                    .with_headers(headers))
            }
        }
    }
    pub async fn execute(
        &self,
        h: &str,
        gh: &str,
        op: &str,
        q: Value,
        trace: &str,
    ) -> Result<Value> {
        let space = string(&q, "spaceId");
        id(space)?;
        id(trace)?;
        if mutation(op) || !string(&q, "requestId").is_empty() {
            id(string(&q, "requestId"))?;
        }
        if !h.starts_with("Actions ")
            && op == "BeginUpload"
            && q["origin"] == "EXECUTION_ORIGIN_GITHUB_ACTIONS"
        {
            return Err(Fault::denied());
        }
        if h.starts_with("Actions ") {
            return self.actions_execute(h, op, q, trace).await;
        }
        let actor = if op == "GetAuthorization" {
            let mut permissions = Vec::new();
            for (p, n) in [
                (Permission::Read, 1),
                (Permission::Experiment, 2),
                (Permission::Publish, 3),
                (Permission::Manage, 4),
            ] {
                match auth::authorize(&self.authority, h, space, p).await {
                    Ok(_) => permissions.push(n),
                    Err(e) if e.code == connectrpc::ErrorCode::PermissionDenied => {}
                    Err(e) => return Err(e),
                }
            }
            return Ok(json!({"authorization":{"spaceId":space,"permissions":permissions}}));
        } else {
            match auth::authorize(&self.authority, h, space, permission(op)).await {
                Err(e)
                    if e.code == connectrpc::ErrorCode::PermissionDenied
                        && matches!(op, "ListConnections" | "GetConnection") =>
                {
                    auth::authorize(&self.authority, h, space, Permission::Manage).await?
                }
                other => other?,
            }
        };
        self.authorized(&actor, gh, op, &q, trace, None).await
    }
    async fn authorized(
        &self,
        actor: &str,
        gh: &str,
        op: &str,
        q: &Value,
        trace: &str,
        actions_connection: Option<&str>,
    ) -> Result<Value> {
        let space = string(q, "spaceId");
        // Count admission before reserving a transaction connection. A second pool
        // acquisition under the space lock can deadlock an exhausted pool.
        if mutation(op)
            && !matches!(
                op,
                "DeleteExperiment"
                    | "DisconnectRepository"
                    | "CancelExperiment"
                    | "SetRawArtifactConsent"
            )
        {
            let count:i64=sqlx::query_scalar("INSERT INTO perfine_rate(space,requests) VALUES($1,1) ON CONFLICT(space) DO UPDATE SET requests=CASE WHEN perfine_rate.window_started<now()-interval '1 minute' THEN 1 ELSE perfine_rate.requests+1 END,window_started=CASE WHEN perfine_rate.window_started<now()-interval '1 minute' THEN now() ELSE perfine_rate.window_started END RETURNING requests").bind(id(space)?).fetch_one(&self.db).await?;
            if count > self.rate {
                return Err(Fault::quota());
            }
        }
        let mut tx = self.db.begin().await?;
        // Serialize space quota reservations and lifecycle mutations across all replicas.
        lock(&mut tx, space).await?;
        if let Some(cid) = actions_connection {
            // Authentication releases its initial row lock before admission. A reconnect
            // can replace the delegate in that gap, so bind the current grant to this
            // same space-serialized transaction as the upload and its idempotency check.
            let c = connection(&mut tx, space, cid).await?;
            inactive(&c)?;
            self.actions_delegate(&mut tx, space, cid).await?;
        }
        if let Some(eid) = q.get("experimentId").and_then(Value::as_str) {
            tombstone(&mut tx, eid).await.or_else(|e| {
                if matches!(op, "DeleteExperiment" | "GetDeletion") {
                    Ok(())
                } else {
                    Err(e)
                }
            })?;
        }
        let digest = hash(&serde_json::to_vec(q)?);
        if mutation(op) {
            let cached=sqlx::query("SELECT digest,response FROM perfine_dedup WHERE actor=$1 AND space=$2 AND method=$3 AND request=$4 AND created_at>now()-interval '30 days'").bind(actor).bind(id(space)?).bind(op).bind(id(trace)?).fetch_optional(&mut *tx).await?;
            // Upload/publication retries re-enter their state machines to recheck mutable authority and bindings.
            if let Some(row) = cached {
                if row.get::<String, _>("digest") != digest {
                    return Err(crate::error::domain(
                        Fault::conflict(),
                        ErrorReason::RequestIdConflict,
                    ));
                }
                if op == "PrepareExperiment" {
                    let c = connection(&mut tx, space, string(q, "connectionId")).await?;
                    inactive(&c)?;
                    self.github.current(&c).await?;
                    return Ok(row.get("response"));
                }
                if op == "ApproveDraft" {
                    let response: Value = row.get("response");
                    let e = experiment(&mut tx, space, string(q, "experimentId")).await?;
                    let c = connection(&mut tx, space, string(&e, "connectionId")).await?;
                    inactive(&c)?;
                    let token = self
                        .github
                        .publication_tokens(&c, &response["approval"]["draft"])
                        .await?;
                    self.github
                        .binding(&response["approval"]["draft"], &token)
                        .await?;
                    return Ok(response);
                }
                if !matches!(
                    op,
                    "BeginUpload"
                        | "CompleteUpload"
                        | "PublishDraft"
                        | "ApproveDraft"
                        | "ConnectRepository"
                ) {
                    return Ok(row.get("response"));
                }
            }
        }
        if op == "CompleteUpload" {
            sqlx::query("SAVEPOINT perfine_completion")
                .execute(&mut *tx)
                .await?;
        }
        let mut rejected = None;
        let out = match self
            .operation(&mut tx, actor, gh, op, q, &mut rejected)
            .await
        {
            Ok(value) => value,
            Err(fault) => {
                if let Some(rejected) = rejected {
                    // Roll back completion changes, then durably enqueue cleanup on
                    // the same connection. Never acquire a second pool connection
                    // while retaining the space lock and transaction connection.
                    sqlx::query("ROLLBACK TO SAVEPOINT perfine_completion")
                        .execute(&mut *tx)
                        .await?;
                    // Rejected content is terminal for this immutable manifest. Never
                    // renew PUT URLs beyond the cleanup deadline retained below.
                    sqlx::query("UPDATE perfine_uploads SET data=jsonb_set(data,'{state}','\"UPLOAD_STATE_FAILED\"') WHERE id=$1")
                        .bind(rejected.id).execute(&mut *tx).await?;
                    for key in &rejected.keys {
                        sqlx::query("INSERT INTO perfine_rejected_objects(object_key,expires_at) SELECT $1,staging_expires_at FROM perfine_uploads WHERE id=$2 ON CONFLICT(object_key) DO UPDATE SET expires_at=GREATEST(perfine_rejected_objects.expires_at,EXCLUDED.expires_at)").bind(key).bind(rejected.id).execute(&mut *tx).await?;
                    }
                    tx.commit().await?;
                    for key in &rejected.keys {
                        let _ = self.storage.delete(key).await;
                    }
                }
                return Err(fault);
            }
        };
        sqlx::query(
            "INSERT INTO perfine_audit(id,space,actor,method,request) VALUES($1,$2,$3,$4,$5)",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(id(space)?)
        .bind(actor)
        .bind(op)
        .bind(id(trace)?)
        .execute(&mut *tx)
        .await?;
        if mutation(op) {
            sqlx::query("INSERT INTO perfine_dedup(actor,space,method,request,digest,response) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(actor,space,method,request) DO UPDATE SET digest=EXCLUDED.digest,response=EXCLUDED.response,created_at=EXCLUDED.created_at WHERE perfine_dedup.created_at<=now()-interval '30 days'").bind(actor).bind(id(space)?).bind(op).bind(id(trace)?).bind(digest).bind(&out).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        tracing::info!(
            request_id = trace,
            procedure = op,
            experiment_id = string(q, "experimentId"),
            "Perfine operation committed"
        );
        Ok(out)
    }
    async fn operation(
        &self,
        db: &mut PgConnection,
        actor: &str,
        gh: &str,
        op: &str,
        q: &Value,
        rejected: &mut Option<RejectedUpload>,
    ) -> Result<Value> {
        let space = string(q, "spaceId");
        match op {
            "ConnectRepository" => {
                let repository = required(q, "repository")?;
                let rid = integer(repository, "githubRepositoryId");
                if rid == 0 || rid > i64::MAX as u64 || integer(q, "githubInstallationId") == 0 {
                    return Err(Fault::invalid("GitHub numeric IDs required"));
                }
                lock(db, &format!("repository:{rid}")).await?;
                let prior:Option<Value>=sqlx::query_scalar("SELECT data FROM perfine_connections WHERE repository=$1 AND (space=$2 OR active) ORDER BY active DESC LIMIT 1 FOR UPDATE").bind(rid as i64).bind(id(space)?).fetch_optional(&mut *db).await?;
                if prior.as_ref().is_some_and(|v| v["spaceId"] != space) {
                    return Err(Fault::exists());
                }
                let mut c=prior.unwrap_or_else(||json!({"id":uuid::Uuid::now_v7().to_string(),"spaceId":space,"createdAt":stamp(),"consent":{"revision":"0","allowedKinds":[],"effectiveAt":stamp()}}));
                c["repository"] = repository.clone();
                c["githubInstallationId"] = q["githubInstallationId"].clone();
                c["state"] = json!("CONNECTION_STATE_CONNECTED");
                c["updatedAt"] = json!(stamp());
                self.github.admin(&c, gh).await?;
                let github_user = self.github.call("GET", "/user", gh, None).await?;
                let github_actor = integer(&github_user, "id");
                if github_actor == 0 {
                    return Err(Fault::denied());
                }
                sqlx::query("INSERT INTO perfine_connections(id,space,repository,active,data,owner_subject,github_actor) VALUES($1,$2,$3,true,$4,$5,$6) ON CONFLICT(id) DO UPDATE SET active=true,data=EXCLUDED.data,owner_subject=EXCLUDED.owner_subject,github_actor=EXCLUDED.github_actor").bind(id(string(&c,"id"))?).bind(id(space)?).bind(rid as i64).bind(&c).bind(actor).bind(github_actor.to_string()).execute(db).await?;
                Ok(json!({"connection":c}))
            }
            "GetConnection"
            | "GetRawArtifactConsent"
            | "SetRawArtifactConsent"
            | "DisconnectRepository" => {
                let mut c = connection(db, space, string(q, "connectionId")).await?;
                if op == "GetRawArtifactConsent" {
                    return Ok(json!({"consent":c["consent"]}));
                }
                if op == "SetRawArtifactConsent" {
                    if integer(q, "expectedRevision") != integer(&c["consent"], "revision") {
                        return Err(Fault::stale());
                    }
                    let kinds = q.get("allowedKinds").cloned().unwrap_or(json!([]));
                    if kinds.as_array().is_none_or(|a| {
                        a.iter().any(|v| {
                            v != "ARTIFACT_KIND_RAW_PROFILE" && v != "ARTIFACT_KIND_RAW_TEST_LOG"
                        })
                    }) {
                        return Err(Fault::invalid("raw artifact kinds required"));
                    }
                    c["consent"] = json!({"revision":(integer(&c["consent"],"revision")+1).to_string(),"allowedKinds":kinds,"effectiveAt":stamp()});
                }
                if op == "DisconnectRepository" {
                    c["state"] = json!("CONNECTION_STATE_DISCONNECTED");
                    c["executionCancellations"] = json!(self.cancel_connection(db, &c).await?);
                }
                if mutation(op) {
                    c["updatedAt"] = json!(stamp());
                    if op == "DisconnectRepository" {
                        sqlx::query(
                            "UPDATE perfine_connections SET data=$2,active=false WHERE id=$1",
                        )
                        .bind(id(string(&c, "id"))?)
                        .bind(&c)
                        .execute(db)
                        .await?;
                    } else {
                        sqlx::query("UPDATE perfine_connections SET data=$2 WHERE id=$1")
                            .bind(id(string(&c, "id"))?)
                            .bind(&c)
                            .execute(db)
                            .await?;
                    }
                }
                Ok(if op == "SetRawArtifactConsent" {
                    json!({"consent":c["consent"]})
                } else {
                    json!({"connection":c})
                })
            }
            "ListConnections" | "ListExperiments" | "ListCandidates" | "ListEvidence" => {
                self.list(db, op, q).await
            }
            "GetExperiment" => {
                Ok(json!({"experiment":experiment(db,space,string(q,"experimentId")).await?}))
            }
            "GetCandidate" => {
                experiment(db, space, string(q, "experimentId")).await?;
                let c: Value = sqlx::query_scalar(
                    "SELECT data FROM perfine_candidates WHERE id=$1 AND experiment=$2",
                )
                .bind(id(string(q, "candidateId"))?)
                .bind(id(string(q, "experimentId"))?)
                .fetch_one(db)
                .await?;
                Ok(json!({"candidate":c}))
            }
            "PrepareExperiment" => {
                let c = connection(db, space, string(q, "connectionId")).await?;
                inactive(&c)?;
                self.github.current(&c).await?;
                let response = crate::preparation::prepare(
                    required(q, "configuration")?,
                    &c["repository"],
                    chrono::Utc::now(),
                )?;
                sqlx::query("INSERT INTO perfine_preparations(space,connection,specification,id) VALUES($1,$2,$3,$4)")
                    .bind(id(space)?).bind(id(string(q,"connectionId"))?).bind(&response["specification"]).bind(uuid::Uuid::now_v7()).execute(db).await?;
                Ok(response)
            }
            "ListCloudTools" | "StartExperiment" | "ResumeExperiment" => {
                let cid = if op == "ResumeExperiment" {
                    let e = experiment(db, space, string(q, "experimentId")).await?;
                    if e["specification"]["fingerprintSha256"] != q["expectedFingerprintSha256"] {
                        return Err(Fault::precondition(
                            "fingerprint mismatch; create a new experiment",
                        ));
                    }
                    string(&e, "connectionId").to_owned()
                } else {
                    string(q, "connectionId").to_owned()
                };
                let c = connection(db, space, &cid).await?;
                inactive(&c)?;
                self.github.current(&c).await?;
                if op == "StartExperiment" {
                    let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM perfine_preparations WHERE space=$1 AND connection=$2 AND specification=$3 AND created_at>now()-interval '30 days')")
                        .bind(id(space)?).bind(id(&cid)?).bind(required(q,"specification")?).fetch_one(&mut *db).await?;
                    if !exists {
                        return Err(Fault::precondition("prepare configuration before start"));
                    }
                }
                if op != "ListCloudTools" {
                    return Err(Fault::precondition(
                        "cloud unavailable: use the local CLI; no isolated unattended adapter has been verified",
                    ));
                }
                self.cloud_page(db, q).await
            }
            "CancelExperiment" => {
                let mut e = experiment(db, space, string(q, "experimentId")).await?;
                if !matches!(
                    string(q, "mode"),
                    "CANCELLATION_MODE_GRACEFUL" | "CANCELLATION_MODE_IMMEDIATE"
                ) {
                    return Err(Fault::invalid("cancellation mode required"));
                }
                let c = connection(db, space, string(&e, "connectionId")).await?;
                inactive(&c)?;
                // Local execution cannot be controlled by the host; the local CLI owns its process.
                if e["origin"] == "EXECUTION_ORIGIN_LOCAL" {
                    return Err(Fault::precondition("cancel with the local CLI"));
                }
                if q["mode"] == "CANCELLATION_MODE_GRACEFUL" {
                    return Err(Fault::precondition(
                        "graceful workflow control is unavailable; use local cancellation or request immediate GitHub cancellation",
                    ));
                }
                let token = self.github.current(&c).await?;
                let path = crate::github::path(&c["repository"]);
                let run = integer(&e["githubExecution"], "githubRunId");
                let result = self
                    .github
                    .call(
                        "POST",
                        &format!("{path}/actions/runs/{run}/cancel"),
                        &token,
                        None,
                    )
                    .await;
                e["cancellation"] = json!({"mode":q["mode"],"state":if result.is_ok(){"CANCELLATION_STATE_REQUESTED"}else{"CANCELLATION_STATE_FAILED"},"reason":"Check GitHub for cancellation confirmation","directCancelUrl":format!("https://github.com/{}/{}/actions/runs/{run}",string(&c["repository"],"owner"),string(&c["repository"],"name"))});
                save_experiment(db, &e).await?;
                Ok(json!({"experiment":e}))
            }
            "CheckUploadEligibility" | "BeginUpload" => self.begin(db, op, q).await,
            "GetUpload" | "CompleteUpload" => self.complete(db, op, q, rejected).await,
            "GetEvidenceDownload" => {
                experiment(db, space, string(q, "experimentId")).await?;
                let rows=sqlx::query("SELECT id,data,staging_expires_at FROM perfine_uploads WHERE experiment=$1 AND data->>'state'='UPLOAD_STATE_COMPLETE' AND data->'manifest'->>'manifestSha256'=$2").bind(id(string(q,"experimentId"))?).bind(string(q,"manifestSha256")).fetch_all(db).await?;
                let row = rows.first().ok_or_else(Fault::missing)?;
                let u: Value = row.get("data");
                let name = string(q, "artifactName");
                if !u["manifest"]["artifacts"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|a| a["name"] == name))
                {
                    return Err(Fault::missing());
                }
                let key = crate::storage::key(
                    string(q, "experimentId"),
                    &format!("accepted-{}", row.get::<uuid::Uuid, _>("id")),
                    name,
                );
                Ok(
                    json!({"signedGetUrl":self.storage.signed("GET",&key)?,"expiresAt":(chrono::Utc::now()+chrono::Duration::minutes(15)).to_rfc3339()}),
                )
            }
            "DeleteExperiment" | "GetDeletion" => {
                let eid = string(q, "experimentId");
                let prior: Option<Value> = sqlx::query_scalar(
                    "SELECT data FROM perfine_cleanup WHERE experiment=$1 AND space=$2",
                )
                .bind(id(eid)?)
                .bind(id(space)?)
                .fetch_optional(&mut *db)
                .await?;
                if let Some(d) = prior {
                    return Ok(json!({"deletion":d}));
                }
                if op == "GetDeletion" {
                    return Err(Fault::missing());
                }
                experiment(db, space, eid).await?;
                let d = json!({"experimentId":eid,"state":"DELETION_STATE_PENDING","acceptedAt":stamp()});
                sqlx::query("INSERT INTO perfine_tombstones VALUES($1) ON CONFLICT DO NOTHING")
                    .bind(id(eid)?)
                    .execute(&mut *db)
                    .await?;
                sqlx::query("INSERT INTO perfine_cleanup(experiment,space,data) VALUES($1,$2,$3)")
                    .bind(id(eid)?)
                    .bind(id(space)?)
                    .bind(&d)
                    .execute(db)
                    .await?;
                Ok(json!({"deletion":d}))
            }
            "ApproveDraft" | "GetApproval" | "PublishDraft" | "GetPublication" => {
                self.publication(db, actor, op, q).await
            }
            _ => Err(Fault::invalid("unknown operation")),
        }
    }
    async fn cloud_page(&self, db: &mut PgConnection, q: &Value) -> Result<Value> {
        let mut binding = q.clone();
        binding.as_object_mut().unwrap().remove("requestId");
        binding.as_object_mut().unwrap().remove("page");
        let binding = hash(&serde_json::to_vec(&("ListCloudTools", binding))?);
        let size = integer(&q["page"], "pageSize");
        if size > 100 {
            return Err(Fault::invalid("page size exceeds 100"));
        }
        let cursor = string(&q["page"], "cursor");
        let offset = if cursor.is_empty() {
            0
        } else {
            let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM perfine_cursors WHERE id=$1 AND binding=$2 AND created_at>now()-interval '1 hour')").bind(id(cursor)?).bind(&binding).fetch_one(&mut *db).await?;
            if !valid {
                return Err(Fault::invalid("cursor invalid or expired"));
            }
            1
        };
        let tools = [("claude", "Claude Code"), ("codex", "Codex CLI")];
        let take = if size == 0 { 2 } else { size as usize };
        let next = if offset + take < tools.len() {
            let id = uuid::Uuid::now_v7();
            sqlx::query("INSERT INTO perfine_cursors(id,binding,after_id,after_time) VALUES($1,$2,$3,now())").bind(id).bind(binding).bind(uuid::Uuid::nil()).execute(db).await?;
            id.to_string()
        } else {
            String::new()
        };
        Ok(
            json!({"tools":tools.iter().skip(offset).take(take).map(|(tool,name)|json!({"toolId":tool,"displayName":name,"state":2,"unavailableReason":2,"reason":"Isolated unattended execution has not been verified for this deployment","localAlternative":"Run perfine locally and upload retained evidence"})).collect::<Vec<_>>(),"page":{"nextCursor":next}}),
        )
    }
    async fn list(&self, db: &mut PgConnection, op: &str, q: &Value) -> Result<Value> {
        let space = string(q, "spaceId");
        let mut binding = q.clone();
        binding.as_object_mut().unwrap().remove("requestId");
        binding.as_object_mut().unwrap().remove("page");
        let binding = hash(&serde_json::to_vec(&(op, binding))?);
        let cursor = string(&q["page"], "cursor");
        let (after, after_time) = if cursor.is_empty() {
            (uuid::Uuid::max(), chrono::DateTime::<chrono::Utc>::MAX_UTC)
        } else {
            sqlx::query_as::<_,(uuid::Uuid,chrono::DateTime<chrono::Utc>)>("SELECT after_id,after_time FROM perfine_cursors WHERE id=$1 AND binding=$2 AND created_at>now()-interval '1 hour'").bind(id(cursor)?).bind(&binding).fetch_optional(&mut *db).await?.ok_or_else(||Fault::invalid("cursor invalid or expired"))?
        };
        let size = integer(&q["page"], "pageSize");
        if size > 100 {
            return Err(Fault::invalid("page size exceeds 100"));
        }
        let size = if size == 0 { 25 } else { size } as usize;
        let (sql, parent, field) = match op {
            "ListConnections" => (
                "SELECT id,data,created_at FROM perfine_connections WHERE space=$1 AND (created_at,id)<($4,$2) ORDER BY created_at DESC,id DESC LIMIT $3",
                id(space)?,
                "connections",
            ),
            "ListExperiments" => {
                connection(db, space, string(q, "connectionId")).await?;
                (
                    "SELECT id,data,created_at FROM perfine_experiments WHERE connection=$1 AND (created_at,id)<($4,$2) AND NOT EXISTS(SELECT 1 FROM perfine_tombstones t WHERE t.id=perfine_experiments.id) AND ($5='' OR data->>'executionState'=$5) AND ($6='' OR data->>'verdict'=$6) ORDER BY created_at DESC,id DESC LIMIT $3",
                    id(string(q, "connectionId"))?,
                    "experiments",
                )
            }
            "ListCandidates" => {
                experiment(db, space, string(q, "experimentId")).await?;
                (
                    "SELECT id,data,created_at FROM perfine_candidates WHERE experiment=$1 AND (created_at,id)<($4,$2) ORDER BY created_at DESC,id DESC LIMIT $3",
                    id(string(q, "experimentId"))?,
                    "candidates",
                )
            }
            _ => {
                experiment(db, space, string(q, "experimentId")).await?;
                (
                    "SELECT id,created_at,data->'manifest' AS data FROM perfine_uploads WHERE experiment=$1 AND (created_at,id)<($4,$2) AND data->>'state'='UPLOAD_STATE_COMPLETE' AND ($5='' OR data->>'candidateId'=$5) ORDER BY created_at DESC,id DESC LIMIT $3",
                    id(string(q, "experimentId"))?,
                    "manifests",
                )
            }
        };
        let mut query = sqlx::query(sql)
            .bind(parent)
            .bind(after)
            .bind((size + 1) as i64)
            .bind(after_time);
        if op == "ListExperiments" {
            query = query
                .bind(string(q, "executionState"))
                .bind(string(q, "verdict"));
        }
        if op == "ListEvidence" {
            query = query.bind(string(q, "candidateId"));
        }
        let rows = query.fetch_all(&mut *db).await?;
        let next = if rows.len() > size {
            let cursor = uuid::Uuid::now_v7();
            sqlx::query(
                "INSERT INTO perfine_cursors(id,binding,after_id,after_time) VALUES($1,$2,$3,$4)",
            )
            .bind(cursor)
            .bind(binding)
            .bind(rows[size - 1].get::<uuid::Uuid, _>("id"))
            .bind(rows[size - 1].get::<chrono::DateTime<chrono::Utc>, _>("created_at"))
            .execute(db)
            .await?;
            cursor.to_string()
        } else {
            String::new()
        };
        Ok(
            json!({field:rows.iter().take(size).map(|r|r.get::<Value,_>("data")).collect::<Vec<_>>(),"page":{"nextCursor":next}}),
        )
    }
    fn eligibility(&self, c: &Value, m: &Value) -> Result<Vec<Value>> {
        if integer(m, "schemaVersion") != 1 {
            return Err(Fault::invalid("unsupported schema version"));
        }
        id(string(m, "experimentId"))?;
        if !string(m, "candidateId").is_empty() {
            id(string(m, "candidateId"))?;
        }
        let mut parsed: EvidenceManifest = serde_json::from_value(m.clone())?;
        let expected = parsed.manifest_sha256.clone();
        parsed.manifest_sha256.clear();
        if hash(&parsed.encode_to_vec()) != expected {
            return Err(Fault::invalid("manifest digest mismatch"));
        }
        let artifacts = m["artifacts"]
            .as_array()
            .ok_or_else(|| Fault::invalid("artifacts required"))?;
        if artifacts.is_empty() || artifacts.len() > 256 {
            return Err(Fault::invalid("artifact count invalid"));
        }
        let started = chrono::DateTime::parse_from_rfc3339(string(m, "runStartedAt"))
            .map_err(|_| Fault::invalid("run start time required"))?;
        let effective = chrono::DateTime::parse_from_rfc3339(string(&c["consent"], "effectiveAt"))
            .map_err(|_| Fault::unavailable())?;
        let mut names = std::collections::HashSet::new();
        let mut engine = 0;
        let mut decisions = Vec::new();
        for a in artifacts {
            let name = string(a, "name");
            if !safe_name(name) || !names.insert(name) {
                return Err(Fault::invalid("unique artifact names required"));
            }
            sha(string(a, "sha256"), 64)?;
            let kind = string(a, "kind");
            let raw = matches!(
                kind,
                "ARTIFACT_KIND_RAW_PROFILE" | "ARTIFACT_KIND_RAW_TEST_LOG"
            );
            if kind == "ARTIFACT_KIND_ENGINE_EVIDENCE" {
                engine += 1;
                if m["engineArtifactName"] != name {
                    return Err(Fault::invalid("engine artifact mismatch"));
                }
            }
            if !matches!(
                kind,
                "ARTIFACT_KIND_ENGINE_EVIDENCE"
                    | "ARTIFACT_KIND_MEASUREMENTS"
                    | "ARTIFACT_KIND_PROFILE_SUMMARY"
                    | "ARTIFACT_KIND_CORRECTNESS_RESULTS"
                    | "ARTIFACT_KIND_RAW_PROFILE"
                    | "ARTIFACT_KIND_RAW_TEST_LOG"
            ) {
                return Err(Fault::invalid("artifact kind invalid"));
            }
            let consent = &c["consent"];
            let eligible =
                if integer(a, "sizeBytes") == 0 || integer(a, "sizeBytes") > self.upload_limit {
                    6
                } else if a["containsSourceSnippets"] == true && !raw {
                    5
                } else if raw
                    && (!consent["allowedKinds"]
                        .as_array()
                        .is_some_and(|a| a.contains(&json!(kind)))
                        || integer(consent, "revision") != integer(m, "consentRevision")
                        || started < effective)
                {
                    2
                } else {
                    1
                };
            decisions.push(json!({"artifactName":name,"eligibility":eligible,"reason":if eligible==1{"Eligible subject to byte verification"}else{"Keep the original locally or on GitHub"}}));
        }
        if engine != 1 {
            return Err(Fault::invalid("exactly one engine artifact required"));
        }
        Ok(decisions)
    }
    async fn begin(&self, db: &mut PgConnection, op: &str, q: &Value) -> Result<Value> {
        let space = string(q, "spaceId");
        let c = connection(db, space, string(q, "connectionId")).await?;
        inactive(&c)?;
        self.github.current(&c).await?;
        let m = required(q, "manifest")?;
        tombstone(db, string(m, "experimentId")).await?;
        let decisions = self.eligibility(&c, m)?;
        let eligible = decisions.iter().all(|d| d["eligibility"] == 1);
        if op == "CheckUploadEligibility" {
            return Ok(json!({"decisions":decisions,"eligibleForCompleteEvaluation":eligible}));
        }
        if !eligible {
            return Err(Fault::precondition(
                "artifact ineligible; retain local evidence",
            ));
        }
        let eid = string(q, "experimentId");
        if m["experimentId"] != eid {
            return Err(Fault::invalid("experiment mismatch"));
        }
        let spec = required(q, "specification")?;
        validate_spec(spec)?;
        crate::preparation::verify_spec(spec, &c["repository"])?;
        if spec["fingerprintSha256"] != m["experimentFingerprintSha256"]
            || spec["baseSha"] != m["sourceBaseSha"]
            || spec["harnessSha"] != m["harnessSha"]
            || spec["environment"] != m["environment"]
            || spec["policy"] != m["policy"]
        {
            return Err(Fault::precondition("immutable evidence mismatch"));
        }
        if !matches!(
            string(q, "origin"),
            "EXECUTION_ORIGIN_LOCAL" | "EXECUTION_ORIGIN_GITHUB_ACTIONS"
        ) {
            return Err(Fault::invalid("execution origin required"));
        }
        let prior: Option<Value> =
            sqlx::query_scalar("SELECT data FROM perfine_experiments WHERE id=$1 FOR UPDATE")
                .bind(id(eid)?)
                .fetch_optional(&mut *db)
                .await?;
        if let Some(e) = prior {
            if e["spaceId"] != space
                || e["connectionId"] != q["connectionId"]
                || e["specification"] != *spec
                || e["origin"] != q["origin"]
                || e.get("githubExecution") != q.get("githubExecution")
            {
                return Err(Fault::precondition("experiment binding mismatch"));
            }
        } else {
            let mut e = json!({"id":eid,"spaceId":space,"connectionId":q["connectionId"],"specification":spec,"origin":q["origin"],"executionState":"EXECUTION_STATE_RUNNING","verdict":"PERFORMANCE_VERDICT_NOT_EVALUATED","evidenceCompletion":"COMPLETION_STATE_INCOMPLETE","createdAt":stamp(),"updatedAt":stamp()});
            if let Some(exec) = q.get("githubExecution") {
                e["githubExecution"] = exec.clone();
            }
            sqlx::query(
                "INSERT INTO perfine_experiments(id,space,connection,data) VALUES($1,$2,$3,$4)",
            )
            .bind(id(eid)?)
            .bind(id(space)?)
            .bind(id(string(q, "connectionId"))?)
            .bind(e)
            .execute(&mut *db)
            .await?;
        }
        let prior:Option<Value>=sqlx::query_scalar("SELECT data FROM perfine_uploads WHERE experiment=$1 AND data->'manifest'->>'manifestSha256'=$2").bind(id(eid)?).bind(string(m,"manifestSha256")).fetch_optional(&mut *db).await?;
        let u = if let Some(u) = prior {
            u
        } else {
            let bytes = m["artifacts"]
                .as_array()
                .unwrap()
                .iter()
                .try_fold(0u64, |a, v| a.checked_add(integer(v, "sizeBytes")))
                .ok_or_else(Fault::quota)?;
            let used:i64=sqlx::query_scalar("SELECT COALESCE(sum(u.reserved_bytes),0)::bigint FROM perfine_uploads u JOIN perfine_experiments e ON e.id=u.experiment WHERE e.space=$1").bind(id(space)?).fetch_one(&mut *db).await?;
            if bytes > self.upload_limit {
                return Err(Fault::quota());
            }
            if bytes > self.quota as u64 / 2 || used as u64 > self.quota as u64 - bytes * 2 {
                return Err(Fault::quota());
            }
            let u = json!({"id":uuid::Uuid::now_v7().to_string(),"experimentId":eid,"candidateId":string(m,"candidateId"),"state":"UPLOAD_STATE_PENDING","manifest":m,"decisions":decisions,"createdAt":stamp()});
            sqlx::query("INSERT INTO perfine_uploads(id,experiment,data,reserved_bytes) VALUES($1,$2,$3,$4)").bind(id(string(&u,"id"))?).bind(id(eid)?).bind(&u).bind((bytes * 2) as i64).execute(&mut *db).await?;
            u
        };
        sqlx::query("UPDATE perfine_uploads SET staging_expires_at=now()+interval '16 minutes' WHERE id=$1 AND data->>'state'='UPLOAD_STATE_PENDING'").bind(id(string(&u,"id"))?).execute(&mut *db).await?;
        let mut targets = Vec::new();
        if u["state"] == "UPLOAD_STATE_PENDING" {
            for a in m["artifacts"].as_array().unwrap() {
                let name = string(a, "name");
                targets.push(json!({"artifactName":name,"requiredHeaders":[{"name":"content-length","value":integer(a,"sizeBytes").to_string()}],"signedPutUrl":self.storage.signed_upload(&crate::storage::key(eid,string(&u,"id"),name),integer(a,"sizeBytes"))?,"expiresAt":(chrono::Utc::now()+chrono::Duration::minutes(15)).to_rfc3339()}));
            }
        }
        Ok(json!({"upload":u,"targets":targets}))
    }
    async fn complete(
        &self,
        db: &mut PgConnection,
        op: &str,
        q: &Value,
        rejected: &mut Option<RejectedUpload>,
    ) -> Result<Value> {
        let space = string(q, "spaceId");
        let uid = string(q, "uploadId");
        let mut u:Value=sqlx::query_scalar("SELECT u.data FROM perfine_uploads u JOIN perfine_experiments e ON e.id=u.experiment WHERE u.id=$1 AND e.space=$2 FOR UPDATE OF u").bind(id(uid)?).bind(id(space)?).fetch_one(&mut *db).await?;
        let eid = string(&u, "experimentId").to_owned();
        let mut e = experiment(db, space, &eid).await?;
        let c = connection(db, space, string(&e, "connectionId")).await?;
        inactive(&c)?;
        self.github.current(&c).await?;
        if op == "GetUpload" {
            return Ok(json!({"upload":u}));
        }
        if !self
            .eligibility(&c, &u["manifest"])?
            .iter()
            .all(|d| d["eligibility"] == 1)
        {
            return Err(Fault::precondition(
                "consent revoked or artifact ineligible",
            ));
        }
        if u["state"] == "UPLOAD_STATE_COMPLETE" {
            return Ok(json!({"upload":u,"evidenceCompletion":e["evidenceCompletion"]}));
        }
        if u["state"] != "UPLOAD_STATE_PENDING" {
            return Err(Fault::precondition("rejected upload cannot be completed"));
        }
        let m = &u["manifest"];
        let mut bytes = std::collections::BTreeMap::new();
        for a in m["artifacts"].as_array().unwrap() {
            let name = string(a, "name");
            let content = self
                .storage
                .read(
                    &crate::storage::key(&eid, uid, name),
                    integer(a, "sizeBytes"),
                )
                .await?;
            if content.len() as u64 != integer(a, "sizeBytes")
                || hash(&content) != string(a, "sha256")
            {
                return Err(Fault::precondition("artifact hash or size mismatch"));
            }
            if let Err(fault) = crate::evidence::content_allowed(&content, string(a, "kind")) {
                *rejected = Some(RejectedUpload {
                    id: id(uid)?,
                    keys: m["artifacts"]
                        .as_array()
                        .ok_or_else(Fault::unavailable)?
                        .iter()
                        .map(|artifact| crate::storage::key(&eid, uid, string(artifact, "name")))
                        .collect(),
                });
                return Err(fault);
            }
            bytes.insert(name.to_owned(), content);
        }
        let candidate = match crate::evidence::evaluate(&e, m, &bytes) {
            Ok(candidate) => candidate,
            Err(fault) => {
                // Evaluation includes structural privacy checks. The manifest and bytes
                // are immutable, so these rejections use the same terminal cleanup path.
                *rejected = Some(RejectedUpload {
                    id: id(uid)?,
                    keys: bytes
                        .keys()
                        .map(|name| crate::storage::key(&eid, uid, name))
                        .collect(),
                });
                return Err(fault);
            }
        };
        if let Some(candidate) = candidate {
            let cid = string(&candidate, "id");
            let old: Option<Value> =
                sqlx::query_scalar("SELECT data FROM perfine_candidates WHERE id=$1")
                    .bind(id(cid)?)
                    .fetch_optional(&mut *db)
                    .await?;
            if old.as_ref().is_some_and(|old| old != &candidate) {
                return Err(Fault::precondition("candidate evidence is immutable"));
            }
            sqlx::query("INSERT INTO perfine_candidates(id,experiment,data) VALUES($1,$2,$3) ON CONFLICT(id) DO NOTHING").bind(id(cid)?).bind(id(&eid)?).bind(&candidate).execute(&mut *db).await?;
            e["verdict"] = candidate["verdict"].clone();
            e["exceptions"] = candidate["exceptions"].clone();
            e["humanJudgmentRequired"] = candidate["humanJudgmentRequired"].clone();
        }
        for (name, content) in &bytes {
            self.storage
                .write(
                    &crate::storage::key(&eid, &format!("accepted-{uid}"), name),
                    content,
                )
                .await?;
        }
        u["state"] = json!("UPLOAD_STATE_COMPLETE");
        complete_evidence(&mut e);
        e["updatedAt"] = json!(stamp());
        sqlx::query("UPDATE perfine_uploads SET data=$2 WHERE id=$1")
            .bind(id(uid)?)
            .bind(&u)
            .execute(&mut *db)
            .await?;
        save_experiment(db, &e).await?;
        Ok(json!({"upload":u,"evidenceCompletion":e["evidenceCompletion"]}))
    }
    async fn publication(
        &self,
        db: &mut PgConnection,
        actor: &str,
        op: &str,
        q: &Value,
    ) -> Result<Value> {
        let space = string(q, "spaceId");
        let approval = if op == "ApproveDraft" {
            // Approval records require a current human subject; API keys upload but cannot impersonate a human decision.
            id(actor).map_err(|_| Fault::denied())?;
            let e = experiment(db, space, string(q, "experimentId")).await?;
            let d = required(q, "draft")?;
            let candidate: Value = sqlx::query_scalar(
                "SELECT data FROM perfine_candidates WHERE id=$1 AND experiment=$2",
            )
            .bind(id(string(d, "candidateId"))?)
            .bind(id(string(&e, "id"))?)
            .fetch_one(&mut *db)
            .await?;
            if candidate["headSha"] != d["headSha"]
                || candidate["baseSha"] != d["baseSha"]
                || candidate["evidenceManifestSha256"] != d["evidenceManifestSha256"]
                || e["evidenceCompletion"] != "COMPLETION_STATE_COMPLETE"
            {
                return Err(Fault::precondition("approval evidence mismatch"));
            }
            if candidate["_publicationAllowed"] != true {
                return Err(Fault::precondition(
                    "candidate is not eligible for publication",
                ));
            }
            if candidate["verdict"] != "PERFORMANCE_VERDICT_PASS"
                && (candidate["humanJudgmentRequired"] != true
                    || string(q, "humanJudgment").trim().is_empty())
            {
                return Err(Fault::precondition(
                    "passing evidence or explicit required human judgment needed",
                ));
            }
            for field in ["baseRef", "headRef", "title", "body"] {
                if string(d, field).trim().is_empty() {
                    return Err(Fault::invalid("complete draft content required"));
                }
            }
            let c = connection(db, space, string(&e, "connectionId")).await?;
            inactive(&c)?;
            if d["headRepository"] != c["repository"] {
                return Err(Fault::denied());
            }
            let token = self.github.publication_tokens(&c, d).await?;
            self.github.binding(d, &token).await?;
            self.github.production_diff(d, &candidate, &token).await?;
            let a = json!({"id":uuid::Uuid::now_v7().to_string(),"experimentId":e["id"],"draft":d,"state":"APPROVAL_STATE_VALID","approverSubject":actor,"createdAt":stamp(),"humanJudgment":string(q,"humanJudgment")});
            sqlx::query("INSERT INTO perfine_approvals(id,experiment,data) VALUES($1,$2,$3)")
                .bind(id(string(&a, "id"))?)
                .bind(id(string(&e, "id"))?)
                .bind(&a)
                .execute(&mut *db)
                .await?;
            a
        } else {
            let a:Value=sqlx::query_scalar("SELECT a.data FROM perfine_approvals a JOIN perfine_experiments e ON e.id=a.experiment WHERE a.id=$1 AND e.space=$2 FOR UPDATE OF a").bind(id(string(q,"approvalId"))?).bind(id(space)?).fetch_one(&mut *db).await?;
            experiment(db, space, string(&a, "experimentId")).await?;
            a
        };
        if op == "ApproveDraft" || op == "GetApproval" {
            return Ok(json!({"approval":approval}));
        }
        let aid = id(string(&approval, "id"))?;
        if op == "PublishDraft" {
            // Persist only a human subject that Delcore can reauthorize at dispatch time.
            // API-key dispatch needs a future key-ID introspection API; never retain the secret.
            id(actor).map_err(|_| Fault::denied())?;
            if approval["draft"] != q["expectedDraft"] {
                return Err(crate::error::domain(
                    Fault::precondition("approval stale"),
                    ErrorReason::ApprovalStale,
                ));
            }
            let e = experiment(db, space, string(&approval, "experimentId")).await?;
            let c = connection(db, space, string(&e, "connectionId")).await?;
            inactive(&c)?;
            let token = self
                .github
                .publication_tokens(&c, &approval["draft"])
                .await?;
            self.github.binding(&approval["draft"], &token).await?;
            let p = json!({"approvalId":approval["id"],"candidateId":approval["draft"]["candidateId"],"state":"PUBLICATION_STATE_PENDING","draft":approval["draft"],"_publisherSubject":actor});
            sqlx::query(
                "UPDATE perfine_approvals SET publication=$2 WHERE id=$1 AND publication IS NULL",
            )
            .bind(aid)
            .bind(p)
            .execute(&mut *db)
            .await?;
        }
        let p: Option<Value> =
            sqlx::query_scalar("SELECT publication FROM perfine_approvals WHERE id=$1")
                .bind(aid)
                .fetch_one(db)
                .await?;
        Ok(json!({"publication":p.ok_or_else(Fault::missing)?}))
    }
    async fn actions_delegate(&self, db: &mut PgConnection, space: &str, cid: &str) -> Result<()> {
        let principal = sqlx::query("SELECT owner_subject FROM perfine_connections WHERE id=$1")
            .bind(id(cid)?)
            .fetch_one(&mut *db)
            .await?;
        let subject: String = principal.get("owner_subject");
        id(&subject).map_err(|_| Fault::denied())?;
        // The connection delegates repository execution to its current Delcore principal.
        // GitHub's signed run actor need not be the human who connected an organization repo.
        let permission = self
            .authority
            .call(
                "Authorize",
                &format!("Bearer {}", self.authority.service_token().await?),
                json!({"tenantId":space,"userId":subject,"capability":"experiment","paid":false}),
            )
            .await?;
        if permission["allowed"] != true {
            return Err(Fault::denied());
        }
        Ok(())
    }
    async fn actions_execute(&self, h: &str, op: &str, q: Value, trace: &str) -> Result<Value> {
        if !matches!(
            op,
            "BeginUpload" | "CompleteUpload" | "GetUpload" | "CheckUploadEligibility"
        ) {
            return Err(Fault::denied());
        }
        let claims = self
            .actions
            .verify(
                h.strip_prefix("Actions ")
                    .ok_or_else(Fault::unauthenticated)?,
            )
            .await?;
        let space = string(&q, "spaceId");
        if self.authority.states(&[space.into()]).await?
            != [server_common::authority::TenantState::Active]
        {
            return Err(Fault::denied());
        }
        let mut tx = self.db.begin().await?;
        let (cid, execution) = if matches!(op, "GetUpload" | "CompleteUpload") {
            let eid: uuid::Uuid =
                sqlx::query_scalar("SELECT experiment FROM perfine_uploads WHERE id=$1")
                    .bind(id(string(&q, "uploadId"))?)
                    .fetch_one(&mut *tx)
                    .await?;
            let e = experiment(&mut tx, space, &eid.to_string()).await?;
            (
                string(&e, "connectionId").to_owned(),
                e["githubExecution"].clone(),
            )
        } else {
            let execution = if op == "CheckUploadEligibility" {
                json!({"githubRunId":claims["run_id"],"runAttempt":string(&claims,"run_attempt").parse::<u32>().unwrap_or(0),"sourceSha":claims["sha"],"workflowSha":claims["workflow_sha"],"workflowPath":string(&claims,"workflow_ref").split('@').next().unwrap_or("").splitn(3,'/').nth(2).unwrap_or("")})
            } else {
                q["githubExecution"].clone()
            };
            (string(&q, "connectionId").to_owned(), execution)
        };
        let c = connection(&mut tx, space, &cid).await?;
        inactive(&c)?;
        self.actions_delegate(&mut tx, space, &cid).await?;
        let token = self.github.current(&c).await?;
        let rid = integer(&c["repository"], "githubRepositoryId");
        let run = integer(&execution, "githubRunId");
        let attempt = integer(&execution, "runAttempt");
        if string(&claims, "repository_id") != rid.to_string()
            || string(&claims, "run_id") != run.to_string()
            || string(&claims, "run_attempt") != attempt.to_string()
            || run == 0
            || attempt == 0
            || claims["sha"] != execution["sourceSha"]
            || claims["workflow_sha"] != execution["workflowSha"]
            || claims["event_name"] != "workflow_dispatch"
        {
            return Err(Fault::denied());
        }
        let remote = self
            .github
            .call(
                "GET",
                &format!(
                    "{}/actions/runs/{run}/attempts/{attempt}",
                    crate::github::path(&c["repository"])
                ),
                &token,
                None,
            )
            .await?;
        crate::auth::execution_scope(&claims, &execution, &c["repository"], &remote)?;
        if matches!(op, "BeginUpload" | "CheckUploadEligibility")
            && q["manifest"]["sourceHeadSha"] != execution["sourceSha"]
        {
            return Err(Fault::denied());
        }
        if op == "BeginUpload"
            && (q["origin"] != "EXECUTION_ORIGIN_GITHUB_ACTIONS"
                || q["specification"]["workflowSha"] != execution["workflowSha"])
        {
            return Err(Fault::denied());
        }
        tx.rollback().await?;
        self.authorized(
            &format!("actions:{rid}:{run}:{attempt}"),
            "",
            op,
            &q,
            trace,
            Some(&cid),
        )
        .await
    }
    pub async fn maintain(&self) -> Result<()> {
        for table in [
            "perfine_audit",
            "perfine_dedup",
            "perfine_cursors",
            "perfine_preparations",
        ] {
            sqlx::query(&format!(
                "DELETE FROM {table} WHERE created_at<=now()-interval '30 days'"
            ))
            .execute(&self.db)
            .await?;
        }
        sqlx::query("DELETE FROM perfine_cleanup WHERE data->>'state'='DELETION_STATE_COMPLETE' AND created_at<=now()-interval '30 days'").execute(&self.db).await?;
        let rejected=sqlx::query("SELECT object_key,expires_at FROM perfine_rejected_objects WHERE next_attempt<=now() LIMIT 100").fetch_all(&self.db).await?;
        for row in rejected {
            let key: String = row.get("object_key");
            let expiry: chrono::DateTime<chrono::Utc> = row.get("expires_at");
            if self.storage.delete(&key).await.is_ok() && expiry < chrono::Utc::now() {
                sqlx::query("DELETE FROM perfine_rejected_objects WHERE object_key=$1")
                    .bind(key)
                    .execute(&self.db)
                    .await?;
            } else {
                sqlx::query("UPDATE perfine_rejected_objects SET next_attempt=now()+interval '1 minute' WHERE object_key=$1").bind(key).execute(&self.db).await?;
            }
        }
        let stages=sqlx::query("SELECT u.id,u.experiment,u.data,e.space FROM perfine_uploads u JOIN perfine_experiments e ON e.id=u.experiment WHERE NOT u.staging_cleaned AND u.staging_expires_at<now() AND u.data->>'state' IN ('UPLOAD_STATE_COMPLETE','UPLOAD_STATE_FAILED') LIMIT 100").fetch_all(&self.db).await?;
        for row in stages {
            let mut tx = self.db.begin().await?;
            let space: uuid::Uuid = row.get("space");
            lock(&mut tx, &space.to_string()).await?;
            let uid: uuid::Uuid = row.get("id");
            let eid: uuid::Uuid = row.get("experiment");
            let data: Value = row.get("data");
            let rejected = data["state"] == "UPLOAD_STATE_FAILED";
            let mut failed = false;
            for a in data["manifest"]["artifacts"]
                .as_array()
                .ok_or_else(Fault::unavailable)?
            {
                let mut prefixes = vec![uid.to_string()];
                if rejected {
                    // A prior completion attempt may have copied only some objects.
                    // Failed uploads retain no accepted bytes after cleanup.
                    prefixes.push(format!("accepted-{uid}"));
                }
                for prefix in prefixes {
                    if self
                        .storage
                        .delete(&crate::storage::key(
                            &eid.to_string(),
                            &prefix,
                            string(a, "name"),
                        ))
                        .await
                        .is_err()
                    {
                        failed = true;
                    }
                }
            }
            if !failed {
                sqlx::query("UPDATE perfine_uploads SET reserved_bytes=CASE WHEN $2 THEN 0 ELSE reserved_bytes/2 END,staging_cleaned=true WHERE id=$1 AND NOT staging_cleaned")
                    .bind(uid).bind(rejected).execute(&mut *tx).await?;
            }
            tx.commit().await?;
        }
        let spaces: Vec<uuid::Uuid> = sqlx::query_scalar(
            "SELECT space FROM perfine_experiments UNION SELECT space FROM perfine_preparations UNION SELECT space FROM perfine_connections WHERE active",
        )
        .fetch_all(&self.db)
        .await?;
        for space in spaces {
            if matches!(self.authority.states(&[space.to_string()]).await,Ok(states) if states==[server_common::authority::TenantState::Deleted])
            {
                let mut tx = self.db.begin().await?;
                lock(&mut tx, &space.to_string()).await?;
                // Release the unique active repository binding, retaining connection and run
                // identities so object deletion and cancellation can still be retried.
                let connections: Vec<Value> = sqlx::query_scalar(
                    "SELECT data FROM perfine_connections WHERE space=$1 AND active",
                )
                .bind(space)
                .fetch_all(&mut *tx)
                .await?;
                for mut c in connections {
                    c["state"] = json!("CONNECTION_STATE_DISCONNECTED");
                    c["updatedAt"] = json!(stamp());
                    c["executionCancellations"] = json!(self.cancel_connection(&mut tx, &c).await?);
                    sqlx::query("UPDATE perfine_connections SET active=false,data=$2 WHERE id=$1")
                        .bind(id(string(&c, "id"))?)
                        .bind(c)
                        .execute(&mut *tx)
                        .await?;
                }
                sqlx::query("DELETE FROM perfine_preparations WHERE space=$1")
                    .bind(space)
                    .execute(&mut *tx)
                    .await?;
                let ids: Vec<uuid::Uuid> =
                    sqlx::query_scalar("SELECT id FROM perfine_experiments WHERE space=$1")
                        .bind(space)
                        .fetch_all(&mut *tx)
                        .await?;
                for eid in ids {
                    sqlx::query("INSERT INTO perfine_tombstones VALUES($1) ON CONFLICT DO NOTHING")
                        .bind(eid)
                        .execute(&mut *tx)
                        .await?;
                    sqlx::query("INSERT INTO perfine_cleanup(experiment,space,data) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(eid).bind(space).bind(json!({"experimentId":eid,"state":"DELETION_STATE_PENDING","acceptedAt":stamp()})).execute(&mut *tx).await?;
                }
                tx.commit().await?;
            }
        }
        let jobs=sqlx::query("SELECT experiment,space FROM perfine_cleanup WHERE next_attempt<=now() AND data->>'state'<>'DELETION_STATE_COMPLETE' ORDER BY next_attempt,experiment LIMIT 100").fetch_all(&self.db).await?;
        for job in jobs {
            let eid: uuid::Uuid = job.get("experiment");
            let space: uuid::Uuid = job.get("space");
            let mut tx = self.db.begin().await?;
            lock(&mut tx, &space.to_string()).await?;
            let uploads = sqlx::query(
                "SELECT id,data,staging_expires_at FROM perfine_uploads WHERE experiment=$1",
            )
            .bind(eid)
            .fetch_all(&mut *tx)
            .await?;
            let mut failed = false;
            let execution: Option<Value> =
                sqlx::query_scalar("SELECT data FROM perfine_experiments WHERE id=$1")
                    .bind(eid)
                    .fetch_optional(&mut *tx)
                    .await?;
            if let Some(mut e) = execution
                && e["origin"] == "EXECUTION_ORIGIN_GITHUB_ACTIONS"
                && matches!(
                    string(&e, "executionState"),
                    "EXECUTION_STATE_RUNNING" | "EXECUTION_STATE_QUEUED"
                )
            {
                // Retain execution identity until remote termination is confirmed, even after object deletion.
                // Otherwise tenant deletion could remove the only durable cancellation retry record.
                failed = true;
                let c = connection(&mut tx, &space.to_string(), string(&e, "connectionId")).await?;
                let run = integer(&e["githubExecution"], "githubRunId");
                let cancelled = match self.github.token(&c).await {
                    Ok(token) => self
                        .github
                        .call(
                            "POST",
                            &format!(
                                "{}/actions/runs/{run}/cancel",
                                crate::github::path(&c["repository"])
                            ),
                            &token,
                            None,
                        )
                        .await
                        .is_ok(),
                    Err(_) => false,
                };
                e["cancellation"] = json!({"mode":"CANCELLATION_MODE_IMMEDIATE","state":if cancelled{"CANCELLATION_STATE_REQUESTED"}else{"CANCELLATION_STATE_FAILED"},"reason":"Deletion awaits GitHub termination; cancellation retries continue","directCancelUrl":format!("https://github.com/{}/{}/actions/runs/{run}",string(&c["repository"],"owner"),string(&c["repository"],"name"))});
                save_experiment(&mut tx, &e).await?;
            }
            for u in uploads {
                let uid = u.get::<uuid::Uuid, _>("id").to_string();
                let data: Value = u.get("data");
                // PUT URLs can recreate staging objects until their fifteen-minute expiry.
                let expiry: chrono::DateTime<chrono::Utc> = u.get("staging_expires_at");
                if expiry > chrono::Utc::now() {
                    failed = true;
                    continue;
                }
                for a in data["manifest"]["artifacts"]
                    .as_array()
                    .ok_or_else(Fault::unavailable)?
                {
                    for suffix in [uid.clone(), format!("accepted-{uid}")] {
                        if self
                            .storage
                            .delete(&crate::storage::key(
                                &eid.to_string(),
                                &suffix,
                                string(a, "name"),
                            ))
                            .await
                            .is_err()
                        {
                            failed = true;
                        }
                    }
                }
            }
            if !failed {
                for table in ["perfine_approvals", "perfine_candidates", "perfine_uploads"] {
                    sqlx::query(&format!("DELETE FROM {table} WHERE experiment=$1"))
                        .bind(eid)
                        .execute(&mut *tx)
                        .await?;
                }
                sqlx::query("DELETE FROM perfine_experiments WHERE id=$1")
                    .bind(eid)
                    .execute(&mut *tx)
                    .await?;
            }
            sqlx::query("UPDATE perfine_cleanup SET attempts=attempts+1,next_attempt=now()+interval '1 minute',data=jsonb_set(jsonb_set(data,'{state}',to_jsonb($2::text)),'{failureReason}',to_jsonb($3::text)) WHERE experiment=$1").bind(eid).bind(if failed{"DELETION_STATE_RETRYING"}else{"DELETION_STATE_COMPLETE"}).bind(if failed{"Cleanup pending; retry continues without expiry"}else{""}).execute(&mut *tx).await?;
            tx.commit().await?;
            tracing::info!(experiment_id=%eid,failed,"Perfine cleanup attempted");
        }
        self.reconcile_executions().await?;
        self.publish_pending().await
    }
    async fn cancel_connection(&self, db: &mut PgConnection, c: &Value) -> Result<Vec<Value>> {
        let rows:Vec<Value>=sqlx::query_scalar("SELECT data FROM perfine_experiments WHERE connection=$1 AND data->>'origin'='EXECUTION_ORIGIN_GITHUB_ACTIONS' AND data->>'executionState' IN ('EXECUTION_STATE_RUNNING','EXECUTION_STATE_QUEUED')").bind(id(string(c,"id"))?).fetch_all(&mut *db).await?;
        let token = self.github.token(c).await;
        let mut cancellations = Vec::new();
        for mut e in rows {
            let run = integer(&e["githubExecution"], "githubRunId");
            let status = match &token {
                Ok(token) => self
                    .github
                    .call(
                        "POST",
                        &format!(
                            "{}/actions/runs/{run}/cancel",
                            crate::github::path(&c["repository"])
                        ),
                        token,
                        None,
                    )
                    .await
                    .is_ok(),
                Err(_) => false,
            };
            let cancellation = json!({"mode":"CANCELLATION_MODE_IMMEDIATE","state":if status{"CANCELLATION_STATE_REQUESTED"}else{"CANCELLATION_STATE_FAILED"},"reason":if status{"GitHub cancellation requested; awaiting confirmation"}else{"Cancellation failed; cancel directly on GitHub"},"directCancelUrl":format!("https://github.com/{}/{}/actions/runs/{run}",string(&c["repository"],"owner"),string(&c["repository"],"name"))});
            e["cancellation"] = cancellation.clone();
            save_experiment(db, &e).await?;
            cancellations.push(cancellation);
        }
        Ok(cancellations)
    }
    async fn reconcile_executions(&self) -> Result<()> {
        let rows=sqlx::query("SELECT c.id,c.space FROM perfine_connections c WHERE EXISTS(SELECT 1 FROM perfine_experiments e WHERE e.connection=c.id AND e.data->>'origin'='EXECUTION_ORIGIN_GITHUB_ACTIONS' AND e.data->>'executionState' IN ('EXECUTION_STATE_RUNNING','EXECUTION_STATE_QUEUED')) ORDER BY (c.data->>'_lastExecutionCheckAt')::timestamptz NULLS FIRST,c.id LIMIT 100").fetch_all(&self.db).await?;
        for row in rows {
            let mut tx = self.db.begin().await?;
            let space: uuid::Uuid = row.get("space");
            lock(&mut tx, &space.to_string()).await?;
            // The candidate query runs before the space lock. A reconnect may have
            // replaced both authority and connection state while this worker waited.
            // Persist the visit even when provider checks cannot finish, so blocked
            // connections yield later batches to the least recently checked work.
            let current = sqlx::query(
                "UPDATE perfine_connections SET data=jsonb_set(data,'{_lastExecutionCheckAt}',to_jsonb(now())) WHERE id=$1 AND space=$2 RETURNING data,owner_subject",
            )
            .bind(row.get::<uuid::Uuid, _>("id"))
            .bind(space)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(current) = current else { continue };
            let mut c: Value = current.get("data");
            let subject: String = current.get("owner_subject");
            let active = matches!(self.authority.states(&[space.to_string()]).await,Ok(states) if states==[server_common::authority::TenantState::Active]);
            let grant = if let Ok(token) = self.authority.service_token().await {
                self.authority.call("Authorize",&format!("Bearer {token}"),json!({"tenantId":space,"userId":subject,"capability":"experiment","paid":false})).await.is_ok_and(|v|v["allowed"]==true)
            } else {
                false
            };
            if !active || !grant || inactive(&c).is_err() || self.github.current(&c).await.is_err()
            {
                if c["state"] == "CONNECTION_STATE_CONNECTED" {
                    c["state"] = json!("CONNECTION_STATE_AUTHORITY_LOST");
                }
                c["executionCancellations"] = json!(self.cancel_connection(&mut tx, &c).await?);
                sqlx::query("UPDATE perfine_connections SET data=$2 WHERE id=$1")
                    .bind(row.get::<uuid::Uuid, _>("id"))
                    .bind(&c)
                    .execute(&mut *tx)
                    .await?;
            }
            if let Ok(token) = self.github.token(&c).await {
                let executions:Vec<Value>=sqlx::query_scalar("SELECT data FROM perfine_experiments WHERE connection=$1 AND data->>'origin'='EXECUTION_ORIGIN_GITHUB_ACTIONS' AND data->>'executionState' IN ('EXECUTION_STATE_RUNNING','EXECUTION_STATE_QUEUED')").bind(id(string(&c,"id"))?).fetch_all(&mut *tx).await?;
                for mut e in executions {
                    let run = integer(&e["githubExecution"], "githubRunId");
                    let attempt = integer(&e["githubExecution"], "runAttempt");
                    if run == 0 || attempt == 0 {
                        continue;
                    }
                    if let Ok(remote) = self
                        .github
                        .call(
                            "GET",
                            &format!(
                                "{}/actions/runs/{run}/attempts/{attempt}",
                                crate::github::path(&c["repository"])
                            ),
                            &token,
                            None,
                        )
                        .await
                        && integer(&remote, "id") == run
                        && integer(&remote, "run_attempt") == attempt
                        && remote["status"] == "completed"
                    {
                        let cancelled = remote["conclusion"] == "cancelled";
                        e["executionState"] = json!(if cancelled {
                            "EXECUTION_STATE_CANCELLED"
                        } else if remote["conclusion"] == "success" {
                            "EXECUTION_STATE_COMPLETED"
                        } else {
                            "EXECUTION_STATE_FAILED"
                        });
                        if e.get("cancellation").is_some() {
                            e["cancellation"]["state"] = json!(if cancelled {
                                "CANCELLATION_STATE_CONFIRMED"
                            } else {
                                "CANCELLATION_STATE_FAILED"
                            });
                            e["cancellation"]["reason"] = json!(if cancelled {
                                "GitHub confirmed cancellation"
                            } else {
                                "Run finished before cancellation was confirmed"
                            });
                        }
                        save_experiment(&mut tx, &e).await?;
                    }
                }
            }
            tx.commit().await?;
        }
        Ok(())
    }
    async fn publish_pending(&self) -> Result<()> {
        let ids:Vec<uuid::Uuid>=sqlx::query_scalar("SELECT id FROM perfine_approvals WHERE publication->>'state' IN ('PUBLICATION_STATE_PENDING','PUBLICATION_STATE_RECONCILING') AND COALESCE((publication->>'_nextAttemptAt')::timestamptz,'-infinity'::timestamptz)<=now() ORDER BY COALESCE((publication->>'_nextAttemptAt')::timestamptz,'-infinity'::timestamptz),created_at,id LIMIT 20").fetch_all(&self.db).await?;
        for aid in ids {
            // A session lock spans the committed dispatch intent and external request. A process crash releases it.
            let mut db = self.db.acquire().await?;
            let locked: bool =
                sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1,1))")
                    .bind(aid.to_string())
                    .fetch_one(&mut *db)
                    .await?;
            if !locked {
                continue;
            }
            let space:Option<uuid::Uuid>=sqlx::query_scalar("SELECT e.space FROM perfine_experiments e JOIN perfine_approvals a ON a.experiment=e.id WHERE a.id=$1").bind(aid).fetch_optional(&mut *db).await?;
            let result = if let Some(space) = space {
                sqlx::query("SELECT pg_advisory_lock(hashtextextended($1,0))")
                    .bind(space.to_string())
                    .execute(&mut *db)
                    .await?;
                // Persist a due-time claim before any fallible authority/provider
                // work. Blocked approvals must yield the next batch to other rows.
                let claim = sqlx::query("UPDATE perfine_approvals SET publication=jsonb_set(publication,'{_nextAttemptAt}',to_jsonb(now()+interval '1 minute')) WHERE id=$1 AND publication->>'state' IN ('PUBLICATION_STATE_PENDING','PUBLICATION_STATE_RECONCILING') AND COALESCE((publication->>'_nextAttemptAt')::timestamptz,'-infinity'::timestamptz)<=now()")
                    .bind(aid).execute(&mut *db).await;
                let result = match claim {
                    Ok(claim) if claim.rows_affected() == 1 => self.publish_one(&mut db, aid).await,
                    Ok(_) => Ok(()),
                    Err(error) => Err(error.into()),
                };
                sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1,0))")
                    .bind(space.to_string())
                    .execute(&mut *db)
                    .await?;
                result
            } else {
                Ok(())
            };
            sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1,1))")
                .bind(aid.to_string())
                .execute(&mut *db)
                .await?;
            if result.is_err() {
                tracing::warn!(approval_id=%aid,"Publication pending; dependency or authority check failed");
            }
        }
        Ok(())
    }
    async fn publish_one(&self, db: &mut PgConnection, aid: uuid::Uuid) -> Result<()> {
        let row=sqlx::query("SELECT a.data,a.publication,a.attempts,e.data AS experiment FROM perfine_approvals a JOIN perfine_experiments e ON e.id=a.experiment WHERE a.id=$1").bind(aid).fetch_one(&mut *db).await?;
        let a: Value = row.get("data");
        let e: Value = row.get("experiment");
        let mut p: Value = row.get("publication");
        let attempts:i64=sqlx::query_scalar("SELECT COALESCE(sum(attempts),0)::bigint FROM perfine_approvals WHERE data->'draft'->>'candidateId'=$1").bind(string(&a["draft"],"candidateId")).fetch_one(&mut *db).await?;
        tombstone(db, string(&e, "id")).await?;
        let space = string(&e, "spaceId");
        if self.authority.states(&[space.into()]).await?
            != [server_common::authority::TenantState::Active]
        {
            return Err(Fault::denied());
        }
        for subject in [
            string(&a, "approverSubject"),
            string(&p, "_publisherSubject"),
        ] {
            id(subject).map_err(|_| Fault::denied())?;
            let v = self
                .authority
                .call(
                    "Authorize",
                    &format!("Bearer {}", self.authority.service_token().await?),
                    json!({"tenantId":space,"userId":subject,"capability":"publish","paid":false}),
                )
                .await?;
            if v["allowed"] != true {
                return Err(Fault::denied());
            }
        }
        let c = connection(db, space, string(&e, "connectionId")).await?;
        inactive(&c)?;
        let d = &a["draft"];
        let token = self.github.publication_tokens(&c, d).await?;
        self.github.binding(d, &token).await?;
        let candidate: Value =
            sqlx::query_scalar("SELECT data FROM perfine_candidates WHERE id=$1 AND experiment=$2")
                .bind(id(string(d, "candidateId"))?)
                .bind(id(string(&e, "id"))?)
                .fetch_one(&mut *db)
                .await?;
        self.github.production_diff(d, &candidate, &token).await?;
        let path = crate::github::path(&d["targetRepository"]);
        // Reconcile every attempt, including a new approval for an already published candidate.
        let head = format!(
            "{}:{}",
            string(&d["headRepository"], "owner"),
            string(d, "headRef")
        );
        let prs = self
            .github
            .call(
                "GET",
                &format!(
                    "{path}/pulls?state=all&head={}&base={}&per_page=100",
                    crate::storage::escape(&head),
                    crate::storage::escape(string(d, "baseRef"))
                ),
                &token.target,
                None,
            )
            .await?;
        let prs = prs.as_array().ok_or_else(Fault::unavailable)?;
        if prs.len() == 100 {
            return Err(Fault::precondition(
                "publication reconciliation requires operator inspection",
            ));
        }
        let matches_approval = |v: &Value| {
            v["head"]["sha"] == d["headSha"]
                && v["base"]["sha"] == d["baseSha"]
                && v["title"] == d["title"]
                && v["body"] == d["body"]
                && v["draft"] == true
        };
        let found = prs.iter().find(|v| matches_approval(v));
        if let Some(pr) = found {
            p["state"] = json!("PUBLICATION_STATE_PUBLISHED");
            p["githubPullRequestNumber"] = json!(integer(pr, "number").to_string());
            p["githubPullRequestUrl"] = pr["html_url"].clone();
        } else if prs.iter().any(|v| v["head"]["sha"] == d["headSha"]) {
            p["state"] = json!("PUBLICATION_STATE_FAILED");
            p["failureReason"] =
                json!("A PR for this source exists with changed content or state; inspect GitHub");
        } else if attempts > 0 {
            // An empty list does not prove non-delivery: GitHub may expose a created PR late.
            // Keep reconciling until a remote outcome is known; never duplicate an uncertain POST.
            p["state"] = json!("PUBLICATION_STATE_RECONCILING");
            p["failureReason"] = json!(
                "Creation outcome remains uncertain; inspect GitHub before operator recovery"
            );
        } else {
            p["state"] = json!("PUBLICATION_STATE_RECONCILING");
            sqlx::query(
                "UPDATE perfine_approvals SET publication=$2,attempts=attempts+1 WHERE id=$1",
            )
            .bind(aid)
            .bind(&p)
            .execute(&mut *db)
            .await?;
            // Persist intent before dispatch. Lost replies are reconciled on the next maintenance cycle.
            let result=self.github.call("POST",&format!("{path}/pulls"),&token.target,Some(json!({"title":d["title"],"body":d["body"],"base":d["baseRef"],"head":head,"head_repo":d["headRepository"]["name"],"draft":true}))).await;
            if let Ok(pr) = result {
                p["githubPullRequestNumber"] = json!(integer(&pr, "number").to_string());
                p["githubPullRequestUrl"] = pr["html_url"].clone();
                if matches_approval(&pr) {
                    p["state"] = json!("PUBLICATION_STATE_PUBLISHED");
                } else {
                    // GitHub creates from mutable branch names, so the response must
                    // match the approved snapshot even after preflight SHA checks pass.
                    p["state"] = json!("PUBLICATION_STATE_FAILED");
                    p["failureReason"] = json!(
                        "Created PR differs from the approved binding; inspect GitHub and obtain new approval"
                    );
                }
            }
        }
        sqlx::query("UPDATE perfine_approvals SET publication=$2 WHERE id=$1")
            .bind(aid)
            .bind(p)
            .execute(db)
            .await?;
        Ok(())
    }
}
fn validate_spec(s: &Value) -> Result<()> {
    if integer(s, "schemaVersion") != 1 || s["workloadRepresentativenessConfirmed"] != true {
        return Err(Fault::invalid("v1 and workload confirmation required"));
    }
    for k in ["baseSha", "initialBaseSha", "harnessSha"] {
        sha(string(s, k), 40)?;
    }
    sha(string(s, "fingerprintSha256"), 64)?;
    let limits = required(s, "limits")?;
    if integer(limits, "candidateCount") == 0
        || integer(limits, "simultaneousCandidates") == 0
        || integer(limits, "simultaneousCandidates") > 5
        || integer(limits, "simultaneousCandidates") > integer(limits, "candidateCount")
    {
        return Err(Fault::invalid(
            "positive candidate limits and concurrency <= 5 required",
        ));
    }
    chrono::DateTime::parse_from_rfc3339(string(limits, "startDeadline"))
        .map_err(|_| Fault::invalid("deadline required"))?;
    let timeout = string(limits, "candidateTimeout")
        .trim_end_matches('s')
        .parse::<f64>()
        .map_err(|_| Fault::invalid("timeout required"))?;
    if !timeout.is_finite() || timeout <= 0.0 {
        return Err(Fault::invalid("positive timeout required"));
    }
    let policy = required(s, "policy")?;
    let environment = required(s, "environment")?;
    if !matches!(
        string(s, "language"),
        "LANGUAGE_RUST"
            | "LANGUAGE_C"
            | "LANGUAGE_CPP"
            | "LANGUAGE_ZIG"
            | "LANGUAGE_GO"
            | "LANGUAGE_JAVASCRIPT"
    ) {
        return Err(Fault::invalid("language required"));
    }
    if integer(policy, "schemaVersion") != 1
        || !matches!(
            string(policy, "kind"),
            "POLICY_KIND_CALLGRIND_IR" | "POLICY_KIND_DECLARED" | "POLICY_KIND_MEASUREMENTS_ONLY"
        )
    {
        return Err(Fault::invalid("versioned policy required"));
    }
    sha(string(policy, "policySha256"), 64)?;
    let metric = required(policy, "metric")?;
    if string(metric, "name").is_empty()
        || string(metric, "unit").is_empty()
        || !matches!(
            string(metric, "objective"),
            "METRIC_OBJECTIVE_MINIMIZE" | "METRIC_OBJECTIVE_MAXIMIZE"
        )
    {
        return Err(Fault::invalid("metric and objective required"));
    }
    if policy["kind"] == "POLICY_KIND_DECLARED" {
        required(policy, "uncertainty")?;
        if integer(policy, "repetitions") < 2 {
            return Err(Fault::invalid("declared repetitions required"));
        }
    }
    for key in ["minimumImprovement", "maximumCaseRegression"] {
        let ratio = policy[key].as_f64().unwrap_or(0.0);
        if !ratio.is_finite() || !(0.0..1.0).contains(&ratio) {
            return Err(Fault::invalid("policy ratio invalid"));
        }
    }
    sha(string(environment, "environmentSha256"), 64)?;
    for key in ["operatingSystem", "architecture", "runner", "runtime"] {
        if string(environment, key).is_empty() {
            return Err(Fault::invalid("environment identity required"));
        }
    }
    if environment["tools"].as_array().is_none_or(|tools| {
        tools.is_empty()
            || tools
                .iter()
                .any(|v| string(v, "name").is_empty() || string(v, "version").is_empty())
    }) {
        return Err(Fault::invalid("tool identities required"));
    }
    let mut names = std::collections::HashSet::new();
    if let Some(cases) = s["cases"].as_array() {
        for case in cases {
            let frequency = case["frequency"].as_f64().unwrap_or(0.0);
            if !names.insert(string(case, "id"))
                || string(case, "id").is_empty()
                || string(case, "family").is_empty()
                || !frequency.is_finite()
                || frequency <= 0.0
                || !matches!(
                    string(case, "frequencySource"),
                    "FREQUENCY_SOURCE_DECLARED" | "FREQUENCY_SOURCE_ASSUMED_ONE"
                )
            {
                return Err(Fault::invalid(
                    "case identities and positive frequencies required",
                ));
            }
        }
    }

    if s["cases"].as_array().is_none_or(Vec::is_empty) {
        return Err(Fault::invalid("cases required"));
    }
    Ok(())
}

#[allow(refining_impl_trait)]
impl AccessService for Server {
    async fn get_authorization(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetAuthorizationRequest>,
    ) -> ServiceResult<GetAuthorizationResponse> {
        self.handle(
            ctx,
            "GetAuthorization",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
}

#[allow(refining_impl_trait)]
impl ConnectionService for Server {
    async fn get_raw_artifact_consent(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetRawArtifactConsentRequest>,
    ) -> ServiceResult<GetRawArtifactConsentResponse> {
        self.handle(
            ctx,
            "GetRawArtifactConsent",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn list_connections(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListConnectionsRequest>,
    ) -> ServiceResult<ListConnectionsResponse> {
        self.handle(
            ctx,
            "ListConnections",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn get_connection(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetConnectionRequest>,
    ) -> ServiceResult<GetConnectionResponse> {
        self.handle(
            ctx,
            "GetConnection",
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
            "ConnectRepository",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn disconnect_repository(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DisconnectRepositoryRequest>,
    ) -> ServiceResult<DisconnectRepositoryResponse> {
        self.handle(
            ctx,
            "DisconnectRepository",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn set_raw_artifact_consent(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, SetRawArtifactConsentRequest>,
    ) -> ServiceResult<SetRawArtifactConsentResponse> {
        self.handle(
            ctx,
            "SetRawArtifactConsent",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
}

#[allow(refining_impl_trait)]
impl ExperimentService for Server {
    async fn prepare_experiment(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, PrepareExperimentRequest>,
    ) -> ServiceResult<PrepareExperimentResponse> {
        self.handle(
            ctx,
            "PrepareExperiment",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn list_experiments(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListExperimentsRequest>,
    ) -> ServiceResult<ListExperimentsResponse> {
        self.handle(
            ctx,
            "ListExperiments",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn get_experiment(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetExperimentRequest>,
    ) -> ServiceResult<GetExperimentResponse> {
        self.handle(
            ctx,
            "GetExperiment",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn start_experiment(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, StartExperimentRequest>,
    ) -> ServiceResult<StartExperimentResponse> {
        self.handle(
            ctx,
            "StartExperiment",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn resume_experiment(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ResumeExperimentRequest>,
    ) -> ServiceResult<ResumeExperimentResponse> {
        self.handle(
            ctx,
            "ResumeExperiment",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn cancel_experiment(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CancelExperimentRequest>,
    ) -> ServiceResult<CancelExperimentResponse> {
        self.handle(
            ctx,
            "CancelExperiment",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn list_candidates(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListCandidatesRequest>,
    ) -> ServiceResult<ListCandidatesResponse> {
        self.handle(
            ctx,
            "ListCandidates",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn get_candidate(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetCandidateRequest>,
    ) -> ServiceResult<GetCandidateResponse> {
        self.handle(
            ctx,
            "GetCandidate",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
}

#[allow(refining_impl_trait)]
impl EvidenceService for Server {
    async fn check_upload_eligibility(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CheckUploadEligibilityRequest>,
    ) -> ServiceResult<CheckUploadEligibilityResponse> {
        self.handle(
            ctx,
            "CheckUploadEligibility",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn begin_upload(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, BeginUploadRequest>,
    ) -> ServiceResult<BeginUploadResponse> {
        self.handle(
            ctx,
            "BeginUpload",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn get_upload(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetUploadRequest>,
    ) -> ServiceResult<GetUploadResponse> {
        self.handle(
            ctx,
            "GetUpload",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn complete_upload(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CompleteUploadRequest>,
    ) -> ServiceResult<CompleteUploadResponse> {
        self.handle(
            ctx,
            "CompleteUpload",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn list_evidence(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListEvidenceRequest>,
    ) -> ServiceResult<ListEvidenceResponse> {
        self.handle(
            ctx,
            "ListEvidence",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn get_evidence_download(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetEvidenceDownloadRequest>,
    ) -> ServiceResult<GetEvidenceDownloadResponse> {
        self.handle(
            ctx,
            "GetEvidenceDownload",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
}

#[allow(refining_impl_trait)]
impl ManagementService for Server {
    async fn delete_experiment(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DeleteExperimentRequest>,
    ) -> ServiceResult<DeleteExperimentResponse> {
        self.handle(
            ctx,
            "DeleteExperiment",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn get_deletion(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetDeletionRequest>,
    ) -> ServiceResult<GetDeletionResponse> {
        self.handle(
            ctx,
            "GetDeletion",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
}

#[allow(refining_impl_trait)]
impl PublicationService for Server {
    async fn approve_draft(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ApproveDraftRequest>,
    ) -> ServiceResult<ApproveDraftResponse> {
        self.handle(
            ctx,
            "ApproveDraft",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn get_approval(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetApprovalRequest>,
    ) -> ServiceResult<GetApprovalResponse> {
        self.handle(
            ctx,
            "GetApproval",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn publish_draft(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, PublishDraftRequest>,
    ) -> ServiceResult<PublishDraftResponse> {
        self.handle(
            ctx,
            "PublishDraft",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn get_publication(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetPublicationRequest>,
    ) -> ServiceResult<GetPublicationResponse> {
        self.handle(
            ctx,
            "GetPublication",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
}

#[allow(refining_impl_trait)]
impl CloudService for Server {
    async fn list_cloud_tools(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListCloudToolsRequest>,
    ) -> ServiceResult<ListCloudToolsResponse> {
        self.handle(
            ctx,
            "ListCloudTools",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;

    #[test]
    fn uploaded_actions_evidence_preserves_remote_lifecycle() {
        for state in [
            "EXECUTION_STATE_RUNNING",
            "EXECUTION_STATE_QUEUED",
            "EXECUTION_STATE_CANCELLED",
            "EXECUTION_STATE_FAILED",
        ] {
            let mut experiment =
                json!({"origin":"EXECUTION_ORIGIN_GITHUB_ACTIONS","executionState":state});
            complete_evidence(&mut experiment);
            assert_eq!(
                experiment["evidenceCompletion"],
                "COMPLETION_STATE_COMPLETE"
            );
            assert_eq!(experiment["executionState"], state);
        }
        let mut local =
            json!({"origin":"EXECUTION_ORIGIN_LOCAL","executionState":"EXECUTION_STATE_RUNNING"});
        complete_evidence(&mut local);
        assert_eq!(local["executionState"], "EXECUTION_STATE_COMPLETED");
    }
}
