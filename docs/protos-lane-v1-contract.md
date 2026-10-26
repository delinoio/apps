# Lane v1 Protocol Contract

## Scope

`protos/lane/v1/lane.proto` defines Lane schemas. Checked-in `lane.pb.go`, `lanev1connect/lane.connect.go` and `lane_pb.ts` provide Go and TypeScript bindings. The server protocol crate and Rust CLI build script register the same schema for generation into their respective Cargo output directories. Shared conformance vectors belong in `protos/lane/v1/fixtures/`. These interfaces do not establish runtime or native acceptance. The [application contract](apps-lane-foundation.md) owns complete product behavior.

## Runtime and Language

Use Protobuf package `lane.v1`, Go package `github.com/delinoio/apps/protos/lane/v1`, Connect-Go package `lanev1connect`, and TypeScript exports from `@repo/protos`. Generate server Rust through `servers/crates/delino-protos` and CLI Rust descriptors through `cmds/lane/build.rs`; do not check in generated Rust or duplicate schemas. Use typed messages and enums, not an arbitrary JSON command API. JSON below means the standard ProtoJSON representation of the named message, except the explicit webhook JSON object.

## Users and Operators

ReactLynx hosts, Rust CLI/local MCP, online web, Rust server/remote MCP, and integration-test authors.

## Interfaces and Contracts

### Version envelope

The following names and field numbers reserve the initial schema. Fields not marked repeated or optional are singular. An enum's zero value is `*_UNSPECIFIED` and is invalid in stored entities and mutation input. Unknown format versions stop sync without changing compatible local state.

```proto
message Origin {
  OriginKind kind = 1; // APP=1, CLI=2, MCP=3
  string device_id = 2;
  string device_name = 3;
  string account_id = 4; // Delcore internal user ID; empty for standalone work
}
message Repository {
  string normalized_url = 1; // host/path, without scheme or terminal .git
  optional string display_name = 2;
}
message Task {
  string repository_id = 1;
  string title = 2;
  string description = 3; // Markdown; empty is allowed
  optional string prompt = 4; // absent differs from present empty Markdown
  Progress progress = 5; // TODO=1, IN_PROGRESS=2, BLOCKED=3, DONE=4, CANCELED=5
  Proposal proposal = 6; // REGULAR=1, PENDING=2, ACCEPTED=3, REJECTED=4
  // captures = 7 is defined by the RealQA extension below.
  // Fields 8-19 are the task planning extension below.
}
message EntityVersion {
  uint32 format_version = 1; // 1 to 3; every writer emits 3
  string entity_id = 2;
  EntityKind kind = 3; // REPOSITORY=1, TASK=2, URL_CONNECTION=3, LABEL=4, MILESTONE=5, CYCLE=6
  string version_id = 4;
  repeated string parent_version_ids = 5;
  google.protobuf.Timestamp created_at = 6;
  Origin origin = 7;
  bool deleted = 8;
  bool prompt_restoration = 9; // explicit RestorePrompt snapshot, including unchanged text
  oneof body {
    Repository repository = 10;
    Task task = 11;
    // url_connection = 12 (RealQA); label = 13, milestone = 14, cycle = 15 (planning)
  }
}
message MutationMeta {
  string request_id = 1;
  Origin origin = 2;
}
message EntityResult {
  ResultKind result = 1; // APPLIED=1, REPLAYED=2, DELETED=3, CONFLICT=4
  EntityVersion version = 2;
  repeated EntityVersion alternatives = 3;
  string correlation_id = 4;
}
```

Tables and comments use enum suffixes for readability. Exact protobuf enum prefixes are `ORIGIN_KIND_`, `ENTITY_KIND_`, `PROGRESS_`, `PROPOSAL_`, `RESULT_KIND_`, `MUTATION_KIND_`, `ERROR_REASON_`, `TASK_PRIORITY_`, `TASK_TYPE_`, `CONSTRAINT_VIOLATION_`, `RELATIONSHIP_KIND_`, `TASK_SORT_KEY_` and `SORT_DIRECTION_` for their correspondingly named types. Thus the JSON progress value is `PROGRESS_TODO`, not the UI label or bare `TODO`. All have prefix plus `UNSPECIFIED=0`. ErrorReason numbers, in the order listed in the error section, are 1 through 19. The server types follow the same rule: `PUSH_PROVIDER_`, `NOTIFICATION_CONTENT_`, `DATA_DELETION_MODE_`, `EVENT_STATE_`, `NOTIFICATION_PERMISSION_`, `LOCAL_RETENTION_`. `EntityKindOrRemoteKind` uses prefix `ENTITY_KIND_OR_REMOTE_KIND_` and values REPOSITORY=1, TASK=2, RECEIVER=3, EVENT=4, ACCOUNT=5. Never renumber these values between adapters.

Entity IDs, version IDs, request IDs and device IDs are lowercase UUID v7 strings. Parent IDs are a sorted unique set, never ordered by arrival or timestamps. A normal edit has exactly the current head as its parent. Creation has no parents. Resolution has all observed conflict heads as parents. Version IDs identify immutable content and cannot be reused for changed content. Timestamps use UTC microsecond precision; they display chronology but never determine winners. A deleted version has no body. Deletion retains only minimal identities, deletion versions and replay fingerprints; remove historical bodies, alternatives and prompt content.

`SyncMutation` request IDs are stable for that entity version and use the immutable normalized mutation as their dedupe content. Interactive LaneService receipts remain keyed by their original method/request ID; local-to-server transport receipts use PushChanges item IDs and never reinterpret them as a different interactive request. A resolution whose expected heads changed must receive a new request/version ID after rereading.

`Origin` is assigned by the authenticated adapter. MCP tools cannot supply an agent name or override origin/account/device attribution. A grant without a physical device has an empty device ID and its server-recorded connection name as device name. Clients may supply their registered device identity during sync; the server validates the association. Preserve original origins when importing accepted history.

A task snapshot is the entire conflict unit. A prompt is embedded, so concurrent prompt/title/progress/move changes cannot merge silently. Prompt history consists of snapshots that change prompt presence/content, plus explicit restoration snapshots even when the restored text equals current text. `EntityVersion.prompt_restoration` is true only for a nondeleted task snapshot created by an explicit `RestorePrompt`, including restoration to an absent or unchanged prompt. It is false for repository versions, tombstones and ordinary saves; do not inherit it from a parent snapshot. Preserve this immutable marker in sync, history and request fingerprints. A `SyncMutation` with kind SAVE can carry a restoration snapshot; receivers use the marker when rebuilding prompt-history membership. A missing marker decodes as false and cannot recover an earlier unmarked equal-text restoration. An explicit successful Save creates a version; a transport retry of that Save does not. History exposes before/after, time and origin for title, description, progress, proposal, repository moves, planning fields and relationships. Preserve conflicting alternatives and the resolving version until deletion.

### Typed business service

Use Connect service `lane.v1.LaneService`; each method has `<Method>Request` and `<Method>Response` messages. Request fields below have consecutive field numbers in listed order. Response fields likewise follow the listed order. Reuse the common types rather than separate UI/MCP models. Mutation responses use `EntityResult` where specified. Names are case-sensitive. Omitted expected versions are legal only on creation, not blind updates.

| Method           | Request fields (in field-number order)                                                                                                                                                                      | Response fields                                                                    |
| ---------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| GetRepository    | `string id`                                                                                                                                                                                                 | `EntityResult result`                                                              |
| ListRepositories | `Page page`                                                                                                                                                                                                 | `repeated EntityResult items`, `string next_cursor`                                |
| CreateRepository | `MutationMeta meta`, `string id`, `string url`, `optional string display_name`                                                                                                                              | `EntityResult result`                                                              |
| SaveRepository   | `MutationMeta meta`, `string id`, `string expected_version_id`, `string url`, `optional string display_name`                                                                                                | `EntityResult result`                                                              |
| DeleteRepository | `MutationMeta meta`, `string id`                                                                                                                                                                            | `EntityResult result`                                                              |
| GetTask          | `string id`                                                                                                                                                                                                 | `EntityResult result`                                                              |
| ListTasks        | `Page page`, `optional string repository_id`, `repeated Progress progress`, `repeated Proposal proposal`, `string search`, `TaskFilter filter`, `TaskSortKey sort_key`, `SortDirection sort_direction`      | `repeated EntityResult items`, `string next_cursor`                                |
| CreateTask       | `MutationMeta meta`, `string id`, `RepositoryTarget repository`, `string title`, `string description`, `optional string prompt`, `Progress progress`, `Proposal proposal`, planning fields 9-21 (see below) | `EntityResult result`, `EntityResult repository`, `repeated EntityResult affected` |
| SaveTask         | `MutationMeta meta`, `string id`, `string expected_version_id`, `Task task`, `RelationshipEdits relationship_edits`                                                                                         | `EntityResult result`, `repeated EntityResult affected`                            |
| SetProgress      | `MutationMeta meta`, `string id`, `string expected_version_id`, `Progress progress`                                                                                                                         | `EntityResult result`                                                              |
| SetProposal      | `MutationMeta meta`, `string id`, `string expected_version_id`, `Proposal proposal`                                                                                                                         | `EntityResult result`                                                              |
| RestorePrompt    | `MutationMeta meta`, `string id`, `string expected_version_id`, `string source_version_id`                                                                                                                  | `EntityResult result`                                                              |
| DeleteTask       | `MutationMeta meta`, `string id`                                                                                                                                                                            | `EntityResult result`                                                              |
| ListHistory      | `string id`, `Page page`, `bool prompts_only`                                                                                                                                                               | `repeated EntityVersion versions`, `string next_cursor`                            |
| ResolveConflict  | `MutationMeta meta`, `string id`, `repeated string expected_head_ids`, `EntityVersion resolved`                                                                                                             | `EntityResult result`                                                              |

`Page` is `uint32 size=1, string cursor=2`; default 100, maximum 1000. A cursor is opaque and scoped to the query/account. Stable order is entity ID, or `(created_at,version_id)` for history. `RepositoryTarget` is a oneof `string id=1, string url=2`. Creation by a new URL atomically creates its repository and task. `CreateTask` maps omitted/unspecified progress to TODO and proposal to REGULAR; creation permits REGULAR or PENDING only. `SetProposal` accepts ACCEPTED or REJECTED for a proposed task (including later acceptance of a rejected proposal); it preserves progress. ACCEPTED tasks behave as regular tasks while retaining their proposal history. Ordinary `SaveTask` cannot turn a regular task into a proposal or bypass these rules.

`SaveTask` replaces the full snapshot, including the target repository ID. Specialized mutations read the current whole task under the write lock and make a new full snapshot with the same compare-and-set check. No operation can mutate a conflicted entity except resolution or permanent deletion. `RestorePrompt` uses a retained version of that same task, including absent prompt, and creates a new version. Repository deletion cascades tasks currently in the repository and events originally retained there; a task moved out retains its content/history, with references to the deleted repository represented by its ID, not retained deleted repository content.

A stale interactive Save returns `aborted/STALE_VERSION`, current heads and the submitted alternative as a conflict result after preserving it. An already conflicted task returns `failed_precondition/CONFLICT_READ_ONLY`; it is not another edit channel. A stale resolution returns `aborted/STALE_RESOLUTION` without making a new head; reread before resolving again. Server serialization decides the first accepted resolution. Local resolution is provisional until accepted by the server; a rejected local resolution remains inspectable history and cannot overwrite the accepted result.

### Task planning schema

The task planning extension adds only new field numbers; existing numbers never change. The data format version written by this feature is **3**. Readers accept envelope formats 1, 2 and 3, where absent planning fields mean unassigned or empty. Because Lane has not been deployed, a store or transport of any other format is refused with `UNSUPPORTED_FORMAT`; no compatibility migration exists (see the [pre-release exception](apps-lane-local-store-contract.md#compatibility-and-development-reset)).

```proto
enum TaskPriority { // UNSPECIFIED is a valid stored value meaning unassigned
  TASK_PRIORITY_UNSPECIFIED = 0; TASK_PRIORITY_URGENT = 1; TASK_PRIORITY_HIGH = 2;
  TASK_PRIORITY_NORMAL = 3; TASK_PRIORITY_LOW = 4;
}
enum TaskType { // UNSPECIFIED is a valid stored value meaning unassigned
  TASK_TYPE_UNSPECIFIED = 0; TASK_TYPE_BUG = 1; TASK_TYPE_FEATURE = 2;
  TASK_TYPE_IMPROVEMENT = 3; TASK_TYPE_DOCUMENTATION = 4; TASK_TYPE_MAINTENANCE = 5;
}
// EntityKind adds LABEL=4, MILESTONE=5, CYCLE=6. ErrorReason adds CONSTRAINT_VIOLATION=19.
// ErrorDetail adds ConstraintViolation violation = 6.
enum ConstraintViolation {
  CONSTRAINT_VIOLATION_UNSPECIFIED = 0; CONSTRAINT_VIOLATION_PARENT_CYCLE = 1;
  CONSTRAINT_VIOLATION_DEPENDENCY_CYCLE = 2; CONSTRAINT_VIOLATION_DUPLICATE_CYCLE = 3;
  CONSTRAINT_VIOLATION_SELF_LINK = 4; CONSTRAINT_VIOLATION_CROSS_REPOSITORY = 5;
  CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND = 6; CONSTRAINT_VIOLATION_DUPLICATE_NAME = 7;
  CONSTRAINT_VIOLATION_LABEL_LIMIT = 8; CONSTRAINT_VIOLATION_DATE_ORDER = 9;
  CONSTRAINT_VIOLATION_FIELD_RANGE = 10;
}
// Task additions
//   TaskPriority priority = 8;               TaskType type = 9;
//   repeated string label_ids = 10;          // sorted unique, at most 100
//   optional string start_date = 11;         optional string due_date = 12;   // YYYY-MM-DD
//   optional uint32 story_points = 13;       // absent differs from 0
//   optional string milestone_id = 14;       optional string cycle_id = 15;
//   optional string parent_task_id = 16;
//   repeated string depends_on_task_ids = 17;  // sorted unique
//   repeated string related_task_ids = 18;     // sorted unique, stored on both endpoints
//   optional string duplicate_of_task_id = 19;
message Label     { string repository_id = 1; string name = 2; string color = 3; string description = 4; }
message Milestone { string repository_id = 1; string name = 2; string description = 3;
                    optional string target_date = 4; bool completed = 5; }
message Cycle     { string repository_id = 1; string name = 2; string description = 3;
                    string start_date = 4; string end_date = 5; }
// EntityVersion body adds Label label = 13; Milestone milestone = 14; Cycle cycle = 15;
```

Subtasks and Blocks are derived inverse views of `parent_task_id` and `depends_on_task_ids`; they have no stored field. `related_task_ids` is symmetric: every write stores the link on both tasks, and readers treat the union of both sides as the relation. `duplicate_of_task_id` is forward only. Label colors use `#RRGGBB` and are stored upper-case. Dates are `YYYY-MM-DD` strings.

`RelationshipEdits` carries the changes a task edit makes to **other** tasks' stored fields: `repeated string add_subtask_ids=1, remove_subtask_ids=2, add_blocked_task_ids=3, remove_blocked_task_ids=4`. Adding a subtask sets that task's parent to the edited task; adding a blocked task adds the edited task to that task's `depends_on_task_ids`. `CreateTaskRequest` adds, after the existing fields 1-8, `TaskPriority priority=9, TaskType type=10, repeated string label_ids=11, optional string start_date=12, optional string due_date=13, optional uint32 story_points=14, optional string milestone_id=15, optional string cycle_id=16, optional string parent_task_id=17, repeated string depends_on_task_ids=18, repeated string related_task_ids=19, optional string duplicate_of_task_id=20, RelationshipEdits relationship_edits=21`. `SaveTaskRequest` adds `RelationshipEdits relationship_edits=5`. `CreateTaskResponse` adds `repeated EntityResult affected=3` and `SaveTaskResponse` adds `repeated EntityResult affected=2`; `affected` lists every other task that received a new version in the same operation. `CompleteEvent` tasks inherit the `CreateTaskRequest` fields; `CreateCaptureTask` is unchanged. Edits to other tasks write one new version each, with the caller's origin, in the same transaction as the edited task. A conflicted affected task fails the whole operation with `CONFLICT_READ_ONLY`, and a deleted one with `NOT_FOUND` or `DELETED`.

### Planning service

Repository labels, milestones and cycles use the typed service `lane.v1.PlanningService`, not an untyped API. `PlanningBody` is a oneof `Label label=1, Milestone milestone=2, Cycle cycle=3`; the body's kind and `repository_id` are immutable after creation.

| Method             | Request fields (in field-number order)                                                              | Response fields                                     |
| ------------------ | --------------------------------------------------------------------------------------------------- | --------------------------------------------------- |
| GetPlanningItem    | `string id`                                                                                         | `EntityResult result`                               |
| ListPlanningItems  | `Page page`, `optional string repository_id`, `EntityKind kind` (unspecified lists all three kinds) | `repeated EntityResult items`, `string next_cursor` |
| CreatePlanningItem | `MutationMeta meta`, `string id`, `PlanningBody item`                                               | `EntityResult result`                               |
| SavePlanningItem   | `MutationMeta meta`, `string id`, `string expected_version_id`, `PlanningBody item`                 | `EntityResult result`                               |
| DeletePlanningItem | `MutationMeta meta`, `string id`                                                                    | `EntityResult result`                               |

Planning items use the same version, expected-version, request-ID receipt, whole-item conflict, deletion-precedence and `ResolveConflict` rules as URL connections; conflict kinds are `label`, `milestone` and `cycle`. `ListHistory(id)` returns the history of a planning item exactly as it does for a task. Deleting an item first writes new task versions that remove its assignment from every task that references it, then deletes the item and erases its history content, all in one transaction. A deleted replay returns `DELETED` without content. Names and descriptions of a deleted item never survive in any task or other entity history.

### Task list filtering and sorting

```proto
message DateRange  { optional string from = 1; optional string to = 2; bool include_unassigned = 3; } // inclusive, from <= to
message PointRange { optional uint32 min = 1; optional uint32 max = 2; bool include_unassigned = 3; }
message TaskFilter {
  repeated TaskPriority priorities = 1;  bool include_unassigned_priority = 2;
  repeated TaskType types = 3;           bool include_unassigned_type = 4;
  repeated string label_ids = 5;         bool include_unlabeled = 6;
  DateRange start_date = 7;  DateRange due_date = 8;  PointRange story_points = 9;
  repeated string milestone_ids = 10;    bool include_no_milestone = 11;
  repeated string cycle_ids = 12;        bool include_no_cycle = 13;
  repeated RelationshipKind relationships = 14;  bool include_no_relationships = 15;
}
enum RelationshipKind { // PARENT=1, SUBTASKS=2, DEPENDS_ON=3, BLOCKS=4, RELATED=5, DUPLICATE_OF=6
  RELATIONSHIP_KIND_UNSPECIFIED = 0; /* ... */ }
enum TaskSortKey { // UNSPECIFIED=0 keeps task-ID order
  TASK_SORT_KEY_UNSPECIFIED = 0; TASK_SORT_KEY_PRIORITY = 1; TASK_SORT_KEY_TYPE = 2;
  TASK_SORT_KEY_START_DATE = 3; TASK_SORT_KEY_DUE_DATE = 4; TASK_SORT_KEY_STORY_POINTS = 5;
  TASK_SORT_KEY_MILESTONE_NAME = 6; TASK_SORT_KEY_CYCLE_START_DATE = 7;
}
enum SortDirection { SORT_DIRECTION_UNSPECIFIED = 0; /* ascending */ SORT_DIRECTION_ASCENDING = 1; SORT_DIRECTION_DESCENDING = 2; }
```

`ListTasksRequest` adds `TaskFilter filter=6, TaskSortKey sort_key=7, SortDirection sort_direction=8` after fields 1-5.

- Fields combine with AND. Selected values within a field combine with OR. A field's `include_*` flag adds that field's unassigned case with OR. A field with no selection and a false flag does not restrict. A range with only bounds and a false flag excludes unassigned tasks. A relationship filter selects tasks that have at least one relationship of a selected kind, or no relationship of any kind when `include_no_relationships` is set; Subtasks and Blocks are evaluated on the derived inverse views. Existing search, progress and proposal filters AND with all of it.
- One sort key and one direction. Unassigned values sort last in both directions, and equal keys break ties by ascending task ID. Priority ascends Urgent, High, Normal, Low; type ascends by enum number; dates compare lexically; story points compare numerically; milestone name compares by Unicode simple lowercase and then by code-point order; cycle sorting uses the cycle's `start_date`. A filter or sort referencing a deleted or unknown item matches nothing and is not an error.
- A cursor remains opaque and query/account-scoped and encodes the last sort tuple plus task ID, so concatenated pages equal the unpaged order. A cursor from a different filter or sort is rejected. All adapters (native bridge, CLI, MCP, server, web) produce identical order for identical data.

### Planning validation and relationship rules

Violations that carry a code use `ErrorReason.CONSTRAINT_VIOLATION` with `ErrorDetail.violation`, Connect `failed_precondition`, `retryable=false`, and `entity_id` naming the offending entity. Other malformed input uses `INVALID_INPUT`.

- Names: trim whitespace; 1 to 100 Unicode scalar values after trimming (otherwise `INVALID_INPUT`). Description at most 4,000 scalar values (otherwise `INVALID_INPUT`). Duplicate names among live items of the same repository and kind, compared with Unicode simple lowercase and excluding the item itself, return `DUPLICATE_NAME`. Different kinds and repositories may share a name.
- Label color must match `^#[0-9A-Fa-f]{6}$` (otherwise `INVALID_INPUT`) and is stored upper-case; it is required on creation and editable by Save.
- Milestone `completed` starts false. A cycle requires both dates, and `start_date` after `end_date` returns `DATE_ORDER`. Malformed or non-calendar dates return `INVALID_INPUT`.
- Task dates must be real calendar dates, and a start date after the due date returns `DATE_ORDER`. Task dates are independent of milestone and cycle dates.
- Story points above 2,147,483,647 return `FIELD_RANGE`; negative or fractional input is rejected by the typed unsigned integer, or by the CLI/MCP adapter, as `FIELD_RANGE` before dispatch. Priority and type enum numbers outside the defined range return `INVALID_INPUT`.
- `label_ids`, `depends_on_task_ids` and `related_task_ids` are deduplicated and sorted on write; more than 100 labels return `LABEL_LIMIT`. Each label, milestone, cycle and relationship endpoint must be a live entity of the task's repository: one that exists in another repository returns `CROSS_REPOSITORY`; a missing or deleted one returns `REFERENCE_NOT_FOUND`.
- A self-link returns `SELF_LINK`. A parent chain cycle returns `PARENT_CYCLE`, a directed dependency cycle `DEPENDENCY_CYCLE`, and a duplicate-of chain cycle `DUPLICATE_CYCLE`. A task has one parent and one original by construction of the single fields. For conflicted tasks, cycle checks use the union of every head's references.
- Ownership, name and graph constraints are checked against current heads inside the write transaction, serialized per account, so concurrent writes cannot jointly violate them. Relationships never alter progress or proposal state and never block a completion.
- Moving a task (a `SaveTask` that changes `repository_id`) clears the moved task's labels, milestone, cycle, parent, dependencies, related tasks and duplicate-of reference, removes every other task's reference to it by writing a new version for each, and keeps priority, type, dates, story points, ID, prompt and captures. Deleting a task removes other tasks' references likewise. Cleanup versions use the acting origin and share the triggering transaction; failure leaves no partial cleanup, history, receipt or pending record.
- Mutation fingerprints cover all planning fields, with set fields sorted, absent optionals omitted and scalar defaults emitted. A retried request with identical content produces no duplicate relationship, entity or history; changed content under the same request ID is `REQUEST_ID_REUSED`.

### Constraint rejection during synchronization

The server validates every pushed task and planning version against the **accepted server state** with the rules above. A violating item returns `SyncResult.error` with `reason=CONSTRAINT_VIOLATION`, the named `violation`, the item's `entity_id` and `retryable=false`; other items in the batch continue. The server never rolls back accepted state. Clients keep the rejected immutable version and its pending record blocked with `error_reason` text `CONSTRAINT_VIOLATION:<VIOLATION>`, never mark it synchronized, surface it in synchronization status, and let the user correct it with an ordinary edit followed by an explicit blocked-item retry. Ordinary stale-version conflicts keep their existing handling.

A correction supersedes the rejected, never-accepted ancestry instead of building on it. Because `PushChanges` requires every parent to exist on the server, a rejected version can never become a parent of an uploaded version. When an ordinary edit changes an entity whose head is an unsynchronized chain containing a rejected version, the client replaces that chain: the corrected version gets a new version ID and request ID, takes the parent version IDs of the oldest version in the chain (the last server-accepted heads, or none for a creation) and the chain's upload position, and is a `CREATE` when the entity was never accepted. The rejected immutable versions stay in local history for inspection with their original origin and time, are never uploaded again and are not retried by the blocked-item retry, which applies to rejected heads that were not superseded. A version that refers to an entity whose creation was rejected waits until that creation is corrected or superseded and accepted; the server reports a reference to an unaccepted entity as `REFERENCE_NOT_FOUND`. Replays of the superseded request IDs remain acknowledged without effect.

### Synchronization service

`lane.v1.SyncService` methods:

- `GetSyncInfo(GetSyncInfoRequest)` (an empty request) returns `uint32 format_version=1, string account_id=2, string tenant_id=3, string epoch=4, uint64 max_item_bytes=5`. `epoch` is a server-issued UUID v7 account-data generation.
- `PushChanges` accepts `uint32 format_version=1, string epoch=2, string device_id=3, repeated SyncMutation mutations=4`; returns `repeated SyncResult results=1, repeated RepositoryMapping repository_mappings=2`. Each mutation is `MutationMeta meta=1, EntityVersion version=2, repeated string expected_head_ids=3, MutationKind kind=4` (`CREATE=1, SAVE=2, RESOLVE=3, DELETE=4`). Each result is `string request_id=1, EntityResult result=2, ErrorDetail error=3`. Apply each item in its own transaction and retain input order in results; one oversized/invalid item does not stop other items. Process parents before descendants; a missing parent is retryable, never an implicit creation.
- `PullChanges` accepts `uint32 format_version=1, string epoch=2, string cursor=3, uint32 page_size=4`; returns `repeated Change changes=1, string next_cursor=2, bool has_more=3`. `Change` is `uint64 sequence=1` plus oneof `EntityResult entity=2, WebhookReceiver receiver=3, WebhookEvent event=4, ProcessingRecord processing=5, DeletionRecord deletion=6`. Read pages at a stable high-water mark; a cursor advances only after durable local apply. Tombstone changes redact earlier content even when a client resumes an older cursor.
- `WatchChanges` accepts `string epoch=1, string cursor=2`; streams `string cursor=1` invalidations. Clients fetch with PullChanges. Reconnect uses the durable pull cursor, never assumes notifications are reliable content delivery. Revocation terminates subscriptions and denies subsequent pulls.

Online web uses LaneService for edits and WatchChanges to invalidate server reads. It never writes a local outbox. Native/CLI sends durable local mutations with their original immutable IDs and parents. Ancestor replay is an acknowledgment, not a conflict. Concurrent non-ancestor versions preserve both complete alternatives. A later edit based on a conflict cannot bypass resolution. Deletes dominate all edits regardless of arrival order, clock or ancestry, including delayed creation and deletes of a parent repository.

Initial synchronization merges repositories by normalized URL, never by display name. The server returns `RepositoryMapping(string local_id=1,string canonical_id=2)`. Persist the mapping transactionally and resolve repository references through it in projections, pending-operation validation and reads. Do not rewrite immutable version bodies or request fingerprints. Store original and canonical IDs in the mapping; compare repository references after mapping when applying domain rules. A mapping is an identity relation within this account, not an old URL alias. Union histories and distinct task IDs; preserve differing repository display-name versions as a repository conflict rather than dropping one. Matching entity/version IDs must have equivalent content. Differing same-ID versions become conflicts. Server repository edits with the same ID and different parents also conflict. URL uniqueness is checked during resolution; no resolution can steal another repository's URL.

### Normalization and input rules

Accept HTTPS URLs, `ssh://` URLs and SCP-style `user@host:path`. Remove credentials, scheme, default transport ports (443 for HTTPS, 22 for SSH), leading/trailing path slashes and one terminal lowercase `.git`. Lowercase and IDNA-normalize the host; preserve path case. A nondefault explicit port remains part of the host identifier. Defaults are removed only from raw input according to its original transport: a stored normalized identifier has no transport, so servers and clients validating one must keep every explicit port, including `:443` from an SSH URL and `:22` from an HTTPS URL, and must never map it to a portless identifier. Reject empty host/path, query, fragment, dot/dot-dot path segments, backslashes, malformed percent escapes and encoded slash/backslash. Decode percent-encoded unreserved ASCII characters; retain other escapes with uppercase hex. Normalize Unicode host names through IDNA; do not normalize or lowercase repository path text. No Git access or provider lookup occurs. Display name defaults to the normalized host/path. URL rename retains repository ID and connections, rejects an existing normalized URL, and removes the old lookup immediately.

Titles must contain a non-whitespace character; preserve supplied text. Description and prompt remain exact Markdown source. Search is a literal Unicode case-folded substring of title or description, not regex or AI search. All 25 progress transitions are valid, including identity and reopening transitions. The same source input yields the same normalized values in all runtimes.

### Error and idempotency contract

`ErrorDetail` fields are `ErrorReason reason=1, string correlation_id=2, string entity_id=3, repeated string current_head_ids=4, bool retryable=5`. `ErrorDetail` also carries `ConstraintViolation violation=6`, set when the reason is CONSTRAINT_VIOLATION. Reasons include INVALID_INPUT, NOT_FOUND, STALE_VERSION, CONFLICT_READ_ONLY, STALE_RESOLUTION, REQUEST_ID_REUSED, DELETED, UNSUPPORTED_FORMAT, STORAGE_BUSY, STORAGE_FULL, ONLINE_REQUIRED, UNAUTHENTICATED, FORBIDDEN, DEPENDENCY_UNAVAILABLE, ITEM_TOO_LARGE, RATE_LIMITED, STALE_EPOCH, LEASE_INVALID and CONSTRAINT_VIOLATION (19). Connect mappings (CONSTRAINT_VIOLATION maps to failed_precondition) are invalid_argument, not_found, aborted, failed_precondition, already_exists, unauthenticated, permission_denied, unavailable and resource_exhausted as appropriate. A deleted replay is a successful `DELETED` result without content, not a recreation.

Compare request content after typed decoding, default expansion and URL normalization. Ignore transport correlation IDs, bearer tokens and retry timestamps. Preserve prompt presence. Arrays are ordered except explicitly defined sets. Do not compare raw ProtoJSON key order. Implementations must use one shared canonical fingerprint definition: UTF-8 [RFC 8785 JSON](https://www.rfc-editor.org/rfc/rfc8785) of the normalized named request, SHA-256 digest. Encode 64-bit integers as decimal strings as in ProtoJSON; enum values use declared names, omit absent optional fields, emit scalar defaults, and sort set arrays. Persist durable `(space/account,method,request_id,fingerprint,result identity)` for creation and event completion. Do not apply an existing 24-hour replay expiry. On permanent deletion, redact saved responses and retain only identity/deletion-version and digest needed to recognize retries. Reusing the ID with changed content is always an error; identical creation replay after deletion returns DELETED without content.

### Required shared conformance vectors

Store executable fixture files in `protos/lane/v1/fixtures/`; run the same vectors in Go, native implementations and Rust, and check UI adapters against the same messages. The following are required expected outcomes, not claims of tests already run. Symbolic IDs below stand for fixed valid UUID v7 values in fixtures.

| Input / sequence                                                                                         | Expected outcome                                                                |
| -------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| `https://GitHub.com/Acme/Repo.git`, `git@github.com:Acme/Repo`, `ssh://git@github.com:22/Acme/Repo.git/` | All normalize to `github.com/Acme/Repo`                                         |
| `https://github.com/Acme/repo`, `https://github.com/Acme/Repo`                                           | Distinct path identities                                                        |
| `https://example.com:8443/a/b.git`, `https://example.com/a/%62.git`                                      | `example.com:8443/a/b`, `example.com/a/b`                                       |
| `https://example.com/a/../b`, `https://example.com/a%2Fb`, empty title                                   | INVALID_INPUT, no writes                                                        |
| Create T at URL U, title only, request R                                                                 | One repository, TODO/REGULAR task, absent prompt, one history version           |
| Retry R with JSON keys reordered or equivalent repository URL                                            | Same task/result; no second version or notification                             |
| Retry R with changed title                                                                               | REQUEST_ID_REUSED                                                               |
| Delete T then retry R                                                                                    | DELETED; no body/history and no recreation                                      |
| Each of 5 progress states to each of 5 states                                                            | New version, valid transition; proposal unchanged                               |
| PENDING/BLOCKED → REJECTED → ACCEPTED                                                                    | BLOCKED throughout, history retains both dispositions                           |
| Prompt absent → empty → `one` → restore empty source version                                             | Presence remains distinct; restoration adds a new prompt version                |
| Base A; edit B changes title, edit C changes prompt                                                      | Heads B,C, both bodies preserved, normal edit prohibited                        |
| Resolve B,C as D; competing resolution E expects B,C                                                     | D wins on server, E returns STALE_RESOLUTION; no overwritten D                  |
| Edit then delete, or delete then delayed edit                                                            | Same tombstone; all content/history removed                                     |
| Move T from repo X to Y                                                                                  | T ID/prompt/history unchanged; linked event stays X                             |
| Rename X URL U to V then create at U                                                                     | X retains tasks; new repository for U; no alias                                 |
| Same URL with local repo X and remote repo Y; different task IDs                                         | Mapping X→Y, both tasks retained, no duplication on replay                      |
| Two accounts use same URL and UUID inputs                                                                | No cross-account read, dedupe, merge or notification                            |
| SQL write failure/rollback after snapshot before outbox                                                  | No snapshot/history/outbox/sequence partial commit                              |
| Oversized item B in upload [A,B,C]                                                                       | A,C apply; B remains pending with ITEM_TOO_LARGE                                |
| Delete server data retaining local; old epoch push                                                       | STALE_EPOCH/revoked device, no reupload                                         |
| Title-only create, then set and clear every planning field                                               | Unassigned by default; story points 0 differs from absent                       |
| Labels 100 versus 101; points 2147483647 versus 2147483648; start after due; cycle reversed              | 100 and the maximum succeed; others LABEL_LIMIT or FIELD_RANGE, DATE_ORDER      |
| Names `  Bug`, `bug`, empty, 101 characters; same name in another kind or repository                     | Trimmed; case-insensitive duplicate DUPLICATE_NAME; others allowed              |
| Self-link, parent A-B-A, dependency A-B-C-A, duplicate chain cycle, second parent                        | SELF_LINK, PARENT_CYCLE, DEPENDENCY_CYCLE, DUPLICATE_CYCLE; one parent only     |
| Link tasks across repositories or to a deleted task                                                      | CROSS_REPOSITORY or REFERENCE_NOT_FOUND; no writes                              |
| Related link A to B                                                                                      | Stored on both tasks; progress of both unchanged                                |
| Move T with incoming and outgoing links and a label, milestone and cycle                                 | T cleared of repository-owned references; peers lose references; all survive    |
| Delete a label, milestone, cycle or linked task                                                          | Assignments and references removed in one transaction; history content erased   |
| Filters across fields with unassigned flags; sort each key and direction with ties; page through results | AND across, OR within; unassigned last; task-ID ties; pages equal unpaged order |
| Push a combined cycle, second parent or duplicate name from a second device                              | CONSTRAINT_VIOLATION per item; server state kept; local version stays blocked   |

Also fixture planning enum boundaries, absent planning fields in format 1 and 2 envelopes, unsupported store/transport formats, absent/unknown enums, unsupported format/schema, prompt history after resolution, concurrent repository edits/URL collision, Unicode URL/search, dedupe after event deletion, and the exact server lease/signature/push boundary vectors in the server contract.

### Fixture consumption

The checked-in fixture files use `{ "formatVersion": 1, "cases": [...] }`. Each case has a stable `id`, `input` and `expected`. Run each case with isolated storage and a fake clock. Treat `expected` as assertions on the operation result and persisted state, not as input to the implementation. Compare only the named assertions; counts are exact. UUIDs and keys in these files are fixed synthetic test values.

- `normalization.json`: normalize `input.url`, or reject it with the expected reason and no writes.
- `tasks.json`: apply the named method to the supplied `before` task. Progress cases use the request's expected version as the initial head. Other cases supply ordered snapshots, proposal decisions or a named validation/search operation. A restore source index refers to the zero-based snapshot index. Each successful explicit save adds one version.
- `sync.json`: seed `initial` versions in order, then apply `mutations` in order. Inputs use `SyncMutation` ProtoJSON. `interactiveRequest` instead invokes its named LaneService method. The remaining cases specify replay, repository mapping, account isolation, failure injection and batch-size scenarios. Byte sizes are measured encoded-item sizes; they are independent of JSON whitespace in the fixture file. `existingReceipt` seeds the original request identity for the changed-content retry case. `replayMutationIndices` selects zero-based mutations to resend after the initial sequence; replay assertions count only those retries. `newPromptHistoryVersionIds` lists added history members, excluding the seeded initial versions.
- `fingerprints.json`: serialize the supplied normalized request with RFC 8785 and compare both exact UTF-8 text and SHA-256. These cases distinguish absent, empty and nonempty prompts. They do not replace URL/default expansion tests.
- `webhooks.json`: verify the exact `rawBody` bytes and timestamp with the synthetic base64url key. Parsing cases run after transport authentication. Lease clocks use decimal microseconds; deadlines are exclusive. Completion fixtures with `validLease` start with a matching unexpired lease. Replay cases seed a previously committed completion receipt; PENDING means the event was reopened after that completion. No replay creates another task or completes the new round.
- `push.json`: `retryIndex` is zero for the first retry after the initial failure. Seconds are decimal Unix times; Retry-After delta values are relative to `previousAttemptSeconds`. A null expected next attempt means no send. Enqueue and dispatch cases test separate phases; missing eligibility inputs use an enabled, approved device with granted permission and available provider. Generic content assertions forbid repository/task text without fixing localized wording.

`conformance_test.go` checks field identities, service shape, optional prompt binary/JSON round trips, fixture request decoding, canonical fingerprint digests and signature known answers. It does not implement a runtime store or establish domain conformance. Rust CLI, native and server adapters must execute the same fixture inputs against their own implementations and report that coverage separately.

## Storage

The envelope is a small immutable domain format, not a generic replacement for typed business APIs. Local SQL layout is fixed by the [local-store contract](apps-lane-local-store-contract.md); PostgreSQL uses the same domain identities with account-scoped keys. Events/receivers/leases are typed server objects, not offline-writable envelope variants. Native clients retain downloaded event data for offline reads only.

## Security

Resolve account and tenant from validated identity, never user-supplied ownership fields. Bind all cursors, request IDs and version references to that account. Do not allow cross-account history/parent references. Unknown format versions and enum values fail explicitly. Sanitize Markdown rendering; never treat prompt source as executable instructions in Lane.

## Logging

Log operation, correlation ID, result category and timing, not bodies, prompt text, webhook payloads, secrets or fingerprints. Follow the application/server retention contract.

## Build and Test

At schema implementation (including planning schema changes), run root `pnpm proto:lint`, `pnpm proto:generate`, generated freshness and repository FILE breaking checks, Go tests/vet and affected consumers. Run all shared vectors across Go/native/Rust. Include disposable PostgreSQL and mock identity/provider integration. Schema generation/build success does not establish native acceptance.

## Dependencies and Integrations

Use local schemas through repository generation. Connect adapters must preserve this contract. [Delcore protocol](protos-delcore-v1-contract.md) remains service-neutral and owns identity/authorization messages.

## Change Triggers

Change local-store, CLI, server and app contracts together when wire fields, domain rules or schema compatibility change. Preserve field numbers and reserve removed fields. Update scoped instructions only for ownership or development-rule changes.

## References

- [Project](project-lane.md)
- [Repository defaults](repository-defaults.md)
- [Local store](apps-lane-local-store-contract.md)
- [Server](servers-lane-api-contract.md)

## RealQA integration

The [RealQA contract](apps-lane-realqa-contract.md) extends this domain with durable
final-PNG attachments, authorized retrieval and synchronized URL-to-repository
rules. Preserve existing account routing, whole-task conflicts, request replay,
history, task moves and deletion precedence. Native saves commit locally before
optional image upload. Pending synchronization must not be reported as failed
local saving. Unsupported schema/format versions must be refused before writes.
These requirements do not establish implementation or release acceptance.

### RealQA format 2 interfaces

Format 2 adds ordered `CaptureReference captures=7` to Task. Each reference owns
an immutable UUID v7 attachment ID, PNG dimensions, SHA-256 digest and byte count,
plus optional page URL, page title and CSS selector. Metadata belongs to the
whole-task version and can be removed without deleting history's image content.
`CreateCaptureTask` accepts final PNG bytes and metadata together and atomically
creates one regular To do task. Its stable request ID covers the complete input.
`GetAttachment` requires both task and attachment identity; local callers receive
PNG bytes and remote callers receive short-lived authorized download access.
Signed URLs never enter persisted prompts or task versions.

URL connections use `EntityKind.URL_CONNECTION=3` and the `url_connection=12`
EntityVersion body. `ListURLConnections`, `CreateURLConnection`,
`SaveURLConnection` and `DeleteURLConnection` share the existing version,
request, conflict and deletion rules. `ResolveConflict` resolves these versions
explicitly. A rule has a normalized HTTP(S) prefix and target repository ID.
Conflicted rules cannot participate in automatic selection.

Existing field numbers remain stable. Format 1 clients must refuse format 2
stores and synchronization; they cannot round-trip these extended snapshots.
Schema upgrades require coordinated consumers. Task planning raises the current format to 3 under the pre-release exception: earlier-format stores and transports are refused rather than migrated.

### Private attachment transfer

Synchronize task versions before transferring their PNG bytes. The current
transport is format 3 and rejects transport requests of any other format. It accepts historical immutable
envelopes of format 1 and 2. Historical format 1 envelopes cannot contain captures or URL
connections, and envelopes before format 3 cannot contain planning fields. New server versions use format 3.

`BeginAttachmentUpload(task_id, attachment_id, epoch)` returns `upload_id`,
`upload_url`, `expires_at`, required `headers` and `state`. PUT the exact final PNG
with those headers, then call
`CompleteAttachmentUpload(task_id, attachment_id, upload_id, epoch)`. Completion
validates byte count, digest, PNG structure and decoded image data before marking
the attachment available. An available response makes retries safe without a
second PUT. Expired uploads require a new begin call. Both calls require the
current account epoch and the attachment's owning task. Signed transfer URLs are
short-lived capabilities; do not forward Lane authorization headers to R2.

The immutable manifest binds attachment ID, owner task, dimensions, digest and
byte count. Browser metadata remains part of each task version. Removing a
reference from the current version preserves bytes required by history. Moving
a task preserves its attachment ownership. Task deletion and account deletion
remove access and queue provider deletion. Retrieval of a pending manifest
returns `WAITING_FOR_SYNC`; it must not be displayed as a failed local save.
