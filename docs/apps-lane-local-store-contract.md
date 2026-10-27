# Lane Local Store Contract

## Scope

`apps/lane/native/store` owns the shared SQL schema and native store support. Existing Android, iOS, macOS and Windows hosts own platform Native Module registration and OS integration. `cmds/lane/src` owns the independent Rust adapter to the **same physical database and schema**. These ownership boundaries do not establish native platform acceptance.

## Runtime and Language

Keep platform languages and toolchains from the [application foundation](apps-lane-foundation.md). Use C++17/Objective-C++ SQLite integration on desktop/iOS and Kotlin platform SQLite or a bundled SQLite adapter on Android. Rust uses `rusqlite` with a pinned bundled SQLite engine suitable for the CLI targets. Do not require a running GUI, JavaScript process or local HTTP daemon for database operations. Share SQL and conformance vectors; do not assume separate language implementations share correctness automatically.

All writers must use a SQLite build with the upstream WAL-reset fix (3.51.3 or a verified patched branch/build). A system version string alone is not evidence of a backport. Bundle a patched engine where the platform engine cannot establish this requirement; do not silently mix unsafe writers into one WAL file. Record exact engine/source IDs and dependency notices with platform validation. See the [upstream WAL notes](https://sqlite.org/wal.html#walreset). This requirement does not claim the system SQLite used by earlier probes is patched.

## Users and Operators

One OS user, their native app, their Rust CLI/local stdio MCP, and background synchronization. OS permissions provide isolation; no Lane password/biometric lock is added.

## Interfaces and Contracts

### Native Module boundary

Register the `LaneStore` bridge before constructing the Lynx view. Calls originate only in ReactLynx Background Thread Scripting (`useEffect` and event handlers), not shared module initialization or the main-thread script.

The module exports:

- `open(requestJson, callback)`: input `{"formatVersion":3,"spaceId":""}`; empty space ID selects the registry's active space. Output includes `spaceId`, `deviceId`, `schemaVersion:3`, `formatVersion:3`, and `sequence` as a decimal string. The callback uses the same success/error envelope as `invoke`, with these fields inside `response` and `sequence` also at envelope level. Opening an explicit space requires it to exist; do not create a new empty store because lookup failed. A store of any other schema or format version fails with `UNSUPPORTED_FORMAT` and a message naming the explicit development reset; `open` never resets or migrates it.
- `invoke(method, requestJson, callback)`: method is the exact LaneService method name or local operation below. The request is ProtoJSON for that method plus an outer `{"spaceId":"…","request":{…}}` routing wrapper. Responses are `{"ok":true,"response":{…},"sequence":"…"}` or `{"ok":false,"error":{"reason":"…","message":"…","correlationId":"…","retryable":false}}`. Never expose SQL, file paths, arbitrary method evaluation or raw OS errors to JavaScript.
- `close(spaceId, callback)`: releases that view's subscription/handle, not another process's database or a background worker. It does not log out or delete data.

Invoke methods include all local LaneService and PlanningService methods, `GetLocalStatus`, `GetNotificationPreferences`, `SetNotificationPreferences`, `ConnectSync`, `DisconnectSync`, and `RemoveDeviceData`. `ResetDevelopmentData` is the explicit development reset below; it accepts `bool confirmed=1` and returns `bool completed=1` for the pinned selected space, and hosts reject any request whose `confirmed` is not true. `GetLocalStatus` has an empty request and returns `spaceId`, `deviceId`, `accountId`, `syncEnabled`, `connected`, `reconnectRequired`, optional `syncErrorReason`, decimal-string `pendingItemCount`, `itemErrors` entries with `requestId`, `entityId` and `reason`, optional Timestamp `lastSyncAt`, and `notificationPreferences` with `enabled`, `content` and `permission`. `SetNotificationPreferences` accepts `bool enabled=1, NotificationContent content=2` (DETAIL=1 default, GENERIC=2). Enabling requests OS permission; denial remains visible. `GetNotificationPreferences` takes an empty request and returns `bool enabled=1, NotificationContent content=2, NotificationPermission permission=3` (NOT_REQUESTED=1, GRANTED=2, DENIED=3, UNSUPPORTED=4). `ConnectSync` accepts `string account_id=1` (empty for first approval) and returns `string authorization_url=1, string challenge_id=2`; credentials return only to the native callback handler. Successful approval selects/creates the account space and returns the selection through a settings invalidation. `DisconnectSync` accepts `LocalRetention retention=1` (KEEP_LOCAL=1, REMOVE_LOCAL=2); `RemoveDeviceData` takes an empty request for the pinned selected space. Both return `bool completed=1`; hosts accept the numeric `retention` values Settings sends and the ProtoJSON names, leave validation to the store, and must not let post-commit host bookkeeping (such as releasing the removed space's watcher) replace the completed store response, so the Windows host classifies `retention` by JSON value type instead of reading it as a string; explicit connection/removal UI owns confirmation, not SQL initialization. Connection/removal operations follow the CLI/server contracts; server-only management dispatches typed RPCs and returns ONLINE_REQUIRED when offline, never queues a lease or account deletion.

`LaneHost` provides the OS-only `copyText(text, callback)` and `openURL(url, callback)` helpers. Both accept strings and return a JSON-string envelope with `ok` and, on failure, a safe typed `error`. `openURL` accepts HTTPS URLs without credentials; authentication callback URLs enter through the native OS handler, never this method. `ConnectSync` opens its authorization URL through the native host. No credential is returned to JavaScript.

Native account deletion requires two explicit UI actions. `BeginDataDeletion` pins its space, account, mode and returned `challengeId`; the UI opens `authorizationUrl` through `LaneHost.openURL`. A matching native `reauth_challenge` callback redeems the challenge with authenticated `POST /auth/reauth/proof` and retains the proof privately. It emits only `lane.reauthenticated` with one `{challengeId}` argument and never deletes data. The UI ignores events outside its outstanding adapter generation/space/account/mode/challenge and requires a second explicit Delete action. Its local `DeleteAccountData` request adds `challengeId` inside `request` alongside ProtoJSON fields and omits `reauthProof`. Hosts validate the pending association and strip this local-only extension before typed RPC dispatch, injecting the private proof. Supplied JavaScript proofs are rejected. No protocol field is added.

`LaneHost.cancelReauthentication(challengeId, callback)` invalidates the matching local flow and returns a JSON-string `{ok:true}` envelope. Cancel, view destruction, store close, account switch/disconnect and a new Begin invalidate previous approvals. Late callbacks, mismatched modes/accounts/challenges, expired proofs and replay cannot delete data. The server independently enforces proof expiry and one-use consumption.

Desktop delegates private proof storage to the bundled helper. `--remote --json --expected-space UUID rpc BeginDataDeletion --request-json -` records pending identity; `--json --space UUID sync callback URI` with `reauth_challenge` returns only `result.challengeId`. Final deletion uses `--remote --json --expected-space UUID --reauth-challenge ID rpc DeleteAccountData --request-json -`; stdin contains typed ProtoJSON without proof or the local challenge extension. Cancellation uses `--json --space UUID sync cancel-reauth --challenge-id ID` and returns `result.completed=true`. Proofs never enter arguments, logs, events or JavaScript.

An old-epoch `STALE_EPOCH` probe sets `store_meta.reconnect_required=1` and `GetLocalStatus.reconnectRequired=true`, stops uploads and retains data. A fresh explicit reconnect first confirms that retained data will upload to the new account dataset. The UI then invokes a new `ConnectSync` with nonempty `accountId` matching the pinned status; empty/mismatched targets fail without approval. This existing local adapter call is the confirmation boundary, not an authentication proof. Desktop translates it to helper `sync connect --confirm-epoch-change`; mobile records the same intent before browser approval. Pending approval binds space/account/old epoch and intent. The callback rechecks the old-epoch pull: an authenticated matching ACCOUNT deletion takes precedence; only `STALE_EPOCH` allows deliberate confirmation of the freshly approved epoch, with unchanged local state and matching account. An initial login that first discovers stale does not authorize reupload. Background retries never initiate approval or confirm a generation change.

Validated OS navigation links are queued until the view opens storage, then emitted as `lane.openRoute` with one `{kind: "open" | "repository" | "task", id?: "<UUID v7>", spaceId?: "<UUID v7>"}` argument. Local notification taps include their pinned existing `spaceId`; public links omit it. Consumers close/open that space explicitly before navigation and must not reinterpret the task ID in an unrelated current account. Subscribe before `open` to receive cold-start routes. Authentication callback failures emit `lane.hostError` with a safe reason/message; callbacks do not act as authorization until native state and PKCE validation succeeds.

Emit global event `lane.storeChanged` with one argument `{"spaceId":"…","sequence":"<decimal>"}` after committed changes. Consumers invalidate and reread typed queries; they retain unsaved editor input and indicate a remote/external change. Do not put content or credentials in events. Coalescing multiple commits is allowed; the monotonic sequence prevents lost invalidations. On resume/focus, compare sequence again before relying on cached reads. `NativeAdapter.subscribe` keeps an independent accepted sequence per subscription: every active subscriber receives exactly one callback per event newer than its baseline for the open space, regardless of registration order, and one subscriber's acceptance never consumes an event for another. The baseline is the opened store's sequence. A subscription registered before `open`, or one that survives close/reopen or a data-space switch, is rebased to the new open store's sequence on its next event. Other-space, malformed, duplicate and older events notify nobody; compare decimal sequences exactly beyond JavaScript's safe integer range.

Android registers a `LynxModule` on `LynxViewBuilder` before `build`, exports `@LynxMethod` callbacks, and uses `sendGlobalEvent`. iOS registers `NSObject<LynxModule>` on `LynxConfig` before view construction and dispatches through `sendGlobalEvent:withParams:`. Desktop loads the C++/Objective-C++ storage and OS service as a standard Node-API v8 module in the Lynxtron main process. ReactLynx uses `NativeModules.bridge.call("lane.native", {module, method, args}, callback)`; the main process permits only the existing `LaneStore` and `LaneHost` methods and string arguments. Host lifecycle controls are private to the main process. Native work uses Node asynchronous work and completes on the originating Node environment. Thread-safe native event queues are drained by the main process, which sends each event through `LynxWindow.sendGlobalEvent(name, ...arguments)`. No filesystem, SQL, process or credential capability is exposed to the renderer. Keep database work off the OS UI thread; marshal callbacks through the platform's supported runtime dispatch, never invoke NAPI values on arbitrary worker threads. Database ownership outlives individual views.

### Physical paths and account routing

The application and CLI must resolve the following **same** data root, not their framework's unrelated config directory:

| Platform  | Data root                                                                                              |
| --------- | ------------------------------------------------------------------------------------------------------ |
| macOS     | `$HOME/Library/Application Support/io.delino.lane`                                                     |
| Windows   | `FOLDERID_LocalAppData/io.delino.lane` (normally `%LOCALAPPDATA%/io.delino.lane`; not Roaming AppData) |
| Linux CLI | `${XDG_DATA_HOME:-$HOME/.local/share}/io.delino.lane`                                                  |
| iOS       | `<application Library/Application Support>/io.delino.lane`                                             |
| Android   | `<Context.filesDir>/io.delino.lane`                                                                    |

Use platform directory APIs and identical Rust path resolution. Each root has `registry.sqlite`; each data space has `spaces/<space-id>/lane.sqlite`. `space-id` is a UUID v7 allocated once under a registry transaction. Root `logs/` is shared by app/CLI retention. Production processes must not silently fall back to a temporary directory. Test roots may be injected only by explicit test configuration. The current macOS desktop distribution is not sandboxed; a later sandboxed distribution needs an explicit shared-container/CLI access contract before changing this path.

Registry schema version 1 uses `PRAGMA application_id=1279348293` (hex `0x4c414e45`) and `user_version=1`:

```sql
CREATE TABLE spaces (
  id TEXT PRIMARY KEY, account_key TEXT NOT NULL UNIQUE,
  account_id TEXT, tenant_id TEXT, issuer TEXT, sync_enabled INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE registry_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
```

`account_key` is the exact issuer URL, NUL, Delcore internal user ID, NUL, personal tenant ID, encoded as UTF-8 then lowercase SHA-256 hex. Standalone space uses the reserved key `local`. `registry_meta` holds `active_space_id` and one generated OS-install `device_id`. Create the first standalone space and active selection in one registry transaction. A connection to an account creates/selects its account space and merges a standalone dataset only after explicit connection approval. Never automatically attach an earlier account's retained data to a new account. Registry identity is not an authentication credential.

A command pins its space for its lifetime. UI space changes close the old handle and explicitly open the new one. Each write verifies the handle/space; a late callback must not update a newly selected account's UI. Logout/disable-sync offers KEEP_LOCAL or REMOVE_LOCAL. Retained account spaces remain segregated. Removing active local data is coordinated with open handles and background workers; never unlink a live database underneath another process. Mark the space unavailable in the registry, drain/refuse new operations, close handles, then remove its files. Restarting processes must check registry routing again.

Every physical store and observer connection holds a shared lifetime lease on `root/locks/<spaceId>.handles.lock`. This rendezvous file is retained permanently outside the removable space directory; never unlink or recreate it. POSIX uses `flock` shared/exclusive locks; Windows uses `LockFileEx` shared/exclusive locks at byte offset zero, length one. These exact cross-runtime primitives are required; POSIX `fcntl` record locks are not interchangeable. After obtaining the shared lease, recheck permanent registry marker `unavailable:<spaceId>=1` before opening SQLite. Deletion writes this marker first, stops its workers and closes its physical connections before releasing their shared leases, then waits at most five seconds for an exclusive lease. Remove `spaces/<spaceId>` only while holding that exclusive lease. Contention returns retryable `STORAGE_BUSY` cleanup-pending failure; retry must work against a marked space without reopening its database. Operations and observer polling drain handles when they encounter the marker. Restart resumes interrupted cleanup, while late opens remain refused. Preserve the unavailable marker and rendezvous file after successful physical removal.

## Storage

### Schema version 3

Each `lane.sqlite` has `application_id=1279348293`, `user_version=3`. UUIDs are lowercase text; times are UTC Unix microseconds in signed 64-bit INTEGER; ProtoJSON is UTF-8 TEXT. `envelope_json` stores EntityVersion; `request_json` stores SyncMutation. Every language reads the same columns:

```sql
CREATE TABLE store_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE entities (
  id TEXT PRIMARY KEY, kind INTEGER NOT NULL CHECK(kind IN (1,2,3,4,5,6)),
  repository_id TEXT, normalized_url TEXT,
  deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN (0,1))
);
CREATE UNIQUE INDEX repository_url ON entities(normalized_url)
  WHERE kind=1 AND deleted=0 AND normalized_url IS NOT NULL;
CREATE TABLE versions (
  version_id TEXT PRIMARY KEY, entity_id TEXT NOT NULL REFERENCES entities(id),
  envelope_json TEXT NOT NULL, created_at_us INTEGER NOT NULL,
  prompt_version INTEGER NOT NULL DEFAULT 0 CHECK(prompt_version IN (0,1))
);
CREATE TABLE heads (
  entity_id TEXT NOT NULL REFERENCES entities(id),
  version_id TEXT NOT NULL REFERENCES versions(version_id),
  PRIMARY KEY(entity_id,version_id)
);
CREATE TABLE receipts (
  method TEXT NOT NULL, request_id TEXT NOT NULL, fingerprint TEXT NOT NULL,
  entity_id TEXT, result_json TEXT NOT NULL, PRIMARY KEY(method,request_id)
);
CREATE TABLE outbox (
  request_id TEXT PRIMARY KEY, entity_id TEXT NOT NULL,
  request_json TEXT NOT NULL, state INTEGER NOT NULL DEFAULT 1,
  error_reason TEXT, created_at_us INTEGER NOT NULL
);
CREATE TABLE changes (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  entity_id TEXT, change_kind INTEGER NOT NULL
);
CREATE TABLE remote_objects (
  kind INTEGER NOT NULL, id TEXT NOT NULL, body_json TEXT,
  deleted INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(kind,id)
);
CREATE TABLE repository_mappings (
  local_id TEXT PRIMARY KEY, canonical_id TEXT NOT NULL
);
CREATE TABLE notification_receipts (
  proposal_id TEXT NOT NULL, device_id TEXT NOT NULL,
  state INTEGER NOT NULL, PRIMARY KEY(proposal_id,device_id)
);
CREATE TABLE attachments (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES entities(id),
  sha256 TEXT NOT NULL,
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  byte_size INTEGER NOT NULL,
  png_base64 TEXT,
  synchronized INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX attachment_owner ON attachments(task_id);

-- Retain only immutable identities after owner deletion; never image content.
CREATE TABLE attachment_identities (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  byte_size INTEGER NOT NULL
);

```

`store_meta` contains `space_id`, `device_id`, `format_version=3`, `account_epoch`, `pull_cursor`, `notification_enabled` (0/1), `notification_content` (DETAIL/GENERIC), and `sync_owner`/`sync_owner_expires_at_us`. No token or webhook signing key belongs here. `outbox.state` is PENDING=1 or BLOCKED=2; successful acknowledgments remove the row after durable local apply. A server rejection of a constraint violation keeps the row BLOCKED with `error_reason` `CONSTRAINT_VIOLATION:<VIOLATION>` (for example `CONSTRAINT_VIOLATION:PARENT_CYCLE`), preserves the local version and surfaces the item in `GetLocalStatus.itemErrors`. `change_kind` is ENTITY=1, REMOTE_OBJECT=2, SETTINGS=3, SYNC_STATUS=4. `remote_objects.kind` is RECEIVER=1, EVENT=2, PROCESSING=3; this table is a read cache, not an offline mutation queue. `notification_receipts.state` is PENDING=1, DELIVERED=2, CANCELED=3. Repository mappings resolve immutable original repository references without rewriting historical snapshots or fingerprints. Tombstones and receipts do not expire while they are needed to prevent replay.

Entity kinds are REPOSITORY=1, TASK=2, URL_CONNECTION=3, LABEL=4, MILESTONE=5 and CYCLE=6. Labels, milestones and cycles reuse the `entities`, `versions`, `heads`, `receipts`, `outbox` and `changes` tables and store their owning repository in `entities.repository_id`; schema version 3 adds no table. Task planning fields and relationship IDs live in the immutable task envelope and are not projected into columns.

`entities` is a query projection, not the authoritative version body. Repository URL lookup uses the current accepted head; conflict alternatives reserve their candidate URLs through domain validation, not extra duplicate entity rows. A conflict is multiple rows in `heads`. Before changing a URL, check current and conflict-candidate URLs under the write lock. Task repository projection follows the accepted head until resolution; reads return all alternatives so the UI never treats the projection as resolution. Validate head/entity linkage in every write; a version for another entity cannot be a head. Deletion clears URL/body projections, removes previous versions/heads, installs only its bodyless deletion version and removes associated event caches, histories and unsent notification content.

Native `ListTasks` accepts the optional typed `CompletedFilter` defined by the [Lane protocol](protos-lane-v1-contract.md#completed-task-selection). When the filter is omitted, the result set remains unchanged. For a duration selection, the native store follows immutable parent version IDs through the complete local history. It compares transition version identities across all current heads and returns an unknown entry when no unique transition is established. The completed predicate runs with the other filters before sorting and pagination. The opaque cursor binds the selection, absolute cutoff, account and space. This read adds no stored timestamp or schema change, and it keeps the existing selected head and conflict alternatives.

### Transactions and observation

Enable `foreign_keys=ON`, `journal_mode=WAL`, `synchronous=FULL` and `busy_timeout=5000` on every physical connection as applicable. Use local disks, not network shares or file-sync services. Use parameterized SQL. Native and Rust writers use `BEGIN IMMEDIATE`; re-read expected heads, deleted state, receipt and URL uniqueness **inside** that transaction. Planning writes validate inside the same transaction, after re-reading current heads: every referenced label, milestone, cycle and related task is a live entity of the task's repository; relationship graphs (parent, dependency, duplicate-of) have no self-link or cycle, using the union of every head's references for a conflicted task; label counts stay within 100; and a planning item's trimmed name is unique ignoring case within its repository and kind among live items. A move, task deletion or planning-item deletion writes the required cleanup versions for other tasks in that same transaction, each with the actor's origin; relationship edits to other tasks (subtasks, blocks, symmetric related links) do likewise. A violation rolls back everything and returns `CONSTRAINT_VIOLATION` with the named violation. Commit entity projection, immutable snapshot/history, head set, request receipt, durable outbox mutation, notification receipt and one `changes` row atomically. Local-only work retains sufficient versions for later initial synchronization; no server is required for commit. Roll back everything on disk-full, I/O, constraint or validation failure. Surface STORAGE_BUSY after the bounded busy wait; the user can retry with the same request ID. Do not retry partial statements outside their original transaction.

Read in short consistent transactions. Close cursors promptly. Poll `MAX(changes.sequence)` on a dedicated observer at 500 ms while the view is active, then immediately on foreground/resume; coalesce invalidations. The query works across Android pools because it reads committed table state. A dedicated persistent connection may use `PRAGMA data_version` as an optimization, but only compare values from that same physical connection. SQLite update hooks alone do not observe other processes. A rollback emits no invalidation; own commits can trigger immediate sequence reads. Keep the sequence monotonic within a database and do not prune it; only the explicit development reset below starts a new database with a new sequence.

Only one background sync owner per space may drain the outbox at a time. Acquire/renew an expiring owner record in a short write transaction; never hold a SQLite lock across network calls. Requests remain idempotent if an owner crashes or a lease expires. Desktop embeds the matching distribution CLI artifact as `helpers/lane` or `helpers/lane.exe` under the packaged Lynxtron application resource directory (`Contents/Resources/app` on macOS, `resources/app` on Windows) (avoiding the case-insensitive host/helper name collision on either platform) and supervises `lane --space <uuid> sync run`. It sends termination only on explicit exit/disconnection, not window closure, and retries failed starts with a bounded delay. Missing helper binaries leave local operations available and report the dependency failure; no global npm executable is assumed. Desktop uses the Rust CLI `sync run` worker supervised by the host; CLI users may run the same worker without the app. Mobile host scheduling runs the same wire protocol at platform-permitted opportunities. Closing the desktop window releases rendering, not an enabled worker. App/CLI writes remain possible when the worker is unavailable.

### Compatibility and development reset

Before any write, check application ID, schema version and format version. Only schema 3 with format 3 is supported. Any other nonempty database, including an earlier schema, fails with UNSUPPORTED_FORMAT and an instruction that names the explicit reset procedure below; never replace it, migrate it or treat it as empty. Creation is allowed only for a missing/empty file selected by the registry. All writers check compatibility at transaction entry so an old long-lived process cannot write after the file is replaced.

Lane is not deployed and development data may be discarded, so this pre-release exception provides no backward compatibility, no data-preserving migration and no old-client support. The earlier `data migrate` command, `Store::migrate` operation and `migrate-1-to-2.sql` script are removed. A store is never reset implicitly.

#### Development data reset

`lane --space <uuid> data reset --confirm-development-reset` and the native `Store::reset(spaceId)` operation reset only the selected existing local space. The CLI rejects `--remote`, an omitted `--space`, a missing confirmation flag and request payloads. Desktop and mobile hosts expose the same operation as the `ResetDevelopmentData` bridge method, used by a Settings "Reset development data" confirmation that requires the user to type the displayed confirmation text; cancelling or mismatched text changes nothing.

1. Validate registry routing and the pinned space. Take the exclusive lifetime lease on `root/locks/<spaceId>.handles.lock`, waiting at most five seconds. Open handles or running workers of any process make it fail with retryable `STORAGE_BUSY` and no change; close the app window's handles and stop `sync run` first.
2. While holding the lease, remove the space's database, WAL and shared-memory files and its attachment content through the existing secure-removal path, then create an empty schema 3 database at the same path with `user_version=3`, `format_version=3`, the same `space_id` and the registry's `device_id`, a fresh change sequence and no pending outbox or receipts. Do not write the permanent `unavailable` marker and never unlink the rendezvous lock file.
3. A standalone space keeps its standalone registry identity. An account space keeps its account binding but clears local data and sets `reconnect_required=1`; synchronization stays stopped until the user confirms reconnection exactly as for an epoch change (`sync connect --confirm-epoch-change`). Reset never contacts the server, deletes server data or signs out.
4. An interrupted reset is repeated by running the same command; a partially removed space is never opened as valid data. After success, hosts close and reopen the space's handles and discard cached reads. Reset is for disposable development data only and never runs automatically.

Logical deletion removes content immediately in connected stores and redacts retained receipt/outbox responses; retain only required identities/fingerprints. Use secure deletion and coordinated checkpointing to reduce residual SQLite content without claiming forensic erasure of storage media. A failure must not report success. No user-facing backup/export/trash feature is introduced.

The macOS host retains the latest APNs token in memory and reconciles `RegisterPush` per space against the store's current status (`syncEnabled`, enabled notification preferences and the space's device), as described in the [notification requirements](apps-lane-foundation.md#notifications). It never persists the token, registers a space under another account's identity, or requests permission again.

Remote APNs and FCM messages are data-only wake hints with `type=lane.proposal`, `proposalId` and the authenticated registration's `accountId`. APNs uses `content-available:1`, background push type and priority 5; FCM uses HIGH priority without a notification payload. The native host selects that retained account, synchronizes, then claims eligible local notification receipts and applies current preferences before OS display. Never fall back to another active account. Apple background wakes can be delayed or omitted; the next permitted synchronization checks pending proposals. This does not guarantee immediate notification delivery while suspended.

## Security

Use per-user directory permissions (0700 and 0600 files on POSIX; user ACLs on Windows). Keep credentials in platform credential storage, referenced by space/device, never in task tables or JS. Validate callback state/PKCE and deep-link IDs. Synced data for one account cannot be relabeled for another. Pending data survives auth/network failures; remote disconnection denies server access but cannot erase an offline device.

## Logging

Use structured local logs without content/secrets. Planning, relationship and reset operations log the operation, correlation ID, result category and duration only; never task content or label/milestone/cycle names and descriptions. Native and CLI share the root log retention budget: 30 days and combined 100 MiB, deleting oldest files when either bound is reached. Serialize append and rotation across processes using the retained `root/logs/.rotation.lock`: POSIX `flock(LOCK_EX)` or Windows `LockFileEx` exclusive byte zero, length one. Never remove the lock file during pruning. The macOS desktop host writes UTC daily buckets named `native-macos-YYYYMMDD.log`, never appends to an earlier date and expires a bucket when its UTC bucket start plus 30 days is reached, independent of modification time (the conservative daily rule can discard the final part of a bucket less than one day early but never retains a record past 30 days). Under the same lock, before its first bucket append, it removes the legacy timestamp-less `native-macos.log` instead of renaming or copying it; then it prunes by age and by the combined 100 MiB budget (oldest modification time first, with room reserved for the new event) and appends. Failed retirement or pruning leaves the old file in place and never relabels its records as current. Other files keep modification-time expiry. No automatic upload. Diagnostic errors expose categories, not user filesystem paths or SQL parameter values.

## Build and Test

Run the common protocol vectors in native and Rust CLI adapters. Test both writer directions against the same physical file, concurrent saves, crash/rollback/disk-full behavior, close/reopen, unsupported schema refusal naming the reset command, the explicit reset (rejections, active-handle `STORAGE_BUSY`, same-space initialization and scope isolation, account-space reconnect requirement, restart and new-format persistence), planning validation/cleanup atomicity with rollback, account routing and commit-only observation in a running app. Test task/history/outbox/sequence atomicity. Check runtime SQLite source IDs and the WAL fix on all targets.

Run app-local `pnpm test` for frontend changes and the existing platform build commands. Native build/install/behavior gates apply separately to macOS, Windows, iOS and Android. Cross-compiling a CLI or compiling a Lynx bundle does not prove native behavior. Preserve existing CI exclusions for macOS PR/main/affected runs; store/provider acceptance is separate.

The native CMake build verifies vendored dependency bytes against
`native/store/dependencies.json`. Vendored text uses LF checkout bytes so
Windows Git CRLF conversion cannot change those hashes; exact-path LFS rules
retain their binary checkout behavior. MSVC compiles the store and its tests
with UTF-8 source/execution encoding so Unicode validation does not depend on
the host code page. Build the native store tests with `LANE_STORE_TESTS=ON`
and run their CTest entry separately from the desktop renderer tests.

## Dependencies and Integrations

Follow [protocol](protos-lane-v1-contract.md), [CLI](cmds-lane-cli-contract.md), [server](servers-lane-api-contract.md), and existing SDK notices. Store conformance fixtures with protocol source; host implementations must not invent incompatible columns or paths.

## Change Triggers

Change this contract and protocol/CLI/app contracts together for schema, path, callback or domain-rule changes. Update scoped instructions when ownership or development procedures change.

## References

- [Project](project-lane.md)
- [Repository defaults](repository-defaults.md)
- [SQLite committed-change observation](https://sqlite.org/pragma.html#pragma_data_version)
- [SQLite WAL](https://sqlite.org/wal.html)

## RealQA integration

The [RealQA contract](apps-lane-realqa-contract.md) extends this domain with durable
final-PNG attachments, authorized retrieval and synchronized URL-to-repository
rules. Preserve existing account routing, whole-task conflicts, request replay,
history, task moves and deletion precedence. Native saves commit locally before
optional image upload. Pending synchronization must not be reported as failed
local saving. Unsupported schema/format versions must be refused before writes.
These requirements do not establish implementation or release acceptance.

### RealQA schema 2

RealQA introduced data-store schema and format version 2, which the current schema 3 retains; the registry remains
version 1. Schema 2 added entity kind 3 for URL connections and an `attachments`
table keyed by attachment UUID, with owning task, SHA-256, dimensions, byte size,
optional final PNG bytes encoded as base64, and synchronization state. Empty
image bytes mean synchronization is pending, not an empty image. Attachment
content is immutable. Per-image browser metadata remains in the versioned task.

Save the task, final images, receipt, outbox and change notification in one
transaction. Retain images after removal from the current prompt while history
can reference them; delete them with their owning task. A task move changes no
attachment ownership. Repository deletion cascades its current tasks and URL
connections. Retrieval requires a matching task and attachment in the pinned
space. Unsupported schema writers must stop before any mutation.

All native and Rust adapters must support the current schema before distribution.
Do not silently initialize, migrate or replace an existing earlier-version store;
refuse it and name the explicit development reset. Historical envelopes of format 1 and 2 remain
readable without rewriting immutable versions; new envelopes use format 3.
Synchronization negotiation must prevent older clients from dropping capture references,
URL connections or planning fields.

Attachment deletion retains `attachment_identities` rows containing only immutable IDs, owner IDs, dimensions, byte counts and SHA-256 digests. Reusing an attachment ID for another owner or content is rejected even after deletion and account-space imports. Local physical removal deletes these records with the space. No image bytes or browser metadata belong in the identity table.
