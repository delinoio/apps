# Lane API Contract

## Scope

`servers/lane-api` owns the Rust API, optional remote MCP endpoint, webhook intake, PostgreSQL state/migrations, synchronization, push workers, OCI artifacts and operator assets. It belongs to the independent server Cargo workspace and uses FSL-1.1-ALv2. This contract defines required behavior, not a deployed service. The [application contract](apps-lane-foundation.md) retains the full product scope.

## Runtime and Language

Use Rust, repository Connect infrastructure and PostgreSQL. Consume canonical local `protos/lane/v1` through `servers/crates/delino-protos`. Add an explicit server workspace member when implementation begins. Do not introduce a Go server or a second protocol repository. Remote MCP and webhook handlers call the same domain transaction layer as Connect.

## Users and Operators

Individual users, their approved MCP clients/devices, webhook senders, and deployment operators. Every approved remote MCP grant covers all repositories in that personal account. Lane runs no agents, AI provider calls, Git fetches, event interpretation or user-rule automation engine.

## Interfaces and Contracts

### Routes and deployment boundary

- `https://lane.api.delino.io/lane.v1.LaneService/<Method>`: typed repository/task/history operations, including planning fields, relationships, filters and sorts.
- `.../lane.v1.PlanningService/<Method>`: typed repository label, milestone and cycle operations.
- `.../lane.v1.SyncService/<Method>`: version negotiation, push/pull and live invalidations.
- `.../lane.v1.AccountService/<Method>`: devices, connection approvals, notification registration and Lane account-data deletion.
- `.../lane.v1.WebhookService/<Method>`: receiver/event/processing management.
- `https://lane.api.delino.io/mcp`: authenticated Streamable HTTP MCP over the same business layer.
- `POST /webhooks/<receiver-id>`: signed Lane common-format intake. This is the only internal intake route.
- `/health/live`, `/health/ready`, `/metrics`: process health, dependency readiness and operator-only metrics.

Serve the static online web client at `lane.delino.io` through Cloudflare Pages. Restrict CORS to configured frontend origins; do not allow arbitrary credentialed origins. Configuration and runbooks must produce deployable artifacts, but runtime work does not authorize production deployment.

### Identity and authorization

Logto owns identity; Delcore owns internal users and tenants. During user connection, obtain a token with the Delcore resource audience and call `delcore.v1.DelcoreService/GetCurrentUser`, following every `next_after_id` page. Resolve the returned internal `user_id` and the tenant with `personal=true` and `owner_id=user_id`. Do not select the first tenant or use a one-item discovery helper that discards pagination. Reject an ambiguous/missing personal tenant; do not substitute an organization tenant or equate a Logto subject with the internal user ID.

Register Lane as a Delcore service with `authorize` scope and capability `manage`. For every protected server operation, use Lane's M2M identity to call `Authorize` with the stored personal `tenant_id`, internal `user_id`, capability `manage`, `paid=false`, no `feature_key`, and no group values. Also call `GetTenantStates` and require ACTIVE: unpaid authorization alone does not establish operator-block/deletion state. Lane has no paid plan, feature gate or metering requirement. Deny BLOCKED, DELETING, DELETED and UNKNOWN access; only explicit DELETED authorizes tenant-lifecycle cleanup, never errors/unavailability/UNKNOWN. User-requested Lane deletion is its separate authorized action and must not delete Logto identity.

Validate issuer, audience, expiry and binding of caller credentials. Invalid caller credentials return unauthenticated. Delcore/M2M token acquisition or dependency failure returns unavailable with a correlation ID, not a claim that the caller's token is invalid. Authorization denial returns permission_denied. Local native/CLI work continues without either server; keep its outbox on dependency/auth failure.

CLI login and remote MCP approval use browser Logto authorization code flow with PKCE, state, exact redirect validation and resource/audience separation. An MCP-audience token must not be forwarded to Delcore. Obtain the separate Delcore-resource token during approval to resolve identity, then bind the durable Lane grant to internal user/personal tenant and use M2M reauthorization afterward. Do not store or accept a caller-selected tenant as proof. Expose standards-compatible MCP authorization metadata and approved-client flow; a bare Logto callback without grant/resource binding is insufficient.

Device and MCP grant revocation checks run under the same account transaction lock as each data operation. A revocation that commits first blocks that operation; an earlier committed operation can complete. Subscriptions recheck authorization/revocation before publishing invalidations and terminate on revocation. Do not rely on token expiry or a process-local approval cache. List individually approved connections and devices with last synchronization times; disconnecting cannot erase an offline device's existing data.

`Device.last_sync_at` is the PostgreSQL server time (`clock_timestamp()`) of the device's last successful data synchronization. It is null until the first qualifying success and is never backfilled. The server writes it only for the authenticated `(account, device)` pair from the principal, never from a request body or version origin, in the same authorized account-locked transaction that commits the work. A rolled-back transaction or a failed commit therefore leaves the previous value, and a revocation that commits first blocks both the operation and its timestamp write. The following qualify:

- every successful `PullChanges`, including an empty pull, a paginated pull and the deletion-directive pull of a newly approved session;
- a validated empty `PushChanges` batch;
- each `PushChanges` item whose own transaction commits, including accepted, replayed and retained-conflict results.

A rejected item, an all-rejected batch, a stale epoch, an invalid format, cursor or device binding, an authorization failure and a storage failure never qualify. A mixed batch advances the time for its committed items only and keeps the per-item transaction model. `GetSyncInfo`, `WatchChanges` initialization and ticks, approval, login and every other API or MCP call do not qualify because they do not establish a data exchange. Clearing account data resets the retained old device rows to null. `ListDevices` returns the stored instant through the existing field.

### Browser and native authorization adapter

`GET /oauth/authorize` accepts authorization-code parameters, an exact registered redirect, S256 PKCE and a UUID v7 `device_id` when the installation already has one. Preserve that device identity through token issuance. `POST /oauth/token` accepts form-encoded code or refresh grants; `POST /oauth/revoke` revokes the bound session. Operator-required `LANE_OAUTH_CLIENTS` binds each client to its redirects, display name, origin kind and API or MCP resource. Browser approval shows full-account consent for MCP. Revalidate registration before callback, consent and token issuance.

Browser approval for native and MCP clients is an API-hosted page and form (`POST /oauth/consent`) served from `LANE_PUBLIC_URL`, which is a different origin from `WEB_ORIGIN` when the web client is hosted separately. The HTTP policy admits a present `Origin` only when it equals `WEB_ORIGIN`, with one exception: the exact `LANE_PUBLIC_URL` origin is admitted for `POST /oauth/consent` alone. That exception grants no credentialed CORS response access; `Access-Control-Allow-*` headers remain exclusive to `WEB_ORIGIN` and absent-Origin requests. The opaque `null` Origin, hostile origins and the API origin on every other method, path, Connect route, `/mcp` and authenticated web mutation stay rejected with 403 before any handler runs, so a rejected request never consumes a consent token. The consent page sends `Referrer-Policy: strict-origin` (never `no-referrer`, which makes browsers serialize the form POST Origin as `null`) so the callback path, authorization code and query never appear in Referer, and keeps `form-action 'self'` in its CSP. Cookie binding, one-use and expiry checks, registration revalidation, account authorization and PKCE are unchanged. Approval redirects to the registered redirect with `code` and `state`; cancellation redirects with `error=access_denied` and `state` and issues no code.

Web uses `/auth/login?return_to=...`, `/auth/callback`, `/auth/session`, `/auth/refresh` and `/auth/logout`. The session cookie is Secure, HttpOnly and SameSite=Lax; mutations additionally require the configured Origin and `X-Lane-CSRF`. Session JSON uses snake_case identity fields, `csrf_token`, and `notification_preferences` with `enabled` and numeric `content` (1 detail, 2 generic). `/auth/push-config` exposes only the public VAPID `public_key`, or null when disabled. Web return URLs must be configured exactly: `return_to` carries no query or fragment and is matched by equality at login and again at callback completion. `/auth/login` also accepts an optional typed navigation intent as separate fields: one of `task` or `repository` (canonical UUID v7) and an optional `account` (canonical UUID v7) that requires a destination. Conflicting route kinds, malformed identifiers and an account without a destination fail with an invalid-argument response before any authentication flow exists; other query parameters, including `next` and `redirect`, are ignored and never become a destination or callback field. The intent is stored only in that flow's request JSON, revalidated at callback completion (a malformed saved intent is denied) and, after a successful web login, appended with URL encoding as `task` or `repository` plus the optional `account` to the registered callback in the single login redirect. Requests saved without intent complete as a plain login. The account value is a navigation guard for the web host only: it neither selects the authenticated identity nor authorizes data access, and the host reports a mismatch with the authenticated account instead of opening the destination. The session's persisted `redirect_uri` stays the query-free registered callback. Native and CLI OAuth are unchanged. Reauthentication returns only `reauth_challenge` to the saved return URL; authenticated `/auth/reauth/proof` redeems it for the one-use proof.

After explicit new approval, `PullChanges` with a known old epoch whose mode was DELETE_EVERYWHERE returns only an ACCOUNT deletion change naming the authenticated internal account, with `deletionVersionId` equal to the current account epoch, exactly one change, `hasMore=false`, and an empty `nextCursor`. A keep-local or unknown old epoch returns STALE_EPOCH and never authorizes erasure. `/auth/deletion-status` permits an old credential to retrieve only its content-free generation directive: `{mode,old_epoch,new_epoch,account_id,device_id,disconnected:true}`. Clients must match `old_epoch`, account and device against their retained local generation before cleanup. Neither route grants old credentials general data access.

### AccountService message catalog

Messages live in the canonical Lane protocol. Fields have consecutive field numbers in the order below. All online mutations include `MutationMeta meta` first. `Device` fields are `string id`, `string name`, `Timestamp last_sync_at`, `Timestamp disconnected_at`; `Connection` fields are `string id`, `string name`, `Timestamp approved_at`, `Timestamp revoked_at`. Lists use the protocol Page/cursor convention.

| Method            | Request fields after meta for mutations                                                                                                                | Response                                          |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------- |
| ListDevices       | `Page page` (read, no meta)                                                                                                                            | `repeated Device items`, `string next_cursor`     |
| DisconnectDevice  | `string device_id`                                                                                                                                     | `Device device`                                   |
| ListConnections   | `Page page` (read, no meta)                                                                                                                            | `repeated Connection items`, `string next_cursor` |
| RevokeConnection  | `string connection_id`                                                                                                                                 | `Connection connection`                           |
| RegisterPush      | `string device_id`, `PushProvider provider`, `string endpoint`, `bytes public_key`, `bytes auth_secret`, `bool enabled`, `NotificationContent content` | `Device device`                                   |
| BeginDataDeletion | `DataDeletionMode mode`, `string confirmed_account_id`                                                                                                 | `string challenge_id`, `string authorization_url` |
| DeleteAccountData | `DataDeletionMode mode`, `string confirmed_account_id`, `string reauth_proof`                                                                          | `string new_epoch`, `bool disconnected`           |

PushProvider is APNS=1, FCM=2, WEB_PUSH=3; Windows remote WNS is not supported. RegisterPush is bound to the calling device, never a cross-device preference edit. Device-local preferences may be copied to that device's delivery registration so the server can honor them; they are not synchronized to other devices.

DataDeletionMode is DELETE_EVERYWHERE=1 or CLEAR_SERVER_KEEP_LOCAL=2. Removing only this device is a local operation, not either server mode. Both server modes require explicit confirmed internal account ID and a fresh Logto reauthentication proof bound to that account, mode, challenge and calling session. The proof is one-use, expires after five minutes, and requires OIDC `auth_time` from the interactive reauthentication after the challenge. Refreshing an ordinary token is insufficient. Consume the proof atomically with deletion.

DELETE_EVERYWHERE clears server content/settings and propagates an account deletion marker to connected stores before disconnecting. Devices that were offline receive a content-free deletion directive on their next authenticated reconnect attempt; they cannot upload old state. CLEAR_SERVER_KEEP_LOCAL clears synchronized content and webhook receiving settings, changes account epoch, and disconnects **all** devices/grants in one transaction. Retained local data is not erased, and synchronization requires explicit reconnection. Keep minimal account-generation/deletion identities to reject old epoch requests without retaining content. Both modes preserve Logto identity and Delcore's shared personal tenant. No stale credential grants general data access through a deletion-status response.

### Webhook messages and intake

`WebhookReceiver` fields are `string id=1, string repository_id=2, string name=3, uint64 revision=4, bool deleted=5`. `WebhookEvent` fields are `string id=1, string receiver_id=2, string repository_id=3, string sender_event_id=4, string event_type=5, Timestamp occurred_at=6, google.protobuf.Struct payload=7, EventState state=8, uint64 revision=9`. EventState is PENDING=1, LEASED=2, COMPLETED=3, DELETED=4. `ProcessingRecord` fields are `string id=1, string event_id=2, repeated string task_ids=3, Timestamp completed_at=4, Origin origin=5`. `DeletionRecord` fields are `EntityKindOrRemoteKind kind=1, string id=2, string deletion_version_id=3`; this dedicated enum covers REPOSITORY, TASK, RECEIVER, EVENT and ACCOUNT. Do not overload a task's EntityKind with server-only variants.

`WebhookService` methods use consecutive request fields below; mutations start with MutationMeta. Response fields likewise follow listed order. All operations are online-only except cached event/processing reads:

| Method            | Request fields after meta for mutations                                              | Response fields                                                                    |
| ----------------- | ------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------- |
| ListReceivers     | `Page page`, `optional string repository_id` (no meta)                               | `repeated WebhookReceiver items`, `string next_cursor`                             |
| CreateReceiver    | `string id`, `string repository_id`, `string name`                                   | `WebhookReceiver receiver`, `string secret`, `string endpoint`                     |
| SaveReceiver      | `string id`, `uint64 expected_revision`, `string repository_id`, `string name`       | `WebhookReceiver receiver`                                                         |
| RotateReceiverKey | `string id`, `uint64 expected_revision`                                              | `WebhookReceiver receiver`, `string secret`                                        |
| DeleteReceiver    | `string id`                                                                          | `DeletionRecord deletion`                                                          |
| ListEvents        | `Page page`, `optional string repository_id`, `repeated EventState states` (no meta) | `repeated WebhookEvent items`, `string next_cursor`                                |
| GetEvent          | `string id` (no meta)                                                                | `WebhookEvent event`, `repeated ProcessingRecord processing`                       |
| DeleteEvent       | `string id`                                                                          | `DeletionRecord deletion`                                                          |
| ClaimEvent        | `string id`                                                                          | `WebhookEvent event`, `string lease_token`, `Timestamp expires_at`                 |
| RenewEventLease   | `string id`, `string lease_token`                                                    | `Timestamp expires_at`                                                             |
| ReleaseEventLease | `string id`, `string lease_token`, `string error_reason`                             | `WebhookEvent event`                                                               |
| CompleteEvent     | `string id`, `string lease_token`, `repeated CreateTaskRequest tasks`                | `WebhookEvent event`, `ProcessingRecord processing`, `repeated EntityResult tasks` |
| ReopenEvent       | `string id`, `uint64 expected_revision`                                              | `WebhookEvent event`                                                               |

`LANE_WEBHOOK_BASE_URL` has two forms, and `CreateReceiver` returns `endpoint` as `<intake base>/<receiver-id>`. An empty or root path is an API origin: the intake base is `<origin>/webhooks`, so `https://lane.api.delino.io` and `https://lane.api.delino.io/` both yield `https://lane.api.delino.io/webhooks/<receiver-id>`. An explicit non-root path is the complete public intake base, kept as configured except for trailing slashes: `/webhooks` and `/webhooks/` yield one `/webhooks` prefix, and `/hooks` yields `/hooks/<receiver-id>`. A non-root path is not an implicit application mount prefix. The server mounts only `/webhooks/<receiver-id>`, so a path other than `/webhooks` requires an operator-managed rewrite to that route. URL validation is unchanged: HTTPS, or HTTP only for loopback hosts, with no credentials, query or fragment. Existing receivers, keys and stored `CreateReceiver` replay receipts are not rewritten; a receipt issued with an origin-relative endpoint (`<origin>/<receiver-id>`) is replayed unchanged, and its sender is corrected by inserting `/webhooks` before the receiver ID. Regression coverage creates a receiver through Connect and POSTs a signed event to the returned endpoint through the real HTTP router for each form.

Intake body is UTF-8 JSON `{"id":"sender-event-id","type":"event-type","occurredAt":"RFC3339 UTC timestamp","payload":{}}`. Require nonempty ID/type, valid timestamp, JSON object payload and no duplicate JSON keys. Sender IDs/types are opaque strings, not GitHub-specific identifiers. Validate configured byte limits before allocating an unbounded body. Canonical parsed content uses RFC 8785 JSON for deduplication; timestamp normalizes to UTC microseconds. A fresh transport timestamp/signature is not new event content.

Require `Lane-Timestamp` as canonical base-10 Unix seconds and `Lane-Signature` as `v1=<lowercase HMAC-SHA256 hex>`. Signed bytes are ASCII timestamp, one period (`.`), then the **exact raw request body bytes**. Secret is 32 cryptographically random bytes shown once as unpadded base64url; senders decode it to bytes for HMAC. Compare signatures in constant time. Accept an absolute timestamp difference of at most 300 seconds, including exactly either boundary; reject ±301 seconds. Retries keep event ID/content and refresh timestamp/signature.

Store signing keys encrypted with an operator-managed encryption key because HMAC verification needs retrievable key bytes. Never expose a retrieve-secret RPC, log plaintext or persist it in replay responses. Creation/rotation returns plaintext once only; response loss requires rotation. Rotation atomically replaces the active key and invalidates the old one immediately. Deleting a receiver stops future intake but keeps its existing events and valid processing leases. Reassignment changes only future events; retained event repository IDs do not change.

Deduplicate by `(account_id,receiver_id,sender_event_id)`. Same identity and content returns the prior event identity; changed content returns conflict. After deletion retain only identity/content fingerprint/deletion version; replay returns an already-received/deleted result without content or recreation. Intake does not create tasks or mark processing complete.

Repository identity in the webhook domain follows the account-local repository mapping of the [protocol contract](protos-lane-v1-contract.md). A synchronized alias and its canonical repository are the same repository within one account, so `CreateReceiver`, `SaveReceiver` and new intake events resolve the supplied repository ID to the live canonical ID and persist that canonical ID. Receiver and event projections resolve a retained alias to the canonical ID when they are read, and `ListReceivers` and `ListEvents` accept either spelling: the filter and its cursor scope use the canonical ID, so an alias and the canonical ID select the same rows with the same cursors and other repositories and accounts stay separate. Stored rows, immutable entity versions, request and event fingerprints, sender deduplication identities and replay receipts are never rewritten to resolve identity. Repository deletion selects, independently and across every spelling of the repository, the receivers it currently owns and the events it originally received, then applies the existing receiver and event deletion semantics in the same account-locked transaction. A receiver reassigned to another repository therefore survives deletion of its former repository while that repository's older events are erased, and deleting the new repository does not erase the older events of a live one. Deleting a receiver alone still preserves its events and leases.

Webhook resources that outlived a deleted repository because they retained an alias are erased by a forward compatibility cleanup. The cleanup selects every nondeleted receiver and event whose own repository reference, after account-scoped mapping resolution, is a deleted repository. It processes one affected account per transaction under the account lock, in bounded pages of accounts, with the same per-resource deletion semantics as repository deletion. A failure rolls back that account's cleanup, and rerunning resumes with the accounts still affected. Repeating a completed cleanup is a no-op: it creates no deletion change and keeps tombstone IDs and revisions. The explicit `lane-api migrate` command runs the cleanup after the schema migrations commit; it needs no flag, no new public API and no repeated user deletion. Remove the step once every deployment has run it.

### Event leases and atomic processing

Use database server time and row/account locks. Claim succeeds only for a nondeleted PENDING event or an expired lease; mint a random opaque token and expiry `now + 5 minutes`. A token is valid only when `now < expires_at`, bound to event, account and claimant. Renewal before expiry sets expiry to renewal time plus five minutes. Store token hashes, not plaintext in logs or lists. Release invalidates the token and permits immediate reclaim; record the error category/reason without payload or credentials.

CompleteEvent validates the active token, nondeleted event/repository, authorization, all proposed tasks and their request identities in **one transaction**, then creates zero or more tasks and a processing record and marks the event COMPLETED. Zero tasks is valid. Invalid task input or any storage error rolls back the entire completion. Preserve a completion receipt with its task IDs so response-loss retries return the original outcome even though that token is consumed; check an identical authorized receipt before active-lease validation. Changed completion content under the same request ID fails. Unknown, released or expired tokens without that receipt always fail, even if nobody else claimed the event.

Before comparing the completion receipt, normalize each embedded task with the same `CreateTask` request rules, including planning and relationship-edit sets. Keep the order of embedded tasks and the presence of optional prompts in the completion fingerprint.

Deleting an event or its repository invalidates its lease; completion afterward cannot create tasks. Receiver deletion alone does not invalidate leases. Reopening a completed event changes its state to PENDING, preserves prior processing records/task links, and allows a new lease. An old completion replay returns its original outcome without completing the new processing round. Tasks moved later retain IDs and event links; the event stays in its original repository.

### Task planning operations

The server implements the planning behavior of the [protocol contract](protos-lane-v1-contract.md): extended task create/save/list, relationship edits with affected tasks, `PlanningService`, cleanup on moves and deletions, and list filtering and sorting with stable pagination. Remote MCP and webhook `CompleteEvent` task creation call the same domain transaction layer as Connect, so they enforce the same limits, ownership and graph rules.

- Validate in the write transaction: lock the account's affected entity rows and take a per-account advisory lock for graph and name checks, then re-read current heads. Check that every label, milestone, cycle and relationship endpoint is a live entity of the task's repository and account. A deleted task named in an explicit `removeSubtaskIds` or `removeBlockedTaskIds` edit returns `NOT_FOUND` or `DELETED`, as required by the protocol contract. Other missing, deleted, wrong-kind or foreign-account endpoints return `CONSTRAINT_VIOLATION` / `REFERENCE_NOT_FOUND`; a live endpoint in another repository returns `CONSTRAINT_VIOLATION` / `CROSS_REPOSITORY`. An absent link to a valid endpoint remains a no-op. Reject self-links and parent, dependency and duplicate-of cycles (using the union of every head's references for a conflicted task); enforce the 100-label limit, date order, story-point range, and case-insensitive trimmed name uniqueness per repository and kind among live items. Roll back the whole operation on rejection.
- Moves, task deletion and planning-item deletion write the required cleanup versions for other tasks in the same transaction with the actor's origin, append every changed entity's change in that one transaction (consecutive account sequence numbers that become visible together at commit) and leave no partial cleanup, history, receipt or push work on failure. A conflicted task has no single body to rewrite: an explicit relationship edit or related-link change that must write one fails the whole operation with `CONFLICT_READ_ONLY`, while automatic cleanup after a move or deletion skips it. Its dangling reference is ignored by derived views, filters and sorts and is rejected as `REFERENCE_NOT_FOUND` or `CROSS_REPOSITORY` when the conflict is resolved. `PushChanges` applies the client-authored versions, including a client's own cleanup versions, after validating them; it never writes server-authored cleanup versions, so a client's cleanup cannot race a duplicate server cleanup into a conflict. Deleting a planning item erases its history content and keeps only identity references in tasks.
- Planning items are whole-item conflict units resolved through `ResolveConflict` (conflict kinds `label`, `milestone`, `cycle`). History for tasks and planning items uses `ListHistory`.
- Filtering and sorting read the account's current accepted heads from the database inside the request transaction and evaluate them with the shared evaluator, with a keyset cursor over the sort tuple and task ID; the result must equal the in-memory reference ordering in the shared fixtures. Prompt and capture content is not loaded. A cursor is bound to its account, filter and sort.
- Request-ID receipts and fingerprints use the shared canonical definition with sorted set fields, so retries never duplicate entities, relationships or history.
- Remote MCP adds `lane_get_planning_item`, `lane_list_planning_items`, `lane_create_planning_item`, `lane_save_planning_item` and `lane_delete_planning_item` and extends the task tool schemas with the planning fields, filters, sorts and relationship edits. The tool list keeps the same shape as the local MCP.
- Planning requires no feature flag, entitlement, paid-plan check, metering, external integration or new credential. Existing input and request-rate limits apply to planning items and tasks.

### Synchronization constraint rejection

`PushChanges` validates every pushed task and planning version with the rules above against the **accepted server state**, in the item's own transaction. A violation returns that item's `SyncResult.error` with `reason=CONSTRAINT_VIOLATION`, the named `violation`, `entity_id` and `retryable=false`, without stopping other items. Rejected items are not written, not published and not rolled back later; accepted server state is never altered by a rejection. Rejection counts appear in the existing per-item rejection metrics and logs by category only. The client keeps the rejected local version for correction and explicit retry (`lane sync run --retry-blocked` or the application action); a corrected version arrives as a new immutable version with a new request ID. Ordinary stale-version conflicts and deletion precedence are unchanged.

### Synchronization and push

Use the envelope, typed operations and whole-entity conflicts in the [protocol contract](protos-lane-v1-contract.md). Allocate account-scoped monotonic change sequences in committed transactions. Publish live invalidations only after commit. Replays, deletion cascades, sync merges and account clearing must be transactional and account-isolated. Remote MCP operates directly on PostgreSQL when all devices are off.

Require positive operator configuration for both input-byte and request-rate limits; provide no defaults and fail startup if absent. Define `MAX_ITEM_BYTES`, `MAX_WEBHOOK_BYTES`, `REQUESTS_PER_MINUTE` and `WEBHOOK_REQUESTS_PER_MINUTE`. Count per authenticated account or receiver, with infrastructure-level abuse protection separate. The sync batch processes bounded individual items; limits reject only oversized new/modified uploads. Never reject reading/downloading/deleting existing data because the limit was lowered. Standalone local input has no server quota. Surface per-item rejection and continue other outbox items.

Only **new MCP-created PENDING proposals** schedule OS notifications. App/CLI edits, progress changes, decisions, acceptance and rejection do not. The creating transaction records proposal/device dedupe and durable delivery work. Local stdio proposals use local delivery; with sync the server schedules other eligible devices and does not duplicate the origin device's local receipt. Use `(proposal_id,device_id)` uniqueness. Device preferences default to repository name/task title, with a generic alternative. Require opt-in OS permission; denial or unsupported push leaves in-app indications/counts usable.

Use direct APNs for Apple, FCM for Android and Web Push for web. Windows uses local notifications and running/background-client synchronization; no WNS or package-registration requirement. Android without required Play services remains usable with the limitation explained.

Persist deliveries and attempts. After the initial transient failure, retry at delays of 30 s, 120 s, 600 s, 1800 s and 3600 s from the preceding failed attempt, at most five retries. Expire at creation plus two hours. Schedule at `max(previous_attempt + delay, provider Retry-After)`; do not send at/after expiry. Parse both delta-seconds and HTTP-date Retry-After. Permanent failures stop immediately. Recheck proposal state and device approval immediately before dispatch; acceptance/rejection/deletion cancels unsent work. A proposal already sent cannot be recalled. Use a Lane-specific policy rather than a helper that caps or ignores Retry-After.

### Provider payload and metric aggregation

APNs sends background priority-5 payloads `{"aps":{"content-available":1},"type":"lane.proposal","proposalId":"...","accountId":"..."}` with push type `background`. FCM sends the same identity fields as data-only, HIGH-priority messages. Neither includes a native display alert: the authenticated native account/device synchronizes and atomically claims its local notification receipt before display. Background delivery remains platform-discretionary. Web Push sends `{title, body, proposalId, tag, accountId}` to the service worker; detail title/body are repository name/task title, and generic content contains no task text. Stable provider collapse identities reduce duplicates, but a provider acceptance followed by a worker crash is an at-least-once boundary, not an exactly-once provider guarantee.

`lane_api_requests_total{outcome}` and `lane_api_request_duration_seconds` are process-local; aggregate them across instances. All four bounded outcome series start at zero. `lane_push_attempts_total{result}` is a persistent database-global counter with all three result series initialized to zero. Push queue count/age and `lane_sync_pending_failures` project the entire database, including unclaimed work, identically on every replica. Deduplicate these series by service with maximum, not sum; for push counter windows take each replica's increase then maximum by service/result. A healthy replica continues projecting the same rows after failover. Empty push queues report age zero. Metrics require a dedicated `LANE_METRICS_TOKEN` bearer credential.

## Storage

PostgreSQL holds account-scoped repositories, immutable versions, conflict heads, minimal tombstones, durable request/event/completion identities, changes, devices, grants, reauthentication challenges, receivers, event leases/processing records and pending push deliveries. Keys include account ID; no global URL deduplication. Preserve all prompts/history/events until explicit owning deletion. Operational log expiry is not content retention or receipt expiry. Do not reuse the existing 24-hour dedupe cleanup for Lane's durable replay identities.

Apply explicit forward migrations outside startup. The planning migration (`00007_planning.sql`) extends the `lane_entities.kind` constraint to the values 1 through 6 (adding label, milestone and cycle) and adds lookup indexes for planning items by account and repository and for task planning lookups. It rewrites no immutable envelope or replay fingerprint. Startup verifies schema compatibility and configuration, then refuses unsupported schemas. Roll back a binary only if it supports the current schema; otherwise recover with a corrected forward release. No down migrations are required. `migrate` also runs the idempotent webhook compatibility cleanup described under webhook intake; it is a data repair, not a schema migration, and does not change the migration digests. Encrypt server storage and transport; encrypted retrievable webhook keys are distinct from hashed grant/lease credentials. E2EE is not required.

### Development data reset

Lane is not deployed, and existing development data may be discarded. The planning schema change therefore ships no data-preserving migration, no compatibility with earlier-format data and no support for older clients or binaries. The server verifies its schema at startup and refuses to run against an incompatible one; it never resets implicitly. To recover a local development database, stop the API, run the root `pnpm dev:db:reset` (see the [repository workflow](repository-workflow-contract.md#lane-development-and-validation)), which deletes the disposable Compose volumes, restarts them and applies all migrations explicitly, and then restart the API. Reset only disposable local databases, never shared or production data, and do not run it merely to validate a change. Clients whose local data predates the format reset their own stores with the explicit local procedure in the [local-store contract](apps-lane-local-store-contract.md#development-data-reset) and reconnect. There is no rollback to an incompatible binary and no down migration.

## Security

Scope every lookup, cursor, history parent, idempotency key and lease to the current personal account. Validate task destinations under that account. Lock account revocation/deletion and resource writes consistently. CSRF-protect browser approval/deletion flows and reject arbitrary redirects. Secrets, tokens, task/prompt content and webhook payloads never enter logs/metrics or public configuration. Use service-local safe `.env.example` names/placeholders only; no root environment file.

## Logging

Use `tracing`, correlation IDs and structured operation/result/duration metadata. Planning operations and constraint rejections log the operation, correlation ID, result category (including the violation name) and duration, never task content, label/milestone/cycle names or descriptions, or relationship titles. Server logs expire after 30 days. Metrics include request/error counts, duration histograms excluding long-lived connection lifetime, DB/service probes, pending sync failures and push queue age/attempt results. Keep account/resource IDs out of high-cardinality metric labels; dashboards and alerts reveal no content.

Supply Prometheus rules, dashboards and Alertmanager email routing. Initial thresholds are configurable operational triggers, **not SLOs**:

- Probe service and DB every 30 seconds, timeout five seconds. Fire after five minutes continuously unhealthy; resolve after five minutes continuously healthy.
- Over a rolling five-minute window with at least 20 requests, fire if server errors/timeouts exceed 5% or p95 exceeds two seconds continuously for five minutes.
- For push, fire when transient failures exceed 10% over 15 minutes with at least 20 attempts, continuously for five minutes. Also fire when oldest pending delivery exceeds ten minutes.
- Resolve API/push alerts after the triggering condition clears continuously for ten minutes. Insufficient samples alone do not prove recovery. An empty push queue clears its age condition.
- Email once on firing, hourly while active and once on recovery. Operators configure transport/recipients; Lane does not integrate a separate email provider.

Operator assets must include startup/configuration, explicit migration, compatible rollback/fix-forward, DB/service outage recovery, auth/sync backlog and per-item rejection handling, webhook signature/key/lease troubleshooting, push retry/expiry/provider failures, and alert firing/recovery tests. User guides cover connection failures, rejected synchronization items with correction and explicit retry, the development reset, retained local input, conflicts, revoked devices and unsupported notifications. No separate diagnostic/support product is added.

## Build and Test

Run applicable root Rust checks plus server workspace format/clippy/tests, protocol checks, disposable PostgreSQL integration and affected OCI builds. Mock Logto, Delcore, APNs, FCM, Web Push and email transport. Include full pagination/internal-user mapping, audience separation, paid=false plus ACTIVE gating, dependency error mapping, account isolation and revoke-during-operation/subscription tests.

Planning tests use disposable PostgreSQL and cover each validation and violation code, concurrent writes racing on names, cycles and references (including delete versus assign), cleanup atomicity and rollback, history redaction after deletion, filter/sort/cursor equivalence with the shared vectors, constraint-rejected pushes with accepted state preserved and corrected retry, replay after response loss, account isolation and authorization revocation. Required exact boundary vectors include signature timestamps ±300/±301 s, changed-body signatures, immediate key rotation, duplicate/deleted event intake, expiry at exactly lease deadline, renewal just before/at expiry, release and stale completion, zero/multiple task completion with rollback, identical response-loss retries after completion/reopen/deletion, repository/receiver deletion distinctions, and stale account epochs. Test every shared version/conflict/deletion vector through all adapters.

For push use a fake clock: transient failures schedule 30/120/600/1800/3600-second delays, sixth retry is absent, Retry-After longer than expiry produces no send, permanent failure stops, and accepted/rejected/deleted proposals cancel pending delivery. Test notification origin/device dedupe, permission denial, generic content and unsupported environments. Validate alert sample thresholds, continuous-duration/recovery behavior and hourly repeat configuration with rule tests.

Origin-policy and consent tests drive the assembled HTTP router with distinct `LANE_PUBLIC_URL` and `WEB_ORIGIN` values, mocked Logto/Delcore and disposable PostgreSQL, using a browser fixture that parses the rendered form and derives `Origin`/`Referer` from its Referrer-Policy. Cover approve and cancel, hostile and `null` Origins without token consumption, wrong or missing cookie or token, expiry, replay, PKCE redemption, API-origin rejection on Connect, `/mcp` and web mutation routes, and unchanged frontend CORS. These fixtures do not establish real-browser behavior; whether a given browser applies CSP `form-action` to the post-submit redirect target of the consent POST is a separate browser check.

External Logto/push/email account/device delivery, production deployment and store publication are not required acceptance claims. Server tests do not establish native build/install/behavior acceptance.

## Dependencies and Integrations

Follow [Delcore server](servers-delcore-api-contract.md), [Delcore protocol](protos-delcore-v1-contract.md), [environment](repository-environment-contract.md), [server workspace](servers-rust-workspace-contract.md) and [workflow](repository-workflow-contract.md). Add Lane development database, fixed loopback route and service-specific Infisical injection to those owning integration files when runtime begins; do not silently inherit another service's database or port. Existing app build routes remain unchanged.

## Change Triggers

Change app, protocol, local-store and CLI contracts together for behavior or interface changes. Update scoped ownership/development instructions with new server paths and procedures. Keep public guides focused on supported user/operator workflows, not internal contracts.

## References

- [Project](project-lane.md)
- [Repository defaults](repository-defaults.md)
- [Protocol](protos-lane-v1-contract.md)
- [Distribution](packages-lane-distribution-contract.md)

## RealQA integration

The [RealQA contract](apps-lane-realqa-contract.md) extends this domain with durable
final-PNG attachments, authorized retrieval and synchronized URL-to-repository
rules. Preserve existing account routing, whole-task conflicts, request replay,
history, task moves and deletion precedence. Native saves commit locally before
optional image upload. Pending synchronization must not be reported as failed
local saving. Unsupported schema/format versions must be refused before writes.
These requirements do not establish implementation or release acceptance.

### Private image persistence and operations

Apply the forward capture migration explicitly. It adds URL-connection entities,
account-scoped immutable attachment manifests, temporary uploads and a durable
object-deletion queue. Existing immutable envelopes and replay fingerprints are
not rewritten. A server binary that predates this migration fails schema checks.
Synchronization uses the current format 3 and refuses transport clients of any other format.

Configure `LANE_R2_ENDPOINT`, `LANE_R2_BUCKET`, `LANE_R2_ACCESS_KEY_ID` and
`LANE_R2_SECRET_ACCESS_KEY` with a private R2 bucket and a bucket-scoped S3 token.
Startup requires this configuration. Disable bucket public access. Browser CORS
must allow the supported Lane origin, PUT/GET and the returned request headers;
never use credentialed wildcard origins. The endpoint uses the R2 S3 API and
Signature V4 with region `auto`, as specified in the
[R2 signed URL documentation](https://developers.cloudflare.com/r2/api/s3/presigned-urls/).

Begin-upload authorizes the account, task, immutable manifest and epoch under the
account lock. It signs a five-minute PUT to a temporary object, including the
expected Content-Length and image/png Content-Type. Completion downloads at most
the manifest byte count, validates the final PNG, and publishes a separate
immutable object. The client never receives PUT access to the immutable object.
An old PUT therefore cannot change an already published image. Download access
expires after one minute. Previously issued download capabilities remain usable
until expiry or provider deletion; no new access is issued after owning deletion.

Inline CreateCaptureTask writes the task, manifests, replay receipt and pending
PNG bytes in one PostgreSQL transaction. The worker publishes pending images to
R2 and clears the database byte copy after success. Failed transfers remain
pending and retry without a second task. MAX_ITEM_BYTES applies to each new image
and task metadata. It also bounds decoder working allocations and the expanded
pixel byte count. Decoder-limit failures return ITEM_TOO_LARGE for that item;
unrelated sync items continue. Already validated pending bytes remain eligible
for publication after a limit reduction. The limit does not restrict existing
downloads or deletion. No new
image-count, retention or local-storage quota is introduced.

History retains image bytes until task deletion. Repository deletion selects the
current task owner, so moving a task protects its images from deletion of its
former repository. Repository deletion also removes URL rules. Account clearing
queues all provider objects before erasing manifests. Garbage work survives
account clearing and provider outages. Temporary keys are queued when allocated;
the worker deletes them after the PUT capability expires, including abandoned
and completed uploads. Failed provider work remains in the queue. Inspect queue
counts without exporting PNG bytes, signed URLs or capture metadata. Restore
provider connectivity and valid bucket credentials to drain pending work; do not
purge pending rows as a recovery shortcut.

### Remote MCP capture and URL connection inputs

The eight `CaptureService` tools, `lane_save_task` and `lane_resolve_conflict`
advertise and accept the canonical format-2 ProtoJSON inputs: capture requests,
`CapturedImage`, `CaptureReference` (with `byteSize` as a decimal string),
`URLConnection`, `Task.captures` (including an empty array), `ENTITY_KIND_URL_CONNECTION`
by name or number 3, and `EntityVersion.urlConnection`. The advertised schema and
runtime validation both reject unknown fields, unknown enum values, malformed
bytes, any client-supplied `origin` and every pair of `EntityVersion` body members.
Typed decoding and backend dispatch are unchanged.

### Disposable S3 integration fixture

The affected Ubuntu Lane server integration step runs
`python3 servers/lane-api/tests/integration_fixture.py` from the repository root.
Set `TEST_DATABASE_URL` to a disposable loopback PostgreSQL database. The helper
rejects non-loopback database hosts and connection parameters that can redirect
the connection. The existing CI database step owns PostgreSQL startup and cleanup.

The helper creates a temporary Python virtual environment, installs the pinned
`moto[server]==5.1.22` test dependency, and verifies that version before serving. It
requires Python 3 with venv/pip, package-index access, and the existing Rust and
protocol build tools. It binds S3 to `127.0.0.1` on an OS-assigned port, creates and
checks a private test bucket, and supplies synthetic R2 credentials. Inherited
AWS, Moto and Lane R2 settings cannot select a live provider. Request logging is
disabled so signed URLs do not enter fixture logs.

After readiness, the helper runs all Lane ignored integration tests with the
PostgreSQL aliases and S3 endpoint configured. It returns Cargo's status and
stops its process groups and removes the temporary environment on success,
failure, SIGINT or SIGTERM. The in-memory bucket disappears with the mock. It
prints structured readiness and cleanup records without credentials or database
URLs. No production credentials, cloud bucket or OCI release job is required.
Other server matrix entries retain their existing PostgreSQL integration command.
These mock tests do not establish live R2 or native/device acceptance.
