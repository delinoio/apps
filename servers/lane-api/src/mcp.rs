// SPDX-License-Identifier: FSL-1.1-ALv2
//! Stateless Streamable HTTP adapter. Authentication and transactions remain server-owned.
use crate::{Fault, Result, Server};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use connectrpc::ErrorCode;
use delino_protocol::lane::v1::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{future::Future, sync::Arc};

// The March revision requires JSON-RPC batches. Negotiate June instead,
// whose Streamable HTTP contract uses exactly one message per POST.
const VERSIONS: &[&str] = &["2025-06-18"];

/// The seam permits transport tests without substituting a server implementation.
pub trait Backend: Send + Sync + 'static {
    fn authenticate(&self, headers: &HeaderMap) -> impl Future<Output = Result<()>> + Send;
    fn execute(
        &self,
        headers: &HeaderMap,
        service: &'static str,
        method: &'static str,
        arguments: Value,
    ) -> impl Future<Output = Result<Value>> + Send;
    fn resource_metadata_url(&self) -> String;
    fn max_request_bytes(&self) -> usize;
}
impl Backend for Server {
    fn max_request_bytes(&self) -> usize {
        Server::max_request_bytes(self)
    }
    async fn authenticate(&self, headers: &HeaderMap) -> Result<()> {
        self.authenticate_mcp(headers).await.map(|_| ())
    }
    async fn execute(
        &self,
        headers: &HeaderMap,
        service: &'static str,
        method: &'static str,
        arguments: Value,
    ) -> Result<Value> {
        Server::execute(self, headers, service, method, arguments, true).await
    }
    fn resource_metadata_url(&self) -> String {
        self.mcp_resource_metadata_url()
    }
}

/// Merge into the server router behind its configured Origin-validation middleware.
pub fn router(server: Arc<Server>) -> Router {
    router_with_backend(server)
}

/// Construct the same HTTP transport with an injected authentication/dispatch boundary.
pub fn router_with_backend<B: Backend>(backend: Arc<B>) -> Router {
    Router::new()
        .route("/mcp", any(handle::<B>))
        .with_state(backend)
}

struct Tool {
    name: &'static str,
    service: &'static str,
    method: &'static str,
    request: &'static str,
    decode: fn(Value) -> Result<Value>,
}
fn decode<T: DeserializeOwned + Serialize>(value: Value) -> Result<Value> {
    let typed: T = serde_json::from_value(value)?;
    Ok(serde_json::to_value(typed)?)
}
macro_rules! tool {
    ($name:literal, $service:literal, $method:literal, $request:ident) => {
        Tool {
            name: $name,
            service: concat!("lane.v1.", $service),
            method: $method,
            request: stringify!($request),
            decode: decode::<$request>,
        }
    };
}
const TOOLS: &[Tool] = &[
    tool!(
        "lane_begin_attachment_upload",
        "CaptureService",
        "BeginAttachmentUpload",
        BeginAttachmentUploadRequest
    ),
    tool!(
        "lane_complete_attachment_upload",
        "CaptureService",
        "CompleteAttachmentUpload",
        CompleteAttachmentUploadRequest
    ),
    tool!(
        "lane_create_capture_task",
        "CaptureService",
        "CreateCaptureTask",
        CreateCaptureTaskRequest
    ),
    tool!(
        "lane_get_attachment",
        "CaptureService",
        "GetAttachment",
        GetAttachmentRequest
    ),
    tool!(
        "lane_list_url_connections",
        "CaptureService",
        "ListURLConnections",
        ListURLConnectionsRequest
    ),
    tool!(
        "lane_create_url_connection",
        "CaptureService",
        "CreateURLConnection",
        CreateURLConnectionRequest
    ),
    tool!(
        "lane_save_url_connection",
        "CaptureService",
        "SaveURLConnection",
        SaveURLConnectionRequest
    ),
    tool!(
        "lane_delete_url_connection",
        "CaptureService",
        "DeleteURLConnection",
        DeleteURLConnectionRequest
    ),
    tool!(
        "lane_get_repository",
        "LaneService",
        "GetRepository",
        GetRepositoryRequest
    ),
    tool!(
        "lane_list_repositories",
        "LaneService",
        "ListRepositories",
        ListRepositoriesRequest
    ),
    tool!(
        "lane_create_repository",
        "LaneService",
        "CreateRepository",
        CreateRepositoryRequest
    ),
    tool!(
        "lane_save_repository",
        "LaneService",
        "SaveRepository",
        SaveRepositoryRequest
    ),
    tool!(
        "lane_delete_repository",
        "LaneService",
        "DeleteRepository",
        DeleteRepositoryRequest
    ),
    tool!("lane_get_task", "LaneService", "GetTask", GetTaskRequest),
    tool!(
        "lane_list_tasks",
        "LaneService",
        "ListTasks",
        ListTasksRequest
    ),
    tool!(
        "lane_create_task",
        "LaneService",
        "CreateTask",
        CreateTaskRequest
    ),
    tool!("lane_save_task", "LaneService", "SaveTask", SaveTaskRequest),
    tool!(
        "lane_set_progress",
        "LaneService",
        "SetProgress",
        SetProgressRequest
    ),
    tool!(
        "lane_set_proposal",
        "LaneService",
        "SetProposal",
        SetProposalRequest
    ),
    tool!(
        "lane_restore_prompt",
        "LaneService",
        "RestorePrompt",
        RestorePromptRequest
    ),
    tool!(
        "lane_delete_task",
        "LaneService",
        "DeleteTask",
        DeleteTaskRequest
    ),
    tool!(
        "lane_list_history",
        "LaneService",
        "ListHistory",
        ListHistoryRequest
    ),
    tool!(
        "lane_resolve_conflict",
        "LaneService",
        "ResolveConflict",
        ResolveConflictRequest
    ),
    tool!(
        "lane_get_planning_item",
        "PlanningService",
        "GetPlanningItem",
        GetPlanningItemRequest
    ),
    tool!(
        "lane_list_planning_items",
        "PlanningService",
        "ListPlanningItems",
        ListPlanningItemsRequest
    ),
    tool!(
        "lane_create_planning_item",
        "PlanningService",
        "CreatePlanningItem",
        CreatePlanningItemRequest
    ),
    tool!(
        "lane_save_planning_item",
        "PlanningService",
        "SavePlanningItem",
        SavePlanningItemRequest
    ),
    tool!(
        "lane_delete_planning_item",
        "PlanningService",
        "DeletePlanningItem",
        DeletePlanningItemRequest
    ),
    tool!(
        "lane_list_devices",
        "AccountService",
        "ListDevices",
        ListDevicesRequest
    ),
    tool!(
        "lane_disconnect_device",
        "AccountService",
        "DisconnectDevice",
        DisconnectDeviceRequest
    ),
    tool!(
        "lane_list_connections",
        "AccountService",
        "ListConnections",
        ListConnectionsRequest
    ),
    tool!(
        "lane_revoke_connection",
        "AccountService",
        "RevokeConnection",
        RevokeConnectionRequest
    ),
    tool!(
        "lane_register_push",
        "AccountService",
        "RegisterPush",
        RegisterPushRequest
    ),
    tool!(
        "lane_begin_data_deletion",
        "AccountService",
        "BeginDataDeletion",
        BeginDataDeletionRequest
    ),
    tool!(
        "lane_delete_account_data",
        "AccountService",
        "DeleteAccountData",
        DeleteAccountDataRequest
    ),
    tool!(
        "lane_list_receivers",
        "WebhookService",
        "ListReceivers",
        ListReceiversRequest
    ),
    tool!(
        "lane_create_receiver",
        "WebhookService",
        "CreateReceiver",
        CreateReceiverRequest
    ),
    tool!(
        "lane_save_receiver",
        "WebhookService",
        "SaveReceiver",
        SaveReceiverRequest
    ),
    tool!(
        "lane_rotate_receiver_key",
        "WebhookService",
        "RotateReceiverKey",
        RotateReceiverKeyRequest
    ),
    tool!(
        "lane_delete_receiver",
        "WebhookService",
        "DeleteReceiver",
        DeleteReceiverRequest
    ),
    tool!(
        "lane_list_events",
        "WebhookService",
        "ListEvents",
        ListEventsRequest
    ),
    tool!(
        "lane_get_event",
        "WebhookService",
        "GetEvent",
        GetEventRequest
    ),
    tool!(
        "lane_delete_event",
        "WebhookService",
        "DeleteEvent",
        DeleteEventRequest
    ),
    tool!(
        "lane_claim_event",
        "WebhookService",
        "ClaimEvent",
        ClaimEventRequest
    ),
    tool!(
        "lane_renew_event_lease",
        "WebhookService",
        "RenewEventLease",
        RenewEventLeaseRequest
    ),
    tool!(
        "lane_release_event_lease",
        "WebhookService",
        "ReleaseEventLease",
        ReleaseEventLeaseRequest
    ),
    tool!(
        "lane_complete_event",
        "WebhookService",
        "CompleteEvent",
        CompleteEventRequest
    ),
    tool!(
        "lane_reopen_event",
        "WebhookService",
        "ReopenEvent",
        ReopenEventRequest
    ),
];

// Field shapes mirror the canonical ProtoJSON request messages. Generated serde
// types below remain the decoder; these shapes additionally close unknown fields
// and remove authenticated Origin fields from the agent-facing schema.
#[derive(Clone, Copy)]
struct Field {
    name: &'static str,
    kind: &'static str,
    repeated: bool,
}
macro_rules! field {
    ($name:literal, $kind:literal, $repeated:literal) => {
        Field {
            name: $name,
            kind: $kind,
            repeated: $repeated,
        }
    };
}
fn fields(message: &str) -> &'static [Field] {
    match message {
        "BeginAttachmentUploadRequest" => &[
            field!("taskId", "string", false),
            field!("attachmentId", "string", false),
            field!("epoch", "string", false),
        ],
        "CompleteAttachmentUploadRequest" => &[
            field!("taskId", "string", false),
            field!("attachmentId", "string", false),
            field!("uploadId", "string", false),
            field!("epoch", "string", false),
        ],
        "CreateCaptureTaskRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("repositoryId", "string", false),
            field!("title", "string", false),
            field!("description", "string", false),
            field!("images", "CapturedImage", true),
        ],
        "GetAttachmentRequest" => &[
            field!("taskId", "string", false),
            field!("attachmentId", "string", false),
        ],
        "ListURLConnectionsRequest" => &[field!("page", "Page", false)],
        "CreateURLConnectionRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("connection", "URLConnection", false),
        ],
        "SaveURLConnectionRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("expectedVersionId", "string", false),
            field!("connection", "URLConnection", false),
        ],
        "DeleteURLConnectionRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
        ],
        "CapturedImage" => &[
            field!("capture", "CaptureReference", false),
            field!("png", "bytes", false),
        ],
        "CaptureReference" => &[
            field!("attachmentId", "string", false),
            field!("width", "uint32", false),
            field!("height", "uint32", false),
            field!("sha256", "string", false),
            field!("byteSize", "uint64", false),
            field!("pageUrl", "string", false),
            field!("pageTitle", "string", false),
            field!("cssSelector", "string", false),
        ],
        "URLConnection" => &[
            field!("prefix", "string", false),
            field!("repositoryId", "string", false),
        ],
        "BeginDataDeletionRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("mode", "DataDeletionMode", false),
            field!("confirmedAccountId", "string", false),
        ],
        "ClaimEventRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
        ],
        "CompleteEventRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("leaseToken", "string", false),
            field!("tasks", "CreateTaskRequest", true),
        ],
        "CreateReceiverRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("repositoryId", "string", false),
            field!("name", "string", false),
        ],
        "CreateRepositoryRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("url", "string", false),
            field!("displayName", "string", false),
        ],
        "CreateTaskRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("repository", "RepositoryTarget", false),
            field!("title", "string", false),
            field!("description", "string", false),
            field!("prompt", "string", false),
            field!("progress", "Progress", false),
            field!("proposal", "Proposal", false),
            field!("priority", "TaskPriority", false),
            field!("type", "TaskType", false),
            field!("labelIds", "string", true),
            field!("startDate", "string", false),
            field!("dueDate", "string", false),
            field!("storyPoints", "uint32", false),
            field!("milestoneId", "string", false),
            field!("cycleId", "string", false),
            field!("parentTaskId", "string", false),
            field!("dependsOnTaskIds", "string", true),
            field!("relatedTaskIds", "string", true),
            field!("duplicateOfTaskId", "string", false),
            field!("relationshipEdits", "RelationshipEdits", false),
        ],
        "DeleteAccountDataRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("mode", "DataDeletionMode", false),
            field!("confirmedAccountId", "string", false),
            field!("reauthProof", "string", false),
        ],
        "DeleteEventRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
        ],
        "DeleteReceiverRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
        ],
        "DeleteRepositoryRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
        ],
        "DeleteTaskRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
        ],
        "DisconnectDeviceRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("deviceId", "string", false),
        ],
        "EntityVersion" => &[
            field!("formatVersion", "uint32", false),
            field!("entityId", "string", false),
            field!("kind", "EntityKind", false),
            field!("versionId", "string", false),
            field!("parentVersionIds", "string", true),
            field!("createdAt", "google.protobuf.Timestamp", false),
            field!("deleted", "bool", false),
            field!("repository", "Repository", false),
            field!("task", "Task", false),
            field!("urlConnection", "URLConnection", false),
            field!("label", "Label", false),
            field!("milestone", "Milestone", false),
            field!("cycle", "Cycle", false),
        ],
        "GetEventRequest" => &[field!("id", "string", false)],
        "GetRepositoryRequest" => &[field!("id", "string", false)],
        "GetTaskRequest" => &[field!("id", "string", false)],
        "GetPlanningItemRequest" => &[field!("id", "string", false)],
        "DeletePlanningItemRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
        ],
        "ListPlanningItemsRequest" => &[
            field!("page", "Page", false),
            field!("repositoryId", "string", false),
            field!("kind", "EntityKind", false),
        ],
        "CreatePlanningItemRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("item", "PlanningBody", false),
        ],
        "SavePlanningItemRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("expectedVersionId", "string", false),
            field!("item", "PlanningBody", false),
        ],
        "PlanningBody" => &[
            field!("label", "Label", false),
            field!("milestone", "Milestone", false),
            field!("cycle", "Cycle", false),
        ],
        "Label" => &[
            field!("repositoryId", "string", false),
            field!("name", "string", false),
            field!("color", "string", false),
            field!("description", "string", false),
        ],
        "Milestone" => &[
            field!("repositoryId", "string", false),
            field!("name", "string", false),
            field!("description", "string", false),
            field!("targetDate", "string", false),
            field!("completed", "bool", false),
        ],
        "Cycle" => &[
            field!("repositoryId", "string", false),
            field!("name", "string", false),
            field!("description", "string", false),
            field!("startDate", "string", false),
            field!("endDate", "string", false),
        ],
        "RelationshipEdits" => &[
            field!("addSubtaskIds", "string", true),
            field!("removeSubtaskIds", "string", true),
            field!("addBlockedTaskIds", "string", true),
            field!("removeBlockedTaskIds", "string", true),
        ],
        "DateRange" => &[
            field!("from", "string", false),
            field!("to", "string", false),
            field!("includeUnassigned", "bool", false),
        ],
        "PointRange" => &[
            field!("min", "uint32", false),
            field!("max", "uint32", false),
            field!("includeUnassigned", "bool", false),
        ],
        "TaskFilter" => &[
            field!("priorities", "TaskPriority", true),
            field!("includeUnassignedPriority", "bool", false),
            field!("types", "TaskType", true),
            field!("includeUnassignedType", "bool", false),
            field!("labelIds", "string", true),
            field!("includeUnlabeled", "bool", false),
            field!("startDate", "DateRange", false),
            field!("dueDate", "DateRange", false),
            field!("storyPoints", "PointRange", false),
            field!("milestoneIds", "string", true),
            field!("includeNoMilestone", "bool", false),
            field!("cycleIds", "string", true),
            field!("includeNoCycle", "bool", false),
            field!("relationships", "RelationshipKind", true),
            field!("includeNoRelationships", "bool", false),
        ],
        "ListConnectionsRequest" => &[field!("page", "Page", false)],
        "ListDevicesRequest" => &[field!("page", "Page", false)],
        "ListEventsRequest" => &[
            field!("page", "Page", false),
            field!("repositoryId", "string", false),
            field!("states", "EventState", true),
        ],
        "ListHistoryRequest" => &[
            field!("id", "string", false),
            field!("page", "Page", false),
            field!("promptsOnly", "bool", false),
        ],
        "ListReceiversRequest" => &[
            field!("page", "Page", false),
            field!("repositoryId", "string", false),
        ],
        "ListRepositoriesRequest" => &[field!("page", "Page", false)],
        "ListTasksRequest" => &[
            field!("page", "Page", false),
            field!("repositoryId", "string", false),
            field!("progress", "Progress", true),
            field!("proposal", "Proposal", true),
            field!("search", "string", false),
            field!("filter", "TaskFilter", false),
            field!("sortKey", "TaskSortKey", false),
            field!("sortDirection", "SortDirection", false),
        ],
        "MutationMeta" => &[field!("requestId", "string", false)],
        "Page" => &[
            field!("size", "uint32", false),
            field!("cursor", "string", false),
        ],
        "RegisterPushRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("deviceId", "string", false),
            field!("provider", "PushProvider", false),
            field!("endpoint", "string", false),
            field!("publicKey", "bytes", false),
            field!("authSecret", "bytes", false),
            field!("enabled", "bool", false),
            field!("content", "NotificationContent", false),
        ],
        "ReleaseEventLeaseRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("leaseToken", "string", false),
            field!("errorReason", "string", false),
        ],
        "RenewEventLeaseRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("leaseToken", "string", false),
        ],
        "ReopenEventRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("expectedRevision", "uint64", false),
        ],
        "Repository" => &[
            field!("normalizedUrl", "string", false),
            field!("displayName", "string", false),
        ],
        "RepositoryTarget" => &[
            field!("id", "string", false),
            field!("url", "string", false),
        ],
        "ResolveConflictRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("expectedHeadIds", "string", true),
            field!("resolved", "EntityVersion", false),
        ],
        "RestorePromptRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("expectedVersionId", "string", false),
            field!("sourceVersionId", "string", false),
        ],
        "RevokeConnectionRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("connectionId", "string", false),
        ],
        "RotateReceiverKeyRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("expectedRevision", "uint64", false),
        ],
        "SaveReceiverRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("expectedRevision", "uint64", false),
            field!("repositoryId", "string", false),
            field!("name", "string", false),
        ],
        "SaveRepositoryRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("expectedVersionId", "string", false),
            field!("url", "string", false),
            field!("displayName", "string", false),
        ],
        "SaveTaskRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("expectedVersionId", "string", false),
            field!("task", "Task", false),
            field!("relationshipEdits", "RelationshipEdits", false),
        ],
        "SetProgressRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("expectedVersionId", "string", false),
            field!("progress", "Progress", false),
        ],
        "SetProposalRequest" => &[
            field!("meta", "MutationMeta", false),
            field!("id", "string", false),
            field!("expectedVersionId", "string", false),
            field!("proposal", "Proposal", false),
        ],
        "Task" => &[
            field!("repositoryId", "string", false),
            field!("title", "string", false),
            field!("description", "string", false),
            field!("prompt", "string", false),
            field!("progress", "Progress", false),
            field!("proposal", "Proposal", false),
            field!("captures", "CaptureReference", true),
            field!("priority", "TaskPriority", false),
            field!("type", "TaskType", false),
            field!("labelIds", "string", true),
            field!("startDate", "string", false),
            field!("dueDate", "string", false),
            field!("storyPoints", "uint32", false),
            field!("milestoneId", "string", false),
            field!("cycleId", "string", false),
            field!("parentTaskId", "string", false),
            field!("dependsOnTaskIds", "string", true),
            field!("relatedTaskIds", "string", true),
            field!("duplicateOfTaskId", "string", false),
        ],
        _ => &[],
    }
}
fn enum_values(kind: &str) -> Option<&'static [&'static str]> {
    match kind {
        "EntityKind" => Some(&[
            "ENTITY_KIND_UNSPECIFIED",
            "ENTITY_KIND_REPOSITORY",
            "ENTITY_KIND_TASK",
            "ENTITY_KIND_URL_CONNECTION",
            "ENTITY_KIND_LABEL",
            "ENTITY_KIND_MILESTONE",
            "ENTITY_KIND_CYCLE",
        ]),
        "TaskPriority" => Some(&[
            "TASK_PRIORITY_UNSPECIFIED",
            "TASK_PRIORITY_URGENT",
            "TASK_PRIORITY_HIGH",
            "TASK_PRIORITY_NORMAL",
            "TASK_PRIORITY_LOW",
        ]),
        "TaskType" => Some(&[
            "TASK_TYPE_UNSPECIFIED",
            "TASK_TYPE_BUG",
            "TASK_TYPE_FEATURE",
            "TASK_TYPE_IMPROVEMENT",
            "TASK_TYPE_DOCUMENTATION",
            "TASK_TYPE_MAINTENANCE",
        ]),
        "RelationshipKind" => Some(&[
            "RELATIONSHIP_KIND_UNSPECIFIED",
            "RELATIONSHIP_KIND_PARENT",
            "RELATIONSHIP_KIND_SUBTASKS",
            "RELATIONSHIP_KIND_DEPENDS_ON",
            "RELATIONSHIP_KIND_BLOCKS",
            "RELATIONSHIP_KIND_RELATED",
            "RELATIONSHIP_KIND_DUPLICATE_OF",
        ]),
        "TaskSortKey" => Some(&[
            "TASK_SORT_KEY_UNSPECIFIED",
            "TASK_SORT_KEY_PRIORITY",
            "TASK_SORT_KEY_TYPE",
            "TASK_SORT_KEY_START_DATE",
            "TASK_SORT_KEY_DUE_DATE",
            "TASK_SORT_KEY_STORY_POINTS",
            "TASK_SORT_KEY_MILESTONE_NAME",
            "TASK_SORT_KEY_CYCLE_START_DATE",
        ]),
        "SortDirection" => Some(&[
            "SORT_DIRECTION_UNSPECIFIED",
            "SORT_DIRECTION_ASCENDING",
            "SORT_DIRECTION_DESCENDING",
        ]),
        "Progress" => Some(&[
            "PROGRESS_UNSPECIFIED",
            "PROGRESS_TODO",
            "PROGRESS_IN_PROGRESS",
            "PROGRESS_BLOCKED",
            "PROGRESS_DONE",
            "PROGRESS_CANCELED",
        ]),
        "Proposal" => Some(&[
            "PROPOSAL_UNSPECIFIED",
            "PROPOSAL_REGULAR",
            "PROPOSAL_PENDING",
            "PROPOSAL_ACCEPTED",
            "PROPOSAL_REJECTED",
        ]),
        "PushProvider" => Some(&[
            "PUSH_PROVIDER_UNSPECIFIED",
            "PUSH_PROVIDER_APNS",
            "PUSH_PROVIDER_FCM",
            "PUSH_PROVIDER_WEB_PUSH",
        ]),
        "NotificationContent" => Some(&[
            "NOTIFICATION_CONTENT_UNSPECIFIED",
            "NOTIFICATION_CONTENT_DETAIL",
            "NOTIFICATION_CONTENT_GENERIC",
        ]),
        "DataDeletionMode" => Some(&[
            "DATA_DELETION_MODE_UNSPECIFIED",
            "DATA_DELETION_MODE_DELETE_EVERYWHERE",
            "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL",
        ]),
        "EventState" => Some(&[
            "EVENT_STATE_UNSPECIFIED",
            "EVENT_STATE_PENDING",
            "EVENT_STATE_LEASED",
            "EVENT_STATE_COMPLETED",
            "EVENT_STATE_DELETED",
        ]),
        _ => None,
    }
}

fn oneof(message: &str) -> &[&str] {
    match message {
        "RepositoryTarget" => &["id", "url"],
        "EntityVersion" => &[
            "repository",
            "task",
            "urlConnection",
            "label",
            "milestone",
            "cycle",
        ],
        "PlanningBody" => &["label", "milestone", "cycle"],
        _ => &[],
    }
}
fn schema(kind: &str) -> Value {
    if let Some(values) = enum_values(kind) {
        return json!({"oneOf": [{"type":"string", "enum":values},
            {"type":"integer", "enum":(0..values.len()).collect::<Vec<_>>()}]});
    }
    match kind {
        "string" => json!({"type":"string"}),
        "bytes" => json!({"type":"string", "contentEncoding":"base64"}),
        "bool" => json!({"type":"boolean"}),
        "uint32" => json!({"type":"integer", "minimum":0, "maximum":u32::MAX}),
        "uint64" => {
            json!({"type":"string", "pattern":"^(0|[1-9][0-9]*)$", "description":"Unsigned 64-bit decimal string."})
        }
        "google.protobuf.Timestamp" => json!({"type":"string", "format":"date-time"}),
        _ => {
            let mut properties = serde_json::Map::new();
            for field in fields(kind) {
                let item = schema(field.kind);
                properties.insert(
                    field.name.into(),
                    if field.repeated {
                        json!({"type":"array", "items":item})
                    } else {
                        item
                    },
                );
            }
            let mut result =
                json!({"type":"object", "properties":properties, "additionalProperties":false});
            let choices = oneof(kind);
            if !choices.is_empty() {
                // Protobuf permits an absent oneof. Business validation decides
                // whether a particular command requires a selected member. Every
                // pair is excluded separately: one `not` over all members would
                // advertise invalid pairs as valid, while runtime validation
                // rejects any two members.
                let pairs = choices
                    .iter()
                    .enumerate()
                    .flat_map(|(index, first)| {
                        choices[index + 1..]
                            .iter()
                            .map(move |second| json!({"not":{"required":[first, second]}}))
                    })
                    .collect::<Vec<_>>();
                result["allOf"] = Value::Array(pairs);
            }
            result
        }
    }
}
fn validate(kind: &str, value: &Value) -> Result<()> {
    let invalid = || Fault::invalid("arguments do not match the tool input schema");
    if let Some(values) = enum_values(kind) {
        return if value.as_str().is_some_and(|v| values.contains(&v))
            || value.as_u64().is_some_and(|v| v < values.len() as u64)
        {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    let valid = match kind {
        "string" | "bytes" | "google.protobuf.Timestamp" => value.is_string(),
        "bool" => value.is_boolean(),
        "uint32" => value.as_u64().is_some_and(|v| u32::try_from(v).is_ok()),
        "uint64" => value
            .as_str()
            .is_some_and(|v| v.parse::<u64>().is_ok_and(|n| n.to_string() == v)),
        _ => {
            let object = value.as_object().ok_or_else(invalid)?;
            if oneof(kind)
                .iter()
                .filter(|key| object.contains_key(**key))
                .count()
                > 1
            {
                return Err(invalid());
            }
            for (key, value) in object {
                let field = fields(kind)
                    .iter()
                    .find(|field| field.name == key)
                    .ok_or_else(invalid)?;
                if field.repeated {
                    for item in value.as_array().ok_or_else(invalid)? {
                        validate(field.kind, item)?;
                    }
                } else {
                    validate(field.kind, value)?;
                }
            }
            true
        }
    };
    if valid { Ok(()) } else { Err(invalid()) }
}
fn tool_description(tool: &Tool) -> String {
    let warning = match tool.method {
        "DeleteRepository" => {
            " Irreversibly deletes the repository and its tasks. Invocation confirms deletion."
        }
        "DeleteTask" | "DeleteEvent" => {
            " Irreversibly deletes this item. Invocation confirms deletion."
        }
        "DeletePlanningItem" => {
            " Irreversibly deletes this label, milestone or cycle and removes it from every task that uses it. Invocation confirms deletion."
        }
        "DeleteReceiver" => " Stops intake; retains previously received events.",
        "BeginDataDeletion" => {
            " Starts mandatory human browser reauthentication for account-data deletion."
        }
        "DeleteAccountData" => {
            " Irreversibly clears server data and disconnects devices and grants. Requires explicit account confirmation and a fresh one-use browser reauthentication proof; never fabricate a proof."
        }
        "CreateReceiver" | "RotateReceiverKey" => {
            " Returns a secret once; protect it. Rotation immediately invalidates the previous key."
        }
        "CompleteEvent" => {
            " Requires a live online lease. Task creation and event completion are atomic."
        }
        _ => "",
    };
    format!(
        "Online-only. {}.{} using canonical Lane ProtoJSON arguments. Origin is set by the authenticated server.{}",
        tool.service, tool.method, warning
    )
}
fn tools() -> Value {
    json!({"tools": TOOLS.iter().map(|tool| json!({
        "name":tool.name, "description":tool_description(tool), "inputSchema":schema(tool.request),
        "annotations":{"readOnlyHint":tool.method.starts_with("Get") || tool.method.starts_with("List"),
            "destructiveHint": !tool.method.starts_with("Get") && !tool.method.starts_with("List"),
            "openWorldHint":false},
        "_meta":{"io.delino.lane/onlineOnly":true}
    })).collect::<Vec<_>>()})
}
fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "error":{"code":code, "message":message}})
}
fn rpc_result(id: Value, result: Value) -> Response {
    Json(json!({"jsonrpc":"2.0", "id":id, "result":result})).into_response()
}
fn protocol_error(id: Value, code: i32, message: &str) -> Response {
    Json(rpc_error(id, code, message)).into_response()
}
fn fault_data(fault: &Fault) -> Value {
    let mut data = json!({"code":fault.code.as_str(), "reason":fault.reason & 0xff, "message":fault.message,
        "correlationId":crate::id(), "retryable":matches!(fault.code, ErrorCode::Unavailable | ErrorCode::ResourceExhausted)});
    if let Some(violation) = crate::planning::violation_of(fault) {
        data["violation"] = json!(violation.name());
    }
    data
}
fn authentication_error<B: Backend>(backend: &B, fault: Fault, id: Value) -> Response {
    let mut body = rpc_error(id, -32000, "MCP authorization failed");
    body["error"]["data"] = fault_data(&fault);
    let mut response = (fault.code.http_status(), Json(body)).into_response();
    if fault.code == ErrorCode::Unauthenticated {
        // Use configured public metadata, never an untrusted Host header.
        let challenge = format!(
            "Bearer resource_metadata=\"{}\"",
            backend.resource_metadata_url()
        );
        if let Ok(value) = HeaderValue::from_str(&challenge) {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, value);
        }
    }
    response
}
fn accepts(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get_all(header::ACCEPT)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|part| {
            part.split(';')
                .next()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case(expected))
        })
}

async fn handle<B: Backend>(State(backend): State<Arc<B>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    if let Err(fault) = backend.authenticate(&headers).await {
        return authentication_error(backend.as_ref(), fault, Value::Null);
    }
    if parts.method != axum::http::Method::POST {
        // No server-initiated SSE stream or session termination exists in this
        // stateless transport. Authentication still protects these requests.
        return (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response();
    }
    if !headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
        })
    {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    if !accepts(&headers, "application/json") || !accepts(&headers, "text/event-stream") {
        return StatusCode::NOT_ACCEPTABLE.into_response();
    }
    if let Some(version) = headers.get("mcp-protocol-version")
        && !version.to_str().is_ok_and(|v| VERSIONS.contains(&v))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(rpc_error(
                Value::Null,
                -32600,
                "unsupported MCP protocol version",
            )),
        )
            .into_response();
    }
    let body = match to_bytes(body, backend.max_request_bytes()).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let message: Value = match serde_json::from_slice(&body) {
        Ok(message) => message,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(rpc_error(Value::Null, -32700, "Parse error")),
            )
                .into_response();
        }
    };
    let object = match message.as_object() {
        Some(object) if object.get("jsonrpc") == Some(&json!("2.0")) => object,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(rpc_error(Value::Null, -32600, "Invalid Request")),
            )
                .into_response();
        }
    };
    let id = object.get("id").cloned();
    if id
        .as_ref()
        .is_some_and(|id| !(id.is_string() || id.as_i64().is_some() || id.as_u64().is_some()))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(rpc_error(Value::Null, -32600, "Invalid request ID")),
        )
            .into_response();
    }
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        // There are no server requests, so unsolicited responses cannot be matched.
        return (
            StatusCode::BAD_REQUEST,
            Json(rpc_error(Value::Null, -32600, "Invalid Request")),
        )
            .into_response();
    };
    if object.contains_key("result") || object.contains_key("error") {
        return (
            StatusCode::BAD_REQUEST,
            Json(rpc_error(Value::Null, -32600, "Invalid Request")),
        )
            .into_response();
    }
    let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
    if !params.is_object() {
        return match id {
            Some(id) => protocol_error(id, -32602, "params must be an object"),
            None => StatusCode::BAD_REQUEST.into_response(),
        };
    }
    let Some(id) = id else {
        // Never execute a tool sent as a notification: it has no result channel.
        return if method.starts_with("notifications/") {
            StatusCode::ACCEPTED.into_response()
        } else {
            StatusCode::BAD_REQUEST.into_response()
        };
    };
    match method {
        "initialize" => {
            let Some(requested) = params.get("protocolVersion").and_then(Value::as_str) else {
                return protocol_error(id, -32602, "protocolVersion is required");
            };
            if !params.get("capabilities").is_some_and(Value::is_object)
                || !params
                    .pointer("/clientInfo/name")
                    .is_some_and(Value::is_string)
                || !params
                    .pointer("/clientInfo/version")
                    .is_some_and(Value::is_string)
            {
                return protocol_error(id, -32602, "clientInfo and capabilities are required");
            }
            let version = if VERSIONS.contains(&requested) {
                requested
            } else {
                VERSIONS[VERSIONS.len() - 1]
            };
            rpc_result(
                id,
                json!({"protocolVersion":version, "capabilities":{"tools":{"listChanged":false}},
                "serverInfo":{"name":"lane", "version":env!("CARGO_PKG_VERSION")},
                "instructions":"All tools require an approved online Lane MCP grant. The server sets origin. Destructive tool invocation confirms the described deletion; account-data deletion additionally requires human browser reauthentication."}),
            )
        }
        "ping" => rpc_result(id, json!({})),
        "tools/list" => {
            if params.get("cursor").is_some_and(|v| v != "") {
                return protocol_error(id, -32602, "This tool catalog has no continuation cursor");
            }
            rpc_result(id, tools())
        }
        "tools/call" => {
            let Some(tool) = params
                .get("name")
                .and_then(Value::as_str)
                .and_then(|name| TOOLS.iter().find(|t| t.name == name))
            else {
                return protocol_error(id, -32602, "Unknown tool name");
            };
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let arguments =
                match validate(tool.request, &arguments).and_then(|()| (tool.decode)(arguments)) {
                    Ok(arguments) => arguments,
                    Err(_) => return protocol_error(id, -32602, "Invalid tool arguments"),
                };
            let started = std::time::Instant::now();
            let result = backend
                .execute(&headers, tool.service, tool.method, arguments)
                .await;
            // Operation, outcome category and duration only: never task, name or description text.
            tracing::info!(
                operation = tool.method,
                success = result.is_ok(),
                result = %result.as_ref().map_or_else(crate::planning::category, |_| "ok".to_owned()),
                duration_ms = started.elapsed().as_millis() as u64,
                category = "mcp",
                "MCP operation completed"
            );
            match result {
                Ok(value) => rpc_result(
                    id,
                    json!({"content":[{"type":"text","text":value.to_string()}], "structuredContent":value, "isError":false}),
                ),
                Err(fault) if fault.code == ErrorCode::Unauthenticated => {
                    authentication_error(backend.as_ref(), fault, id)
                }
                Err(fault) => {
                    let error = json!({"error":fault_data(&fault)});
                    rpc_result(
                        id,
                        json!({"content":[{"type":"text","text":error.to_string()}], "structuredContent":error, "isError":true}),
                    )
                }
            }
        }
        _ => protocol_error(id, -32601, "Method not found"),
    }
}
