// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::{Fault, Result, Server, account_lock, string};
use axum::http::HeaderMap;
use connectrpc::{RequestContext, Response, ServiceRequest, ServiceResult, ServiceStream};
use delino_protocol::lane::v1::*;
use serde_json::{Value, json};

impl Server {
    pub async fn authenticate_mcp(&self, headers: &HeaderMap) -> Result<crate::Principal> {
        let p = self
            .auth
            .authenticate_resource(headers, &format!("{}/mcp", self.config.public_url))
            .await?;
        let mut tx = self.db.begin().await?;
        account_lock(&mut tx, &p.account_id).await?;
        self.auth.authorize(&mut tx, &p).await?;
        tx.commit().await?;
        Ok(p)
    }
    pub fn mcp_resource_metadata_url(&self) -> String {
        format!(
            "{}/.well-known/oauth-protected-resource/mcp",
            self.config.public_url
        )
    }
    pub async fn execute(
        &self,
        headers: &HeaderMap,
        service: &str,
        method: &str,
        q: Value,
        mcp: bool,
    ) -> Result<Value> {
        let service = service.strip_prefix("lane.v1.").unwrap_or(service);
        let mut principal = if mcp {
            self.auth
                .authenticate_resource(headers, &format!("{}/mcp", self.config.public_url))
                .await?
        } else {
            self.auth.authenticate(headers).await?
        };
        if !mcp
            && !method.starts_with("Get")
            && !method.starts_with("List")
            && !matches!(method, "PullChanges" | "WatchChanges")
        {
            self.auth.check_csrf(headers, &principal).await?;
        }
        if mcp {
            principal.origin_kind = "ORIGIN_KIND_MCP".into();
        }
        crate::rate::check(
            &self.db,
            "account",
            &principal.account_id,
            self.config.requests_per_minute,
        )
        .await?;
        if service == "SyncService" && method == "PushChanges" {
            return self.push_changes(&principal, &q).await;
        }
        let mut tx = self.db.begin().await?;
        account_lock(&mut tx, &principal.account_id).await?;
        self.auth.authorize(&mut tx, &principal).await?;
        if service == "SyncService" && method == "PullChanges" {
            if q["formatVersion"].as_u64() != Some(crate::entities::FORMAT_VERSION) {
                return Err(Fault::new(
                    connectrpc::ErrorCode::InvalidArgument,
                    8,
                    "unsupported sync format",
                ));
            }
            if let Some(out) = crate::control::deletion_pull(&mut tx, &principal, &q).await? {
                tx.commit().await?;
                return Ok(out);
            }
        }
        let output = match service {
            "CaptureService" => {
                crate::capture::dispatch(
                    &mut tx,
                    &principal,
                    method,
                    &q,
                    self.config.max_item_bytes,
                )
                .await?
            }
            "LaneService" | "PlanningService" => {
                crate::entities::dispatch(
                    &mut tx,
                    &principal,
                    method,
                    &q,
                    self.config.max_item_bytes,
                )
                .await?
            }
            "SyncService" => {
                crate::sync::dispatch(&mut tx, &principal, method, &q, self.config.max_item_bytes)
                    .await?
            }
            "WebhookService" => {
                crate::webhooks::dispatch_with_config(
                    &mut tx,
                    &principal,
                    method,
                    &q,
                    self.config.max_item_bytes,
                    Some(&self.webhooks),
                )
                .await?
            }
            "AccountService" => {
                self.account_dispatch(&mut tx, &principal, method, &q)
                    .await?
            }
            _ => return Err(Fault::invalid("unknown service")),
        };
        tx.commit().await?;
        Ok(output)
    }
    async fn watch_tick(&self, headers: &HeaderMap, q: &Value) -> Result<Value> {
        let p = self.auth.authenticate(headers).await?;
        let mut tx = self.db.begin().await?;
        account_lock(&mut tx, &p.account_id).await?;
        self.auth.authorize(&mut tx, &p).await?;
        let out = crate::sync::dispatch(&mut tx, &p, "WatchChanges", q, self.config.max_item_bytes)
            .await?;
        tx.commit().await?;
        Ok(out)
    }
    async fn push_changes(&self, p: &crate::Principal, q: &Value) -> Result<Value> {
        let mut results = Vec::new();
        let mut mappings = Vec::new();
        let empty = Vec::new();
        let items = q
            .get("mutations")
            .and_then(Value::as_array)
            .unwrap_or(&empty);
        // Even an empty batch checks epoch, device binding, format, and current authorization.
        if items.is_empty() {
            let mut tx = self.db.begin().await?;
            account_lock(&mut tx, &p.account_id).await?;
            self.auth.authorize(&mut tx, p).await?;
            let info = crate::sync::dispatch(
                &mut tx,
                p,
                "GetSyncInfo",
                &json!({}),
                self.config.max_item_bytes,
            )
            .await?;
            if q["formatVersion"].as_u64() != Some(crate::entities::FORMAT_VERSION) {
                return Err(Fault::new(
                    connectrpc::ErrorCode::FailedPrecondition,
                    8,
                    "unsupported sync format",
                ));
            }
            if q["epoch"] != info["epoch"] {
                return Err(Fault::new(
                    connectrpc::ErrorCode::FailedPrecondition,
                    17,
                    "stale account epoch",
                ));
            }
            if string(q, "deviceId") != p.device_id || p.device_id.is_empty() {
                return Err(Fault::denied());
            }
            tx.commit().await?;
            return Ok(json!({"results":[],"repositoryMappings":[]}));
        }
        for item in items {
            let mut request = q.clone();
            request["mutations"] = json!([item]);
            let mut tx = self.db.begin().await?;
            account_lock(&mut tx, &p.account_id).await?;
            self.auth.authorize(&mut tx, p).await?;
            match crate::sync::dispatch(
                &mut tx,
                p,
                "PushChanges",
                &request,
                self.config.max_item_bytes,
            )
            .await
            {
                Ok(out) => {
                    sqlx::query(
                        "DELETE FROM lane_sync_rejections WHERE account_id=$1 AND request_id=$2",
                    )
                    .bind(&p.account_id)
                    .bind(string(&item["meta"], "requestId"))
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    if let Some(items) = out["results"].as_array() {
                        results.extend(items.iter().cloned());
                    }
                    if let Some(items) = out["repositoryMappings"].as_array() {
                        mappings.extend(items.iter().cloned());
                    }
                }
                Err(error) => {
                    tx.rollback().await?;
                    let request_id = string(&item["meta"], "requestId");
                    if crate::canonical_id(request_id) {
                        sqlx::query("INSERT INTO lane_sync_rejections(account_id,request_id,reason) VALUES($1,$2,$3) ON CONFLICT(account_id,request_id) DO UPDATE SET reason=EXCLUDED.reason").bind(&p.account_id).bind(request_id).bind(error.reason).execute(&self.db).await?;
                    }
                    results.push(json!({"requestId":item["meta"]["requestId"],"error":crate::error::detail(&error, &crate::id(), string(&item["version"], "entityId"), vec![])}));
                }
            }
        }
        Ok(json!({"results":results,"repositoryMappings":mappings}))
    }
    async fn handle<R: serde::de::DeserializeOwned>(
        &self,
        ctx: RequestContext,
        service: &str,
        method: &str,
        q: Value,
    ) -> ServiceResult<R> {
        let trace = crate::id();
        let started = std::time::Instant::now();
        match self
            .execute(ctx.headers(), service, method, q.clone(), false)
            .await
        {
            Ok(output) => {
                if matches!(service, "LaneService" | "PlanningService") {
                    // Content-free: never log task text, names, descriptions or relationship titles.
                    tracing::info!(correlation_id=%trace,operation=method,result="ok",duration_ms=started.elapsed().as_millis() as u64,"Lane operation completed");
                }
                if matches!(
                    service,
                    "LaneService" | "CaptureService" | "PlanningService"
                ) && output["result"]["result"] == "RESULT_KIND_CONFLICT"
                    && !method.starts_with("Get")
                    && !method.starts_with("List")
                {
                    let mut heads: Vec<String> = output["result"]["alternatives"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|v| string(v, "versionId").to_owned())
                                .collect()
                        })
                        .unwrap_or_default();
                    heads.push(string(&output["result"]["version"], "versionId").to_owned());
                    heads.sort();
                    heads.dedup();
                    let f = Fault::new(
                        connectrpc::ErrorCode::Aborted,
                        3,
                        "stale version; submitted alternative retained",
                    );
                    return Err(crate::error::connect(f, &trace, string(&q, "id"), heads));
                }
                Ok(Response::new(serde_json::from_value(output).map_err(|_| {
                    crate::error::connect(Fault::unavailable(), &trace, "", vec![])
                })?)
                .with_header("x-request-id", trace)
                .with_header("cache-control", "no-store"))
            }
            Err(f) => {
                tracing::warn!(request_id=%trace,correlation_id=%trace,procedure=method,operation=method,code=?f.code,result=%crate::planning::category(&f),duration_ms=started.elapsed().as_millis() as u64,"Lane operation rejected");
                Err(crate::error::connect(f, &trace, string(&q, "id"), vec![]))
            }
        }
    }
}

#[allow(refining_impl_trait)]
impl LaneService for Server {
    async fn get_repository(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetRepositoryRequest>,
    ) -> ServiceResult<GetRepositoryResponse> {
        self.handle(
            ctx,
            "LaneService",
            "GetRepository",
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
            "LaneService",
            "ListRepositories",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn create_repository(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CreateRepositoryRequest>,
    ) -> ServiceResult<CreateRepositoryResponse> {
        self.handle(
            ctx,
            "LaneService",
            "CreateRepository",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn save_repository(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, SaveRepositoryRequest>,
    ) -> ServiceResult<SaveRepositoryResponse> {
        self.handle(
            ctx,
            "LaneService",
            "SaveRepository",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn delete_repository(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DeleteRepositoryRequest>,
    ) -> ServiceResult<DeleteRepositoryResponse> {
        self.handle(
            ctx,
            "LaneService",
            "DeleteRepository",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn get_task(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetTaskRequest>,
    ) -> ServiceResult<GetTaskResponse> {
        self.handle(
            ctx,
            "LaneService",
            "GetTask",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn list_tasks(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListTasksRequest>,
    ) -> ServiceResult<ListTasksResponse> {
        self.handle(
            ctx,
            "LaneService",
            "ListTasks",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn create_task(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CreateTaskRequest>,
    ) -> ServiceResult<CreateTaskResponse> {
        self.handle(
            ctx,
            "LaneService",
            "CreateTask",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn save_task(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, SaveTaskRequest>,
    ) -> ServiceResult<SaveTaskResponse> {
        self.handle(
            ctx,
            "LaneService",
            "SaveTask",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn set_progress(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, SetProgressRequest>,
    ) -> ServiceResult<SetProgressResponse> {
        self.handle(
            ctx,
            "LaneService",
            "SetProgress",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn set_proposal(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, SetProposalRequest>,
    ) -> ServiceResult<SetProposalResponse> {
        self.handle(
            ctx,
            "LaneService",
            "SetProposal",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn restore_prompt(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, RestorePromptRequest>,
    ) -> ServiceResult<RestorePromptResponse> {
        self.handle(
            ctx,
            "LaneService",
            "RestorePrompt",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn delete_task(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DeleteTaskRequest>,
    ) -> ServiceResult<DeleteTaskResponse> {
        self.handle(
            ctx,
            "LaneService",
            "DeleteTask",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn list_history(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListHistoryRequest>,
    ) -> ServiceResult<ListHistoryResponse> {
        self.handle(
            ctx,
            "LaneService",
            "ListHistory",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn resolve_conflict(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ResolveConflictRequest>,
    ) -> ServiceResult<ResolveConflictResponse> {
        self.handle(
            ctx,
            "LaneService",
            "ResolveConflict",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
}

#[allow(refining_impl_trait)]
impl PlanningService for Server {
    async fn get_planning_item(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetPlanningItemRequest>,
    ) -> ServiceResult<GetPlanningItemResponse> {
        self.handle(
            ctx,
            "PlanningService",
            "GetPlanningItem",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn list_planning_items(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListPlanningItemsRequest>,
    ) -> ServiceResult<ListPlanningItemsResponse> {
        self.handle(
            ctx,
            "PlanningService",
            "ListPlanningItems",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn create_planning_item(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CreatePlanningItemRequest>,
    ) -> ServiceResult<CreatePlanningItemResponse> {
        self.handle(
            ctx,
            "PlanningService",
            "CreatePlanningItem",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn save_planning_item(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, SavePlanningItemRequest>,
    ) -> ServiceResult<SavePlanningItemResponse> {
        self.handle(
            ctx,
            "PlanningService",
            "SavePlanningItem",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn delete_planning_item(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DeletePlanningItemRequest>,
    ) -> ServiceResult<DeletePlanningItemResponse> {
        self.handle(
            ctx,
            "PlanningService",
            "DeletePlanningItem",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
}

#[allow(refining_impl_trait)]
impl AccountService for Server {
    async fn list_devices(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListDevicesRequest>,
    ) -> ServiceResult<ListDevicesResponse> {
        self.handle(
            ctx,
            "AccountService",
            "ListDevices",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn disconnect_device(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DisconnectDeviceRequest>,
    ) -> ServiceResult<DisconnectDeviceResponse> {
        self.handle(
            ctx,
            "AccountService",
            "DisconnectDevice",
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
            "AccountService",
            "ListConnections",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn revoke_connection(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, RevokeConnectionRequest>,
    ) -> ServiceResult<RevokeConnectionResponse> {
        self.handle(
            ctx,
            "AccountService",
            "RevokeConnection",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn register_push(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, RegisterPushRequest>,
    ) -> ServiceResult<RegisterPushResponse> {
        self.handle(
            ctx,
            "AccountService",
            "RegisterPush",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn begin_data_deletion(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, BeginDataDeletionRequest>,
    ) -> ServiceResult<BeginDataDeletionResponse> {
        self.handle(
            ctx,
            "AccountService",
            "BeginDataDeletion",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn delete_account_data(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DeleteAccountDataRequest>,
    ) -> ServiceResult<DeleteAccountDataResponse> {
        self.handle(
            ctx,
            "AccountService",
            "DeleteAccountData",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
}

#[allow(refining_impl_trait)]
impl WebhookService for Server {
    async fn list_receivers(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListReceiversRequest>,
    ) -> ServiceResult<ListReceiversResponse> {
        self.handle(
            ctx,
            "WebhookService",
            "ListReceivers",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn create_receiver(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CreateReceiverRequest>,
    ) -> ServiceResult<CreateReceiverResponse> {
        self.handle(
            ctx,
            "WebhookService",
            "CreateReceiver",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn save_receiver(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, SaveReceiverRequest>,
    ) -> ServiceResult<SaveReceiverResponse> {
        self.handle(
            ctx,
            "WebhookService",
            "SaveReceiver",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn rotate_receiver_key(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, RotateReceiverKeyRequest>,
    ) -> ServiceResult<RotateReceiverKeyResponse> {
        self.handle(
            ctx,
            "WebhookService",
            "RotateReceiverKey",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn delete_receiver(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DeleteReceiverRequest>,
    ) -> ServiceResult<DeleteReceiverResponse> {
        self.handle(
            ctx,
            "WebhookService",
            "DeleteReceiver",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn list_events(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListEventsRequest>,
    ) -> ServiceResult<ListEventsResponse> {
        self.handle(
            ctx,
            "WebhookService",
            "ListEvents",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn get_event(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetEventRequest>,
    ) -> ServiceResult<GetEventResponse> {
        self.handle(
            ctx,
            "WebhookService",
            "GetEvent",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn delete_event(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DeleteEventRequest>,
    ) -> ServiceResult<DeleteEventResponse> {
        self.handle(
            ctx,
            "WebhookService",
            "DeleteEvent",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn claim_event(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ClaimEventRequest>,
    ) -> ServiceResult<ClaimEventResponse> {
        self.handle(
            ctx,
            "WebhookService",
            "ClaimEvent",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn renew_event_lease(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, RenewEventLeaseRequest>,
    ) -> ServiceResult<RenewEventLeaseResponse> {
        self.handle(
            ctx,
            "WebhookService",
            "RenewEventLease",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn release_event_lease(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ReleaseEventLeaseRequest>,
    ) -> ServiceResult<ReleaseEventLeaseResponse> {
        self.handle(
            ctx,
            "WebhookService",
            "ReleaseEventLease",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn complete_event(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CompleteEventRequest>,
    ) -> ServiceResult<CompleteEventResponse> {
        self.handle(
            ctx,
            "WebhookService",
            "CompleteEvent",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn reopen_event(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ReopenEventRequest>,
    ) -> ServiceResult<ReopenEventResponse> {
        self.handle(
            ctx,
            "WebhookService",
            "ReopenEvent",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
}

#[allow(refining_impl_trait)]
impl SyncService for Server {
    async fn get_sync_info(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetSyncInfoRequest>,
    ) -> ServiceResult<GetSyncInfoResponse> {
        self.handle(
            ctx,
            "SyncService",
            "GetSyncInfo",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn push_changes(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, PushChangesRequest>,
    ) -> ServiceResult<PushChangesResponse> {
        self.handle(
            ctx,
            "SyncService",
            "PushChanges",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn pull_changes(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, PullChangesRequest>,
    ) -> ServiceResult<PullChangesResponse> {
        self.handle(
            ctx,
            "SyncService",
            "PullChanges",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn watch_changes(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, WatchChangesRequest>,
    ) -> ServiceResult<ServiceStream<WatchChangesResponse>> {
        let q = q.to_owned_message();
        let headers = ctx.headers().clone();
        let server = self.clone();
        let mut request = json!({"epoch":q.epoch,"cursor":q.cursor});
        self.execute(
            &headers,
            "SyncService",
            "WatchChanges",
            request.clone(),
            false,
        )
        .await
        .map_err(|e| crate::error::connect(e, &crate::id(), "", vec![]))?;
        let stream = async_stream::try_stream! {
            let mut previous=String::new();
            let mut ticker=tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                ticker.tick().await;
                let out=server.watch_tick(&headers,&request).await.map_err(|e| crate::error::connect(e,&crate::id(),"",vec![]))?;
                let cursor=string(&out,"cursor").to_owned();
                if cursor!=previous {
                    previous=cursor.clone();
                    request["cursor"]=json!(cursor);
                    yield WatchChangesResponse {cursor,..Default::default()};
                }
            }
        };
        Ok(Response::stream(stream).with_header("cache-control", "no-store"))
    }
}

#[allow(refining_impl_trait)]
impl CaptureService for Server {
    async fn create_capture_task(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CreateCaptureTaskRequest>,
    ) -> ServiceResult<CreateCaptureTaskResponse> {
        self.handle(
            ctx,
            "CaptureService",
            "CreateCaptureTask",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn get_attachment(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, GetAttachmentRequest>,
    ) -> ServiceResult<GetAttachmentResponse> {
        self.handle(
            ctx,
            "CaptureService",
            "GetAttachment",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn list_url_connections(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, ListURLConnectionsRequest>,
    ) -> ServiceResult<ListURLConnectionsResponse> {
        self.handle(
            ctx,
            "CaptureService",
            "ListURLConnections",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn create_url_connection(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CreateURLConnectionRequest>,
    ) -> ServiceResult<CreateURLConnectionResponse> {
        self.handle(
            ctx,
            "CaptureService",
            "CreateURLConnection",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn save_url_connection(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, SaveURLConnectionRequest>,
    ) -> ServiceResult<SaveURLConnectionResponse> {
        self.handle(
            ctx,
            "CaptureService",
            "SaveURLConnection",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn delete_url_connection(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, DeleteURLConnectionRequest>,
    ) -> ServiceResult<DeleteURLConnectionResponse> {
        self.handle(
            ctx,
            "CaptureService",
            "DeleteURLConnection",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn begin_attachment_upload(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, BeginAttachmentUploadRequest>,
    ) -> ServiceResult<BeginAttachmentUploadResponse> {
        self.handle(
            ctx,
            "CaptureService",
            "BeginAttachmentUpload",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
    async fn complete_attachment_upload(
        &self,
        ctx: RequestContext,
        q: ServiceRequest<'_, CompleteAttachmentUploadRequest>,
    ) -> ServiceResult<CompleteAttachmentUploadResponse> {
        self.handle(
            ctx,
            "CaptureService",
            "CompleteAttachmentUpload",
            serde_json::to_value(q.to_owned_message())
                .map_err(|_| Fault::invalid("invalid request").connect())?,
        )
        .await
    }
}
