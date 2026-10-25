# Lane CLI and Local MCP Contract

## Scope

`cmds/lane` owns the Rust `lane` executable and local stdio MCP. `cmds/lane/src` owns Rust domain/store/sync adapters in the root Cargo workspace. Do not create a companion module, `go.work`, private checkout or Go server. The [distribution contract](packages-lane-distribution-contract.md) owns release artifacts and `@delinoio/lane`.

## Runtime and Language

Install `protoc` for schema compilation. Use the root Cargo workspace, bindings generated from the canonical Lane schema, and a pinned bundled SQLite driver. Shared native/Rust SQL paths and columns are fixed by the [local-store contract](apps-lane-local-store-contract.md). Rust code uses `tracing`. Keep stdout reserved for requested results or MCP protocol messages; diagnostics use stderr.

## Users and Operators

Individuals use local commands without login. Existing agents use local stdio MCP without a GUI/server. Users explicitly approve optional online connections. Operators install reviewed executables or the npm launcher.

## Interfaces and Contracts

### Target selection and output

`lane [--remote] [--json] [--space <id>] <command>` defaults to the active local space. Only explicit `--remote` targets server data; login alone does not change the default. `--space` selects an existing local space and is invalid with `--remote`. Server-only operations return ONLINE_REQUIRED with a direct instruction when their target is local/offline; never silently redirect a local task command to the server. Local `sync connect` explicitly authorizes contacting the server.

Human-readable output is default. `--json` emits one result object `{"ok":true,"result":{…}}` or `{"ok":false,"error":{"reason":"…","message":"…","correlationId":"…","retryable":false}}`. Preserve typed protocol enum names and decimal 64-bit values. Exit 0 means completed/read/replayed/deleted-result success; exit 1 means operation failure; exit 2 means invalid invocation. JSON errors also go to stdout; safe diagnostics go to stderr. Use ANSI color in interactive human output, with `NO_COLOR` and `--color=never` opt-out; never add ANSI to JSON/MCP.

Mutating invocations accept `--request-id <uuid>` for reliable retries and print the allocated request ID when omitted. Updates require `--expected-version <uuid>`; interactive clients can obtain it by a read but cannot issue an unconditional write. Conflict resolution requires the complete expected head set. Deletion commands are themselves confirmation; no extra prompt is required for individual task/repository/event deletion. Server account-data deletion still requires explicit account confirmation and browser reauthentication.

### Command surface and typed mapping

| Commands                                                     | Typed operation / behavior                                                                                               |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------ |
| `repository list/get/create/edit/delete`                     | LaneService repository methods; URL registration/rename rules apply                                                      |
| `task list/get/create/edit/delete`                           | LaneService task methods; create accepts a repository ID or URL, title and optional description/prompt/progress/proposal |
| `task progress`, `task accept`, `task reject`                | SetProgress / SetProposal with unchanged separate state                                                                  |
| `task move`                                                  | SaveTask whole snapshot with new repository ID and expected version                                                      |
| `task history`, `task prompt-history`, `task restore-prompt` | ListHistory / RestorePrompt; source version belongs to the same task                                                     |
| `conflict list/get/resolve`                                  | Read heads/alternatives and ResolveConflict; resolution creates a new version                                            |
| `sync connect/status/run/disconnect`                         | Browser approval, durable outbox status, background loop, KEEP_LOCAL/REMOVE_LOCAL choice                                 |
| `login`, `logout`                                            | Browser connection approval; logout requires retained-local-data choice                                                  |
| `device list/disconnect`                                     | AccountService; explicit remote target                                                                                   |
| `connection list/revoke`                                     | AccountService; explicit remote target                                                                                   |
| `notification get/set`                                       | Per-device preferences, not account-wide settings                                                                        |
| `receiver list/create/edit/rotate-key/delete`                | WebhookService receiving settings; online-only                                                                           |
| `event list/get/delete/claim/renew/release/complete/reopen`  | WebhookService; local list/get reads retained cached data; other operations online-only                                  |
| `data remove-local`                                          | Remove only selected local space through coordinated handle shutdown                                                     |
| `data delete-everywhere`, `data clear-server`                | AccountService deletion modes, explicit remote/account confirmation and reauthentication                                 |
| `mcp serve`                                                  | Local stdio MCP; default local store                                                                                     |

Names are public command groups; help must spell out required arguments and local/online distinctions. `task list` supports all repositories or one repository, progress/proposal filters and title/description substring search. Prompt input may use a literal or stdin; reading source text is not a database import/export feature. Explicit task edits save immediately as one command/version. Shared CLI execution continues when the app window is closed.

`sync run` owns a bounded-retry loop and the per-space ownership lease from the local-store contract. It releases ownership on clean exit; crashes leave durable outbox data and an expiring owner record. Persist acknowledgments/cursors before removing pending work. Desktop host supervision uses the same executable, not a second queue implementation. Mobile hosts implement the protocol within their own lifecycle. Do not promise background execution while the OS suspends an app.

### MCP surface

`lane mcp serve` implements stdio MCP. Do not print banners or logs on stdout. Use explicit JSON schemas derived from typed messages. Tool names are `lane_<snake_case_method>` for LaneService, CaptureService, AccountService and WebhookService methods, for example `lane_create_task`, `lane_resolve_conflict`, `lane_claim_event` and `lane_complete_event`. Also expose `lane_get_local_status`, `lane_connect_sync`, `lane_disconnect_sync`, `lane_remove_device_data`, `lane_get_notification_preferences` and `lane_set_notification_preferences` for local device management. Local conflict listing can use the typed list response's alternatives; a separate untyped mutation is not allowed.

MCP arguments omit Origin; the adapter sets MCP and its real device/account connection identity. A new proposed task through MCP can notify; the same operation through CLI uses CLI origin and cannot notify. Tool descriptions identify online-only functions and explicit irreversible deletion semantics. Task/repository deletion invocation is confirmation. Remote account deletion still requires a human browser reauthentication flow and account confirmation; an agent cannot supply a fabricated proof.

Local MCP uses the same SQLite transactions, durable request IDs and conflict rules as CLI. Server-only operations require explicit remote selection/approval; no offline event processing lease exists. Remote MCP is the separate server endpoint, authenticated with a browser-approved full-account grant, and remains available with all devices off. Revoke grants through app/CLI; do not interpret a local stdio launch as a remote approval.

## Storage

Use the exact registry and SQLite layout in the local-store contract. Never use `os.UserConfigDir` as a substitute for the Windows LocalAppData path. CLI/local MCP must interoperate with an already running app and with a closed app. Credentials live in per-OS credential storage; account switch selects a separate local space. Keep local data/outbox on expired authorization or network failure. Unsupported schema is an error, never a fresh empty database.

## Security

Use browser login approval with PKCE/state and correct OAuth audiences. Do not accept user-supplied account/tenant attribution as authorization. Local process access is bounded by OS-user permissions. Never upload a prior account's retained data on login to a different account. One-time receiver secrets are shown only in create/rotate responses; loss requires rotation, not retrieval. Do not run agents, shell commands from prompts, Git access verification or AI services.

## Logging

Use structured stderr/file metadata with correlation IDs. Exclude task/prompt/webhook content, secrets and token values. Apply the combined native/CLI 100 MiB and 30-day log limits and no-upload rule. Failure output must distinguish local save failure, pending sync rejection, auth failure and unavailable dependency.

## Build and Test

At implementation run root Cargo tests and Clippy for the Lane package and CLI builds for macOS, Windows and Linux. Run shared protocol vectors, human/JSON output tests, stdio MCP framing/schema tests, SQLite concurrency and native cross-process compatibility tests. Exercise local CLI/MCP with no account, no server and no app window. Test explicit remote targeting, request retries, whole-task conflicts, event completion atomicity, account deletion proof and revocation with mocked services.

Cross-compilation alone does not establish executable installation or native app behavior on that OS. Record exact runtime/platform checks separately. Preserve root Rust/frontend checks when those sources also change.

## Dependencies and Integrations

Use repository protocol generation, the root Cargo workspace and platform credential adapters. Coordinate the helper's packaging with native hosts. No external agent runtime is a Lane dependency.

## Change Triggers

Change protocol/local-store/app/distribution contracts together for command, schema or installation changes. Update scoped instructions only for ownership or development procedures.

## References

- [Project](project-lane.md)
- [Repository defaults](repository-defaults.md)
- [Protocol](protos-lane-v1-contract.md)
- [Server](servers-lane-api-contract.md)

## Desktop helper and approval arguments

The executable entry point is `cargo build --release -p lane`. `lane --version` prints
`lane 0.1.0`; release builds use the Cargo package version.
`--help` lists command arguments without opening storage or requiring credentials.

Desktop hosts use the following helper interfaces:

- `lane --json --space <uuid> sync connect --redirect-uri <uri>` returns
  `authorizationUrl` and `challengeId`. The host opens the URL in the browser.
  `--endpoint <url>` selects the approval server explicitly; the default is the
  production Lane API. The callback URI must be registered by that server.
- `lane --json --space <original-space-uuid> sync callback <callback-uri>` checks
  the stored approval state and PKCE verifier, stores credentials in OS credential
  storage, and returns the selected `spaceId` and `deviceId`. It never prints tokens.
  Approval binds the existing installation device ID. It must not replace the
  registry device ID with a new server-generated value.
- `lane --space <uuid> sync run` runs the supervised worker. `--once` runs one
  bounded upload batch and drains available pull pages. SIGINT or SIGTERM stops
  the worker and releases its lease. Window closure does not stop this process.
- `lane --json --space <uuid> sync disconnect --retention keep-local|remove-local`
  requires an explicit local-retention choice.
- `lane --remote --expected-space <uuid> --json rpc <Method> --request-json -`
  reads one typed ProtoJSON request from stdin. Literal JSON is also accepted.
  The helper calls the generated Connect method. `--expected-space` verifies the
  active registry selection before dispatch and pins that account for the request;
  a mismatch returns FORBIDDEN. It does not select a different space. Public
  `--space` remains invalid with `--remote`.

Typed commands also accept `--request-json` for complete nested requests, including
conflict resolutions and event-completion tasks. Ordinary task edits read the full
snapshot and preserve unchanged fields, but still submit the caller's required
`--expected-version`. The read cannot make a stale save unconditional.

Without `--redirect-uri`, `login` and `sync connect` listen on the fixed
`http://127.0.0.1:46327/auth/callback` address until browser approval completes.
The CLI prints the authorization URL to stderr. A port conflict fails explicitly.
The server must register `lane-cli` and this exact redirect; the CLI does not
register an OS custom-scheme handler. Desktop hosts explicitly select the
registered `lane://auth/callback` or `lane-dev://auth/callback` URI.

Account deletion requires separate browser reauthentication and final confirmation.
`data delete-everywhere` or `data clear-server`, with
`--remote --confirmed-account-id`, starts reauthentication. With a loopback login,
it waits for browser approval and returns only `challengeId`; repeat the command
with `--challenge-id` to perform deletion. With a desktop redirect it returns the
authorization URL and challenge; the callback helper below redeems the proof.
`--reauth-callback` can also redeem the callback during the explicit final command.
An authorization URL or successful callback is not a completed deletion.

Desktop deletion uses these exact interfaces:

- Remote `rpc BeginDataDeletion` stores the returned challenge in OS credential
  storage, bound to the pinned space, account, tenant, device, session and mode.
- `lane --json --space <uuid> sync callback <callback-uri>` with
  `reauth_challenge` redeems and stores the proof privately. Its result contains
  only `challengeId`. It never invokes deletion.
- `lane --remote --json --expected-space <uuid> --reauth-challenge <id> rpc
DeleteAccountData --request-json -` accepts a typed request without `reauthProof`.
  The helper consumes the matching private proof before dispatch. The host strips
  its UI-only `challengeId` property and passes it as the public flag.
- `lane --json --space <uuid> sync cancel-reauth --challenge-id <id>` invalidates
  exactly that pending challenge and returns `completed: true`.

Proofs never enter argv, stdout, logs or UI events. Expired, cancelled, consumed,
mismatched or switched-account challenges fail. A transport failure after proof
consumption requires a new browser reauthentication; it does not report deletion
success. Raw caller-supplied proofs are rejected by the helper.

After an authentication failure, the worker can query `/auth/deletion-status`
with its existing access token, without refreshing it. Only a directive matching
the pinned account, device and stored old epoch can change local state.
DELETE_EVERYWHERE triggers durable redaction and coordinated physical removal.
CLEAR_SERVER_KEEP_LOCAL records the reconnect requirement. Missing, unknown or
mismatched directives retain local data and report the authentication failure.

The CLI shares `logs/.rotation.lock` with native writers. POSIX participants take
an exclusive `flock`; Windows participants take an exclusive `LockFileEx` lock
on byte zero for length one. Append and retention pruning occur under this lock.

A stale account epoch sets `GetLocalStatus.reconnectRequired` and stops uploads.
Status also includes the pinned `accountId` and typed `syncErrorReason`. Login or
an automatic retry does not authorize uploading retained data into a new server
dataset. A new `sync connect --confirm-epoch-change` invocation explicitly
confirms that consequence and starts a new browser approval. Without the flag,
the CLI explains the pending confirmation and does not begin approval. Native
hosts pass the flag only after the user's reconnect confirmation.

The approval binds the space, account, prior epoch and reconnect intent before
opening the browser. Callback processing refuses changed local state. It probes
the prior epoch again: a matching account-deletion directive takes precedence;
only a typed STALE_EPOCH response permits a confirmed rebuild for the newly
approved epoch. A first connection that discovers a stale epoch stops and requires
a new explicit approval. `--account-id` constrains approval to that exact account;
reconnecting an existing account space also pins its current account identity.

`sync run --retry-blocked` explicitly retries items previously rejected by the
server. Their original request IDs and immutable versions remain unchanged. It
does not confirm an epoch change or clear account-deletion state.

## RealQA integration

The [RealQA contract](apps-lane-realqa-contract.md) extends this domain with durable
final-PNG attachments, authorized retrieval and synchronized URL-to-repository
rules. Preserve existing account routing, whole-task conflicts, request replay,
history, task moves and deletion precedence. Native saves commit locally before
optional image upload. Pending synchronization must not be reported as failed
local saving. Unsupported schema/format versions must be refused before writes.
These requirements do not establish implementation or release acceptance.

### RealQA CLI and MCP operations

- `capture create --repository-id UUID --title TEXT --images-json JSON_ARRAY`
  accepts final PNGs as typed `CapturedImage` values (base64 `png` and a `capture`
  manifest). `--description` is optional. `--request-json -` accepts the complete
  request without placing image bytes in shell arguments. Local saving is atomic.
- `attachment get --task-id UUID --attachment-id UUID` reads an authorized image.
  Local results include base64 PNG bytes or `WAITING_FOR_SYNC`; remote results
  include a short-lived signed download URL. Deleted owners return `DELETED`.
  Do not persist signed URLs or send Lane authorization headers to object storage.
- `url-rule list/create/edit/delete` manages URL-to-repository rules. Creation and
  editing take `--prefix URL --repository-id UUID`; edits also require the rule ID
  and `--expected-version UUID`. `conflict list/get --kind url-rule` reads rule
  conflicts; `conflict resolve` submits the complete expected head set.
- `data migrate --space UUID` explicitly upgrades the selected local schema.
  It rejects remote targets, omitted spaces and request payloads. Close other
  clients before migration; ordinary opens never silently migrate an old store.

MCP discovery includes the CaptureService request schemas and attachment/rule
operations. Upload preparation/completion are online-only. The local sync worker
negotiates format 2, publishes manifests before signed uploads, and validates
image digests, sizes and dimensions before storing downloads. A failed image
transfer preserves local bytes and does not stop unrelated transfers. Account
epoch changes require renewed explicit approval and re-upload retained images;
standalone import copies required historical images and rules into the approved
account space without relabeling data already assigned to another account.
