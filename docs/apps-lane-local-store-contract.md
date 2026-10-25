# Lane Local Store Contract

## Scope

Planned `apps/lane/native/store` owns the shared SQL schema and native store support. Existing Android, iOS, macOS and Windows hosts own platform Native Module registration and OS integration. Planned `cmds/lane/internal` owns the independent Go adapter to the **same physical database and schema**. These are planned interfaces, not native implementation evidence.

## Runtime and Language

Keep platform languages and toolchains from the [application foundation](apps-lane-foundation.md). Use C++17/Objective-C++ SQLite integration on desktop/iOS and Kotlin platform SQLite or a bundled SQLite adapter on Android. Go uses `database/sql` with a pinned CGO-free SQLite driver suitable for the CLI targets. Do not require a running GUI, JavaScript process or local HTTP daemon for database operations. Share SQL and conformance vectors; do not assume separate language implementations share correctness automatically.

All writers must use a SQLite build with the upstream WAL-reset fix (3.51.3 or a verified patched branch/build). A system version string alone is not evidence of a backport. Bundle a patched engine where the platform engine cannot establish this requirement; do not silently mix unsafe writers into one WAL file. Record exact engine/source IDs and dependency notices with platform validation. See the [upstream WAL notes](https://sqlite.org/wal.html#walreset). This requirement does not claim the system SQLite used by earlier probes is patched.

## Users and Operators

One OS user, their native app, their Go CLI/local stdio MCP, and background synchronization. OS permissions provide isolation; no Lane password/biometric lock is added.

## Interfaces and Contracts

### Native Module boundary

Register `LaneStore` before constructing the Lynx view. Calls originate only in ReactLynx Background Thread Scripting (`useEffect` and event handlers), not shared module initialization or the main-thread script.

The module exports:

- `open(requestJson, callback)`: input `{"formatVersion":1,"spaceId":""}`; empty space ID selects the registry's active space. Output includes `spaceId`, `deviceId`, `schemaVersion:1`, `formatVersion:1`, and `sequence` as a decimal string. Opening an explicit space requires it to exist; do not create a new empty store because lookup failed.
- `invoke(method, requestJson, callback)`: method is the exact LaneService method name or local operation below. The request is ProtoJSON for that method plus an outer `{"spaceId":"…","request":{…}}` routing wrapper. Responses are `{"ok":true,"response":{…},"sequence":"…"}` or `{"ok":false,"error":{"reason":"…","message":"…","correlationId":"…","retryable":false}}`. Never expose SQL, file paths, arbitrary method evaluation or raw OS errors to JavaScript.
- `close(spaceId, callback)`: releases that view's subscription/handle, not another process's database or a background worker. It does not log out or delete data.

Invoke methods include all local LaneService methods, `GetLocalStatus`, `GetNotificationPreferences`, `SetNotificationPreferences`, `ConnectSync`, `DisconnectSync`, and `RemoveDeviceData`. `GetLocalStatus` has an empty request and returns the selected space/device IDs, sync enabled/connected status, pending item count, per-item error identities/reasons, last sync time and notification preferences. `SetNotificationPreferences` accepts `bool enabled=1, NotificationContent content=2` (DETAIL=1 default, GENERIC=2). Enabling requests OS permission; denial remains visible. `GetNotificationPreferences` takes an empty request and returns `bool enabled=1, NotificationContent content=2, NotificationPermission permission=3` (NOT_REQUESTED=1, GRANTED=2, DENIED=3, UNSUPPORTED=4). `ConnectSync` accepts `string account_id=1` (empty for first approval) and returns `string authorization_url=1, string challenge_id=2`; credentials return only to the native callback handler. Successful approval selects/creates the account space and returns the selection through a settings invalidation. `DisconnectSync` accepts `LocalRetention retention=1` (KEEP_LOCAL=1, REMOVE_LOCAL=2); `RemoveDeviceData` takes an empty request for the pinned selected space. Both return `bool completed=1`; explicit connection/removal UI owns confirmation, not SQL initialization. Connection/removal operations follow the CLI/server contracts; server-only management dispatches typed RPCs and returns ONLINE_REQUIRED when offline, never queues a lease or account deletion.

Emit global event `lane.storeChanged` with one argument `{"spaceId":"…","sequence":"<decimal>"}` after committed changes. Consumers invalidate and reread typed queries; they retain unsaved editor input and indicate a remote/external change. Do not put content or credentials in events. Coalescing multiple commits is allowed; the monotonic sequence prevents lost invalidations. On resume/focus, compare sequence again before relying on cached reads.

Android registers a `LynxModule` on `LynxViewBuilder` before `build`, exports `@LynxMethod` callbacks, and uses `sendGlobalEvent`. iOS registers `NSObject<LynxModule>` on `LynxConfig` before view construction and dispatches through `sendGlobalEvent:withParams:`. Desktop registers `LaneStore` using `LynxView::Builder::RegisterNativeModule`; use the pinned SDK's Weak NAPI mapping (`USE_WEAK_SUFFIX_NAPI`) and `SendGlobalEvent` with a JSON argument array. Keep database work off the OS UI thread; marshal callbacks through the platform's supported runtime dispatch, never invoke NAPI values on arbitrary worker threads. Database ownership outlives individual views.

### Physical paths and account routing

The application and CLI must resolve the following **same** data root, not their framework's unrelated config directory:

| Platform  | Data root                                                                                              |
| --------- | ------------------------------------------------------------------------------------------------------ |
| macOS     | `$HOME/Library/Application Support/io.delino.lane`                                                     |
| Windows   | `FOLDERID_LocalAppData/io.delino.lane` (normally `%LOCALAPPDATA%/io.delino.lane`; not Roaming AppData) |
| Linux CLI | `${XDG_DATA_HOME:-$HOME/.local/share}/io.delino.lane`                                                  |
| iOS       | `<application Library/Application Support>/io.delino.lane`                                             |
| Android   | `<Context.filesDir>/io.delino.lane`                                                                    |

Use platform directory APIs and identical Go path resolution. Each root has `registry.sqlite`; each data space has `spaces/<space-id>/lane.sqlite`. `space-id` is a UUID v7 allocated once under a registry transaction. Root `logs/` is shared by app/CLI retention. Production processes must not silently fall back to a temporary directory. Test roots may be injected only by explicit test configuration. The current macOS desktop distribution is not sandboxed; a later sandboxed distribution needs an explicit shared-container/CLI access contract before changing this path.

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

## Storage

### Schema version 1

Each `lane.sqlite` has `application_id=1279348293`, `user_version=1`. UUIDs are lowercase text; times are UTC Unix microseconds in signed 64-bit INTEGER; ProtoJSON is UTF-8 TEXT. `envelope_json` stores EntityVersion; `request_json` stores SyncMutation. Every language reads the same columns:

```sql
CREATE TABLE store_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE entities (
  id TEXT PRIMARY KEY, kind INTEGER NOT NULL CHECK(kind IN (1,2)),
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
```

`store_meta` contains `space_id`, `device_id`, `format_version=1`, `account_epoch`, `pull_cursor`, `notification_enabled` (0/1), `notification_content` (DETAIL/GENERIC), and `sync_owner`/`sync_owner_expires_at_us`. No token or webhook signing key belongs here. `outbox.state` is PENDING=1 or BLOCKED=2; successful acknowledgments remove the row after durable local apply. `change_kind` is ENTITY=1, REMOTE_OBJECT=2, SETTINGS=3, SYNC_STATUS=4. `remote_objects.kind` is RECEIVER=1, EVENT=2, PROCESSING=3; this table is a read cache, not an offline mutation queue. `notification_receipts.state` is PENDING=1, DELIVERED=2, CANCELED=3. Repository mappings resolve immutable original repository references without rewriting historical snapshots or fingerprints. Tombstones and receipts do not expire while they are needed to prevent replay.

`entities` is a query projection, not the authoritative version body. Repository URL lookup uses the current accepted head; conflict alternatives reserve their candidate URLs through domain validation, not extra duplicate entity rows. A conflict is multiple rows in `heads`. Before changing a URL, check current and conflict-candidate URLs under the write lock. Task repository projection follows the accepted head until resolution; reads return all alternatives so the UI never treats the projection as resolution. Validate head/entity linkage in every write; a version for another entity cannot be a head. Deletion clears URL/body projections, removes previous versions/heads, installs only its bodyless deletion version and removes associated event caches, histories and unsent notification content.

### Transactions and observation

Enable `foreign_keys=ON`, `journal_mode=WAL`, `synchronous=FULL` and `busy_timeout=5000` on every physical connection as applicable. Use local disks, not network shares or file-sync services. Use parameterized SQL. Native and Go writers use `BEGIN IMMEDIATE`; re-read expected heads, deleted state, receipt and URL uniqueness **inside** that transaction. Commit entity projection, immutable snapshot/history, head set, request receipt, durable outbox mutation, notification receipt and one `changes` row atomically. Local-only work retains sufficient versions for later initial synchronization; no server is required for commit. Roll back everything on disk-full, I/O, constraint or validation failure. Surface STORAGE_BUSY after the bounded busy wait; the user can retry with the same request ID. Do not retry partial statements outside their original transaction.

Read in short consistent transactions. Close cursors promptly. Poll `MAX(changes.sequence)` on a dedicated observer at 500 ms while the view is active, then immediately on foreground/resume; coalesce invalidations. The query works across Android pools because it reads committed table state. A dedicated persistent connection may use `PRAGMA data_version` as an optimization, but only compare values from that same physical connection. SQLite update hooks alone do not observe other processes. A rollback emits no invalidation; own commits can trigger immediate sequence reads. Keep the sequence monotonic and do not prune/reset it in version 1.

Only one background sync owner per space may drain the outbox at a time. Acquire/renew an expiring owner record in a short write transaction; never hold a SQLite lock across network calls. Requests remain idempotent if an owner crashes or a lease expires. Desktop uses the Go CLI `sync run` worker supervised by the host; CLI users may run the same worker without the app. Mobile host scheduling runs the same wire protocol at platform-permitted opportunities. Closing the desktop window releases rendering, not an enabled worker. App/CLI writes remain possible when the worker is unavailable.

### Compatibility and migrations

Before any write, check application ID, schema version and format version. Unknown nonempty databases fail with UNSUPPORTED_FORMAT and an update instruction; never replace them or treat them as empty. Creation is allowed only for a missing/empty file selected by the registry. Version 1 has no automatic migration from unrelated data. Future local migrations require an exclusive schema transaction, all writers checking compatibility at transaction entry, and a reviewed forward-migration path. Do not let an old long-lived process write after another process upgrades the schema.

Logical deletion removes content immediately in connected stores and redacts retained receipt/outbox responses; retain only required identities/fingerprints. Use secure deletion and coordinated checkpointing to reduce residual SQLite content without claiming forensic erasure of storage media. A failure must not report success. No user-facing backup/export/trash feature is introduced.

## Security

Use per-user directory permissions (0700 and 0600 files on POSIX; user ACLs on Windows). Keep credentials in platform credential storage, referenced by space/device, never in task tables or JS. Validate callback state/PKCE and deep-link IDs. Synced data for one account cannot be relabeled for another. Pending data survives auth/network failures; remote disconnection denies server access but cannot erase an offline device.

## Logging

Use structured local logs without content/secrets. Native and CLI share the root log retention budget: 30 days and combined 100 MiB, deleting oldest files when either bound is reached. Serialize rotation across processes. No automatic upload. Diagnostic errors expose categories, not user filesystem paths or SQL parameter values.

## Build and Test

Run the common protocol vectors in native and Go adapters. Test both writer directions against the same physical file, concurrent saves, crash/rollback/disk-full behavior, close/reopen, unsupported schema refusal, account routing and commit-only observation in a running app. Test task/history/outbox/sequence atomicity. Check runtime SQLite source IDs and the WAL fix on all targets.

Run app-local `pnpm test` for frontend changes and the existing platform build commands. Native build/install/behavior gates apply separately to macOS, Windows, iOS and Android. Cross-compiling a CLI or compiling a Lynx bundle does not prove native behavior. Preserve existing CI exclusions for macOS PR/main/affected runs; store/provider acceptance is separate.

## Dependencies and Integrations

Follow [protocol](protos-lane-v1-contract.md), [CLI](cmds-lane-cli-contract.md), [server](servers-lane-api-contract.md), and existing SDK notices. Store conformance fixtures with protocol source; host implementations must not invent incompatible columns or paths.

## Change Triggers

Change this contract and protocol/CLI/app contracts together for schema, path, callback or domain-rule changes. Update scoped instructions when ownership or development procedures change.

## References

- [Project](project-lane.md)
- [Repository defaults](repository-defaults.md)
- [SQLite committed-change observation](https://sqlite.org/pragma.html#pragma_data_version)
- [SQLite WAL](https://sqlite.org/wal.html)
