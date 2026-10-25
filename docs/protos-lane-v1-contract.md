# Lane v1 Protocol Contract

## Scope

`protos/lane/v1/lane.proto` defines Lane schemas. Checked-in `lane.pb.go`, `lanev1connect/lane.connect.go` and `lane_pb.ts` provide Go and TypeScript bindings. The server protocol crate and Rust CLI build script register the same schema for generation into their respective Cargo output directories. Shared conformance vectors belong in `protos/lane/v1/fixtures/`. These interfaces do not establish runtime or native acceptance. The [application contract](apps-lane-foundation.md) owns complete product behavior.

## Runtime and Language

Use Protobuf package `lane.v1`, Go package `github.com/delinoio/apps/protos/lane/v1`, Connect-Go package `lanev1connect`, and TypeScript exports from `@delino/protos`. Generate server Rust through `servers/crates/delino-protos` and CLI Rust descriptors through `cmds/lane/build.rs`; do not check in generated Rust or duplicate schemas. Use typed messages and enums, not an arbitrary JSON command API. JSON below means the standard ProtoJSON representation of the named message, except the explicit webhook JSON object.

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
}
message EntityVersion {
  uint32 format_version = 1; // exactly 1
  string entity_id = 2;
  EntityKind kind = 3; // REPOSITORY=1, TASK=2
  string version_id = 4;
  repeated string parent_version_ids = 5;
  google.protobuf.Timestamp created_at = 6;
  Origin origin = 7;
  bool deleted = 8;
  bool prompt_restoration = 9; // explicit RestorePrompt snapshot, including unchanged text
  oneof body {
    Repository repository = 10;
    Task task = 11;
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

Tables and comments use enum suffixes for readability. Exact protobuf enum prefixes are `ORIGIN_KIND_`, `ENTITY_KIND_`, `PROGRESS_`, `PROPOSAL_`, `RESULT_KIND_`, `MUTATION_KIND_` and `ERROR_REASON_` for their correspondingly named types. Thus the JSON progress value is `PROGRESS_TODO`, not the UI label or bare `TODO`. All have prefix plus `UNSPECIFIED=0`. ErrorReason numbers, in the order listed in the error section, are 1 through 18. The server types follow the same rule: `PUSH_PROVIDER_`, `NOTIFICATION_CONTENT_`, `DATA_DELETION_MODE_`, `EVENT_STATE_`, `NOTIFICATION_PERMISSION_`, `LOCAL_RETENTION_`. `EntityKindOrRemoteKind` uses prefix `ENTITY_KIND_OR_REMOTE_KIND_` and values REPOSITORY=1, TASK=2, RECEIVER=3, EVENT=4, ACCOUNT=5. Never renumber these values between adapters.

Entity IDs, version IDs, request IDs and device IDs are lowercase UUID v7 strings. Parent IDs are a sorted unique set, never ordered by arrival or timestamps. A normal edit has exactly the current head as its parent. Creation has no parents. Resolution has all observed conflict heads as parents. Version IDs identify immutable content and cannot be reused for changed content. Timestamps use UTC microsecond precision; they display chronology but never determine winners. A deleted version has no body. Deletion retains only minimal identities, deletion versions and replay fingerprints; remove historical bodies, alternatives and prompt content.

`SyncMutation` request IDs are stable for that entity version and use the immutable normalized mutation as their dedupe content. Interactive LaneService receipts remain keyed by their original method/request ID; local-to-server transport receipts use PushChanges item IDs and never reinterpret them as a different interactive request. A resolution whose expected heads changed must receive a new request/version ID after rereading.

`Origin` is assigned by the authenticated adapter. MCP tools cannot supply an agent name or override origin/account/device attribution. A grant without a physical device has an empty device ID and its server-recorded connection name as device name. Clients may supply their registered device identity during sync; the server validates the association. Preserve original origins when importing accepted history.

A task snapshot is the entire conflict unit. A prompt is embedded, so concurrent prompt/title/progress/move changes cannot merge silently. Prompt history consists of snapshots that change prompt presence/content, plus explicit restoration snapshots even when the restored text equals current text. `EntityVersion.prompt_restoration` is true only for a nondeleted task snapshot created by an explicit `RestorePrompt`, including restoration to an absent or unchanged prompt. It is false for repository versions, tombstones and ordinary saves; do not inherit it from a parent snapshot. Preserve this immutable marker in sync, history and request fingerprints. A `SyncMutation` with kind SAVE can carry a restoration snapshot; receivers use the marker when rebuilding prompt-history membership. A missing marker decodes as false and cannot recover an earlier unmarked equal-text restoration. An explicit successful Save creates a version; a transport retry of that Save does not. History exposes before/after, time and origin for title, description, progress, proposal and repository moves. Preserve conflicting alternatives and the resolving version until deletion.

### Typed business service

Use Connect service `lane.v1.LaneService`; each method has `<Method>Request` and `<Method>Response` messages. Request fields below have consecutive field numbers in listed order. Response fields likewise follow the listed order. Reuse the common types rather than separate UI/MCP models. Mutation responses use `EntityResult` where specified. Names are case-sensitive. Omitted expected versions are legal only on creation, not blind updates.

| Method           | Request fields (in field-number order)                                                                                                                                    | Response fields                                         |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------- |
| GetRepository    | `string id`                                                                                                                                                               | `EntityResult result`                                   |
| ListRepositories | `Page page`                                                                                                                                                               | `repeated EntityResult items`, `string next_cursor`     |
| CreateRepository | `MutationMeta meta`, `string id`, `string url`, `optional string display_name`                                                                                            | `EntityResult result`                                   |
| SaveRepository   | `MutationMeta meta`, `string id`, `string expected_version_id`, `string url`, `optional string display_name`                                                              | `EntityResult result`                                   |
| DeleteRepository | `MutationMeta meta`, `string id`                                                                                                                                          | `EntityResult result`                                   |
| GetTask          | `string id`                                                                                                                                                               | `EntityResult result`                                   |
| ListTasks        | `Page page`, `optional string repository_id`, `repeated Progress progress`, `repeated Proposal proposal`, `string search`                                                 | `repeated EntityResult items`, `string next_cursor`     |
| CreateTask       | `MutationMeta meta`, `string id`, `RepositoryTarget repository`, `string title`, `string description`, `optional string prompt`, `Progress progress`, `Proposal proposal` | `EntityResult result`, `EntityResult repository`        |
| SaveTask         | `MutationMeta meta`, `string id`, `string expected_version_id`, `Task task`                                                                                               | `EntityResult result`                                   |
| SetProgress      | `MutationMeta meta`, `string id`, `string expected_version_id`, `Progress progress`                                                                                       | `EntityResult result`                                   |
| SetProposal      | `MutationMeta meta`, `string id`, `string expected_version_id`, `Proposal proposal`                                                                                       | `EntityResult result`                                   |
| RestorePrompt    | `MutationMeta meta`, `string id`, `string expected_version_id`, `string source_version_id`                                                                                | `EntityResult result`                                   |
| DeleteTask       | `MutationMeta meta`, `string id`                                                                                                                                          | `EntityResult result`                                   |
| ListHistory      | `string id`, `Page page`, `bool prompts_only`                                                                                                                             | `repeated EntityVersion versions`, `string next_cursor` |
| ResolveConflict  | `MutationMeta meta`, `string id`, `repeated string expected_head_ids`, `EntityVersion resolved`                                                                           | `EntityResult result`                                   |

`Page` is `uint32 size=1, string cursor=2`; default 100, maximum 1000. A cursor is opaque and scoped to the query/account. Stable order is entity ID, or `(created_at,version_id)` for history. `RepositoryTarget` is a oneof `string id=1, string url=2`. Creation by a new URL atomically creates its repository and task. `CreateTask` maps omitted/unspecified progress to TODO and proposal to REGULAR; creation permits REGULAR or PENDING only. `SetProposal` accepts ACCEPTED or REJECTED for a proposed task (including later acceptance of a rejected proposal); it preserves progress. ACCEPTED tasks behave as regular tasks while retaining their proposal history. Ordinary `SaveTask` cannot turn a regular task into a proposal or bypass these rules.

`SaveTask` replaces the full snapshot, including the target repository ID. Specialized mutations read the current whole task under the write lock and make a new full snapshot with the same compare-and-set check. No operation can mutate a conflicted entity except resolution or permanent deletion. `RestorePrompt` uses a retained version of that same task, including absent prompt, and creates a new version. Repository deletion cascades tasks currently in the repository and events originally retained there; a task moved out retains its content/history, with references to the deleted repository represented by its ID, not retained deleted repository content.

A stale interactive Save returns `aborted/STALE_VERSION`, current heads and the submitted alternative as a conflict result after preserving it. An already conflicted task returns `failed_precondition/CONFLICT_READ_ONLY`; it is not another edit channel. A stale resolution returns `aborted/STALE_RESOLUTION` without making a new head; reread before resolving again. Server serialization decides the first accepted resolution. Local resolution is provisional until accepted by the server; a rejected local resolution remains inspectable history and cannot overwrite the accepted result.

### Synchronization service

`lane.v1.SyncService` methods:

- `GetSyncInfo(GetSyncInfoRequest)` (an empty request) returns `uint32 format_version=1, string account_id=2, string tenant_id=3, string epoch=4, uint64 max_item_bytes=5`. `epoch` is a server-issued UUID v7 account-data generation.
- `PushChanges` accepts `uint32 format_version=1, string epoch=2, string device_id=3, repeated SyncMutation mutations=4`; returns `repeated SyncResult results=1, repeated RepositoryMapping repository_mappings=2`. Each mutation is `MutationMeta meta=1, EntityVersion version=2, repeated string expected_head_ids=3, MutationKind kind=4` (`CREATE=1, SAVE=2, RESOLVE=3, DELETE=4`). Each result is `string request_id=1, EntityResult result=2, ErrorDetail error=3`. Apply each item in its own transaction and retain input order in results; one oversized/invalid item does not stop other items. Process parents before descendants; a missing parent is retryable, never an implicit creation.
- `PullChanges` accepts `uint32 format_version=1, string epoch=2, string cursor=3, uint32 page_size=4`; returns `repeated Change changes=1, string next_cursor=2, bool has_more=3`. `Change` is `uint64 sequence=1` plus oneof `EntityResult entity=2, WebhookReceiver receiver=3, WebhookEvent event=4, ProcessingRecord processing=5, DeletionRecord deletion=6`. Read pages at a stable high-water mark; a cursor advances only after durable local apply. Tombstone changes redact earlier content even when a client resumes an older cursor.
- `WatchChanges` accepts `string epoch=1, string cursor=2`; streams `string cursor=1` invalidations. Clients fetch with PullChanges. Reconnect uses the durable pull cursor, never assumes notifications are reliable content delivery. Revocation terminates subscriptions and denies subsequent pulls.

Online web uses LaneService for edits and WatchChanges to invalidate server reads. It never writes a local outbox. Native/CLI sends durable local mutations with their original immutable IDs and parents. Ancestor replay is an acknowledgment, not a conflict. Concurrent non-ancestor versions preserve both complete alternatives. A later edit based on a conflict cannot bypass resolution. Deletes dominate all edits regardless of arrival order, clock or ancestry, including delayed creation and deletes of a parent repository.

Initial synchronization merges repositories by normalized URL, never by display name. The server returns `RepositoryMapping(string local_id=1,string canonical_id=2)`. Persist the mapping transactionally and resolve repository references through it in projections, pending-operation validation and reads. Do not rewrite immutable version bodies or request fingerprints. Store original and canonical IDs in the mapping; compare repository references after mapping when applying domain rules. A mapping is an identity relation within this account, not an old URL alias. Union histories and distinct task IDs; preserve differing repository display-name versions as a repository conflict rather than dropping one. Matching entity/version IDs must have equivalent content. Differing same-ID versions become conflicts. Server repository edits with the same ID and different parents also conflict. URL uniqueness is checked during resolution; no resolution can steal another repository's URL.

### Normalization and input rules

Accept HTTPS URLs, `ssh://` URLs and SCP-style `user@host:path`. Remove credentials, scheme, default transport ports (443 for HTTPS, 22 for SSH), leading/trailing path slashes and one terminal lowercase `.git`. Lowercase and IDNA-normalize the host; preserve path case. A nondefault explicit port remains part of the host identifier. Reject empty host/path, query, fragment, dot/dot-dot path segments, backslashes, malformed percent escapes and encoded slash/backslash. Decode percent-encoded unreserved ASCII characters; retain other escapes with uppercase hex. Normalize Unicode host names through IDNA; do not normalize or lowercase repository path text. No Git access or provider lookup occurs. Display name defaults to the normalized host/path. URL rename retains repository ID and connections, rejects an existing normalized URL, and removes the old lookup immediately.

Titles must contain a non-whitespace character; preserve supplied text. Description and prompt remain exact Markdown source. Search is a literal Unicode case-folded substring of title or description, not regex or AI search. All 25 progress transitions are valid, including identity and reopening transitions. The same source input yields the same normalized values in all runtimes.

### Error and idempotency contract

`ErrorDetail` fields are `ErrorReason reason=1, string correlation_id=2, string entity_id=3, repeated string current_head_ids=4, bool retryable=5`. Reasons include INVALID_INPUT, NOT_FOUND, STALE_VERSION, CONFLICT_READ_ONLY, STALE_RESOLUTION, REQUEST_ID_REUSED, DELETED, UNSUPPORTED_FORMAT, STORAGE_BUSY, STORAGE_FULL, ONLINE_REQUIRED, UNAUTHENTICATED, FORBIDDEN, DEPENDENCY_UNAVAILABLE, ITEM_TOO_LARGE, RATE_LIMITED, STALE_EPOCH and LEASE_INVALID. Connect mappings are invalid_argument, not_found, aborted, failed_precondition, already_exists, unauthenticated, permission_denied, unavailable and resource_exhausted as appropriate. A deleted replay is a successful `DELETED` result without content, not a recreation.

Compare request content after typed decoding, default expansion and URL normalization. Ignore transport correlation IDs, bearer tokens and retry timestamps. Preserve prompt presence. Arrays are ordered except explicitly defined sets. Do not compare raw ProtoJSON key order. Implementations must use one shared canonical fingerprint definition: UTF-8 [RFC 8785 JSON](https://www.rfc-editor.org/rfc/rfc8785) of the normalized named request, SHA-256 digest. Encode 64-bit integers as decimal strings as in ProtoJSON; enum values use declared names, omit absent optional fields, emit scalar defaults, and sort set arrays. Persist durable `(space/account,method,request_id,fingerprint,result identity)` for creation and event completion. Do not apply an existing 24-hour replay expiry. On permanent deletion, redact saved responses and retain only identity/deletion-version and digest needed to recognize retries. Reusing the ID with changed content is always an error; identical creation replay after deletion returns DELETED without content.

### Required shared conformance vectors

Store executable fixture files in `protos/lane/v1/fixtures/`; run the same vectors in Go, native implementations and Rust, and check UI adapters against the same messages. The following are required expected outcomes, not claims of tests already run. Symbolic IDs below stand for fixed valid UUID v7 values in fixtures.

| Input / sequence                                                                                         | Expected outcome                                                      |
| -------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| `https://GitHub.com/Acme/Repo.git`, `git@github.com:Acme/Repo`, `ssh://git@github.com:22/Acme/Repo.git/` | All normalize to `github.com/Acme/Repo`                               |
| `https://github.com/Acme/repo`, `https://github.com/Acme/Repo`                                           | Distinct path identities                                              |
| `https://example.com:8443/a/b.git`, `https://example.com/a/%62.git`                                      | `example.com:8443/a/b`, `example.com/a/b`                             |
| `https://example.com/a/../b`, `https://example.com/a%2Fb`, empty title                                   | INVALID_INPUT, no writes                                              |
| Create T at URL U, title only, request R                                                                 | One repository, TODO/REGULAR task, absent prompt, one history version |
| Retry R with JSON keys reordered or equivalent repository URL                                            | Same task/result; no second version or notification                   |
| Retry R with changed title                                                                               | REQUEST_ID_REUSED                                                     |
| Delete T then retry R                                                                                    | DELETED; no body/history and no recreation                            |
| Each of 5 progress states to each of 5 states                                                            | New version, valid transition; proposal unchanged                     |
| PENDING/BLOCKED → REJECTED → ACCEPTED                                                                    | BLOCKED throughout, history retains both dispositions                 |
| Prompt absent → empty → `one` → restore empty source version                                             | Presence remains distinct; restoration adds a new prompt version      |
| Base A; edit B changes title, edit C changes prompt                                                      | Heads B,C, both bodies preserved, normal edit prohibited              |
| Resolve B,C as D; competing resolution E expects B,C                                                     | D wins on server, E returns STALE_RESOLUTION; no overwritten D        |
| Edit then delete, or delete then delayed edit                                                            | Same tombstone; all content/history removed                           |
| Move T from repo X to Y                                                                                  | T ID/prompt/history unchanged; linked event stays X                   |
| Rename X URL U to V then create at U                                                                     | X retains tasks; new repository for U; no alias                       |
| Same URL with local repo X and remote repo Y; different task IDs                                         | Mapping X→Y, both tasks retained, no duplication on replay            |
| Two accounts use same URL and UUID inputs                                                                | No cross-account read, dedupe, merge or notification                  |
| SQL write failure/rollback after snapshot before outbox                                                  | No snapshot/history/outbox/sequence partial commit                    |
| Oversized item B in upload [A,B,C]                                                                       | A,C apply; B remains pending with ITEM_TOO_LARGE                      |
| Delete server data retaining local; old epoch push                                                       | STALE_EPOCH/revoked device, no reupload                               |

Also fixture absent/unknown enums, unsupported format/schema, prompt history after resolution, concurrent repository edits/URL collision, Unicode URL/search, dedupe after event deletion, and the exact server lease/signature/push boundary vectors in the server contract.

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

At schema implementation, run root `pnpm proto:lint`, `pnpm proto:generate`, generated freshness and repository FILE breaking checks, Go tests/vet and affected consumers. Run all shared vectors across Go/native/Rust. Include disposable PostgreSQL and mock identity/provider integration. Schema generation/build success does not establish native acceptance.

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
