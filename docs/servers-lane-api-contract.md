# Lane API Contract

## Scope

`servers/lane-api` owns the Rust API, optional remote MCP endpoint, webhook intake, PostgreSQL state/migrations, synchronization, push workers, OCI artifacts and operator assets. It belongs to the independent server Cargo workspace and uses FSL-1.1-ALv2. This contract defines required behavior, not a deployed service. The [application contract](apps-lane-foundation.md) retains the full product scope.

## Runtime and Language

Use Rust, repository Connect infrastructure and PostgreSQL. Consume canonical local `protos/lane/v1` through `servers/crates/delino-protos`. Add an explicit server workspace member when implementation begins. Do not introduce a Go server or a second protocol repository. Remote MCP and webhook handlers call the same domain transaction layer as Connect.

## Users and Operators

Individual users, their approved MCP clients/devices, webhook senders, and deployment operators. Every approved remote MCP grant covers all repositories in that personal account. Lane runs no agents, AI provider calls, Git fetches, event interpretation or user-rule automation engine.

## Interfaces and Contracts

### Routes and deployment boundary

- `https://lane.api.delino.io/lane.v1.LaneService/<Method>`: typed repository/task/history operations.
- `.../lane.v1.SyncService/<Method>`: version negotiation, push/pull and live invalidations.
- `.../lane.v1.AccountService/<Method>`: devices, connection approvals, notification registration and Lane account-data deletion.
- `.../lane.v1.WebhookService/<Method>`: receiver/event/processing management.
- `https://lane.api.delino.io/mcp`: authenticated Streamable HTTP MCP over the same business layer.
- `POST /webhooks/<receiver-id>`: signed Lane common-format intake.
- `/health/live`, `/health/ready`, `/metrics`: process health, dependency readiness and operator-only metrics.

Serve the static online web client at `lane.delino.io` through Cloudflare Pages. Restrict CORS to configured frontend origins; do not allow arbitrary credentialed origins. Configuration and runbooks must produce deployable artifacts, but runtime work does not authorize production deployment.

### Identity and authorization

Logto owns identity; Delcore owns internal users and tenants. During user connection, obtain a token with the Delcore resource audience and call `delcore.v1.DelcoreService/GetCurrentUser`, following every `next_after_id` page. Resolve the returned internal `user_id` and the tenant with `personal=true` and `owner_id=user_id`. Do not select the first tenant or use a one-item discovery helper that discards pagination. Reject an ambiguous/missing personal tenant; do not substitute an organization tenant or equate a Logto subject with the internal user ID.

Register Lane as a Delcore service with `authorize` scope and capability `manage`. For every protected server operation, use Lane's M2M identity to call `Authorize` with the stored personal `tenant_id`, internal `user_id`, capability `manage`, `paid=false`, no `feature_key`, and no group values. Also call `GetTenantStates` and require ACTIVE: unpaid authorization alone does not establish operator-block/deletion state. Lane has no paid plan, feature gate or metering requirement. Deny BLOCKED, DELETING, DELETED and UNKNOWN access; only explicit DELETED authorizes tenant-lifecycle cleanup, never errors/unavailability/UNKNOWN. User-requested Lane deletion is its separate authorized action and must not delete Logto identity.

Validate issuer, audience, expiry and binding of caller credentials. Invalid caller credentials return unauthenticated. Delcore/M2M token acquisition or dependency failure returns unavailable with a correlation ID, not a claim that the caller's token is invalid. Authorization denial returns permission_denied. Local native/CLI work continues without either server; keep its outbox on dependency/auth failure.

CLI login and remote MCP approval use browser Logto authorization code flow with PKCE, state, exact redirect validation and resource/audience separation. An MCP-audience token must not be forwarded to Delcore. Obtain the separate Delcore-resource token during approval to resolve identity, then bind the durable Lane grant to internal user/personal tenant and use M2M reauthorization afterward. Do not store or accept a caller-selected tenant as proof. Expose standards-compatible MCP authorization metadata and approved-client flow; a bare Logto callback without grant/resource binding is insufficient.

Device and MCP grant revocation checks run under the same account transaction lock as each data operation. A revocation that commits first blocks that operation; an earlier committed operation can complete. Subscriptions recheck authorization/revocation before publishing invalidations and terminate on revocation. Do not rely on token expiry or a process-local approval cache. List individually approved connections and devices with last synchronization times; disconnecting cannot erase an offline device's existing data.

### Browser and native authorization adapter

`GET /oauth/authorize` accepts authorization-code parameters, an exact registered redirect, S256 PKCE and a UUID v7 `device_id` when the installation already has one. Preserve that device identity through token issuance. `POST /oauth/token` accepts form-encoded code or refresh grants; `POST /oauth/revoke` revokes the bound session. Operator-required `LANE_OAUTH_CLIENTS` binds each client to its redirects, display name, origin kind and API or MCP resource. Browser approval shows full-account consent for MCP. Revalidate registration before callback, consent and token issuance.

Web uses `/auth/login?return_to=...`, `/auth/callback`, `/auth/session`, `/auth/refresh` and `/auth/logout`. The session cookie is Secure, HttpOnly and SameSite=Lax; mutations additionally require the configured Origin and `X-Lane-CSRF`. Session JSON uses snake_case identity fields, `csrf_token`, and `notification_preferences` with `enabled` and numeric `content` (1 detail, 2 generic). `/auth/push-config` exposes only the public VAPID `public_key`, or null when disabled. Web return URLs must be configured exactly. Reauthentication returns only `reauth_challenge` to the saved return URL; authenticated `/auth/reauth/proof` redeems it for the one-use proof.

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

Intake body is UTF-8 JSON `{"id":"sender-event-id","type":"event-type","occurredAt":"RFC3339 UTC timestamp","payload":{}}`. Require nonempty ID/type, valid timestamp, JSON object payload and no duplicate JSON keys. Sender IDs/types are opaque strings, not GitHub-specific identifiers. Validate configured byte limits before allocating an unbounded body. Canonical parsed content uses RFC 8785 JSON for deduplication; timestamp normalizes to UTC microseconds. A fresh transport timestamp/signature is not new event content.

Require `Lane-Timestamp` as canonical base-10 Unix seconds and `Lane-Signature` as `v1=<lowercase HMAC-SHA256 hex>`. Signed bytes are ASCII timestamp, one period (`.`), then the **exact raw request body bytes**. Secret is 32 cryptographically random bytes shown once as unpadded base64url; senders decode it to bytes for HMAC. Compare signatures in constant time. Accept an absolute timestamp difference of at most 300 seconds, including exactly either boundary; reject ±301 seconds. Retries keep event ID/content and refresh timestamp/signature.

Store signing keys encrypted with an operator-managed encryption key because HMAC verification needs retrievable key bytes. Never expose a retrieve-secret RPC, log plaintext or persist it in replay responses. Creation/rotation returns plaintext once only; response loss requires rotation. Rotation atomically replaces the active key and invalidates the old one immediately. Deleting a receiver stops future intake but keeps its existing events and valid processing leases. Reassignment changes only future events; retained event repository IDs do not change.

Deduplicate by `(account_id,receiver_id,sender_event_id)`. Same identity and content returns the prior event identity; changed content returns conflict. After deletion retain only identity/content fingerprint/deletion version; replay returns an already-received/deleted result without content or recreation. Intake does not create tasks or mark processing complete.

### Event leases and atomic processing

Use database server time and row/account locks. Claim succeeds only for a nondeleted PENDING event or an expired lease; mint a random opaque token and expiry `now + 5 minutes`. A token is valid only when `now < expires_at`, bound to event, account and claimant. Renewal before expiry sets expiry to renewal time plus five minutes. Store token hashes, not plaintext in logs or lists. Release invalidates the token and permits immediate reclaim; record the error category/reason without payload or credentials.

CompleteEvent validates the active token, nondeleted event/repository, authorization, all proposed tasks and their request identities in **one transaction**, then creates zero or more tasks and a processing record and marks the event COMPLETED. Zero tasks is valid. Invalid task input or any storage error rolls back the entire completion. Preserve a completion receipt with its task IDs so response-loss retries return the original outcome even though that token is consumed; check an identical authorized receipt before active-lease validation. Changed completion content under the same request ID fails. Unknown, released or expired tokens without that receipt always fail, even if nobody else claimed the event.

Deleting an event or its repository invalidates its lease; completion afterward cannot create tasks. Receiver deletion alone does not invalidate leases. Reopening a completed event changes its state to PENDING, preserves prior processing records/task links, and allows a new lease. An old completion replay returns its original outcome without completing the new processing round. Tasks moved later retain IDs and event links; the event stays in its original repository.

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

Apply explicit forward migrations outside startup. Startup verifies schema compatibility and configuration, then refuses unsupported schemas. Roll back a binary only if it supports the current schema; otherwise recover with a corrected forward release. No down migrations are required. Encrypt server storage and transport; encrypted retrievable webhook keys are distinct from hashed grant/lease credentials. E2EE is not required.

## Security

Scope every lookup, cursor, history parent, idempotency key and lease to the current personal account. Validate task destinations under that account. Lock account revocation/deletion and resource writes consistently. CSRF-protect browser approval/deletion flows and reject arbitrary redirects. Secrets, tokens, task/prompt content and webhook payloads never enter logs/metrics or public configuration. Use service-local safe `.env.example` names/placeholders only; no root environment file.

## Logging

Use `tracing`, correlation IDs and structured operation/result/duration metadata. Server logs expire after 30 days. Metrics include request/error counts, duration histograms excluding long-lived connection lifetime, DB/service probes, pending sync failures and push queue age/attempt results. Keep account/resource IDs out of high-cardinality metric labels; dashboards and alerts reveal no content.

Supply Prometheus rules, dashboards and Alertmanager email routing. Initial thresholds are configurable operational triggers, **not SLOs**:

- Probe service and DB every 30 seconds, timeout five seconds. Fire after five minutes continuously unhealthy; resolve after five minutes continuously healthy.
- Over a rolling five-minute window with at least 20 requests, fire if server errors/timeouts exceed 5% or p95 exceeds two seconds continuously for five minutes.
- For push, fire when transient failures exceed 10% over 15 minutes with at least 20 attempts, continuously for five minutes. Also fire when oldest pending delivery exceeds ten minutes.
- Resolve API/push alerts after the triggering condition clears continuously for ten minutes. Insufficient samples alone do not prove recovery. An empty push queue clears its age condition.
- Email once on firing, hourly while active and once on recovery. Operators configure transport/recipients; Lane does not integrate a separate email provider.

Operator assets must include startup/configuration, explicit migration, compatible rollback/fix-forward, DB/service outage recovery, auth/sync backlog and per-item rejection handling, webhook signature/key/lease troubleshooting, push retry/expiry/provider failures, and alert firing/recovery tests. User guides cover connection failures, retained local input, conflicts, revoked devices and unsupported notifications. No separate diagnostic/support product is added.

## Build and Test

Run applicable root Rust checks plus server workspace format/clippy/tests, protocol checks, disposable PostgreSQL integration and affected OCI builds. Mock Logto, Delcore, APNs, FCM, Web Push and email transport. Include full pagination/internal-user mapping, audience separation, paid=false plus ACTIVE gating, dependency error mapping, account isolation and revoke-during-operation/subscription tests.

Required exact boundary vectors include signature timestamps ±300/±301 s, changed-body signatures, immediate key rotation, duplicate/deleted event intake, expiry at exactly lease deadline, renewal just before/at expiry, release and stale completion, zero/multiple task completion with rollback, identical response-loss retries after completion/reopen/deletion, repository/receiver deletion distinctions, and stale account epochs. Test every shared version/conflict/deletion vector through all adapters.

For push use a fake clock: transient failures schedule 30/120/600/1800/3600-second delays, sixth retry is absent, Retry-After longer than expiry produces no send, permanent failure stops, and accepted/rejected/deleted proposals cancel pending delivery. Test notification origin/device dedupe, permission denial, generic content and unsupported environments. Validate alert sample thresholds, continuous-duration/recovery behavior and hourly repeat configuration with rule tests.

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
